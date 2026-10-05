//! Google Speech-to-Text v2 `Recognize` over REST (`google_recognize`):
//! `POST {base}/v2/projects/{project}/locations/{location}/recognizers/{recognizer}:recognize`
//! with a JSON body holding `config` and the audio as base64 `content`.
//!
//! REST rather than a gRPC unary call, so this family shares the attempt code, timers and breaker
//! of the others; the cost is base64 (a 25 s segment grows from 0.8 MB to about 1.07 MB). The
//! vendor limit is 60 s or 10 MB per request. Whether the regional hosts
//! (`{location}-speech.googleapis.com`) serve the REST path is not shown in the vendor pages the
//! audit read; the gRPC client already uses that host naming, and a live probe settles REST.
//!
//! The credential is an OAuth access token that expires. The gateway owns its source and pushes
//! each new token with [`GoogleRecognizeTranscriber::set_bearer_token`]; every request reads the
//! current one, so a call that outlives a token keeps working.
//!
//! Signals: an empty `results` array is the vendor saying there was no speech. `confidence` is
//! reported as given, but the vendor says it "isn't truly a confidence score" for the Chirp models,
//! so a row for them should not list it; 0.0 is the "not set" sentinel and is reported as none.

use std::time::Duration;

use base64::Engine as _;
use parking_lot::RwLock;
use serde_json::{Map, Value, json};

use super::{
    Auth, Exchanged, Failure, LanguageFormat, RowLimits, check_limits, detected_language, exchange,
    info_for, join_url, languages_to_send, may_send, request_id,
};
use crate::transcriber::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};

pub const GLOBAL_BASE_URL: &str = "https://speech.googleapis.com";

/// `https://speech.googleapis.com` for `global`, `https://{location}-speech.googleapis.com` else.
pub fn base_url_for(location: &str) -> String {
    match location.trim() {
        "" | "global" => GLOBAL_BASE_URL.to_string(),
        l => format!("https://{l}-speech.googleapis.com"),
    }
}

#[derive(Debug, Clone)]
pub struct GoogleRecognizeConfig {
    pub base_url: String,
    pub project: String,
    pub location: String,
    /// `_`, the implicit recognizer, unless the deployment made one.
    pub recognizer: String,
    /// [`Auth::Bearer`] with an OAuth access token.
    pub auth: Auth,
    pub model: String,
    /// Sent when the session pinned no language and named no candidates; `["auto"]` for
    /// `chirp_3`. The implicit recognizer needs at least one code.
    pub fallback_language_codes: Vec<String>,
    /// Key terms as an inline phrase set (`adaptation`). Not for `chirp`, which has none.
    pub phrase_hints: bool,
    /// Headerless LINEAR16 at 16 kHz instead of a WAV the vendor detects. The WAV is the
    /// vendor's preferred form; this exists for a recognizer configured for raw audio.
    pub explicit_linear16: bool,
    pub client: reqwest::Client,
    pub limits: RowLimits,
}

impl GoogleRecognizeConfig {
    pub fn new(
        project: &str,
        location: &str,
        model: &str,
        token: &str,
        client: reqwest::Client,
    ) -> Self {
        Self {
            base_url: base_url_for(location),
            project: project.to_string(),
            location: location.to_string(),
            recognizer: "_".into(),
            auth: Auth::Bearer(token.to_string()),
            model: model.to_string(),
            fallback_language_codes: if model.trim() == "chirp_3" {
                vec!["auto".into()]
            } else {
                Vec::new()
            },
            phrase_hints: model.trim() != "chirp",
            explicit_linear16: false,
            client,
            // The adapter refuses above 55 s, inside the vendor's 60 s, and 10 MB of JSON.
            limits: RowLimits {
                max_audio_ms: Some(55_000),
                max_upload_bytes: Some(10_000_000),
                ..Default::default()
            },
        }
    }
}

#[derive(Debug)]
pub struct GoogleRecognizeTranscriber {
    cfg: GoogleRecognizeConfig,
    auth: RwLock<Auth>,
    url: String,
    info: TranscriberInfo,
}

impl GoogleRecognizeTranscriber {
    pub fn new(cfg: GoogleRecognizeConfig) -> Result<Self, String> {
        for (what, v) in [
            ("project", &cfg.project),
            ("location", &cfg.location),
            ("recognizer", &cfg.recognizer),
            ("model", &cfg.model),
        ] {
            let v = v.trim();
            if v.is_empty() || v.contains(['/', '?', '#', ':']) {
                return Err(format!(
                    "a Google Recognize target needs a plain {what}, got {v:?}"
                ));
            }
        }
        let path = format!(
            "/v2/projects/{}/locations/{}/recognizers/{}:recognize",
            cfg.project.trim(),
            cfg.location.trim(),
            cfg.recognizer.trim()
        );
        let url = join_url(&cfg.base_url, &path)?;
        let droppable = if cfg.phrase_hints {
            vec!["adaptation".to_string()]
        } else {
            Vec::new()
        };
        let info = info_for("google_recognize", &url, &cfg.model, cfg.limits, droppable);
        Ok(Self {
            auth: RwLock::new(cfg.auth.clone()),
            cfg,
            url,
            info,
        })
    }

    /// Replaces the access token for every later request.
    pub fn set_bearer_token(&self, token: impl Into<String>) {
        *self.auth.write() = Auth::Bearer(token.into());
    }

    /// The `config` object and the optional fields it carries.
    fn config(&self, ctx: &SegmentContext) -> (Value, Vec<String>) {
        let mut config = Map::new();
        let mut sent = Vec::new();
        if self.cfg.explicit_linear16 {
            config.insert(
                "explicitDecodingConfig".into(),
                json!({"encoding": "LINEAR16", "sampleRateHertz": 16000, "audioChannelCount": 1}),
            );
        } else {
            config.insert("autoDecodingConfig".into(), json!({}));
        }
        config.insert("model".into(), json!(self.cfg.model.trim()));
        let mut codes = languages_to_send(ctx, LanguageFormat::Bcp47, true);
        if codes.is_empty() {
            codes = self.cfg.fallback_language_codes.clone();
        }
        // Required by the implicit recognizer, so not optional and kept by the minimal repair.
        if !codes.is_empty() {
            config.insert("languageCodes".into(), json!(codes));
        }
        let phrases: Vec<Value> = ctx
            .keywords
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .map(|k| json!({ "value": k }))
            .collect();
        if self.cfg.phrase_hints && !phrases.is_empty() && may_send(ctx, "adaptation") {
            config.insert(
                "adaptation".into(),
                json!({"phraseSets": [{"inlinePhraseSet": {"phrases": phrases}}]}),
            );
            sent.push("adaptation".to_string());
        }
        (Value::Object(config), sent)
    }

    fn parse(&self, ex: &Exchanged) -> Result<SegmentTranscript, SegmentError> {
        let v = ex.json()?;
        if !v.is_object() {
            return Err(ex.not_a_transcript("it is not an object"));
        }
        let results = v
            .get("results")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let firsts: Vec<&Value> = results
            .iter()
            .filter_map(|r| r.pointer("/alternatives/0"))
            .collect();
        let text = firsts
            .iter()
            .filter_map(|a| a.get("transcript").and_then(Value::as_str))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let confidences: Vec<f64> = firsts
            .iter()
            .filter_map(|a| a.get("confidence").and_then(Value::as_f64))
            .filter(|c| *c > 0.0)
            .collect();
        Ok(SegmentTranscript {
            text,
            vendor_confidence: (!confidences.is_empty())
                .then(|| (confidences.iter().sum::<f64>() / confidences.len() as f64) as f32),
            detected_language: results
                .iter()
                .find_map(|r| r.get("languageCode").and_then(Value::as_str))
                .and_then(detected_language),
            vendor_said_no_speech: results.is_empty(),
            vendor_request_id: v
                .pointer("/metadata/requestId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| request_id(&ex.headers, &["x-goog-request-id", "x-request-id"])),
            billed_ms: v
                .pointer("/metadata/totalBilledDuration")
                .and_then(Value::as_str)
                .and_then(|d| d.trim().strip_suffix('s'))
                .and_then(|s| s.parse::<f64>().ok())
                .map(|s| (s * 1000.0).round() as u32),
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for GoogleRecognizeTranscriber {
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
        let raw = if self.cfg.explicit_linear16 {
            audio.pcm_bytes()
        } else {
            audio.wav()
        };
        let content = base64::engine::general_purpose::STANDARD.encode(raw);
        let (config, sent) = self.config(ctx);
        let body = json!({ "config": config, "content": content }).to_string();
        check_limits(&self.info, audio, body.len())?;
        let auth = self.auth.read().clone();
        let req = self
            .cfg
            .client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        let req = auth.apply(req)?;
        let ex = exchange(req, timeout, progress).await?;
        if !ex.is_success() {
            let failure = Failure {
                vendor: "google",
                exchanged: &ex,
                sent_optional: &sent,
                secret: auth.secret(),
            };
            return Err(failure.classify().0);
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

    fn config(base: &str, model: &str) -> GoogleRecognizeConfig {
        GoogleRecognizeConfig {
            base_url: base.to_string(),
            ..GoogleRecognizeConfig::new("proj-1", "us", model, SECRET, kit::client())
        }
    }

    fn build(cfg: GoogleRecognizeConfig) -> Arc<GoogleRecognizeTranscriber> {
        Arc::new(GoogleRecognizeTranscriber::new(cfg).unwrap())
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("en-us".into()),
            keywords: vec!["Acme".into(), "Zed".into()],
            ..Default::default()
        }
    }

    const SUCCESS: &str = r#"{"results":[
        {"alternatives":[{"transcript":"hello world","confidence":0.92}],"resultEndOffset":"1.200s","languageCode":"en-us"},
        {"alternatives":[{"transcript":" again","confidence":0.0}],"languageCode":"en-us"}],
        "metadata":{"totalBilledDuration":"2s","requestId":"g-req-1"}}"#;

    fn harness() -> Harness {
        Harness {
            build: |base| build(config(base, "chirp_3")),
            ctx: ctx(),
            success: Reply::json(200, SUCCESS),
            unauthorized: Reply::json(
                401,
                r#"{"error":{"code":401,"message":"Request had invalid authentication credentials.","status":"UNAUTHENTICATED"}}"#,
            ),
            model_missing: Reply::json(404, r#"{"error":{"code":404,"message":"Recognizer not found.","status":"NOT_FOUND"}}"#),
            rate_limited: Reply::json(429, r#"{"error":{"code":429,"message":"Quota exceeded.","status":"RESOURCE_EXHAUSTED"}}"#)
                .header("retry-after", "2"),
            unavailable: Reply::json(503, r#"{"error":{"code":503,"message":"The service is currently unavailable.","status":"UNAVAILABLE"}}"#),
            refused: Reply::json(
                400,
                r#"{"error":{"code":400,"message":"Invalid recognition 'config': adaptation is not supported for this model.","status":"INVALID_ARGUMENT"}}"#,
            ),
            refused_field: "adaptation",
        }
    }

    contract_tests!(super::harness);

    #[tokio::test]
    async fn the_request_is_v2_recognize_with_base64_wav_and_a_bearer_token() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "chirp_3"));
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();

        let r = vendor.last();
        assert_eq!(r.method, "POST");
        assert_eq!(
            r.path,
            "/v2/projects/proj-1/locations/us/recognizers/_:recognize"
        );
        assert_eq!(
            r.header("authorization").as_deref(),
            Some(format!("Bearer {SECRET}").as_str())
        );
        assert_eq!(
            r.header("content-type").as_deref(),
            Some("application/json")
        );
        let body = r.json();
        assert_eq!(
            body["config"],
            json!({
                "autoDecodingConfig": {},
                "model": "chirp_3",
                "languageCodes": ["en-US"],
                "adaptation": {"phraseSets": [{"inlinePhraseSet": {"phrases": [{"value": "Acme"}, {"value": "Zed"}]}}]}
            })
        );
        let content = base64::engine::general_purpose::STANDARD
            .decode(body["content"].as_str().unwrap())
            .unwrap();
        assert_eq!(content, kit::audio().wav());

        assert_eq!(out.text, "hello world again");
        assert_eq!(
            out.vendor_confidence,
            Some(0.92),
            "0.0 is the not-set sentinel"
        );
        assert_eq!(out.detected_language.as_deref(), Some("en"));
        assert_eq!(out.vendor_request_id.as_deref(), Some("g-req-1"));
        assert_eq!(out.billed_ms, Some(2000));
        assert!(!out.vendor_said_no_speech);
    }

    #[tokio::test]
    async fn empty_results_are_the_vendor_saying_no_speech() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"metadata":{"totalBilledDuration":"1s"}}"#,
        ))
        .await;
        let out = kit::run(build(config(&vendor.base, "chirp_3")).as_ref(), &ctx())
            .await
            .0
            .unwrap();
        assert_eq!(out.text, "");
        assert!(out.vendor_said_no_speech);
        assert_eq!(out.billed_ms, Some(1000), "an empty answer is still billed");
    }

    #[tokio::test]
    async fn without_a_language_chirp_3_gets_auto_and_candidates_win_over_it() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "chirp_3"));
        kit::run(t.as_ref(), &SegmentContext::default())
            .await
            .0
            .unwrap();
        assert_eq!(
            vendor.last().json()["config"]["languageCodes"],
            json!(["auto"])
        );
        let candidates = SegmentContext {
            candidate_languages: vec!["en-us".into(), "es-us".into()],
            ..Default::default()
        };
        kit::run(t.as_ref(), &candidates).await.0.unwrap();
        assert_eq!(
            vendor.last().json()["config"]["languageCodes"],
            json!(["en-US", "es-US"])
        );
    }

    #[tokio::test]
    async fn explicit_linear16_sends_headerless_pcm() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(GoogleRecognizeConfig {
            explicit_linear16: true,
            ..config(&vendor.base, "chirp_2")
        });
        kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        let body = vendor.last().json();
        assert_eq!(
            body["config"]["explicitDecodingConfig"],
            json!({"encoding": "LINEAR16", "sampleRateHertz": 16000, "audioChannelCount": 1})
        );
        assert!(body["config"].get("autoDecodingConfig").is_none());
        let content = base64::engine::general_purpose::STANDARD
            .decode(body["content"].as_str().unwrap())
            .unwrap();
        assert_eq!(content, kit::audio().pcm_bytes());
    }

    #[tokio::test]
    async fn a_new_token_is_used_by_the_next_request() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "chirp_3"));
        t.set_bearer_token("ya29.rotated-token");
        kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(
            vendor.last().header("authorization").as_deref(),
            Some("Bearer ya29.rotated-token")
        );
    }

    #[tokio::test]
    async fn minimal_keeps_the_decoding_the_model_and_the_required_language() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "chirp_3"));
        kit::run(
            t.as_ref(),
            &SegmentContext {
                minimal: true,
                ..ctx()
            },
        )
        .await
        .0
        .unwrap();
        assert_eq!(
            vendor.last().json()["config"],
            json!({"autoDecodingConfig": {}, "model": "chirp_3", "languageCodes": ["en-US"]})
        );
    }

    #[tokio::test]
    async fn an_omitted_field_is_left_out_and_chirp_has_no_phrase_sets() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "chirp_3"));
        kit::run(
            t.as_ref(),
            &SegmentContext {
                omit_fields: vec!["adaptation".into()],
                ..ctx()
            },
        )
        .await
        .0
        .unwrap();
        assert!(vendor.last().json()["config"].get("adaptation").is_none());

        let chirp = build(config(&vendor.base, "chirp"));
        assert!(chirp.info().droppable_fields.is_empty());
        kit::run(chirp.as_ref(), &ctx()).await.0.unwrap();
        assert!(vendor.last().json()["config"].get("adaptation").is_none());
    }

    #[test]
    fn info_url_and_config_checks() {
        let t = GoogleRecognizeTranscriber::new(GoogleRecognizeConfig::new(
            "p",
            "eu",
            "chirp_3",
            "tok",
            kit::client(),
        ))
        .unwrap();
        kit::info_is_a_file_target(&t, "google_recognize", "https://eu-speech.googleapis.com");
        assert_eq!(
            t.url,
            "https://eu-speech.googleapis.com/v2/projects/p/locations/eu/recognizers/_:recognize"
        );
        assert_eq!(t.info().max_audio_ms, Some(55_000));
        assert_eq!(base_url_for("global"), "https://speech.googleapis.com");
        let bad = GoogleRecognizeConfig::new("p/../q", "us", "chirp_3", "tok", kit::client());
        assert!(GoogleRecognizeTranscriber::new(bad).is_err());
    }
}
