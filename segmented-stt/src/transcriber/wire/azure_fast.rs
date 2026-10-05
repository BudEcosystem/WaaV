//! Azure AI Speech fast transcription (`azure_fast_transcription`):
//! `POST {endpoint}/speechtotext/transcriptions:transcribe?api-version=2025-10-15`, multipart with
//! an `audio` part and a `definition` part holding JSON.
//!
//! The only way to reach `MAI-Transcribe-2`, `MAI-Transcribe-1.5` and LLM Speech, which are
//! selected inside the definition (`enhancedMode`), not by a model field. API version 2025-10-15
//! is the one the vendor audit and its fact-check read (2024-11-15 is the batch API's); phrase
//! lists need it.
//!
//! Language: the default model and LLM Speech take a BCP-47 `locales` list, and several locales
//! turn on language identification. The MAI models take exactly one ISO-style code, and the vendor
//! warns against setting it unless certain, so for them only a pinned language is sent.
//!
//! Confidence: the duration-weighted mean of `phrases[].confidence`. LLM Speech always reports 0,
//! which means "not measured", so an all-zero answer reports none.

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde_json::{Map, Value, json};

use super::{
    Auth, Exchanged, Failure, LanguageFormat, RowLimits, check_limits, detected_language, exchange,
    info_for, join_url, languages_to_send, may_send, request_id,
};
use crate::transcriber::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};

pub const DEFAULT_API_VERSION: &str = "2025-10-15";

/// `enhancedMode`: `model: Some("MAI-Transcribe-2")` for MAI, `None` for LLM Speech.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AzureEnhancedMode {
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AzureFastConfig {
    /// `https://{region}.api.cognitive.microsoft.com` or `https://{resource}.cognitiveservices.azure.com`.
    pub endpoint: String,
    pub api_version: String,
    /// [`Auth::azure_speech`] for a key, [`Auth::Bearer`] for an Entra token.
    pub auth: Auth,
    /// The row's model id, for the target's identity; the wire selects it through `enhanced`.
    pub model: String,
    pub enhanced: Option<AzureEnhancedMode>,
    pub language_format: LanguageFormat,
    /// Whether candidate locales may be sent when none is pinned (not for the MAI models).
    pub candidate_locales: bool,
    /// Key terms as `phraseList.phrases`.
    pub phrase_list: bool,
    /// The session prompt as `enhancedMode.prompt` (LLM Speech only).
    pub prompt: bool,
    pub client: reqwest::Client,
    pub limits: RowLimits,
}

impl AzureFastConfig {
    /// The default model: BCP-47 locales, phrase list, no enhanced mode.
    pub fn default_model(endpoint: &str, auth: Auth, client: reqwest::Client) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            api_version: DEFAULT_API_VERSION.into(),
            auth,
            model: "default".into(),
            enhanced: None,
            language_format: LanguageFormat::Bcp47,
            candidate_locales: true,
            phrase_list: true,
            prompt: false,
            client,
            limits: RowLimits::default(),
        }
    }

    /// `MAI-Transcribe-2` or `MAI-Transcribe-1.5`: one ISO-style locale, phrase list, no prompt.
    pub fn mai(endpoint: &str, auth: Auth, model: &str, client: reqwest::Client) -> Self {
        Self {
            model: model.to_string(),
            enhanced: Some(AzureEnhancedMode {
                model: Some(model.to_string()),
            }),
            language_format: LanguageFormat::Iso639_1,
            candidate_locales: false,
            ..Self::default_model(endpoint, auth, client)
        }
    }

    /// LLM Speech: enhanced mode with no model, BCP-47 locales, phrase list and prompt.
    pub fn llm_speech(endpoint: &str, auth: Auth, client: reqwest::Client) -> Self {
        Self {
            model: "llm-speech".into(),
            enhanced: Some(AzureEnhancedMode { model: None }),
            prompt: true,
            ..Self::default_model(endpoint, auth, client)
        }
    }
}

#[derive(Debug)]
pub struct AzureFastTranscriber {
    cfg: AzureFastConfig,
    url: String,
    info: TranscriberInfo,
}

impl AzureFastTranscriber {
    pub fn new(cfg: AzureFastConfig) -> Result<Self, String> {
        if cfg.api_version.trim().is_empty() {
            return Err("an Azure fast transcription target needs an api-version".into());
        }
        let mut url = url::Url::parse(&join_url(
            &cfg.endpoint,
            "/speechtotext/transcriptions:transcribe",
        )?)
        .map_err(|e| e.to_string())?;
        url.query_pairs_mut()
            .append_pair("api-version", cfg.api_version.trim());
        let url = url.to_string();
        let mut droppable = vec!["locales".to_string()];
        if cfg.phrase_list {
            droppable.push("phraseList".into());
        }
        if cfg.prompt && cfg.enhanced.is_some() {
            droppable.push("enhancedMode.prompt".into());
        }
        let info = info_for(
            "azure_fast_transcription",
            &url,
            &cfg.model,
            cfg.limits,
            droppable,
        );
        Ok(Self { cfg, url, info })
    }

    /// The `definition` JSON and the optional fields it carries. `enhancedMode` itself is not
    /// optional: it is what selects an MAI model or LLM Speech.
    fn definition(&self, ctx: &SegmentContext) -> (Value, Vec<String>) {
        let mut def = Map::new();
        let mut sent = Vec::new();
        let locales = languages_to_send(ctx, self.cfg.language_format, self.cfg.candidate_locales);
        if !locales.is_empty() && may_send(ctx, "locales") {
            def.insert("locales".into(), json!(locales));
            sent.push("locales".to_string());
        }
        let phrases: Vec<&str> = ctx
            .keywords
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .collect();
        if self.cfg.phrase_list && !phrases.is_empty() && may_send(ctx, "phraseList") {
            def.insert("phraseList".into(), json!({ "phrases": phrases }));
            sent.push("phraseList".to_string());
        }
        if let Some(mode) = &self.cfg.enhanced {
            let mut enhanced = Map::new();
            enhanced.insert("enabled".into(), json!(true));
            if let Some(model) = &mode.model {
                enhanced.insert("model".into(), json!(model));
            }
            if self.cfg.prompt
                && let Some(p) = ctx
                    .prompt
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                && may_send(ctx, "enhancedMode.prompt")
            {
                enhanced.insert("prompt".into(), json!([p]));
                sent.push("enhancedMode.prompt".to_string());
            }
            def.insert("enhancedMode".into(), Value::Object(enhanced));
        }
        (Value::Object(def), sent)
    }

    fn parse(&self, ex: &Exchanged) -> Result<SegmentTranscript, SegmentError> {
        let v = ex.json()?;
        let combined = v.get("combinedPhrases").and_then(Value::as_array);
        let phrases = v.get("phrases").and_then(Value::as_array);
        if combined.is_none() && phrases.is_none() {
            return Err(ex.not_a_transcript("it has neither combinedPhrases nor phrases"));
        }
        let text = combined
            .and_then(|c| c.first())
            .and_then(|p| p.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let phrases = phrases.map(Vec::as_slice).unwrap_or_default();
        let (mut weighted, mut weight) = (0.0f64, 0.0f64);
        for p in phrases {
            let conf = p.get("confidence").and_then(Value::as_f64).unwrap_or(0.0);
            let dur = p
                .get("durationMilliseconds")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(1.0);
            if conf > 0.0 {
                weighted += conf * dur;
            }
            weight += dur;
        }
        let longest = phrases.iter().max_by_key(|p| {
            p.get("durationMilliseconds")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        });
        Ok(SegmentTranscript {
            text,
            vendor_confidence: (weighted > 0.0).then(|| (weighted / weight) as f32),
            detected_language: longest
                .and_then(|p| p.get("locale"))
                .and_then(Value::as_str)
                .and_then(detected_language),
            vendor_said_no_speech: phrases.is_empty(),
            vendor_request_id: request_id(
                &ex.headers,
                &["apim-request-id", "x-requestid", "x-ms-request-id"],
            ),
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for AzureFastTranscriber {
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
        let (definition, sent) = self.definition(ctx);
        let file = Part::bytes(wav)
            .file_name("segment.wav")
            .mime_str("audio/wav")
            .expect("audio/wav is a valid MIME type");
        let form = Form::new()
            .part("audio", file)
            .text("definition", definition.to_string());
        let req = self
            .cfg
            .auth
            .apply(self.cfg.client.post(&self.url).multipart(form))?;
        let ex = exchange(req, timeout, progress).await?;
        if !ex.is_success() {
            let failure = Failure {
                vendor: "azure-speech",
                exchanged: &ex,
                sent_optional: &sent,
                secret: self.cfg.auth.secret(),
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

    fn config(base: &str) -> AzureFastConfig {
        AzureFastConfig::default_model(base, Auth::azure_speech(SECRET), kit::client())
    }

    fn build(cfg: AzureFastConfig) -> Arc<dyn SegmentTranscriber> {
        Arc::new(AzureFastTranscriber::new(cfg).unwrap())
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("en_us".into()),
            keywords: vec!["Acme".into(), "Zed".into()],
            prompt: Some("Spell product names as written.".into()),
            ..Default::default()
        }
    }

    fn definition(r: &kit::Recorded) -> Value {
        serde_json::from_str(&r.text("definition").unwrap()).unwrap()
    }

    const SUCCESS: &str = r#"{"durationMilliseconds":2000,"combinedPhrases":[{"text":"Weather is nice."}],"phrases":[
        {"offsetMilliseconds":40,"durationMilliseconds":1500,"text":"Weather","locale":"en-US","confidence":0.8},
        {"offsetMilliseconds":1600,"durationMilliseconds":500,"text":"is nice.","locale":"en-US","confidence":0.4}]}"#;

    fn harness() -> Harness {
        Harness {
            build: |base| build(config(base)),
            ctx: ctx(),
            success: Reply::json(200, SUCCESS).header("apim-request-id", "apim-9"),
            unauthorized: Reply::json(
                401,
                r#"{"error":{"code":"401","message":"Access denied due to invalid subscription key or wrong API endpoint."}}"#,
            ),
            model_missing: Reply::json(
                404,
                r#"{"error":{"code":"404","message":"Resource not found"}}"#,
            ),
            rate_limited: Reply::json(
                429,
                r#"{"error":{"code":"429","message":"Rate limit is exceeded."}}"#,
            )
            .header("retry-after", "2"),
            unavailable: Reply::json(
                503,
                r#"{"error":{"code":"ServiceUnavailable","message":"Try again."}}"#,
            ),
            refused: Reply::json(
                400,
                r#"{"error":{"code":"InvalidArgument","message":"Invalid locales: the locale is not supported."}}"#,
            ),
            refused_field: "locales",
        }
    }

    contract_tests!(super::harness);

    #[tokio::test]
    async fn the_default_model_gets_a_locale_list_and_a_phrase_list() {
        let vendor =
            MockVendor::start(Reply::json(200, SUCCESS).header("apim-request-id", "apim-9")).await;
        let t = build(config(&vendor.base));
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();

        let r = vendor.last();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/speechtotext/transcriptions:transcribe");
        assert_eq!(r.query_values("api-version"), vec!["2025-10-15"]);
        assert_eq!(
            r.header("ocp-apim-subscription-key").as_deref(),
            Some(SECRET)
        );
        assert!(r.header("authorization").is_none());
        assert_eq!(r.part_names(), vec!["audio", "definition"]);
        let audio = r.part("audio").unwrap();
        assert_eq!(audio.content_type.as_deref(), Some("audio/wav"));
        assert_eq!(&audio.data[..], &kit::audio().wav()[..]);
        assert_eq!(
            definition(&r),
            json!({"locales": ["en-US"], "phraseList": {"phrases": ["Acme", "Zed"]}})
        );

        assert_eq!(out.text, "Weather is nice.");
        // (0.8 * 1500 + 0.4 * 500) / 2000.
        assert!((out.vendor_confidence.unwrap() - 0.7).abs() < 1e-6);
        assert_eq!(out.detected_language.as_deref(), Some("en"));
        assert!(!out.vendor_said_no_speech);
        assert_eq!(out.vendor_request_id.as_deref(), Some("apim-9"));
    }

    #[tokio::test]
    async fn mai_is_selected_in_the_definition_with_one_iso_locale() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(AzureFastConfig::mai(
            &vendor.base,
            Auth::azure_speech(SECRET),
            "MAI-Transcribe-2",
            kit::client(),
        ));
        kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(
            definition(&vendor.last()),
            json!({"locales": ["en"], "phraseList": {"phrases": ["Acme", "Zed"]}, "enhancedMode": {"enabled": true, "model": "MAI-Transcribe-2"}})
        );

        // Unsure of the language: MAI gets none rather than a guess.
        let unsure = SegmentContext {
            candidate_languages: vec!["en".into(), "es".into()],
            ..Default::default()
        };
        kit::run(t.as_ref(), &unsure).await.0.unwrap();
        assert_eq!(
            definition(&vendor.last()),
            json!({"enhancedMode": {"enabled": true, "model": "MAI-Transcribe-2"}})
        );
    }

    #[tokio::test]
    async fn llm_speech_takes_candidates_and_a_prompt_and_its_zero_confidence_is_none() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"combinedPhrases":[{"text":"Hola."}],"phrases":[{"durationMilliseconds":800,"text":"Hola.","locale":"es-ES","confidence":0}]}"#,
        ))
        .await;
        let t = build(AzureFastConfig::llm_speech(
            &vendor.base,
            Auth::Bearer("entra-token-123456".into()),
            kit::client(),
        ));
        let ctx = SegmentContext {
            candidate_languages: vec!["en-us".into(), "es-ES".into()],
            prompt: Some("Spell names.".into()),
            ..Default::default()
        };
        let out = kit::run(t.as_ref(), &ctx).await.0.unwrap();
        let r = vendor.last();
        assert_eq!(
            r.header("authorization").as_deref(),
            Some("Bearer entra-token-123456")
        );
        assert!(r.header("ocp-apim-subscription-key").is_none());
        assert_eq!(
            definition(&r),
            json!({"locales": ["en-US", "es-ES"], "enhancedMode": {"enabled": true, "prompt": ["Spell names."]}})
        );
        assert_eq!(out.vendor_confidence, None);
        assert_eq!(out.detected_language.as_deref(), Some("es"));
    }

    #[tokio::test]
    async fn an_empty_phrase_list_is_the_vendor_saying_no_speech() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"durationMilliseconds":900,"combinedPhrases":[],"phrases":[]}"#,
        ))
        .await;
        let out = kit::run(build(config(&vendor.base)).as_ref(), &ctx())
            .await
            .0
            .unwrap();
        assert_eq!(out.text, "");
        assert!(out.vendor_said_no_speech);
        assert_eq!(out.vendor_confidence, None);
    }

    #[tokio::test]
    async fn minimal_keeps_only_what_selects_the_model() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let minimal = SegmentContext {
            minimal: true,
            ..ctx()
        };
        kit::run(build(config(&vendor.base)).as_ref(), &minimal)
            .await
            .0
            .unwrap();
        assert_eq!(vendor.last().part_names(), vec!["audio", "definition"]);
        assert_eq!(definition(&vendor.last()), json!({}));
        let mai = build(AzureFastConfig::mai(
            &vendor.base,
            Auth::azure_speech(SECRET),
            "MAI-Transcribe-1.5",
            kit::client(),
        ));
        kit::run(mai.as_ref(), &minimal).await.0.unwrap();
        assert_eq!(
            definition(&vendor.last()),
            json!({"enhancedMode": {"enabled": true, "model": "MAI-Transcribe-1.5"}})
        );
    }

    #[tokio::test]
    async fn an_omitted_field_is_left_out() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let omit = SegmentContext {
            omit_fields: vec!["phraseList".into()],
            ..ctx()
        };
        kit::run(build(config(&vendor.base)).as_ref(), &omit)
            .await
            .0
            .unwrap();
        assert_eq!(definition(&vendor.last()), json!({"locales": ["en-US"]}));
    }

    #[test]
    fn info_and_url() {
        let t = AzureFastTranscriber::new(AzureFastConfig::llm_speech(
            "https://eastus.api.cognitive.microsoft.com/",
            Auth::azure_speech("k"),
            kit::client(),
        ))
        .unwrap();
        kit::info_is_a_file_target(
            &t,
            "azure_fast_transcription",
            "https://eastus.api.cognitive.microsoft.com",
        );
        assert_eq!(
            t.url,
            "https://eastus.api.cognitive.microsoft.com/speechtotext/transcriptions:transcribe?api-version=2025-10-15"
        );
        assert_eq!(
            t.info().droppable_fields,
            vec!["locales", "phraseList", "enhancedMode.prompt"]
        );
        assert_eq!(t.info().model, "llm-speech");
    }
}
