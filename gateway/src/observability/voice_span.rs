//! Recording one voice call on its spans (FRD-021 §6.1, §6.7, §6.8).
//!
//! A `/v1/audio/*` call has two spans: the HTTP SERVER root that `request_id_middleware` opens,
//! and the `voice.turn` child the handler opens. `voice.turn` is the analytics source — budmetrics'
//! ingest keys on its name — and the root is what the trace listing shows and filters on. The
//! attribution, units, cost and error class they share are written through [`VoiceSpans`] so the
//! two cannot disagree: one call records both, and the root takes exactly the keys in
//! [`ROOT_MIRRORED`], which [`server_root_span`] declares.
//!
//! Every field is DECLARED where its span is created. `Span::record` on an undeclared field is a
//! silent no-op (FRD-021 GT-3); the tests below pin that both constructors declare what is
//! recorded on them.

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::Response;
use tracing::Span;

use crate::core::voice_error::VoiceErrorType;
use crate::observability::trace_redact;
use crate::observability::vendor_span::{self, VendorScope};
use crate::observability::voice_attrs::{leg, turn};

/// OTel HTTP semantic-convention names the SERVER root carries.
pub mod http {
    pub const REQUEST_METHOD: &str = "http.request.method";
    pub const ROUTE: &str = "http.route";
    pub const URL_PATH: &str = "url.path";
    pub const URL_SCHEME: &str = "url.scheme";
    pub const USER_AGENT: &str = "user_agent.original";
    pub const SERVER_ADDRESS: &str = "server.address";
    pub const RESPONSE_STATUS_CODE: &str = "http.response.status_code";
    /// The HTTP status code, for responses ≥ 400.
    pub const ERROR_TYPE: &str = "error.type";
    /// FRD-021 §6.8: the platform's content-capture attributes, as budgateway sets them.
    pub const REQUEST_BODY: &str = "http.request.body";
    pub const RESPONSE_BODY: &str = "http.response.body";
}

/// The GenAI names the trace listing's model badge reads (FRD-021 GT-23).
pub mod gen_ai {
    /// The voice vendor.
    pub const PROVIDER_NAME: &str = "gen_ai.provider.name";
    /// The vendor's model.
    pub const REQUEST_MODEL: &str = "gen_ai.request.model";
}

/// What the SERVER root carries with the same value as `voice.turn` (FRD-021 §6.7): the six ids
/// the listing filters on, and the summary it shows.
///
/// Never `gen_ai.inference_id`, `gateway_analytics.*` or `bud.prompt_id`: the InferenceFact stage
/// views key on those regardless of service, and a voice root carrying one would fabricate an LLM
/// row (FRD-021 R-11).
pub const ROOT_MIRRORED: &[&str] = &[
    turn::PROJECT_ID,
    turn::API_KEY_PROJECT_ID,
    turn::ENDPOINT_ID,
    turn::MODEL_ID,
    turn::USER_ID,
    turn::API_KEY_ID,
    turn::CAPABILITY,
    turn::ENDPOINT_NAME,
    turn::CHARACTERS,
    turn::AUDIO_SECONDS,
    turn::COST,
    turn::PRICING_UNIT,
    turn::ERROR_TYPE,
    turn::OUTPUT_AUDIO_SECONDS,
    turn::AUDIO_FORMAT,
];

/// The request's root span when it is the HTTP SERVER span of FRD-021 §6.7, stashed as a request
/// extension by `request_id_middleware` so the handler can record on it.
#[derive(Debug, Clone)]
pub struct RootSpan(pub Span);

/// Open the HTTP SERVER root for one `/v1/audio/*` request.
///
/// `tracing` span names are static, so the exported name comes from `otel.name`, and the kind
/// from `otel.kind` — both honoured by `tracing-opentelemetry` 0.30 (`layer.rs`, `SPAN_NAME_FIELD`
/// / `SPAN_KIND_FIELD`), the same mechanism as the `otel.status_*` fields. The kind is read when
/// the span is built, so it is set here and never recorded later.
pub fn server_root_span(
    method: &str,
    route: &'static str,
    path: &str,
    scheme: &str,
    request_id: &str,
) -> Span {
    let name = format!("{method} {route}");
    tracing::info_span!(
        "http.server",
        otel.name = name.as_str(),
        otel.kind = "server",
        { http::REQUEST_METHOD } = method,
        { http::ROUTE } = route,
        { http::URL_PATH } = path,
        { http::URL_SCHEME } = scheme,
        { http::USER_AGENT } = tracing::field::Empty,
        { http::SERVER_ADDRESS } = tracing::field::Empty,
        request_id = request_id,
        { http::RESPONSE_STATUS_CODE } = tracing::field::Empty,
        { http::ERROR_TYPE } = tracing::field::Empty,
        otel.status_code = tracing::field::Empty,
        otel.status_message = tracing::field::Empty,
        { turn::PROJECT_ID } = tracing::field::Empty,
        { turn::API_KEY_PROJECT_ID } = tracing::field::Empty,
        { turn::ENDPOINT_ID } = tracing::field::Empty,
        { turn::MODEL_ID } = tracing::field::Empty,
        { turn::USER_ID } = tracing::field::Empty,
        { turn::API_KEY_ID } = tracing::field::Empty,
        { turn::CAPABILITY } = tracing::field::Empty,
        { turn::ENDPOINT_NAME } = tracing::field::Empty,
        { turn::CHARACTERS } = tracing::field::Empty,
        { turn::AUDIO_SECONDS } = tracing::field::Empty,
        { turn::COST } = tracing::field::Empty,
        { turn::PRICING_UNIT } = tracing::field::Empty,
        { turn::ERROR_TYPE } = tracing::field::Empty,
        { turn::OUTPUT_AUDIO_SECONDS } = tracing::field::Empty,
        { turn::AUDIO_FORMAT } = tracing::field::Empty,
        { gen_ai::PROVIDER_NAME } = tracing::field::Empty,
        { gen_ai::REQUEST_MODEL } = tracing::field::Empty,
        { http::REQUEST_BODY } = tracing::field::Empty,
        { http::RESPONSE_BODY } = tracing::field::Empty
    )
}

/// Record the response on the SERVER root, per the OTel HTTP server conventions: the status
/// code; `error.type` = the status code for anything ≥ 400; span status ERROR only for 5xx — a
/// 4xx is the client's error, not the server's.
pub fn record_http_outcome(span: &Span, status: StatusCode) {
    span.record(http::RESPONSE_STATUS_CODE, i64::from(status.as_u16()));
    if status.as_u16() >= 400 {
        span.record(http::ERROR_TYPE, status.as_str());
    }
    if status.is_server_error() {
        span.record("otel.status_code", "ERROR");
    }
}

/// The root side of one voice call: where its bodies go.
///
/// Empty when the request has no SERVER root (`WAAV_HTTP_SERVER_SPAN=false`), in which case
/// nothing is captured and nothing is mirrored — the INTERNAL `request` span declares none of it.
#[derive(Debug, Clone, Default)]
pub struct Root(Option<Span>);

impl Root {
    pub fn new(span: Option<Span>) -> Self {
        Self(span)
    }

    /// From the handler's optional extractor.
    pub fn from_extension(ext: Option<axum::Extension<RootSpan>>) -> Self {
        Self(ext.map(|axum::Extension(RootSpan(span))| span))
    }

    pub fn span(&self) -> Option<&Span> {
        self.0.as_ref()
    }

    /// Whether a body recorded now would be kept: there is a root, and content capture is on.
    /// Lets a caller skip building a body nobody will record.
    pub fn captures(&self) -> bool {
        self.0.is_some() && trace_redact::capture_content()
    }

    /// Record the request as the caller sent it — redacted and size-capped, and only when
    /// content capture is on (FRD-021 §6.8).
    pub fn record_request_body(&self, body: &str) {
        if let Some(span) = &self.0
            && trace_redact::capture_content()
        {
            span.record(
                http::REQUEST_BODY,
                trace_redact::sanitize_body(body).as_str(),
            );
        }
    }

    /// Record the response returned to the caller, and hand it back unchanged.
    ///
    /// An error response is always recorded (it is the error JSON the caller was shown);
    /// a success only when `with_success_body` — the transcript, for STT. A TTS success is
    /// audio and is never captured (NG-3).
    pub async fn capture_response(&self, response: Response, with_success_body: bool) -> Response {
        let Some(span) = &self.0 else {
            return response;
        };
        if !trace_redact::capture_content()
            || (response.status().is_success() && !with_success_body)
        {
            return response;
        }
        let (parts, body) = response.into_parts();
        // Every body on these routes is built in memory (JSON, text, a subtitle file), so
        // buffering it again costs a copy of what is already held, not a stream drained.
        match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => {
                span.record(
                    http::RESPONSE_BODY,
                    trace_redact::sanitize_body(&String::from_utf8_lossy(&bytes)).as_str(),
                );
                Response::from_parts(parts, Body::from(bytes))
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not buffer a voice response to capture it");
                Response::from_parts(parts, Body::empty())
            }
        }
    }
}

/// An STT request as captured: the multipart fields as sent, and the file as metadata — never
/// its bytes (FRD-021 §6.8).
///
/// Recorded on the root when dropped, so every exit from the handler records what had been read,
/// the refusals included, without a record call at each of them.
pub struct FormCapture<'a> {
    root: &'a Root,
    fields: serde_json::Map<String, serde_json::Value>,
    file: Option<serde_json::Value>,
}

impl<'a> FormCapture<'a> {
    pub fn new(root: &'a Root) -> Self {
        Self {
            root,
            fields: serde_json::Map::new(),
            file: None,
        }
    }

    /// One text field. A repeated field (`timestamp_granularities[]`) becomes an array.
    pub fn field(&mut self, name: &str, value: &str) {
        match self.fields.get_mut(name) {
            Some(serde_json::Value::Array(items)) => items.push(value.into()),
            Some(existing) => {
                let first = existing.take();
                *existing = serde_json::Value::Array(vec![first, value.into()]);
            }
            None => {
                self.fields.insert(name.to_string(), value.into());
            }
        }
    }

    /// The uploaded file, described.
    pub fn file(&mut self, filename: &str, content_type: Option<&str>, bytes: usize) {
        self.file = Some(serde_json::json!({
            "filename": filename,
            "content_type": content_type,
            "bytes": bytes,
        }));
    }
}

impl Drop for FormCapture<'_> {
    fn drop(&mut self) {
        if !self.root.captures() {
            return;
        }
        let mut body = std::mem::take(&mut self.fields);
        if let Some(file) = self.file.take() {
            body.insert("file".to_string(), file);
        }
        self.root
            .record_request_body(&serde_json::Value::Object(body).to_string());
    }
}

/// Which vendor leg an HTTP call runs: the names its vendor, model and voice are recorded under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegKind {
    Tts,
    Stt,
}

impl LegKind {
    fn vendor_key(self) -> &'static str {
        match self {
            Self::Tts => leg::TTS_VENDOR,
            Self::Stt => leg::STT_VENDOR,
        }
    }

    fn model_key(self) -> &'static str {
        match self {
            Self::Tts => leg::TTS_MODEL,
            Self::Stt => leg::STT_MODEL,
        }
    }
}

/// The deployment a call is attributed to, as its leg attributes describe it.
///
/// Held on [`VoiceSpans`] and written ONCE, when the call ends — never as the call goes. A fallback
/// chain (FRD-022 §6.4) learns which deployment served only at its end, and recording the primary
/// up front and the served hop again is not an overwrite: `tracing-opentelemetry` appends a second
/// `KeyValue` for a re-recorded field, the exporter ships both, and ClickHouse's
/// `SpanAttributes['<key>']` reads whichever map entry won — live rows paired one hop's vendor
/// with the other's model. So the handler names the leg as it learns it ([`VoiceSpans::set_leg`],
/// last write wins) and the span receives one value per key: the SERVED deployment on success, the
/// primary when every hop failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Leg {
    pub vendor: String,
    /// The model the vendor was CALLED with — the deployment's own, or the one its settings
    /// substitute (`stt.model`) — not merely the one the deployment was saved with.
    pub model: Option<String>,
    /// TTS: the voice the synthesis ran with. Ignored for STT.
    pub voice: Option<String>,
    /// The canonical language the leg ran in (`bud.voice.language`).
    pub language: Option<String>,
    /// STT: whether the upload went through WaaV's denoiser. `None` when the leg never reached
    /// that step (a passthrough deployment, an open breaker). Ignored for TTS.
    pub noise_suppression: Option<bool>,
}

/// The spans of one HTTP voice call: `voice.turn`, and the SERVER root when there is one.
pub struct VoiceSpans {
    turn: Span,
    root: Option<Span>,
    /// The attribution ids as recorded, for the vendor spans beneath the turn (CONTRACTS §1.2a):
    /// a `tracing` span cannot be read back, so the values are kept as they are written.
    ids: std::sync::Mutex<Vec<(&'static str, String)>>,
    /// The leg to record when the call ends; see [`Leg`].
    leg: std::sync::Mutex<Option<(LegKind, Leg)>>,
}

impl VoiceSpans {
    /// Open `voice.turn` for an HTTP call, beneath the current span (the root).
    pub fn open(capability: &'static str, root: &Root) -> Self {
        let turn = crate::voice_turn_span!(capability = capability, transport = "http");
        if let Some(r) = root.span() {
            r.record(turn::CAPABILITY, capability);
        }
        Self {
            turn,
            root: root.span().cloned(),
            ids: std::sync::Mutex::new(Vec::new()),
            leg: std::sync::Mutex::new(None),
        }
    }

    /// `voice.turn`, for instrumenting the vendor work and recording leg-only attributes.
    pub fn turn(&self) -> &Span {
        &self.turn
    }

    /// The scope a vendor request of this call runs in: its spans are children of `voice.turn`
    /// and carry the ids recorded here (CONTRACTS §1.2a).
    pub fn vendor_scope(&self, operation: &'static str) -> VendorScope {
        let ids = self.ids.lock().map(|ids| ids.clone()).unwrap_or_default();
        ids.iter().fold(
            VendorScope::new(self.turn.clone(), operation),
            |scope, (key, value)| scope.with_id(key, value),
        )
    }

    /// Record on `voice.turn`, and on the root when it is one of [`ROOT_MIRRORED`].
    pub fn record<V: tracing::Value + Copy>(&self, key: &'static str, value: V) {
        self.turn.record(key, value);
        if let Some(root) = &self.root
            && ROOT_MIRRORED.contains(&key)
        {
            root.record(key, value);
        }
    }

    /// [`VoiceSpans::record`] for text, skipping an absent or empty value: an empty string is
    /// not NULL, and makes a column look populated (FRD-021 FR-5).
    pub fn record_text(&self, key: &'static str, value: Option<&str>) {
        if let Some(v) = waav_openai_audio::recordable(value) {
            self.record(key, v);
            if vendor_span::IDS.contains(&key)
                && let Ok(mut ids) = self.ids.lock()
            {
                ids.retain(|(k, _)| *k != key);
                ids.push((key, v.to_string()));
            }
        }
    }

    /// Name the leg the call is attributed to, replacing whatever was named before (see [`Leg`]).
    /// Nothing is recorded until the call ends.
    pub fn set_leg(&self, kind: LegKind, leg: Leg) {
        *self.leg.lock().unwrap_or_else(|e| e.into_inner()) = Some((kind, leg));
    }

    /// Amend the leg named so far — the primary's plan has resolved its voice, say.
    pub fn update_leg(&self, update: impl FnOnce(&mut Leg)) {
        if let Some((_, leg)) = self.leg.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            update(leg);
        }
    }

    /// Write the leg, once: the vendor and its model under the leg's own names on `voice.turn`,
    /// the GenAI names on the root so the listing's model badge renders them (FRD-021 §6.7), and
    /// the voice, language and denoiser flag beside them.
    fn record_leg(&self) {
        let Some((kind, l)) = self.leg.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        if let Some(v) = waav_openai_audio::recordable(Some(l.vendor.as_str())) {
            self.turn.record(kind.vendor_key(), v);
            if let Some(root) = &self.root {
                root.record(gen_ai::PROVIDER_NAME, v);
            }
        }
        if let Some(m) = waav_openai_audio::recordable(l.model.as_deref()) {
            self.turn.record(kind.model_key(), m);
            if let Some(root) = &self.root {
                root.record(gen_ai::REQUEST_MODEL, m);
            }
        }
        if kind == LegKind::Tts {
            self.record_text(leg::TTS_VOICE, l.voice.as_deref());
        }
        // Recorded only when there is one: `""` is not NULL (FRD-021 GT-12).
        self.record_text(turn::LANGUAGE, l.language.as_deref());
        if kind == LegKind::Stt
            && let Some(applied) = l.noise_suppression
        {
            self.turn.record(leg::STT_NOISE_SUPPRESSION, applied);
        }
    }

    /// The cost and the unit it was computed from — both, or neither (FRD-021 §6.4).
    pub fn record_cost(&self, cost: Option<(f64, &'static str)>) {
        if let Some((cost, unit)) = cost {
            self.record(turn::COST, cost);
            self.record(turn::PRICING_UNIT, unit);
        }
    }

    /// Mark the call failed: span status ERROR with the message on `voice.turn`, the error class
    /// on both spans, and the vendor's status when a vendor response caused it (FRD-021 FR-4).
    ///
    /// The root's own status is left to the HTTP outcome: ERROR there means a 5xx, and a vendor's
    /// refusal answered 400 is a failed call but not a failed server.
    pub fn fail(&self, class: VoiceErrorType, vendor_status: Option<u16>, message: &str) {
        let message = if message.trim().is_empty() {
            class.as_str()
        } else {
            message
        };
        self.turn.record("otel.status_code", "ERROR");
        self.turn.record("otel.status_message", message);
        self.record(turn::ERROR_TYPE, class.as_str());
        if let Some(status) = vendor_status {
            self.turn
                .record(turn::VENDOR_STATUS_CODE, i64::from(status));
        }
    }
}

/// The call has ended, whichever `return` ended it: its leg is written now, once. The span is still
/// open here — `turn` is dropped only after this runs — and so is the root, which the middleware
/// closes after the handler returns.
impl Drop for VoiceSpans {
    fn drop(&mut self) {
        self.record_leg();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;

    /// `span name -> field -> value`, for spans created and recorded on this thread.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<HashMap<String, HashMap<String, String>>>>);

    struct V<'a>(&'a mut HashMap<String, String>);
    impl Visit for V<'_> {
        fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
            self.0.insert(f.name().to_string(), format!("{v:?}"));
        }
        fn record_str(&mut self, f: &Field, v: &str) {
            self.0.insert(f.name().to_string(), v.to_string());
        }
    }

    struct Capture(Seen);
    impl<S> Layer<S> for Capture
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
            let name = ctx
                .span(id)
                .map(|s| s.name().to_string())
                .unwrap_or_default();
            let mut all = self.0.0.lock().unwrap();
            attrs.record(&mut V(all.entry(name).or_default()));
        }
        fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
            let name = ctx
                .span(id)
                .map(|s| s.name().to_string())
                .unwrap_or_default();
            let mut all = self.0.0.lock().unwrap();
            values.record(&mut V(all.entry(name).or_default()));
        }
    }

    fn with_capture(f: impl FnOnce()) -> HashMap<String, HashMap<String, String>> {
        let seen = Seen::default();
        let subscriber = tracing_subscriber::registry().with(Capture(seen.clone()));
        tracing::subscriber::with_default(subscriber, f);
        seen.0.lock().unwrap().clone()
    }

    /// The guard behind "the two spans cannot diverge": every key `VoiceSpans` mirrors, and every
    /// root-only key, is declared by the root's constructor — so recording it is never the
    /// silent no-op of GT-3.
    #[test]
    fn the_server_root_declares_everything_recorded_on_it() {
        let seen = with_capture(|| {
            let root = server_root_span(
                "POST",
                "/v1/audio/speech",
                "/v1/audio/speech",
                "http",
                "r-1",
            );
            for key in ROOT_MIRRORED {
                root.record(*key, "x");
            }
            for key in [
                http::USER_AGENT,
                http::SERVER_ADDRESS,
                http::REQUEST_BODY,
                http::RESPONSE_BODY,
                gen_ai::PROVIDER_NAME,
                gen_ai::REQUEST_MODEL,
            ] {
                root.record(key, "x");
            }
            record_http_outcome(&root, StatusCode::BAD_GATEWAY);
        });
        let root = &seen["http.server"];
        let mut missing: Vec<&str> = ROOT_MIRRORED
            .iter()
            .copied()
            .chain([
                http::USER_AGENT,
                http::SERVER_ADDRESS,
                http::REQUEST_BODY,
                http::RESPONSE_BODY,
                gen_ai::PROVIDER_NAME,
                gen_ai::REQUEST_MODEL,
                http::RESPONSE_STATUS_CODE,
                http::ERROR_TYPE,
                "otel.status_code",
            ])
            .filter(|k| !root.contains_key(*k))
            .collect();
        missing.sort_unstable();
        assert!(
            missing.is_empty(),
            "the SERVER root does not declare {missing:?}; recording them is a silent no-op"
        );
        assert_eq!(root["otel.name"], "POST /v1/audio/speech");
        assert_eq!(root["otel.kind"], "server");
        assert_eq!(root[http::ERROR_TYPE], "502");
        assert_eq!(root["otel.status_code"], "ERROR");
    }

    #[test]
    fn a_4xx_is_an_error_type_but_not_an_error_status() {
        let seen = with_capture(|| {
            let root =
                server_root_span("POST", "/v1/audio/speech", "/v1/audio/speech", "http", "r");
            record_http_outcome(&root, StatusCode::BAD_REQUEST);
        });
        let root = &seen["http.server"];
        assert_eq!(root[http::ERROR_TYPE], "400");
        assert!(!root.contains_key("otel.status_code"));
        let ok = with_capture(|| {
            let root =
                server_root_span("POST", "/v1/audio/speech", "/v1/audio/speech", "http", "r");
            record_http_outcome(&root, StatusCode::OK);
        });
        assert!(!ok["http.server"].contains_key(http::ERROR_TYPE));
    }

    /// One call writes both spans, and they agree; a leg-only key stays on the child.
    #[test]
    fn voice_spans_mirror_exactly_the_shared_keys() {
        let seen = with_capture(|| {
            let root_span =
                server_root_span("POST", "/v1/audio/speech", "/v1/audio/speech", "http", "r");
            let _entered = root_span.enter();
            let root = Root::new(Some(root_span.clone()));
            let spans = VoiceSpans::open("text_to_speech", &root);
            spans.record_text(turn::ENDPOINT_ID, Some("ep-uuid"));
            spans.record_text(turn::ENDPOINT_NAME, Some(""));
            spans.record(turn::CHARACTERS, 120u64);
            spans.record_cost(Some((0.0036, "character")));
            spans.set_leg(
                LegKind::Tts,
                Leg {
                    vendor: "elevenlabs".into(),
                    model: Some("eleven_v3".into()),
                    ..Leg::default()
                },
            );
            spans.fail(VoiceErrorType::RateLimited, Some(429), "slow down");
        });
        let turn_fields = &seen["voice.turn"];
        let root = &seen["http.server"];
        for key in [
            turn::CAPABILITY,
            turn::ENDPOINT_ID,
            turn::CHARACTERS,
            turn::COST,
            turn::PRICING_UNIT,
            turn::ERROR_TYPE,
        ] {
            assert_eq!(turn_fields.get(key), root.get(key), "{key} differs");
            assert!(root.contains_key(key), "{key} missing on the root");
        }
        // FR-5: an empty endpoint name is not recorded on either.
        assert!(!turn_fields.contains_key(turn::ENDPOINT_NAME));
        assert!(!root.contains_key(turn::ENDPOINT_NAME));
        // The leg's names on the child, the GenAI names on the root.
        assert_eq!(
            turn_fields[crate::observability::voice_attrs::leg::TTS_VENDOR],
            "elevenlabs"
        );
        assert_eq!(root[gen_ai::PROVIDER_NAME], "elevenlabs");
        assert_eq!(root[gen_ai::REQUEST_MODEL], "eleven_v3");
        // The failure: status on the child, class on both, vendor status on the child only.
        assert_eq!(turn_fields["otel.status_code"], "ERROR");
        assert_eq!(turn_fields["otel.status_message"], "slow down");
        assert_eq!(turn_fields[turn::VENDOR_STATUS_CODE], "429");
        assert!(!root.contains_key("otel.status_code"));
        assert!(!root.contains_key(turn::VENDOR_STATUS_CODE));
    }

    /// The spans as EXPORTED — `tracing-opentelemetry` over the `Registry`, into a real SDK
    /// provider — because the defect is in what that layer does with a re-recorded field.
    mod exported {
        use std::future::Future;
        use std::sync::{Arc, Mutex};

        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::error::OTelSdkResult;
        use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Debug, Clone, Default)]
        struct Collect(Arc<Mutex<Vec<SpanData>>>);

        impl SpanExporter for Collect {
            fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
                self.0.lock().unwrap().extend(batch);
                std::future::ready(Ok(()))
            }
        }

        /// Run `f` under the production span pipeline and return every span it finished.
        pub fn run(f: impl FnOnce()) -> Vec<SpanData> {
            let collect = Collect::default();
            let provider = SdkTracerProvider::builder()
                .with_simple_exporter(collect.clone())
                .build();
            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("voice-span")));
            tracing::subscriber::with_default(subscriber, f);
            std::mem::take(&mut *collect.0.lock().unwrap())
        }

        pub fn named<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
            spans
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no `{name}` span exported"))
        }

        /// Every value the exported span carries under `key`, in order.
        pub fn values(span: &SpanData, key: &str) -> Vec<String> {
            span.attributes
                .iter()
                .filter(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.as_str().into_owned())
                .collect()
        }
    }

    fn tts_leg(vendor: &str, model: Option<&str>, voice: &str) -> Leg {
        Leg {
            vendor: vendor.into(),
            model: model.map(Into::into),
            voice: Some(voice.into()),
            language: Some("en".into()),
            noise_suppression: None,
        }
    }

    /// The premise of [`Leg`], pinned: a field recorded twice is exported twice. If the layer ever
    /// starts overwriting, recording the leg as the call goes would be safe again and this says so.
    #[test]
    fn a_re_recorded_field_is_exported_twice() {
        let spans = exported::run(|| {
            let span = crate::voice_turn_span!(capability = "text_to_speech", transport = "http");
            span.record(leg::TTS_VENDOR, "elevenlabs");
            span.record(leg::TTS_VENDOR, "deepgram");
        });
        let turn = exported::named(&spans, "voice.turn");
        assert_eq!(
            exported::values(turn, leg::TTS_VENDOR),
            ["elevenlabs", "deepgram"]
        );
    }

    /// A turn a fallback served exports ONE value per leg key — the served deployment's — on the
    /// turn, and one GenAI name each on the root.
    #[test]
    fn a_fallback_served_turn_exports_one_value_per_leg_key() {
        let spans = exported::run(|| {
            let root_span =
                server_root_span("POST", "/v1/audio/speech", "/v1/audio/speech", "http", "r");
            let _entered = root_span.enter();
            let root = Root::new(Some(root_span.clone()));
            let spans = VoiceSpans::open("text_to_speech", &root);
            // The primary, as `open_turn` and the plan name it...
            spans.set_leg(
                LegKind::Tts,
                tts_leg("elevenlabs", Some("eleven_v3"), "George"),
            );
            spans.update_leg(|l| l.language = Some("en-GB".into()));
            // ...then the fallback that served.
            spans.set_leg(
                LegKind::Tts,
                tts_leg("deepgram", Some("aura-2"), "aura-2-thalia-en"),
            );
            spans.record_text(
                crate::observability::voice_attrs::resilience::FALLBACK_FROM,
                Some("ep-primary"),
            );
            spans.record_text(
                crate::observability::voice_attrs::resilience::SERVED_ENDPOINT_ID,
                Some("ep-fallback"),
            );
        });
        let turn = exported::named(&spans, "voice.turn");
        for (key, want) in [
            (leg::TTS_VENDOR, "deepgram"),
            (leg::TTS_MODEL, "aura-2"),
            (leg::TTS_VOICE, "aura-2-thalia-en"),
            (turn::LANGUAGE, "en"),
            (
                crate::observability::voice_attrs::resilience::FALLBACK_FROM,
                "ep-primary",
            ),
            (
                crate::observability::voice_attrs::resilience::SERVED_ENDPOINT_ID,
                "ep-fallback",
            ),
        ] {
            assert_eq!(exported::values(turn, key), [want], "{key}");
        }
        // Exported under its `otel.name`.
        let root = exported::named(&spans, "POST /v1/audio/speech");
        assert_eq!(exported::values(root, gen_ai::PROVIDER_NAME), ["deepgram"]);
        assert_eq!(exported::values(root, gen_ai::REQUEST_MODEL), ["aura-2"]);
    }

    /// A served hop with no model does not inherit the primary's; a TTS voice is not an STT
    /// attribute; the denoiser flag is written once, for the leg that ran.
    #[test]
    fn the_leg_written_is_the_last_one_named_whole() {
        let spans = exported::run(|| {
            let root_span = server_root_span(
                "POST",
                "/v1/audio/transcriptions",
                "/v1/audio/transcriptions",
                "http",
                "r",
            );
            let _entered = root_span.enter();
            let root = Root::new(Some(root_span.clone()));
            let spans = VoiceSpans::open("audio_transcription", &root);
            spans.set_leg(
                LegKind::Stt,
                Leg {
                    vendor: "deepgram".into(),
                    model: Some("nova-3".into()),
                    noise_suppression: Some(false),
                    ..Leg::default()
                },
            );
            spans.set_leg(
                LegKind::Stt,
                Leg {
                    vendor: "elevenlabs".into(),
                    model: None,
                    voice: Some("not-an-stt-attribute".into()),
                    language: None,
                    noise_suppression: Some(true),
                },
            );
        });
        let turn = exported::named(&spans, "voice.turn");
        assert_eq!(exported::values(turn, leg::STT_VENDOR), ["elevenlabs"]);
        assert!(exported::values(turn, leg::STT_MODEL).is_empty());
        assert!(exported::values(turn, leg::TTS_VOICE).is_empty());
        assert!(exported::values(turn, turn::LANGUAGE).is_empty());
        assert_eq!(exported::values(turn, leg::STT_NOISE_SUPPRESSION), ["true"]);
        let root = exported::named(&spans, "POST /v1/audio/transcriptions");
        assert_eq!(
            exported::values(root, gen_ai::PROVIDER_NAME),
            ["elevenlabs"]
        );
        assert!(exported::values(root, gen_ai::REQUEST_MODEL).is_empty());
    }

    /// Every hop failed: the call is attributed to the primary, as amended, once.
    #[test]
    fn a_failed_call_keeps_the_primary_leg() {
        let spans = exported::run(|| {
            let spans = VoiceSpans::open("text_to_speech", &Root::default());
            spans.set_leg(
                LegKind::Tts,
                Leg {
                    vendor: "elevenlabs".into(),
                    model: Some("eleven_v3".into()),
                    ..Leg::default()
                },
            );
            spans.update_leg(|l| l.voice = Some("George".into()));
            spans.fail(VoiceErrorType::Vendor5xx, Some(503), "down");
        });
        let turn = exported::named(&spans, "voice.turn");
        assert_eq!(exported::values(turn, leg::TTS_VENDOR), ["elevenlabs"]);
        assert_eq!(exported::values(turn, leg::TTS_MODEL), ["eleven_v3"]);
        assert_eq!(exported::values(turn, leg::TTS_VOICE), ["George"]);
        assert_eq!(exported::values(turn, turn::ERROR_TYPE), ["vendor_5xx"]);
    }

    #[test]
    fn a_form_is_captured_as_fields_and_file_metadata() {
        let root = Root::default();
        let mut fields = FormCapture::new(&root);
        fields.field("model", "stt-a");
        fields.field("timestamp_granularities[]", "word");
        fields.field("timestamp_granularities[]", "segment");
        fields.file("visit.wav", Some("audio/wav"), 204_800);
        let mut body = std::mem::take(&mut fields.fields);
        body.insert("file".into(), fields.file.take().unwrap());
        let v = serde_json::Value::Object(body);
        assert_eq!(v["model"], "stt-a");
        assert_eq!(
            v["timestamp_granularities[]"],
            serde_json::json!(["word", "segment"])
        );
        assert_eq!(v["file"]["bytes"], 204_800);
        assert_eq!(v["file"]["content_type"], "audio/wav");
    }
}
