//! The vendor call span (FRD-021, CONTRACTS §1.2a): what WaaV sent the provider, and what came back.
//!
//! A `/v1/audio/*` call's trace is the SERVER root, its `voice.turn` child — and, beneath the turn,
//! one CLIENT span per vendor HTTP request, named `{operation} {vendor model}`
//! (`text_to_speech eleven_v3`, `transcription nova-3`). It is the voice counterpart of budgateway's
//! `chat {model}` provider spans, so the trace drawer shows the hop to the vendor the same way for
//! both.
//!
//! # How a vendor call finds its turn
//!
//! The vendor request is made deep inside a provider, which knows the URL, the body and the answer
//! but not the call it belongs to. The handler knows the call: it opens `voice.turn` and resolves
//! the six attribution ids. [`VendorScope`] carries the turn, the operation and the ids from the
//! handler to the provider in a task-local, and [`VendorCall`] opens the span from it.
//!
//! No scope, no span: the `/speak` route, the websocket sessions and every other caller of the same
//! providers are untouched.
//!
//! The TTS provider makes its request on a long-lived worker task, which a task-local does not
//! reach; it captures [`VendorScope::current`] when the synthesis is queued (on the handler's task)
//! and carries it with the job.
//!
//! # What is never recorded
//!
//! Request header values (credentials travel in headers), `gen_ai.inference_id`,
//! `gateway_analytics.*`, `bud.prompt_id` and `gen_ai.usage.*` — the InferenceFact stage views key
//! on those regardless of service, and a vendor span carrying one would fabricate an LLM row.
//! Bodies are recorded only under `BUD_TRACE_CAPTURE_CONTENT`, redacted and size-capped like every
//! other captured body (`trace_redact`), and never contain audio.

use std::future::Future;
use std::time::{Duration, Instant};

use tracing::Span;

use crate::observability::trace_redact;
use crate::observability::voice_attrs::turn;

/// The names a vendor span carries: the OTel GenAI and HTTP-client conventions, as budgateway's
/// provider spans use them.
pub mod attr {
    pub const OPERATION_NAME: &str = "gen_ai.operation.name";
    pub const PROVIDER_NAME: &str = "gen_ai.provider.name";
    pub const REQUEST_MODEL: &str = "gen_ai.request.model";
    /// Content: what WaaV sent. Only under `BUD_TRACE_CAPTURE_CONTENT`.
    pub const REQUEST_BODY: &str = "gen_ai.request.body";
    /// Content: what the vendor answered (a TTS success: the audio described, never stored).
    pub const RESPONSE_BODY: &str = "gen_ai.response.body";
    pub const HTTP_METHOD: &str = "http.request.method";
    pub const SERVER_ADDRESS: &str = "server.address";
    /// The request URL, with any credential in it redacted.
    pub const URL_FULL: &str = "url.full";
    pub const RESPONSE_STATUS_CODE: &str = "http.response.status_code";
    /// The vendor's status code for an HTTP failure; a transport class when no response arrived.
    pub const ERROR_TYPE: &str = "error.type";
}

/// `error.type` for a vendor call abandoned before it completed — the caller's deadline elapsed,
/// or the synthesis was cleared.
pub const CANCELLED: &str = "cancelled";

/// What replaces a credential in `url.full` (the OTel URL convention's placeholder).
const URL_REDACTED: &str = "REDACTED";

/// What replaces a credential in a captured body (`trace_redact`'s placeholder).
const BODY_REDACTED: &str = "***";

/// `gen_ai.operation.name` values (CONTRACTS §1.2a).
pub mod operation {
    pub const TEXT_TO_SPEECH: &str = "text_to_speech";
    pub const TRANSCRIPTION: &str = "transcription";
    pub const TRANSLATION: &str = "translation";
}

/// The six attribution ids a vendor span carries, with the values `voice.turn` has.
pub const IDS: &[&str] = &[
    turn::PROJECT_ID,
    turn::API_KEY_PROJECT_ID,
    turn::ENDPOINT_ID,
    turn::MODEL_ID,
    turn::USER_ID,
    turn::API_KEY_ID,
];

tokio::task_local! {
    static SCOPE: VendorScope;
}

/// The call a vendor request belongs to: its `voice.turn`, the operation, and the attribution ids.
#[derive(Debug, Clone)]
pub struct VendorScope {
    parent: Span,
    operation: &'static str,
    ids: Vec<(&'static str, String)>,
}

impl VendorScope {
    /// A scope whose vendor spans are children of `parent` (the call's `voice.turn`).
    pub fn new(parent: Span, operation: &'static str) -> Self {
        Self {
            parent,
            operation,
            ids: Vec::new(),
        }
    }

    /// One of the six ids of [`IDS`]; anything else, and an empty value, is ignored.
    pub fn with_id(mut self, key: &str, value: &str) -> Self {
        let value = value.trim();
        if let Some(key) = IDS.iter().copied().find(|k| *k == key)
            && !value.is_empty()
        {
            self.ids.retain(|(k, _)| *k != key);
            self.ids.push((key, value.to_string()));
        }
        self
    }

    /// Run `fut` with this scope current, so a vendor request made inside it (on this task) opens
    /// its span beneath the turn.
    pub async fn run<F: Future>(self, fut: F) -> F::Output {
        SCOPE.scope(self, fut).await
    }

    /// The scope of the running task, if the handler set one.
    pub fn current() -> Option<Self> {
        SCOPE.try_with(Clone::clone).ok()
    }

    /// The operation this scope's vendor calls perform.
    pub fn operation(&self) -> &'static str {
        self.operation
    }

    /// The span the vendor spans are children of.
    pub fn parent(&self) -> &Span {
        &self.parent
    }

    /// The ids recorded on every vendor span of this scope.
    pub fn ids(&self) -> &[(&'static str, String)] {
        &self.ids
    }
}

/// One vendor request's CLIENT span (CONTRACTS §1.2a).
///
/// Opened just before the request is sent and ended when the exchange is over — for a synthesis,
/// after the last audio chunk. Every exit ends it with a status: [`VendorCall::finish`] after a
/// response (an HTTP error status already marked it failed), [`VendorCall::transport_error`] when
/// no response arrived, [`VendorCall::fail`] for a failure after one did. A call dropped without
/// any of them — its task aborted by the caller's deadline — ends [`CANCELLED`].
///
/// With no [`VendorScope`] it is disabled: every method is a no-op and nothing is exported.
#[derive(Debug)]
pub struct VendorCall {
    span: Span,
    started: Instant,
    first_byte: Option<Duration>,
    /// The deployment's credential, scrubbed from every string recorded here. Belt and braces:
    /// credentials travel in headers, which are never recorded, but a vendor that takes one in the
    /// body or the query under an unusual name must not put it on a span either.
    secrets: Vec<String>,
    done: bool,
}

impl VendorCall {
    /// A call that records nothing.
    pub fn disabled() -> Self {
        Self {
            span: Span::none(),
            started: Instant::now(),
            first_byte: None,
            secrets: Vec::new(),
            done: true,
        }
    }

    /// Open the span for a request about to be sent, in `scope` — or a disabled call without one.
    ///
    /// `credential` is the key the request authenticates with; it is never recorded, and is
    /// scrubbed from anything that is.
    pub fn open(
        scope: Option<&VendorScope>,
        vendor: &str,
        model: &str,
        method: &str,
        url: &str,
        credential: &str,
    ) -> Self {
        // An explicit parent that is disabled would make this span a ROOT, not a child.
        let Some(scope) = scope.filter(|s| !s.parent.is_disabled()) else {
            return Self::disabled();
        };
        let vendor = vendor.trim();
        let model = model.trim();
        // `{operation} {vendor model}`; a deployment that names no model is named by its vendor.
        let name = match (model.is_empty(), vendor.is_empty()) {
            (false, _) => format!("{} {model}", scope.operation),
            (true, false) => format!("{} {vendor}", scope.operation),
            (true, true) => scope.operation.to_string(),
        };
        let span = tracing::info_span!(
            parent: &scope.parent,
            "voice.vendor_call",
            otel.name = name.as_str(),
            otel.kind = "client",
            { attr::OPERATION_NAME } = scope.operation,
            { attr::PROVIDER_NAME } = tracing::field::Empty,
            { attr::REQUEST_MODEL } = tracing::field::Empty,
            { attr::HTTP_METHOD } = tracing::field::Empty,
            { attr::SERVER_ADDRESS } = tracing::field::Empty,
            { attr::URL_FULL } = tracing::field::Empty,
            { attr::RESPONSE_STATUS_CODE } = tracing::field::Empty,
            { attr::ERROR_TYPE } = tracing::field::Empty,
            { turn::VENDOR_REQUEST_ID } = tracing::field::Empty,
            { turn::PROJECT_ID } = tracing::field::Empty,
            { turn::API_KEY_PROJECT_ID } = tracing::field::Empty,
            { turn::ENDPOINT_ID } = tracing::field::Empty,
            { turn::MODEL_ID } = tracing::field::Empty,
            { turn::USER_ID } = tracing::field::Empty,
            { turn::API_KEY_ID } = tracing::field::Empty,
            { attr::REQUEST_BODY } = tracing::field::Empty,
            { attr::RESPONSE_BODY } = tracing::field::Empty,
            otel.status_code = tracing::field::Empty,
            otel.status_message = tracing::field::Empty
        );
        let call = Self {
            span,
            started: Instant::now(),
            first_byte: None,
            secrets: secrets_of(credential),
            done: false,
        };
        if !vendor.is_empty() {
            call.span.record(attr::PROVIDER_NAME, vendor);
        }
        if !model.is_empty() {
            call.span.record(attr::REQUEST_MODEL, model);
        }
        if !method.is_empty() {
            call.span.record(attr::HTTP_METHOD, method);
        }
        if let Some(host) = server_address(url) {
            call.span.record(attr::SERVER_ADDRESS, host.as_str());
        }
        call.span
            .record(attr::URL_FULL, call.scrub(&redact_url(url)).as_str());
        for (key, value) in &scope.ids {
            call.span.record(*key, value.as_str());
        }
        call
    }

    /// [`VendorCall::open`] in the running task's scope.
    pub fn start(vendor: &str, model: &str, method: &str, url: &str, credential: &str) -> Self {
        Self::open(
            VendorScope::current().as_ref(),
            vendor,
            model,
            method,
            url,
            credential,
        )
    }

    /// Whether a body recorded now would be kept: the span is live and content capture is on. Lets
    /// a caller skip describing a request nobody will record.
    pub fn captures(&self) -> bool {
        !self.span.is_disabled() && trace_redact::capture_content()
    }

    /// The request as sent: scrubbed, redacted and size-capped, under content capture only.
    pub fn request_body(&self, body: &str) {
        if self.captures() {
            self.span
                .record(attr::REQUEST_BODY, self.sanitize(body).as_str());
        }
    }

    /// A request body as the bytes on the wire. A form-encoded body has its credential parameters
    /// masked, as a JSON one has its secret keys; a body that is not text is not recorded.
    pub fn request_body_bytes(&self, bytes: Option<&[u8]>, content_type: Option<&str>) {
        if !self.captures() {
            return;
        }
        let Some(text) = bytes.and_then(|b| std::str::from_utf8(b).ok()) else {
            return;
        };
        let form = content_type
            .is_some_and(|ct| ct.to_ascii_lowercase().contains("x-www-form-urlencoded"));
        if form {
            self.request_body(&redact_form(text));
        } else {
            self.request_body(text);
        }
    }

    /// The vendor's HTTP status. At or above 400 the call has failed: `error.type` is the status,
    /// and the span status is ERROR — a client span's 4xx is a failed call (OTel HTTP client
    /// conventions), unlike the SERVER root's.
    pub fn status(&self, status: u16) {
        self.span
            .record(attr::RESPONSE_STATUS_CODE, i64::from(status));
        if status >= 400 {
            self.span
                .record(attr::ERROR_TYPE, status.to_string().as_str());
            self.span.record("otel.status_code", "ERROR");
            self.span.record(
                "otel.status_message",
                format!("the vendor answered HTTP {status}").as_str(),
            );
        }
    }

    /// The vendor's own id for the request, when it sent one.
    pub fn vendor_request_id(&self, id: Option<&str>) {
        if let Some(id) = id
            .map(str::trim)
            .filter(|v| !v.is_empty() && v.len() <= 200)
        {
            self.span
                .record(turn::VENDOR_REQUEST_ID, self.scrub(id).as_str());
        }
    }

    /// What the vendor answered, under content capture only: an STT body, or any error body.
    pub fn response_body(&self, body: &str) {
        if self.captures() {
            self.span
                .record(attr::RESPONSE_BODY, self.sanitize(body).as_str());
        }
    }

    /// Mark the first byte of the vendor's answer, once.
    pub fn first_byte(&mut self) {
        if self.first_byte.is_none() {
            self.first_byte = Some(self.started.elapsed());
        }
    }

    /// A synthesis's answer, described: the audio itself is never stored (NG-3).
    pub fn audio_response(
        &self,
        content_type: Option<&str>,
        bytes: usize,
        sample_rate: Option<u32>,
    ) {
        if !self.captures() {
            return;
        }
        let mut described = serde_json::Map::new();
        described.insert("audio".into(), "not stored".into());
        described.insert("content_type".into(), content_type.into());
        described.insert("bytes".into(), bytes.into());
        if let Some(rate) = sample_rate {
            described.insert("sample_rate".into(), rate.into());
        }
        if let Some(first) = self.first_byte {
            described.insert(
                "first_byte_ms".into(),
                serde_json::json!(first.as_secs_f64() * 1000.0),
            );
        }
        self.response_body(&serde_json::Value::Object(described).to_string());
    }

    /// End the exchange as it stands: a success, or the HTTP failure [`VendorCall::status`]
    /// already recorded.
    pub fn finish(mut self) {
        self.done = true;
    }

    /// End a call that got no usable response: `error.type` is the transport class, and the
    /// message is reqwest's without the URL (which can carry a credential).
    pub fn transport_error(self, e: &reqwest::Error) {
        let class = if e.is_timeout() {
            crate::core::voice_error::VoiceErrorType::VendorTimeout.as_str()
        } else {
            crate::core::voice_error::VoiceErrorType::Network.as_str()
        };
        let what = if e.is_timeout() {
            "the vendor did not answer in time"
        } else if e.is_connect() {
            "could not connect to the vendor"
        } else if e.is_body() || e.is_decode() {
            "the vendor's response could not be read"
        } else {
            "the request to the vendor failed"
        };
        let message = match std::error::Error::source(e) {
            Some(source) => format!("{what}: {source}"),
            None => what.to_string(),
        };
        self.fail(class, &message);
    }

    /// End the call failed, with `error_type` and `message`.
    pub fn fail(mut self, error_type: &str, message: &str) {
        self.mark_failed(error_type, message);
        self.done = true;
    }

    fn mark_failed(&self, error_type: &str, message: &str) {
        self.span.record(attr::ERROR_TYPE, error_type);
        self.span.record("otel.status_code", "ERROR");
        self.span
            .record("otel.status_message", self.scrub(message).as_str());
    }

    fn scrub(&self, text: &str) -> String {
        scrub(&self.secrets, text)
    }

    /// Scrub the credential first, so a cap cannot cut it into a prefix that no longer matches.
    fn sanitize(&self, body: &str) -> String {
        trace_redact::sanitize_body(&self.scrub(body))
    }
}

impl Drop for VendorCall {
    fn drop(&mut self) {
        if !self.done {
            self.mark_failed(
                CANCELLED,
                "the vendor call was abandoned before it completed",
            );
        }
    }
}

/// `{"params": {…}, "file": {…}}`: an STT request as sent — the vendor's query and form
/// parameters, and the audio as metadata, never its bytes (CONTRACTS §1.2a).
#[derive(Debug, Default, Clone)]
pub struct UploadBody {
    params: serde_json::Map<String, serde_json::Value>,
    file: Option<serde_json::Value>,
}

impl UploadBody {
    pub fn new() -> Self {
        Self::default()
    }

    /// The URL's query parameters.
    pub fn query(mut self, url: &str) -> Self {
        if let Ok(parsed) = url::Url::parse(url) {
            for (name, value) in parsed.query_pairs() {
                self.param(&name, value.as_ref());
            }
        }
        self
    }

    /// Form fields, in the order sent.
    pub fn fields<'a>(mut self, fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        for (name, value) in fields {
            self.param(name, value);
        }
        self
    }

    /// A JSON request body's top-level fields.
    pub fn json(mut self, body: &serde_json::Value) -> Self {
        if let Some(object) = body.as_object() {
            for (name, value) in object {
                self.param_value(name, value.clone());
            }
        }
        self
    }

    /// The audio, described.
    pub fn file(
        mut self,
        filename: Option<&str>,
        content_type: Option<&str>,
        bytes: usize,
    ) -> Self {
        self.file = Some(serde_json::json!({
            "filename": filename,
            "content_type": content_type,
            "bytes": bytes,
        }));
        self
    }

    /// One parameter. A repeated name (`keyterm`, `timestamp_granularities[]`) becomes an array;
    /// a credential-shaped name has its value masked.
    pub fn param(&mut self, name: &str, value: &str) {
        self.param_value(name, value.into());
    }

    fn param_value(&mut self, name: &str, value: serde_json::Value) {
        let value = if is_credential_param(name) {
            BODY_REDACTED.into()
        } else {
            value
        };
        match self.params.get_mut(name) {
            Some(serde_json::Value::Array(items)) => items.push(value),
            Some(existing) => {
                let first = existing.take();
                *existing = serde_json::Value::Array(vec![first, value]);
            }
            None => {
                self.params.insert(name.to_string(), value);
            }
        }
    }

    /// The JSON the span records.
    pub fn render(&self) -> String {
        let mut body = serde_json::Map::new();
        body.insert(
            "params".into(),
            serde_json::Value::Object(self.params.clone()),
        );
        if let Some(file) = &self.file {
            body.insert("file".into(), file.clone());
        }
        serde_json::Value::Object(body).to_string()
    }
}

/// Whether a query or form parameter carries a credential: `key`, `api_key`, `apikey`,
/// `subscription-key`, anything with `token`, `secret`, `password`, `signature` or `credential`
/// in it, `sig`, `auth`, `authorization`. Not `keyterm` or `keywords` — Deepgram's vocabulary.
pub fn is_credential_param(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase().replace('-', "_");
    n == "key"
        || n.ends_with("_key")
        || n.ends_with("apikey")
        || n.contains("keyid")
        || n.contains("token")
        || n.contains("secret")
        || n.contains("password")
        || n.contains("passwd")
        || n.contains("signature")
        || n.contains("credential")
        || n == "sig"
        || n == "auth"
        || n == "authorization"
}

/// `url` with any credential in it redacted: user info, and the value of every credential-shaped
/// query parameter ([`is_credential_param`]). A URL without either is returned as it was sent.
pub fn redact_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else {
        // Not a URL reqwest could have sent; keep the part no credential rides on.
        return raw.split(['?', '#']).next().unwrap_or_default().to_string();
    };
    if !url.username().is_empty() {
        let _ = url.set_username(URL_REDACTED);
    }
    if url.password().is_some() {
        let _ = url.set_password(Some(URL_REDACTED));
    }
    if url
        .query_pairs()
        .any(|(name, _)| is_credential_param(&name))
    {
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(name, value)| {
                let value = if is_credential_param(&name) {
                    URL_REDACTED.to_string()
                } else {
                    value.into_owned()
                };
                (name.into_owned(), value)
            })
            .collect();
        url.query_pairs_mut().clear().extend_pairs(pairs);
    }
    url.to_string()
}

/// A form-encoded body with its credential parameters masked.
fn redact_form(body: &str) -> String {
    if !url::form_urlencoded::parse(body.as_bytes()).any(|(name, _)| is_credential_param(&name)) {
        return body.to_string();
    }
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(
            url::form_urlencoded::parse(body.as_bytes()).map(|(name, value)| {
                let value = if is_credential_param(&name) {
                    BODY_REDACTED.into()
                } else {
                    value
                };
                (name, value)
            }),
        )
        .finish()
}

fn server_address(raw: &str) -> Option<String> {
    url::Url::parse(raw)
        .ok()?
        .host_str()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string())
}

/// The strings scrubbed from everything a call records: the credential, and the secret half of a
/// `user:secret` one. Short strings are not scrubbed — they would mangle unrelated text.
fn secrets_of(credential: &str) -> Vec<String> {
    let credential = credential.trim();
    let mut secrets = Vec::new();
    if credential.len() >= 6 {
        secrets.push(credential.to_string());
    }
    if let Some((_, secret)) = credential.split_once(':')
        && secret.len() >= 6
    {
        secrets.push(secret.to_string());
    }
    secrets
}

fn scrub(secrets: &[String], text: &str) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), BODY_REDACTED);
        }
    }
    out
}

/// The vendor's id for a request, from the response headers vendors use for one.
pub fn request_id_from_headers(headers: &reqwest::header::HeaderMap) -> Option<String> {
    waav_segmented_stt::vendor::request_id(
        headers,
        &["dg-request-id", "request-id", "x-request-id"],
    )
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
            attrs.record(&mut V(self.0.0.lock().unwrap().entry(name).or_default()));
        }
        fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
            let name = ctx
                .span(id)
                .map(|s| s.name().to_string())
                .unwrap_or_default();
            values.record(&mut V(self.0.0.lock().unwrap().entry(name).or_default()));
        }
    }

    fn with_capture(f: impl FnOnce()) -> HashMap<String, HashMap<String, String>> {
        let seen = Seen::default();
        let subscriber = tracing_subscriber::registry().with(Capture(seen.clone()));
        tracing::subscriber::with_default(subscriber, f);
        seen.0.lock().unwrap().clone()
    }

    fn scope_under(parent: &Span) -> VendorScope {
        IDS.iter().fold(
            VendorScope::new(parent.clone(), operation::TEXT_TO_SPEECH),
            |scope, id| scope.with_id(id, &format!("{id}-value")),
        )
    }

    /// GT-3 for the vendor span: everything recorded on it is declared by its constructor, so no
    /// recording is the silent no-op of an undeclared field.
    #[test]
    fn the_vendor_span_declares_everything_recorded_on_it() {
        let seen = with_capture(|| {
            let turn = tracing::info_span!("voice.turn");
            let mut call = VendorCall::open(
                Some(&scope_under(&turn)),
                "elevenlabs",
                "eleven_v3",
                "POST",
                "https://api.elevenlabs.io/v1/text-to-speech/v?output_format=pcm_24000",
                "sk-the-deployments-key",
            );
            call.first_byte();
            call.request_body(r#"{"text":"hi"}"#);
            call.status(429);
            call.vendor_request_id(Some("req-1"));
            call.response_body(r#"{"detail":"slow down"}"#);
            call.finish();
        });
        let vendor = &seen["voice.vendor_call"];
        let mut want: Vec<&str> = vec![
            attr::OPERATION_NAME,
            attr::PROVIDER_NAME,
            attr::REQUEST_MODEL,
            attr::HTTP_METHOD,
            attr::SERVER_ADDRESS,
            attr::URL_FULL,
            attr::RESPONSE_STATUS_CODE,
            attr::ERROR_TYPE,
            attr::REQUEST_BODY,
            attr::RESPONSE_BODY,
            turn::VENDOR_REQUEST_ID,
            "otel.name",
            "otel.kind",
            "otel.status_code",
            "otel.status_message",
        ];
        want.extend(IDS);
        let missing: Vec<&str> = want
            .into_iter()
            .filter(|k| !vendor.contains_key(*k))
            .collect();
        assert!(missing.is_empty(), "the vendor span lacks {missing:?}");
        assert_eq!(vendor["otel.name"], "text_to_speech eleven_v3");
        assert_eq!(vendor["otel.kind"], "client");
        assert_eq!(vendor[attr::ERROR_TYPE], "429");
        assert_eq!(vendor["otel.status_code"], "ERROR");
        assert_eq!(vendor[attr::SERVER_ADDRESS], "api.elevenlabs.io");
        assert_eq!(vendor[turn::PROJECT_ID], "bud.project_id-value");
        for (k, v) in vendor {
            assert!(!v.contains("sk-the-deployments-key"), "{k} carries the key");
        }
    }

    #[test]
    fn without_a_scope_nothing_is_opened() {
        let seen = with_capture(|| {
            let call = VendorCall::start("deepgram", "nova-3", "POST", "https://x.test/", "k");
            assert!(!call.captures());
            call.status(500);
        });
        assert!(!seen.contains_key("voice.vendor_call"));
    }

    #[test]
    fn a_call_dropped_before_it_ends_is_cancelled() {
        let seen = with_capture(|| {
            let turn = tracing::info_span!("voice.turn");
            let call = VendorCall::open(
                Some(&scope_under(&turn)),
                "self_hosted",
                "kokoro",
                "POST",
                "http://tts.local/v1/audio/speech",
                "",
            );
            drop(call);
        });
        let vendor = &seen["voice.vendor_call"];
        assert_eq!(vendor[attr::ERROR_TYPE], CANCELLED);
        assert_eq!(vendor["otel.status_code"], "ERROR");
    }

    #[test]
    fn a_finished_success_has_no_error() {
        let seen = with_capture(|| {
            let turn = tracing::info_span!("voice.turn");
            let call = VendorCall::open(
                Some(&scope_under(&turn)),
                "self_hosted",
                "",
                "POST",
                "http://tts.local/v1/audio/speech",
                "",
            );
            call.status(200);
            call.finish();
        });
        let vendor = &seen["voice.vendor_call"];
        assert!(!vendor.contains_key(attr::ERROR_TYPE));
        assert!(!vendor.contains_key("otel.status_code"));
        // No model: named by the vendor, and no empty model recorded.
        assert_eq!(vendor["otel.name"], "text_to_speech self_hosted");
        assert!(!vendor.contains_key(attr::REQUEST_MODEL));
    }

    #[test]
    fn credentials_in_a_url_are_redacted_and_nothing_else_is() {
        let url = "https://user:pw@api.vendor.test/v1/tts?key=abc&voice=v&keyterm=Bud&api_key=zzz\
                   &X-Amz-Signature=sig1&access_token=t";
        let out = redact_url(url);
        let parsed = url::Url::parse(&out).unwrap();
        let q: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["key"], "REDACTED");
        assert_eq!(q["api_key"], "REDACTED");
        assert_eq!(q["X-Amz-Signature"], "REDACTED");
        assert_eq!(q["access_token"], "REDACTED");
        assert_eq!(q["voice"], "v");
        assert_eq!(q["keyterm"], "Bud");
        assert_eq!(parsed.username(), "REDACTED");
        assert_eq!(parsed.password(), Some("REDACTED"));
        for survived in ["key=abc", "zzz", "sig1", ":pw@", "access_token=t"] {
            assert!(!out.contains(survived), "{survived} survived: {out}");
        }
        // A URL with nothing to redact is untouched, encoding included.
        let plain = "https://api.deepgram.com/v1/listen?model=nova-3&keyterm=Bud%20Studio";
        assert_eq!(redact_url(plain), plain);
    }

    #[test]
    fn credential_params_are_named_by_shape() {
        for yes in [
            "key",
            "api_key",
            "apikey",
            "API-KEY",
            "subscription-key",
            "token",
            "access_token",
            "client_secret",
            "password",
            "sig",
            "Signature",
            "X-Amz-Credential",
            "AWSAccessKeyId",
            "auth",
            "Authorization",
        ] {
            assert!(is_credential_param(yes), "{yes}");
        }
        for no in [
            "keyterm",
            "keywords",
            "model",
            "voice",
            "language",
            "punctuate",
            "model_id",
        ] {
            assert!(!is_credential_param(no), "{no}");
        }
    }

    #[test]
    fn an_upload_is_its_params_and_the_files_metadata() {
        let body = UploadBody::new()
            .query("https://api.deepgram.com/v1/listen?model=nova-3&keyterm=a&keyterm=b&token=t")
            .fields([("model_id", "scribe_v1"), ("api_key", "k")])
            .json(&serde_json::json!({"audio_url": "https://cdn/x", "speaker_labels": true}))
            .file(Some("audio.wav"), Some("audio/wav"), 204_844)
            .render();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["params"]["model"], "nova-3");
        assert_eq!(v["params"]["keyterm"], serde_json::json!(["a", "b"]));
        assert_eq!(v["params"]["token"], "***");
        assert_eq!(v["params"]["api_key"], "***");
        assert_eq!(v["params"]["model_id"], "scribe_v1");
        assert_eq!(v["params"]["speaker_labels"], true);
        assert_eq!(v["file"]["bytes"], 204_844);
        assert_eq!(v["file"]["filename"], "audio.wav");
        let bare = UploadBody::new().render();
        assert_eq!(bare, r#"{"params":{}}"#);
    }

    #[test]
    fn the_credential_is_scrubbed_from_whatever_carries_it() {
        let secrets = secrets_of("user@example.com:hunter2-password");
        assert_eq!(
            scrub(
                &secrets,
                r#"{"login":"user@example.com","pwd":"hunter2-password"}"#
            ),
            r#"{"login":"user@example.com","pwd":"***"}"#
        );
        assert!(
            secrets_of("abc").is_empty(),
            "a short key would mangle text"
        );
        assert_eq!(
            redact_form("user=a&password=b&text=hello+world"),
            "user=a&password=***&text=hello+world"
        );
        assert_eq!(redact_form("text=hello"), "text=hello");
    }

    #[test]
    fn only_the_six_ids_are_carried_and_empty_values_are_not() {
        let scope = VendorScope::new(Span::none(), operation::TRANSCRIPTION)
            .with_id(turn::PROJECT_ID, "p")
            .with_id(turn::ENDPOINT_ID, "  ")
            .with_id("bud.prompt_id", "never")
            .with_id(turn::PROJECT_ID, "p2");
        assert_eq!(scope.ids(), &[(turn::PROJECT_ID, "p2".to_string())]);
    }

    #[tokio::test]
    async fn the_scope_is_current_only_inside_run() {
        assert!(VendorScope::current().is_none());
        let scope = VendorScope::new(Span::none(), operation::TEXT_TO_SPEECH);
        let seen = scope
            .run(async { VendorScope::current().map(|s| s.operation()) })
            .await;
        assert_eq!(seen, Some(operation::TEXT_TO_SPEECH));
        assert!(VendorScope::current().is_none());
    }
}
