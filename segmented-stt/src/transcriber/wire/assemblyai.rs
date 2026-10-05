//! AssemblyAI Sync STT (`assemblyai_sync`): `POST {base}/v1/transcribe`, one multipart request,
//! one JSON answer, no job and no polling.
//!
//! Wire facts, from the vendor audit and its fact-check (2026-10-03, both read from the Sync API
//! reference): hosts `sync.assemblyai.com`, `sync.us.` and `sync.eu.`; `Authorization: <key>`
//! (a `Bearer ` prefix is optional); a required `X-AAI-Model` header whose only value is
//! `universal-3-5-pro`; a multipart body with an `audio` part (WAV or raw PCM S16LE) and an
//! optional `config` part holding JSON; audio from 80 ms to 120 s and at most 40 MB.
//!
//! Not documented, and chosen here: the `config` part goes first (the vendor requires that order
//! only on `/v1/transcribe/live`, and it costs nothing on this route) and is labelled
//! `application/json`. A live probe settles whether the server reads a plain text part the same.
//!
//! `language_codes` is always sent: this endpoint does not detect the language, it assumes `en`
//! when the field is absent, so an unset session language gets the configured fallback instead
//! of a silent English transcript. That is also why the minimal repair keeps it. `prompt` is off
//! by default: it is a paid add-on and the vendor ignores `language_codes` when it is set.

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde_json::{Map, Value, json};

use super::{
    Auth, Exchanged, Failure, LanguageFormat, RowLimits, check_limits, exchange, info_for,
    join_url, languages_to_send, may_send, request_id,
};
use crate::transcriber::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};
use crate::types::ErrorClass;

pub const DEFAULT_BASE_URL: &str = "https://sync.assemblyai.com";
pub const MODEL: &str = "universal-3-5-pro";

#[derive(Debug, Clone)]
pub struct AssemblyAiSyncConfig {
    /// `https://sync.assemblyai.com`, `https://sync.us.assemblyai.com` or `https://sync.eu.assemblyai.com`.
    pub base_url: String,
    /// [`Auth::assemblyai`].
    pub auth: Auth,
    /// Sent as `X-AAI-Model`.
    pub model: String,
    /// Sent when the session pinned no language and named no candidates. `["en"]` by default.
    pub fallback_languages: Vec<String>,
    /// `Some(n)` sends at most `n` terms as `keyterms_prompt` (the vendor allows 100).
    pub keyterms_max: Option<usize>,
    /// Send the session prompt as `prompt`. Off by default; see the module documentation.
    pub send_prompt: bool,
    pub client: reqwest::Client,
    pub limits: RowLimits,
}

impl AssemblyAiSyncConfig {
    pub fn new(auth: Auth, client: reqwest::Client) -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            auth,
            model: MODEL.into(),
            fallback_languages: vec!["en".into()],
            keyterms_max: Some(100),
            send_prompt: false,
            client,
            limits: RowLimits {
                min_audio_ms: Some(80),
                max_audio_ms: Some(120_000),
                max_upload_bytes: Some(40_000_000),
                single_process_server: false,
            },
        }
    }
}

#[derive(Debug)]
pub struct AssemblyAiSyncTranscriber {
    cfg: AssemblyAiSyncConfig,
    url: String,
    warm_url: String,
    info: TranscriberInfo,
}

impl AssemblyAiSyncTranscriber {
    pub fn new(cfg: AssemblyAiSyncConfig) -> Result<Self, String> {
        if cfg.model.trim().is_empty() {
            return Err("an AssemblyAI Sync transcriber needs a model".into());
        }
        let url = join_url(&cfg.base_url, "/v1/transcribe")?;
        let warm_url = join_url(&cfg.base_url, "/v1/warm")?;
        let mut droppable = Vec::new();
        if cfg.keyterms_max.is_some() {
            droppable.push("keyterms_prompt".to_string());
        }
        if cfg.send_prompt {
            droppable.push("prompt".to_string());
        }
        let info = info_for("assemblyai_sync", &url, &cfg.model, cfg.limits, droppable);
        Ok(Self {
            cfg,
            url,
            warm_url,
            info,
        })
    }

    /// The `config` JSON and the optional fields it carries.
    fn config(&self, ctx: &SegmentContext) -> (Value, Vec<String>) {
        let mut config = Map::new();
        let mut sent = Vec::new();
        let mut codes = languages_to_send(ctx, LanguageFormat::Iso639_1, true);
        if codes.is_empty() {
            let fallback = SegmentContext {
                candidate_languages: self.cfg.fallback_languages.clone(),
                ..Default::default()
            };
            codes = languages_to_send(&fallback, LanguageFormat::Iso639_1, true);
        }
        if !codes.is_empty() {
            config.insert("language_codes".into(), json!(codes));
        }
        if let Some(max) = self.cfg.keyterms_max {
            let terms: Vec<&str> = ctx
                .keywords
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .take(max)
                .collect();
            if !terms.is_empty() && may_send(ctx, "keyterms_prompt") {
                config.insert("keyterms_prompt".into(), json!(terms));
                sent.push("keyterms_prompt".to_string());
            }
        }
        if self.cfg.send_prompt
            && let Some(p) = ctx
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
            && may_send(ctx, "prompt")
        {
            config.insert("prompt".into(), json!(p));
            sent.push("prompt".to_string());
        }
        (Value::Object(config), sent)
    }

    fn classify(&self, ex: &Exchanged, sent: &[String]) -> SegmentError {
        let failure = Failure {
            vendor: "assemblyai",
            exchanged: ex,
            sent_optional: sent,
            secret: self.cfg.auth.secret(),
        };
        let (mut err, body) = failure.classify();
        // "Model cold-starting or concurrency cap reached": capacity, not an outage.
        if body.code_key().as_deref() == Some("capacityexceeded") {
            err.class = ErrorClass::RateLimited;
            err.vendor_busy = true;
        }
        err
    }

    fn parse(&self, ex: &Exchanged) -> Result<SegmentTranscript, SegmentError> {
        let v = ex.json()?;
        let text = match v.get("text") {
            Some(Value::String(t)) => t.trim().to_string(),
            Some(Value::Null) => String::new(),
            _ => return Err(ex.not_a_transcript("it has no text field")),
        };
        Ok(SegmentTranscript {
            text,
            vendor_confidence: v
                .get("confidence")
                .and_then(Value::as_f64)
                .map(|c| c as f32),
            vendor_request_id: v
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| request_id(&ex.headers, &["x-request-id", "request-id"])),
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for AssemblyAiSyncTranscriber {
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
        let (config, sent) = self.config(ctx);
        let config = Part::text(config.to_string())
            .mime_str("application/json")
            .expect("application/json is a valid MIME type");
        let file = Part::bytes(wav)
            .file_name("segment.wav")
            .mime_str("audio/wav")
            .expect("audio/wav is a valid MIME type");
        let form = Form::new().part("config", config).part("audio", file);
        let req = self
            .cfg
            .client
            .post(&self.url)
            .header("x-aai-model", self.cfg.model.trim())
            .multipart(form);
        let req = self.cfg.auth.apply(req)?;
        let ex = exchange(req, timeout, progress).await?;
        if !ex.is_success() {
            return Err(self.classify(&ex, &sent));
        }
        self.parse(&ex)
    }

    /// The vendor's own warm route: an unauthenticated no-op on the same host.
    async fn prewarm(&self, connections: usize) {
        let client = &self.cfg.client;
        super::super::http::warm_with(connections, super::super::http::WARM_TIMEOUT, || {
            client
                .get(&self.warm_url)
                .header("x-aai-model", self.cfg.model.trim())
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::transcriber::wire::testkit::{
        self as kit, Harness, MockVendor, Reply, SECRET, contract_tests,
    };

    fn config(base: &str) -> AssemblyAiSyncConfig {
        AssemblyAiSyncConfig {
            base_url: base.to_string(),
            ..AssemblyAiSyncConfig::new(Auth::assemblyai(SECRET), kit::client())
        }
    }

    fn build(cfg: AssemblyAiSyncConfig) -> Arc<dyn SegmentTranscriber> {
        Arc::new(AssemblyAiSyncTranscriber::new(cfg).unwrap())
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("es-MX".into()),
            keywords: vec!["Acme".into(), "Zed".into()],
            prompt: Some("A booking call.".into()),
            ..Default::default()
        }
    }

    const SUCCESS: &str = r#"{"text":"Hola mundo.","confidence":0.91,"words":[{"text":"Hola","confidence":0.95},{"text":"mundo.","confidence":0.87}],"audio_duration_ms":1200,"session_id":"sess_1","request_time_ms":243.7}"#;

    fn harness() -> Harness {
        Harness {
            build: |base| build(config(base)),
            ctx: ctx(),
            success: Reply::json(200, SUCCESS),
            unauthorized: Reply::json(
                401,
                &format!(r#"{{"error_code":"unauthorized","message":"Invalid API key {SECRET}"}}"#),
            ),
            model_missing: Reply::json(
                422,
                r#"{"detail":[{"loc":["header","x-aai-model"],"msg":"Input should be 'universal-3-5-pro'","type":"enum"}]}"#,
            ),
            rate_limited: Reply::json(
                429,
                r#"{"error_code":"rate_limited","message":"Too many requests."}"#,
            )
            .header("retry-after", "2"),
            unavailable: Reply::json(
                503,
                r#"{"error_code":"service_unavailable","message":"Try again."}"#,
            ),
            refused: Reply::json(
                400,
                r#"{"error_code":"invalid_request","message":"keyterms_prompt: at most 100 terms are allowed"}"#,
            ),
            refused_field: "keyterms_prompt",
        }
    }

    contract_tests!(super::harness);

    #[tokio::test]
    async fn the_request_carries_the_model_header_config_first_and_the_language() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base));
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();

        let r = vendor.last();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/v1/transcribe");
        assert_eq!(
            r.header("authorization").as_deref(),
            Some(SECRET),
            "the bare key"
        );
        assert_eq!(
            r.header("x-aai-model").as_deref(),
            Some("universal-3-5-pro")
        );
        assert_eq!(r.part_names(), vec!["config", "audio"], "config first");
        let audio = r.part("audio").unwrap();
        assert_eq!(audio.content_type.as_deref(), Some("audio/wav"));
        assert_eq!(&audio.data[..], &kit::audio().wav()[..]);
        let config: Value = serde_json::from_slice(&r.part("config").unwrap().data).unwrap();
        assert_eq!(
            config,
            json!({"language_codes": ["es"], "keyterms_prompt": ["Acme", "Zed"]})
        );
        assert!(
            config.get("prompt").is_none(),
            "the prompt is a paid add-on that disables language_codes"
        );

        assert_eq!(out.text, "Hola mundo.");
        assert_eq!(out.vendor_confidence, Some(0.91));
        assert_eq!(out.vendor_request_id.as_deref(), Some("sess_1"));
        assert_eq!(
            out.detected_language, None,
            "the endpoint returns no language"
        );
    }

    #[tokio::test]
    async fn language_codes_are_always_sent_and_never_default_to_english_silently() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base));
        let config_of = |r: &kit::Recorded| {
            serde_json::from_slice::<Value>(&r.part("config").unwrap().data).unwrap()
        };

        kit::run(t.as_ref(), &SegmentContext::default())
            .await
            .0
            .unwrap();
        assert_eq!(config_of(&vendor.last())["language_codes"], json!(["en"]));

        let candidates = SegmentContext {
            candidate_languages: vec!["es".into(), "fr-CA".into()],
            ..Default::default()
        };
        kit::run(t.as_ref(), &candidates).await.0.unwrap();
        assert_eq!(
            config_of(&vendor.last())["language_codes"],
            json!(["es", "fr"])
        );

        // The minimal repair keeps the one field whose absence means English.
        let minimal = SegmentContext {
            minimal: true,
            ..ctx()
        };
        kit::run(t.as_ref(), &minimal).await.0.unwrap();
        assert_eq!(config_of(&vendor.last()), json!({"language_codes": ["es"]}));
    }

    #[tokio::test]
    async fn an_omitted_field_is_left_out_and_the_prompt_goes_only_when_asked() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(AssemblyAiSyncConfig {
            send_prompt: true,
            ..config(&vendor.base)
        });
        let config_of = |r: &kit::Recorded| {
            serde_json::from_slice::<Value>(&r.part("config").unwrap().data).unwrap()
        };
        kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(
            config_of(&vendor.last())["prompt"],
            json!("A booking call.")
        );

        let omit = SegmentContext {
            omit_fields: vec!["keyterms_prompt".into()],
            ..ctx()
        };
        kit::run(t.as_ref(), &omit).await.0.unwrap();
        let c = config_of(&vendor.last());
        assert!(c.get("keyterms_prompt").is_none());
        assert_eq!(c["language_codes"], json!(["es"]));
    }

    #[tokio::test]
    async fn capacity_exceeded_is_a_capacity_answer() {
        let vendor = MockVendor::start(
            Reply::json(503, r#"{"error_code":"capacity_exceeded","message":"Model cold-starting or concurrency cap reached."}"#)
                .header("retry-after", "1"),
        )
        .await;
        let err = kit::run(build(config(&vendor.base)).as_ref(), &ctx())
            .await
            .0
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::RateLimited);
        assert!(err.vendor_busy && err.is_fast_retryable());
        assert!(!err.counts_for_breaker());
        assert_eq!(err.retry_after, Some(Duration::from_secs(1)));

        vendor.set_default(Reply::json(
            504,
            r#"{"error_code":"inference_timeout","message":"30 s deadline"}"#,
        ));
        let err = kit::run(build(config(&vendor.base)).as_ref(), &ctx())
            .await
            .0
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Vendor);
        assert!(err.is_fast_retryable(), "documented as safe to retry once");
    }

    #[tokio::test]
    async fn prewarm_uses_the_unauthenticated_warm_route() {
        let vendor = MockVendor::start(Reply::json(200, "{}")).await;
        build(config(&vendor.base)).prewarm(2).await;
        let reqs = vendor.requests();
        assert_eq!(reqs.len(), 2);
        for r in reqs {
            assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/v1/warm"));
            assert_eq!(
                r.header("x-aai-model").as_deref(),
                Some("universal-3-5-pro")
            );
            assert!(r.header("authorization").is_none());
        }
    }

    #[test]
    fn info_carries_the_vendor_bounds() {
        let t = AssemblyAiSyncTranscriber::new(AssemblyAiSyncConfig::new(
            Auth::assemblyai("k"),
            kit::client(),
        ))
        .unwrap();
        kit::info_is_a_file_target(&t, "assemblyai_sync", "https://sync.assemblyai.com");
        let info = t.info();
        assert_eq!(
            (info.min_audio_ms, info.max_audio_ms),
            (Some(80), Some(120_000))
        );
        assert_eq!(info.max_upload_bytes, Some(40_000_000));
        assert_eq!(info.droppable_fields, vec!["keyterms_prompt"]);
        assert_eq!(info.model, "universal-3-5-pro");
    }
}
