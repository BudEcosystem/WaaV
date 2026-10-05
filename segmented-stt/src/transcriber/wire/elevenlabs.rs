//! ElevenLabs batch speech-to-text (`elevenlabs_batch`): `POST {base}/v1/speech-to-text`.
//!
//! The live profile differs from the vendor defaults in one field: `tag_audio_events=false`,
//! because the default `true` writes `(laughter)` into the text and a noise-only segment would
//! start an agent turn. `timestamps_granularity=word` is the default, sent explicitly because the
//! per-word `logprob` is the only quality signal this vendor returns. The text is rebuilt from the
//! `word` and `spacing` tokens, so an audio event never reaches it even when the vendor tags one.
//!
//! Key terms add a 20 % surcharge and, above 100 per request, a 20 s minimum bill; the plan step
//! warns about the first and caps the second through `keyterms_max`.

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde_json::Value;

use super::{
    Auth, Exchanged, Failure, Fields, RowLimits, check_limits, confidence_from_logprob,
    detected_language, exchange, info_for, iso639_1, join_url, request_id, session_language,
};
use crate::transcriber::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};

pub const DEFAULT_BASE_URL: &str = "https://api.elevenlabs.io";

/// The vendor's limits on one key term.
const KEYTERM_MAX_CHARS: usize = 49;
const KEYTERM_MAX_WORDS: usize = 5;

#[derive(Debug, Clone)]
pub struct ElevenLabsConfig {
    /// `https://api.elevenlabs.io` or a residency host.
    pub base_url: String,
    /// [`Auth::elevenlabs`].
    pub auth: Auth,
    /// `scribe_v2`, `scribe_v2_medical`.
    pub model: String,
    /// `Some(n)` sends at most `n` key terms; `None` (a row without key terms) sends none.
    pub keyterms_max: Option<usize>,
    /// `?enable_logging=false`: zero retention, enterprise keys only.
    pub zero_retention: bool,
    pub client: reqwest::Client,
    pub limits: RowLimits,
}

impl ElevenLabsConfig {
    pub fn new(auth: Auth, model: &str, client: reqwest::Client) -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            auth,
            model: model.to_string(),
            keyterms_max: Some(100),
            zero_retention: false,
            client,
            // The vendor's documented minimum.
            limits: RowLimits {
                min_audio_ms: Some(100),
                ..Default::default()
            },
        }
    }
}

#[derive(Debug)]
pub struct ElevenLabsTranscriber {
    cfg: ElevenLabsConfig,
    url: String,
    info: TranscriberInfo,
}

impl ElevenLabsTranscriber {
    pub fn new(cfg: ElevenLabsConfig) -> Result<Self, String> {
        if cfg.model.trim().is_empty() {
            return Err("an ElevenLabs transcriber needs a model".into());
        }
        let mut url = join_url(&cfg.base_url, "/v1/speech-to-text")?;
        if cfg.zero_retention {
            url.push_str("?enable_logging=false");
        }
        let mut droppable: Vec<String> = [
            "language_code",
            "tag_audio_events",
            "timestamps_granularity",
            "file_format",
        ]
        .map(String::from)
        .into();
        if cfg.keyterms_max.is_some() {
            droppable.push("keyterms".into());
        }
        let info = info_for("elevenlabs_batch", &url, &cfg.model, cfg.limits, droppable);
        Ok(Self { cfg, url, info })
    }

    fn fields(&self, ctx: &SegmentContext) -> Fields {
        let mut f = Fields::default();
        f.required("model_id", self.cfg.model.trim());
        let lang = session_language(ctx.language.as_deref()).and_then(|l| iso639_1(&l));
        f.optional(ctx, "language_code", lang);
        f.optional(ctx, "tag_audio_events", ["false"]);
        f.optional(ctx, "timestamps_granularity", ["word"]);
        if let Some(max) = self.cfg.keyterms_max {
            // A term over the vendor's limits would refuse the whole list; leave it out instead.
            let terms = ctx
                .keywords
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty() && k.chars().count() <= KEYTERM_MAX_CHARS)
                .filter(|k| k.split_whitespace().count() <= KEYTERM_MAX_WORDS)
                .take(max)
                .map(str::to_string);
            f.optional(ctx, "keyterms", terms.collect::<Vec<_>>());
        }
        // A WAV container. The headerless `pcm_s16le_16` fast path waits for a live check.
        f.optional(ctx, "file_format", ["other"]);
        f
    }

    fn classify(&self, ex: &Exchanged, sent: &[String]) -> SegmentError {
        let failure = Failure {
            vendor: "elevenlabs",
            exchanged: ex,
            sent_optional: sent,
            secret: self.cfg.auth.secret(),
        };
        let (mut err, body) = failure.classify();
        // `system_busy` is the vendor's own "try again"; the two limit codes go through the gate.
        if body.code_key().as_deref() == Some("systembusy") {
            err.vendor_busy = true;
        }
        err
    }

    fn parse(&self, ex: &Exchanged) -> Result<SegmentTranscript, SegmentError> {
        let v = ex.json()?;
        let words = v
            .get("words")
            .and_then(Value::as_array)
            .filter(|w| !w.is_empty());
        let text = match words {
            Some(words) => {
                let mut text = String::new();
                for w in words {
                    if matches!(
                        w.get("type").and_then(Value::as_str),
                        Some("word" | "spacing")
                    ) {
                        text.push_str(w.get("text").and_then(Value::as_str).unwrap_or_default());
                    }
                }
                // Removing an audio event can leave two spacing tokens side by side.
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            }
            None => match v.get("text").and_then(Value::as_str) {
                Some(t) => t.trim().to_string(),
                None => return Err(ex.not_a_transcript("it has neither text nor words")),
            },
        };
        let logprobs: Vec<f64> = words
            .into_iter()
            .flatten()
            .filter(|w| w.get("type").and_then(Value::as_str) == Some("word"))
            .filter_map(|w| w.get("logprob").and_then(Value::as_f64))
            .collect();
        let mean =
            (!logprobs.is_empty()).then(|| logprobs.iter().sum::<f64>() / logprobs.len() as f64);
        Ok(SegmentTranscript {
            text,
            derived_confidence: mean.map(|m| confidence_from_logprob(m as f32)),
            // Documented as ISO 639-3 (`eng`); reported as ISO 639-1.
            detected_language: v
                .get("language_code")
                .and_then(Value::as_str)
                .and_then(detected_language),
            vendor_request_id: v
                .get("transcription_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| request_id(&ex.headers, &["request-id", "x-request-id"])),
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for ElevenLabsTranscriber {
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
            .apply(self.cfg.client.post(&self.url).multipart(form))?;
        let ex = exchange(req, timeout, progress).await?;
        if !ex.is_success() {
            return Err(self.classify(&ex, &fields.sent_optional));
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
    use crate::types::ErrorClass;

    fn config(base: &str) -> ElevenLabsConfig {
        ElevenLabsConfig {
            base_url: base.to_string(),
            ..ElevenLabsConfig::new(Auth::elevenlabs(SECRET), "scribe_v2", kit::client())
        }
    }

    fn build(cfg: ElevenLabsConfig) -> Arc<dyn SegmentTranscriber> {
        Arc::new(ElevenLabsTranscriber::new(cfg).unwrap())
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("en-US".into()),
            keywords: vec!["Acme".into(), "Zed".into()],
            ..Default::default()
        }
    }

    const SUCCESS: &str = r#"{"language_code":"eng","language_probability":0.98,"text":"Hello (laughter) world","words":[
        {"text":"Hello","type":"word","start":0.0,"end":0.4,"logprob":-0.1},
        {"text":" ","type":"spacing","start":0.4,"end":0.5,"logprob":0.0},
        {"text":"(laughter)","type":"audio_event","start":0.5,"end":0.9,"logprob":-0.2},
        {"text":" ","type":"spacing","start":0.9,"end":1.0,"logprob":0.0},
        {"text":"world","type":"word","start":1.0,"end":1.2,"logprob":-0.3}],
        "transcription_id":"tx_1","audio_duration_secs":1.2}"#;

    fn harness() -> Harness {
        Harness {
            build: |base| build(config(base)),
            ctx: ctx(),
            success: Reply::json(200, SUCCESS),
            unauthorized: Reply::json(
                401,
                &format!(
                    r#"{{"detail":{{"status":"invalid_api_key","message":"Invalid API key: {SECRET}"}}}}"#
                ),
            ),
            model_missing: Reply::json(
                400,
                r#"{"detail":{"status":"model_not_found","message":"Model 'scribe_v9' was not found."}}"#,
            ),
            rate_limited: Reply::json(
                429,
                r#"{"detail":{"status":"rate_limit_exceeded","message":"Too many requests."}}"#,
            )
            .header("retry-after", "2"),
            unavailable: Reply::json(
                503,
                r#"{"detail":{"status":"service_unavailable","message":"Try later."}}"#,
            ),
            refused: Reply::json(
                422,
                r#"{"detail":[{"loc":["body","language_code"],"msg":"Unsupported language code","type":"value_error"}]}"#,
            ),
            refused_field: "language_code",
        }
    }

    contract_tests!(super::harness);

    #[tokio::test]
    async fn the_live_profile_reaches_the_vendor_and_the_text_drops_audio_events() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base));
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();

        let r = vendor.last();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/v1/speech-to-text");
        assert!(r.query.is_empty());
        assert_eq!(r.header("xi-api-key").as_deref(), Some(SECRET));
        assert!(r.header("authorization").is_none());
        let file = r.part("file").unwrap();
        assert_eq!(file.file_name.as_deref(), Some("segment.wav"));
        assert_eq!(file.content_type.as_deref(), Some("audio/wav"));
        assert_eq!(&file.data[..], &kit::audio().wav()[..]);
        assert_eq!(r.text("model_id").as_deref(), Some("scribe_v2"));
        assert_eq!(r.text("language_code").as_deref(), Some("en"));
        assert_eq!(r.text("tag_audio_events").as_deref(), Some("false"));
        assert_eq!(r.text("timestamps_granularity").as_deref(), Some("word"));
        assert_eq!(r.texts("keyterms"), vec!["Acme", "Zed"]);
        assert_eq!(r.text("file_format").as_deref(), Some("other"));
        assert!(r.part("diarize").is_none() && r.part("webhook").is_none());

        assert_eq!(out.text, "Hello world");
        assert_eq!(out.detected_language.as_deref(), Some("en"));
        assert!((out.derived_confidence.unwrap() - (-0.2f32).exp()).abs() < 1e-6);
        assert_eq!(out.vendor_confidence, None);
        assert_eq!(out.vendor_request_id.as_deref(), Some("tx_1"));
    }

    #[tokio::test]
    async fn without_words_the_vendor_text_is_used_and_an_event_alone_is_empty() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"language_code":"spa","text":" hola ","words":[]}"#,
        ))
        .await;
        let t = build(config(&vendor.base));
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(out.text, "hola");
        assert_eq!(out.detected_language.as_deref(), Some("es"));
        assert_eq!(out.derived_confidence, None);

        vendor.set_default(Reply::json(
            200,
            r#"{"text":"(cough)","words":[{"text":"(cough)","type":"audio_event","logprob":-0.5}]}"#,
        ));
        assert_eq!(kit::run(t.as_ref(), &ctx()).await.0.unwrap().text, "");
    }

    #[tokio::test]
    async fn key_terms_are_capped_and_invalid_ones_left_out() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(ElevenLabsConfig {
            keyterms_max: Some(2),
            ..config(&vendor.base)
        });
        let ctx = SegmentContext {
            keywords: vec![
                "one two three four five six".into(),
                "Acme".into(),
                " ".into(),
                "Zed".into(),
                "Third".into(),
            ],
            ..Default::default()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        assert_eq!(vendor.last().texts("keyterms"), vec!["Acme", "Zed"]);

        let none = build(ElevenLabsConfig {
            keyterms_max: None,
            ..config(&vendor.base)
        });
        kit::run(none.as_ref(), &ctx).await.0.unwrap();
        assert!(vendor.last().part("keyterms").is_none());
        assert!(
            !none
                .info()
                .droppable_fields
                .contains(&"keyterms".to_string())
        );
    }

    #[tokio::test]
    async fn minimal_sends_only_the_file_and_the_model() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base));
        let ctx = SegmentContext {
            minimal: true,
            ..ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        assert_eq!(vendor.last().part_names(), vec!["file", "model_id"]);
    }

    #[tokio::test]
    async fn an_omitted_field_is_left_out() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base));
        let ctx = SegmentContext {
            omit_fields: vec!["tag_audio_events".into()],
            ..ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        let r = vendor.last();
        assert!(r.part("tag_audio_events").is_none());
        assert_eq!(r.text("language_code").as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn the_plan_and_busy_codes_are_told_apart() {
        let vendor = MockVendor::start(Reply::json(
            402,
            r#"{"detail":{"status":"payment_required","message":"Insufficient credits."}}"#,
        ))
        .await;
        let t = build(config(&vendor.base));
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(err.class, ErrorClass::Auth, "the plan or the balance");

        vendor.set_default(Reply::json(
            429,
            r#"{"detail":{"status":"system_busy","message":"The system is currently busy."}}"#,
        ));
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(err.class, ErrorClass::RateLimited);
        assert!(err.vendor_busy);
        assert!(err.is_fast_retryable());

        vendor.set_default(Reply::json(
            429,
            r#"{"detail":{"status":"concurrent_limit_exceeded","message":"Too many concurrent requests."}}"#,
        ));
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(err.class, ErrorClass::RateLimited);
        assert!(!err.vendor_busy);
    }

    #[tokio::test]
    async fn zero_retention_goes_on_the_query_string() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(ElevenLabsConfig {
            zero_retention: true,
            ..config(&vendor.base)
        });
        kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        let r = vendor.last();
        assert_eq!(r.query_values("enable_logging"), vec!["false"]);
        assert!(r.part("enable_logging").is_none());
    }

    #[test]
    fn info_declares_the_target() {
        let t = ElevenLabsTranscriber::new(ElevenLabsConfig::new(
            Auth::elevenlabs("k"),
            "scribe_v2_medical",
            kit::client(),
        ))
        .unwrap();
        kit::info_is_a_file_target(&t, "elevenlabs_batch", "https://api.elevenlabs.io");
        assert_eq!(t.info().model, "scribe_v2_medical");
        assert_eq!(t.info().min_audio_ms, Some(100));
        assert_eq!(
            t.info().droppable_fields,
            vec![
                "language_code",
                "tag_audio_events",
                "timestamps_granularity",
                "file_format",
                "keyterms"
            ]
        );
        assert!(
            ElevenLabsTranscriber::new(ElevenLabsConfig {
                base_url: "nope".into(),
                ..config("x")
            })
            .is_err()
        );
    }
}
