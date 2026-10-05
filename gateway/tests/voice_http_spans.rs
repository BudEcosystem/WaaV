//! FRD-021 voice analytics: what the HTTP audio handlers record, asserted on the spans as they are
//! EXPORTED.
//!
//! The subscriber is the production shape — `tracing_opentelemetry` over the `Registry`, behind an
//! INFO filter, feeding a real `opentelemetry_sdk` provider whose exporter keeps finished spans in
//! memory. So a field a span never declared is absent here exactly as it is absent in ClickHouse
//! (GT-3); `otel.name`, `otel.kind` and `otel.status_*` are interpreted by the same layer that
//! interprets them in production; and every assertion reads a VALUE, never a declaration (R-6).
//!
//! The gateway is the real router — `create_api_router` behind the auth middleware, the
//! operability routes beside it, `request_id_middleware` outermost, as `main.rs` mounts them — over
//! a Bud plane hydrated from a `MemoryStore`: an API key whose aliases carry endpoint/model/project
//! metadata, and `voice_table` entries with pricing, pointed at wiremock vendors.
//!
//! Hosted vendors cannot be pointed at a mock through the handler (their base URLs are compiled
//! in), so the vendor paths here use the self-hosted provider — which shares the TTS HTTP driver
//! and its status classification with the hosted TTS vendors — and the hosted STT cases are the
//! ones that fail before any vendor call (decode, credential, request validation).
//!
//! Content capture and the root-span switch are read once per process, so the cases that need
//! them set run in a child process of this same binary (`child_*`, ignored unless spawned).
//!
//! The `vendor_span*` cases pin the vendor call span of CONTRACTS §1.2a — the CLIENT child of
//! `voice.turn` showing what was sent to the vendor and what came back. The hosted prerecorded STT
//! vendors are driven below the handler, inside a turn's vendor scope, as the handler runs them.
//! `VOICE_HTTP_SPANS_DUMP=1` prints the tree a vendor case exported.
//!
//! Case ids refer to `bud-runtime/specs/021-voice-analytics/TEST_CASES.md`.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::get;
use futures::StreamExt;
use opentelemetry::Value;
use opentelemetry::trace::{SpanId, SpanKind, Status, TracerProvider as _};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use serde_json::{Value as Json, json};
use tower::util::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use waav_gateway::config::{DAGTimeoutsConfig, PluginConfig};
use waav_gateway::{ServerConfig, state::AppState};

// =============================================================================================
// Attribute names, written out rather than taken from `voice_attrs`: a renamed constant must
// fail here, not silently move both sides of the comparison.
// =============================================================================================

const ENDPOINT_ID: &str = "bud.endpoint_id";
const ENDPOINT_NAME: &str = "bud.voice.endpoint_name";
const MODEL_ID: &str = "bud.model_id";
const PROJECT_ID: &str = "bud.project_id";
const API_KEY_PROJECT_ID: &str = "bud.api_key_project_id";
const USER_ID: &str = "bud.user_id";
const API_KEY_ID: &str = "bud.api_key_id";
const CAPABILITY: &str = "bud.voice.capability";
const TRANSPORT: &str = "bud.voice.transport";
const TTS_VENDOR: &str = "bud.voice.tts.vendor";
const TTS_MODEL: &str = "bud.voice.tts.model";
const TTS_DURATION: &str = "bud.voice.tts.duration_ms";
const TTS_TTFB: &str = "bud.voice.tts.ttfb_ms";
const TTS_VOICE: &str = "bud.voice.tts.voice";
const STT_VENDOR: &str = "bud.voice.stt.vendor";
const STT_MODEL: &str = "bud.voice.stt.model";
const STT_DURATION: &str = "bud.voice.stt.duration_ms";
const STT_CONFIDENCE: &str = "bud.voice.stt.confidence";
const CHARACTERS: &str = "bud.voice.characters";
const AUDIO_SECONDS: &str = "bud.voice.audio_seconds";
const COST: &str = "bud.voice.cost";
const PRICING_UNIT: &str = "bud.voice.pricing_unit";
const ERROR_TYPE: &str = "bud.voice.error_type";
const VENDOR_STATUS: &str = "bud.voice.vendor_status_code";
const LANGUAGE: &str = "bud.voice.language";
const OUTPUT_AUDIO_SECONDS: &str = "bud.voice.output_audio_seconds";
const DETECTED_LANGUAGE: &str = "bud.voice.detected_language";
const AUDIO_FORMAT: &str = "bud.voice.audio_format";
const SAMPLE_RATE: &str = "bud.voice.sample_rate";
const INPUT_AUDIO_BYTES: &str = "bud.voice.input_audio_bytes";
const VENDOR_REQUEST_ID: &str = "bud.voice.vendor_request_id";
const SERVED_ENDPOINT_ID: &str = "bud.voice.served_endpoint_id";
const FALLBACK_FROM: &str = "bud.voice.fallback_from";
const RETRY_COUNT: &str = "bud.voice.retry_count";
const GEN_AI_PROVIDER: &str = "gen_ai.provider.name";
const GEN_AI_MODEL: &str = "gen_ai.request.model";

const REQUEST_BODY: &str = "http.request.body";
const RESPONSE_BODY: &str = "http.response.body";

/// The six ids FRD §6.7 puts on the root, equal to the child's.
const SIX_IDS: &[&str] = &[
    PROJECT_ID,
    API_KEY_PROJECT_ID,
    ENDPOINT_ID,
    MODEL_ID,
    USER_ID,
    API_KEY_ID,
];

/// TC-EMIT-01: what a successful synthesis must carry on `voice.turn`.
const TTS_SUCCESS: &[&str] = &[
    CAPABILITY,
    TRANSPORT,
    ENDPOINT_ID,
    ENDPOINT_NAME,
    MODEL_ID,
    PROJECT_ID,
    API_KEY_PROJECT_ID,
    USER_ID,
    API_KEY_ID,
    TTS_VENDOR,
    TTS_MODEL,
    TTS_DURATION,
    TTS_TTFB,
    TTS_VOICE,
    CHARACTERS,
    COST,
    PRICING_UNIT,
    OUTPUT_AUDIO_SECONDS,
    AUDIO_FORMAT,
    SAMPLE_RATE,
];

/// TC-EMIT-01: what a successful self-hosted transcription (verbose_json, WAV upload) must carry.
const STT_SUCCESS: &[&str] = &[
    CAPABILITY,
    TRANSPORT,
    ENDPOINT_ID,
    ENDPOINT_NAME,
    MODEL_ID,
    PROJECT_ID,
    API_KEY_PROJECT_ID,
    USER_ID,
    API_KEY_ID,
    STT_VENDOR,
    STT_MODEL,
    STT_DURATION,
    AUDIO_SECONDS,
    COST,
    PRICING_UNIT,
    INPUT_AUDIO_BYTES,
    AUDIO_FORMAT,
    SAMPLE_RATE,
    DETECTED_LANGUAGE,
    VENDOR_REQUEST_ID,
];

/// TC-EMIT-01: what every failure after endpoint resolution must carry.
const FAILURE: &[&str] = &[
    CAPABILITY,
    TRANSPORT,
    ENDPOINT_ID,
    ENDPOINT_NAME,
    PROJECT_ID,
    API_KEY_PROJECT_ID,
    ERROR_TYPE,
];

/// TC-EMIT-04: units and cost exist only for work that was done.
const SUCCESS_ONLY: &[&str] = &[
    CHARACTERS,
    AUDIO_SECONDS,
    COST,
    PRICING_UNIT,
    OUTPUT_AUDIO_SECONDS,
];

/// TC-TRACE-01: the SERVER root's own shape.
const ROOT_SHAPE: &[&str] = &[
    "http.request.method",
    "http.route",
    "url.path",
    "url.scheme",
    "user_agent.original",
    "server.address",
    "request_id",
    "http.response.status_code",
];

// =============================================================================================
// Fixture identities
// =============================================================================================

const KEY: &str = "bud_voice_analytics_http_span_key";
/// The principal's project: `api_key_project_id` in the key blob's metadata.
const KEY_PROJECT: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f00000001";
/// The ENDPOINT's project, from the alias metadata. Deliberately different from the key's.
const EP_PROJECT: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f00000002";
const USER: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f000000a1";
const API_KEY: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f000000b1";

const TTS_EP: &str = "7d3c9a10-1111-4a2b-8c3d-000000000001";
const TTS_MODEL_ID: &str = "7d3c9a10-1111-4a2b-8c3d-0000000000a1";
const STT_EP: &str = "7d3c9a10-2222-4a2b-8c3d-000000000002";
const STT_MODEL_ID: &str = "7d3c9a10-2222-4a2b-8c3d-0000000000a2";
const DG_EP: &str = "7d3c9a10-3333-4a2b-8c3d-000000000003";
const DG_BARE_EP: &str = "7d3c9a10-4444-4a2b-8c3d-000000000004";
const DG_TTS_EP: &str = "7d3c9a10-5555-4a2b-8c3d-000000000005";
const NO_BASE_EP: &str = "7d3c9a10-6666-4a2b-8c3d-000000000006";
/// A voice endpoint in the table that NO key's allowlist names and nothing publishes.
const UNLISTED_EP: &str = "7d3c9a10-7777-4a2b-8c3d-000000000007";
/// Fallback deployments (FRD-022 §6.4): reached by id from a primary's `fallback_models`, never by
/// alias, so no allowlist names them.
const FB_TTS_EP: &str = "7d3c9a10-8888-4a2b-8c3d-000000000008";
const FB_STT_EP: &str = "7d3c9a10-9999-4a2b-8c3d-000000000009";

/// A customer (`client_app`) key: its own project holds no voice deployment, so its map is empty.
const CLIENT_KEY: &str = "bud_client_voice_http_spans_customer";
const CLIENT_PROJECT: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f00000003";
const CLIENT_USER: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f000000a3";
const CLIENT_API_KEY: &str = "0e1f7c2a-4b1d-4c0e-9a55-5a1f000000b3";

/// The vendor model each `voice_table` entry names.
const TTS_VENDOR_MODEL: &str = "kokoro-v1";
const STT_VENDOR_MODEL: &str = "whisper-large-v3";

/// The ciphertext half of bud-auth's fixture key pair: a hosted endpoint needs a credential that
/// opens. The private half (`test_cred_private.pem`) is `*.pem` and therefore git-ignored, so it is
/// read at run time rather than compiled in — see [`test_decryptor`].
const TEST_CREDENTIAL: &str = include_str!("../../bud-auth/tests/fixtures/test_cred_encrypted.hex");

/// bud-auth's fixture private key, when this checkout has it (its own credential tests need it too).
fn test_pem() -> Option<String> {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bud-auth/tests/fixtures/test_cred_private.pem"
    ))
    .ok()
}

fn test_decryptor() -> bud_auth::CredentialDecryptor {
    match test_pem() {
        Some(pem) => bud_auth::CredentialDecryptor::from_pem(&pem).expect("fixture key loads"),
        None => bud_auth::CredentialDecryptor::disabled(),
    }
}
const HELLO_OPUS: &[u8] = include_bytes!("../../waav-openai-audio/tests/fixtures/hello.opus");

// =============================================================================================
// Capture: a real OTel pipeline, scoped to this test's thread
// =============================================================================================

#[derive(Debug, Clone, Default)]
struct Exported(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Exported {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

/// Thread-local, via `set_default`: a global subscriber would see every other test's spans.
/// `#[tokio::test]` is a current-thread runtime, so every task the handler spawns runs here too.
struct Capture {
    exported: Exported,
    _guard: tracing::subscriber::DefaultGuard,
    _provider: SdkTracerProvider,
}

impl Capture {
    fn install() -> Self {
        let exported = Exported::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exported.clone())
            .build();
        let layer = tracing_opentelemetry::layer().with_tracer(provider.tracer("voice-http-spans"));
        // `main` runs the OTel layer behind `EnvFilter::new("info")`; DEBUG spans (tower-http's
        // TraceLayer) are therefore never exported, and must not be here either.
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::INFO)
            .with(layer);
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            exported,
            _guard: guard,
            _provider: provider,
        }
    }

    /// The spans finished since the last call, once the request's root has closed.
    ///
    /// A span is exported when its last handle drops; a vendor task can outlive the response by a
    /// moment, so this yields to the runtime briefly rather than reading the instant it returns.
    async fn take(&self) -> Vec<SpanData> {
        let rooted = || {
            self.exported
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.parent_span_id == SpanId::INVALID)
        };
        let count = || self.exported.0.lock().unwrap().len();
        for _ in 0..250 {
            if rooted() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Then until nothing more arrives: a vendor task the handler aborted can hold a clone of
        // `voice.turn` until the runtime drops it, a tick after the response.
        let mut last = count();
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let now = count();
            if now == last {
                break;
            }
            last = now;
        }
        std::mem::take(&mut *self.exported.0.lock().unwrap())
    }

    /// Everything exported so far, without waiting for a root (for requests that export none).
    async fn drain(&self) -> Vec<SpanData> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::mem::take(&mut *self.exported.0.lock().unwrap())
    }
}

fn names(spans: &[SpanData]) -> Vec<String> {
    spans.iter().map(|s| s.name.to_string()).collect()
}

#[track_caller]
fn only<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
    let found: Vec<&SpanData> = spans.iter().filter(|s| s.name == name).collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one `{name}` span, exported: {:?}",
        names(spans)
    );
    found[0]
}

#[track_caller]
fn root_of(spans: &[SpanData]) -> &SpanData {
    let roots: Vec<&SpanData> = spans
        .iter()
        .filter(|s| s.parent_span_id == SpanId::INVALID)
        .collect();
    assert_eq!(
        roots.len(),
        1,
        "expected exactly one root span, exported: {:?}",
        names(spans)
    );
    roots[0]
}

fn attr(span: &SpanData, key: &str) -> Option<Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.clone())
}

fn text(span: &SpanData, key: &str) -> Option<String> {
    attr(span, key).map(|v| v.as_str().into_owned())
}

fn number(span: &SpanData, key: &str) -> Option<f64> {
    match attr(span, key)? {
        Value::F64(f) => Some(f),
        Value::I64(i) => Some(i as f64),
        Value::String(s) => s.as_str().parse().ok(),
        _ => None,
    }
}

fn keys(span: &SpanData) -> Vec<String> {
    let mut k: Vec<String> = span
        .attributes
        .iter()
        .map(|kv| kv.key.as_str().to_string())
        .collect();
    k.sort();
    k
}

fn is_error(span: &SpanData) -> bool {
    matches!(span.status, Status::Error { .. })
}

/// Accumulates every mismatch of a case, so one run reports all of them — the ⚑ falsifiers need
/// the full picture of what today's code gets wrong, not just the first line of it.
#[derive(Default)]
struct Findings(Vec<String>);

impl Findings {
    fn check(&mut self, ok: bool, what: impl FnOnce() -> String) {
        if !ok {
            self.0.push(what());
        }
    }

    fn eq_text(&mut self, ctx: &str, span: &SpanData, key: &str, want: Option<&str>) {
        let got = text(span, key);
        self.check(got.as_deref() == want, || {
            format!("{ctx}: `{}`.{key} = {got:?}, want {want:?}", span.name)
        });
    }

    fn eq_number(&mut self, ctx: &str, span: &SpanData, key: &str, want: Option<f64>) {
        let got = number(span, key);
        let ok = match (got, want) {
            (Some(g), Some(w)) => (g - w).abs() <= 1e-9_f64.max(w.abs() * 1e-9),
            (None, None) => true,
            _ => false,
        };
        self.check(ok, || {
            format!("{ctx}: `{}`.{key} = {got:?}, want {want:?}", span.name)
        });
    }

    fn present(&mut self, ctx: &str, span: &SpanData, required: &[&str]) {
        let missing: Vec<&str> = required
            .iter()
            .copied()
            .filter(|k| attr(span, k).is_none())
            .collect();
        self.check(missing.is_empty(), || {
            format!(
                "{ctx}: `{}` does not carry {missing:?} (recorded without being declared, or never \
                 recorded); it has {:?}",
                span.name,
                keys(span)
            )
        });
    }

    fn absent(&mut self, ctx: &str, span: &SpanData, forbidden: &[&str]) {
        let found: Vec<&str> = forbidden
            .iter()
            .copied()
            .filter(|k| attr(span, k).is_some())
            .collect();
        self.check(found.is_empty(), || {
            format!("{ctx}: `{}` must not carry {found:?}", span.name)
        });
    }

    /// FR-5: an empty string is not NULL; it makes a column look populated.
    fn no_empty_strings(&mut self, ctx: &str, span: &SpanData) {
        let empty: Vec<String> = span
            .attributes
            .iter()
            .filter(|kv| matches!(&kv.value, Value::String(s) if s.as_str().trim().is_empty()))
            .map(|kv| kv.key.as_str().to_string())
            .collect();
        self.check(empty.is_empty(), || {
            format!("{ctx}: `{}` records empty strings for {empty:?}", span.name)
        });
    }

    #[track_caller]
    fn assert_none(self) {
        assert!(
            self.0.is_empty(),
            "{} finding(s):\n  - {}",
            self.0.len(),
            self.0.join("\n  - ")
        );
    }
}

// =============================================================================================
// The gateway under test
// =============================================================================================

fn config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: None,
        livekit_api_secret: None,
        deepgram_api_key: None,
        elevenlabs_api_key: None,
        google_credentials: None,
        azure_speech_subscription_key: None,
        azure_speech_region: None,
        cartesia_api_key: None,
        openai_api_key: None,
        azure_openai_api_key: None,
        azure_openai_endpoint: None,
        grok_api_key: None,
        inworld_api_key: None,
        gemini_api_key: None,
        ultravox_api_key: None,
        speechmatics_api_key: None,
        yandex_api_key: None,
        yandex_folder_id: None,
        assemblyai_api_key: None,
        hume_api_key: None,
        groq_api_key: None,
        ibm_watson_api_key: None,
        ibm_watson_instance_id: None,
        ibm_watson_region: None,
        aws_access_key_id: None,
        aws_secret_access_key: None,
        aws_region: None,
        gnani_token: None,
        gnani_access_key: None,
        gnani_certificate_path: None,
        recording_s3_bucket: None,
        recording_s3_region: None,
        recording_s3_endpoint: None,
        recording_s3_access_key: None,
        recording_s3_secret_key: None,
        recording_s3_prefix: None,
        cache_path: None,
        cache_ttl_seconds: Some(3600),
        auth_service_url: None,
        auth_signing_key_path: None,
        auth_api_secrets: Vec::new(),
        auth_timeout_seconds: 5,
        auth_required: false,
        sip: None,
        cors_allowed_origins: None,
        rate_limit_requests_per_second: 60,
        rate_limit_burst_size: 10,
        max_websocket_connections: None,
        max_connections_per_ip: 100,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        realtime_endpoint_overrides: Default::default(),
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
        aliases: Default::default(),
    }
}

/// The alias map on the test key: every alias carries the metadata budapp writes (GT-5).
fn aliases() -> Json {
    json!({
        "tts-a": {"endpoint_id": TTS_EP, "model_id": TTS_MODEL_ID, "project_id": EP_PROJECT, "kind": "model"},
        "stt-a": {"endpoint_id": STT_EP, "model_id": STT_MODEL_ID, "project_id": EP_PROJECT, "kind": "model"},
        "dg-a": {"endpoint_id": DG_EP, "model_id": STT_MODEL_ID, "project_id": EP_PROJECT, "kind": "model"},
        "dg-bare": {"endpoint_id": DG_BARE_EP, "model_id": STT_MODEL_ID, "project_id": EP_PROJECT, "kind": "model"},
        "dg-tts": {"endpoint_id": DG_TTS_EP, "model_id": TTS_MODEL_ID, "project_id": EP_PROJECT, "kind": "model"},
        "no-base": {"endpoint_id": NO_BASE_EP, "model_id": STT_MODEL_ID, "project_id": EP_PROJECT, "kind": "model"},
    })
}

fn tts_entry(api_base: &str, pricing: Option<Json>, config: Option<Json>) -> Json {
    let mut e = json!({
        "vendor": "self_hosted",
        "api_base": api_base,
        "endpoints": ["text_to_speech"],
        "model": TTS_VENDOR_MODEL,
    });
    if let Some(p) = pricing {
        e["pricing"] = p;
    }
    if let Some(c) = config {
        e["config"] = c;
    }
    e
}

fn stt_entry(api_base: &str, pricing: Option<Json>) -> Json {
    let mut e = json!({
        "vendor": "self_hosted",
        "api_base": api_base,
        "endpoints": ["audio_transcription", "audio_translation"],
        "model": STT_VENDOR_MODEL,
    });
    if let Some(p) = pricing {
        e["pricing"] = p;
    }
    e
}

/// Hosted Deepgram endpoints: one with a credential that opens, one with none, and a TTS one with
/// none. They never reach the vendor — every case on them fails before the call.
fn hosted_entries() -> Vec<(&'static str, Json)> {
    vec![
        (
            DG_EP,
            json!({"vendor": "deepgram", "endpoints": ["audio_transcription"], "model": "nova-3",
                   "credential": TEST_CREDENTIAL.trim()}),
        ),
        (
            DG_BARE_EP,
            json!({"vendor": "deepgram", "endpoints": ["audio_transcription"], "model": "nova-3"}),
        ),
        (
            DG_TTS_EP,
            json!({"vendor": "deepgram", "endpoints": ["text_to_speech"], "model": "aura-2",
                   "voice": "aura-2-thalia-en"}),
        ),
        (
            NO_BASE_EP,
            json!({"vendor": "self_hosted", "endpoints": ["audio_transcription"],
                   "model": STT_VENDOR_MODEL}),
        ),
    ]
}

fn pricing(unit: &str, cost_per_unit: f64, per_units: u64) -> Json {
    json!({"unit": unit, "cost_per_unit": cost_per_unit, "currency": "USD", "per_units": per_units})
}

/// A gateway over a plane hydrated with `table` (`endpoint id -> voice_table entry`) and the
/// test key.
async fn gateway(table: Vec<(&str, Json)>) -> axum::Router {
    gateway_with(table, &[]).await
}

/// [`gateway`] plus raw control-plane keys (another key's blob, the published overlay).
async fn gateway_with(table: Vec<(&str, Json)>, extra: &[(String, String)]) -> axum::Router {
    gateway_full(table, extra, None).await
}

/// [`gateway`] with the deployment policies (rate limits, circuit breakers) switched on.
async fn gateway_with_policies(
    table: Vec<(&str, Json)>,
    policies: Arc<waav_gateway::core::deployment_policy::DeploymentPolicies>,
) -> axum::Router {
    gateway_full(table, &[], Some(policies)).await
}

async fn gateway_full(
    table: Vec<(&str, Json)>,
    extra: &[(String, String)],
    policies: Option<Arc<waav_gateway::core::deployment_policy::DeploymentPolicies>>,
) -> axum::Router {
    let store = Arc::new(bud_auth::MemoryStore::new());
    for (key, value) in extra {
        store.set(key, value);
    }
    let mut blob = aliases().as_object().cloned().unwrap();
    blob.insert(
        "__metadata__".into(),
        json!({"api_key_id": API_KEY, "user_id": USER, "api_key_project_id": KEY_PROJECT}),
    );
    store.set(
        &format!("api_key:{}", bud_auth::hash_api_key(KEY)),
        &Json::Object(blob).to_string(),
    );
    for (id, entry) in table {
        let mut wrapped = serde_json::Map::new();
        wrapped.insert(id.to_string(), entry);
        store.set(
            &format!("voice_table:{id}"),
            &Json::Object(wrapped).to_string(),
        );
    }
    let plane = Arc::new(bud_auth::BudPlane::with_decryptor(
        store as Arc<dyn bud_auth::ControlPlaneStore>,
        None,
        test_decryptor(),
    ));
    plane.boot().await.expect("plane boots");

    let mut state = AppState::new(config()).await;
    let owned = Arc::get_mut(&mut state).expect("the state is not shared yet");
    owned.bud_mode = Some(
        waav_gateway::auth::bud_mode::BudMode::for_plane(plane).expect("bud mode over the plane"),
    );
    owned.policies = policies;
    router(state)
}

/// The routes and layers `main.rs` mounts that matter here.
fn router(state: Arc<AppState>) -> axum::Router {
    use waav_gateway::handlers::api;
    let public = axum::Router::new()
        .route("/", get(api::health_check))
        .route("/ready", get(api::readiness_check))
        .route("/livez", get(api::livez))
        .route("/readyz", get(api::readyz))
        .route("/metrics", get(api::metrics_handler));
    let protected =
        waav_gateway::routes::api::create_api_router().layer(axum::middleware::from_fn_with_state(
            state.clone(),
            waav_gateway::middleware::auth_middleware,
        ));
    public
        .merge(protected)
        .with_state(state)
        .layer(axum::middleware::from_fn(
            waav_gateway::middleware::request_id_middleware,
        ))
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Reply {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn send(app: &axum::Router, req: Request<Body>) -> Reply {
    let resp = app.clone().oneshot(req).await.expect("router answers");
    let (parts, body) = resp.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

async fn speech(app: &axum::Router, body: Json) -> Reply {
    speech_as(app, KEY, body).await
}

async fn speech_as(app: &axum::Router, bearer: &str, body: Json) -> Reply {
    send(
        app,
        Request::post("/v1/audio/speech")
            .header("authorization", format!("Bearer {bearer}"))
            .header("content-type", "application/json")
            .header("user-agent", "voice-http-spans/1.0")
            .header("host", "waav.test:3001")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn upload(
    app: &axum::Router,
    route: &str,
    fields: &[(&str, &str)],
    file: (&str, &str, &[u8]),
) -> Reply {
    let boundary = "voice-http-spans-boundary-7d3c9a10";
    let mut body: Vec<u8> = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    let (filename, content_type, bytes) = file;
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    send(
        app,
        Request::post(route)
            .header("authorization", format!("Bearer {KEY}"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header("user-agent", "voice-http-spans/1.0")
            .header("host", "waav.test:3001")
            .body(Body::from(body))
            .unwrap(),
    )
    .await
}

/// A 16-bit mono WAV of `secs` seconds at `rate` (a quiet tone, not digital silence).
fn wav_secs(secs: f64, rate: u32) -> Vec<u8> {
    wav_samples((secs * rate as f64).round() as usize, rate)
}

/// A 16-bit mono WAV whose file is exactly `total` bytes (`total` even, > 44).
fn wav_of_len(total: usize, rate: u32) -> Vec<u8> {
    wav_samples((total - 44) / 2, rate)
}

fn wav_samples(samples: usize, rate: u32) -> Vec<u8> {
    let data_len = (samples * 2) as u32;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes()); // PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // mono
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        let s = ((i as f64 * 0.05).sin() * 1000.0) as i16;
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// Exactly `n` characters of speakable text.
fn sentence(n: usize) -> String {
    "The quick brown fox jumps over the lazy dog. "
        .chars()
        .cycle()
        .take(n)
        .collect()
}

/// A self-hosted TTS vendor answering 200 with `body` for any `/v1/audio/speech`.
async fn tts_vendor(body: Vec<u8>, content_type: &str) -> MockServer {
    let vendor = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", content_type)
                .set_body_bytes(body),
        )
        .mount(&vendor)
        .await;
    vendor
}

/// A self-hosted STT vendor for both routes, answering verbose JSON with a request id header.
async fn stt_vendor() -> MockServer {
    let vendor = MockServer::start().await;
    for (route, task, text) in [
        ("/v1/audio/transcriptions", "transcribe", "hola a todos"),
        ("/v1/audio/translations", "translate", "hello everyone"),
    ] {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .insert_header("x-request-id", "vendor-req-7")
                    .set_body_json(json!({
                        "task": task,
                        "language": "es",
                        "duration": 12.5,
                        "text": text,
                        "segments": [],
                    })),
            )
            .mount(&vendor)
            .await;
    }
    vendor
}

/// Matches a request whose raw body contains `0`. wiremock's own `body_string_contains` gives up
/// on a body that is not UTF-8, and a multipart upload carrying audio never is.
struct BodyHas(&'static str);

impl wiremock::Match for BodyHas {
    fn matches(&self, request: &wiremock::Request) -> bool {
        let needle = self.0.as_bytes();
        request.body.windows(needle.len()).any(|w| w == needle)
    }
}

fn base(vendor: &MockServer) -> String {
    format!("{}/v1", vendor.uri())
}

/// An address nothing listens on: bind, read the port, drop the listener.
fn closed_port_base() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}/v1")
}

/// TC-TRACE-01: the root is a SERVER span named for the route, and carries the child's identity.
fn check_root(
    f: &mut Findings,
    ctx: &str,
    spans: &[SpanData],
    route: &str,
    reply: &Reply,
    vendor_model: Option<&str>,
) {
    let root = root_of(spans);
    let turn = only(spans, "voice.turn");
    let want_name = format!("POST {route}");
    f.check(root.name == want_name.as_str(), || {
        format!("{ctx}: root is named {:?}, want {want_name:?}", root.name)
    });
    f.check(root.span_kind == SpanKind::Server, || {
        format!("{ctx}: root kind is {:?}, want Server", root.span_kind)
    });
    f.check(turn.parent_span_id == root.span_context.span_id(), || {
        format!("{ctx}: voice.turn is not a child of the root")
    });
    f.present(ctx, root, ROOT_SHAPE);
    f.eq_text(ctx, root, "http.request.method", Some("POST"));
    f.eq_text(ctx, root, "http.route", Some(route));
    f.eq_text(ctx, root, "url.path", Some(route));
    f.eq_text(
        ctx,
        root,
        "user_agent.original",
        Some("voice-http-spans/1.0"),
    );
    f.eq_number(
        ctx,
        root,
        "http.response.status_code",
        Some(reply.status.as_u16() as f64),
    );
    if reply.status.as_u16() >= 400 {
        f.eq_text(ctx, root, "error.type", Some(reply.status.as_str()));
    } else {
        f.absent(ctx, root, &["error.type"]);
    }
    let want_error = reply.status.is_server_error();
    f.check(is_error(root) == want_error, || {
        format!(
            "{ctx}: root status {:?} for HTTP {} (ERROR only for 5xx)",
            root.status, reply.status
        )
    });
    for id in SIX_IDS {
        f.check(text(root, id) == text(turn, id), || {
            format!(
                "{ctx}: root {id} = {:?} but voice.turn has {:?}",
                text(root, id),
                text(turn, id)
            )
        });
    }
    for shared in [
        CAPABILITY,
        ENDPOINT_NAME,
        CHARACTERS,
        AUDIO_SECONDS,
        COST,
        PRICING_UNIT,
        ERROR_TYPE,
        OUTPUT_AUDIO_SECONDS,
        AUDIO_FORMAT,
    ] {
        f.check(text(root, shared) == text(turn, shared), || {
            format!(
                "{ctx}: root {shared} = {:?} but voice.turn has {:?}",
                text(root, shared),
                text(turn, shared)
            )
        });
    }
    f.eq_text(
        ctx,
        root,
        "gen_ai.provider.name",
        text(turn, TTS_VENDOR).or(text(turn, STT_VENDOR)).as_deref(),
    );
    f.eq_text(ctx, root, "gen_ai.request.model", vendor_model);
    // One value per key: a re-recorded field is exported twice, and ClickHouse reads either.
    for span in [root, turn] {
        let twice = duplicated(span);
        f.check(twice.is_empty(), || {
            format!("{ctx}: `{}` exports {twice:?} more than once", span.name)
        });
    }
    // R-11: the InferenceFact stage MVs are ServiceName-agnostic.
    for s in spans {
        let leaked: Vec<String> = keys(s)
            .into_iter()
            .filter(|k| {
                k == "gen_ai.inference_id"
                    || k.starts_with("gateway_analytics.")
                    || k == "bud.prompt_id"
            })
            .collect();
        f.check(leaked.is_empty(), || {
            format!(
                "{ctx}: `{}` carries InferenceFact discriminators {leaked:?}",
                s.name
            )
        });
    }
}

// =============================================================================================
// Success paths
// =============================================================================================

/// TC-EMIT-01 (TTS success), TC-EMIT-02 (TTS), TC-EMIT-05 (`character`), TC-EMIT-07,
/// TC-EMIT-09 (PCM), TC-EMIT-11 (TTS), TC-TRACE-01 (success row).
#[tokio::test]
async fn tts_success_records_identity_units_cost_and_the_server_root() {
    let cap = Capture::install();
    // 48,000 bytes of 16-bit PCM at 24 kHz is exactly one second.
    let vendor = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let app = gateway(vec![(
        TTS_EP,
        tts_entry(
            &base(&vendor),
            Some(pricing("character", 30.0, 1_000_000)),
            None,
        ),
    )])
    .await;

    let input = sentence(120);
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": input, "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.body.len(), 48_000);

    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    let mut f = Findings::default();
    let ctx = "tts success";
    f.present(ctx, turn, TTS_SUCCESS);
    f.eq_text(ctx, turn, CAPABILITY, Some("text_to_speech"));
    f.eq_text(ctx, turn, TRANSPORT, Some("http"));
    f.eq_text(ctx, turn, ENDPOINT_ID, Some(TTS_EP));
    f.eq_text(ctx, turn, ENDPOINT_NAME, Some("tts-a"));
    f.eq_text(ctx, turn, MODEL_ID, Some(TTS_MODEL_ID));
    f.eq_text(ctx, turn, PROJECT_ID, Some(EP_PROJECT));
    f.eq_text(ctx, turn, API_KEY_PROJECT_ID, Some(KEY_PROJECT));
    f.eq_text(ctx, turn, USER_ID, Some(USER));
    f.eq_text(ctx, turn, API_KEY_ID, Some(API_KEY));
    f.eq_text(ctx, turn, TTS_VENDOR, Some("self_hosted"));
    f.eq_text(ctx, turn, TTS_MODEL, Some(TTS_VENDOR_MODEL));
    f.eq_number(ctx, turn, CHARACTERS, Some(120.0));
    // 120 / 1,000,000 × 30
    f.eq_number(ctx, turn, COST, Some(0.0036));
    f.eq_text(ctx, turn, PRICING_UNIT, Some("character"));
    f.eq_number(ctx, turn, OUTPUT_AUDIO_SECONDS, Some(1.0));
    f.eq_text(ctx, turn, TTS_VOICE, Some("George"));
    f.eq_text(ctx, turn, AUDIO_FORMAT, Some("pcm"));
    f.eq_number(ctx, turn, SAMPLE_RATE, Some(24_000.0));
    // TC-EMIT-07: nothing named a language, so there is none — not "".
    f.absent(ctx, turn, &[LANGUAGE, ERROR_TYPE, VENDOR_STATUS]);
    f.no_empty_strings(ctx, turn);
    f.check(!is_error(turn), || {
        format!("{ctx}: turn status {:?}", turn.status)
    });

    check_root(
        &mut f,
        ctx,
        &spans,
        "/v1/audio/speech",
        &reply,
        Some(TTS_VENDOR_MODEL),
    );
    let root = root_of(&spans);
    f.eq_text(ctx, root, "url.scheme", Some("http"));
    f.eq_text(ctx, root, "server.address", Some("waav.test"));
    f.no_empty_strings(ctx, root);
    // FRD §6.8: the JSON request is captured; the audio response is not.
    let body = text(root, REQUEST_BODY).unwrap_or_default();
    let parsed: Json = serde_json::from_str(&body).unwrap_or(Json::Null);
    f.check(parsed["input"] == json!(input), || {
        format!("{ctx}: http.request.body is not the request JSON: {body:?}")
    });
    f.absent(ctx, root, &[RESPONSE_BODY]);
    f.absent(ctx, turn, &[REQUEST_BODY, RESPONSE_BODY]);
    f.assert_none();
}

// =============================================================================================
// Published deployments and the endpoint-id boundary
// =============================================================================================

/// The customer key and budapp's published overlay (`published_model_info`), which lists TTS_EP
/// under the name customers call it by.
fn published_extra() -> Vec<(String, String)> {
    vec![
        (
            format!("api_key:{}", bud_auth::hash_api_key(CLIENT_KEY)),
            json!({"__metadata__": {"api_key_id": CLIENT_API_KEY, "user_id": CLIENT_USER,
                                    "api_key_project_id": CLIENT_PROJECT}})
            .to_string(),
        ),
        (
            "published_model_info".to_string(),
            json!({"pub-tts": {"endpoint_id": TTS_EP, "model_id": TTS_MODEL_ID,
                               "project_id": EP_PROJECT, "kind": "model", "created_at": 1}})
            .to_string(),
        ),
    ]
}

/// The reported defect: a customer key named a PUBLISHED voice deployment and got
/// `404 Model 'pub-tts' not found`, because WaaV only ever read the key's own map. budgateway
/// extends a `bud_client_*` key's map with the published overlay; WaaV now does the same, and the
/// call is attributed to BOTH projects (CONTRACTS §1): the endpoint's and the key's.
#[tokio::test]
async fn a_published_deployment_serves_a_customer_key_attributed_to_both_projects() {
    let cap = Capture::install();
    let vendor = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let app = gateway_with(
        vec![(
            TTS_EP,
            tts_entry(
                &base(&vendor),
                Some(pricing("character", 30.0, 1_000_000)),
                None,
            ),
        )],
        &published_extra(),
    )
    .await;

    let reply = speech_as(
        &app,
        CLIENT_KEY,
        json!({"model": "pub-tts", "input": sentence(120), "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    let root = root_of(&spans);
    let mut f = Findings::default();
    for (ctx, span) in [("published turn", turn), ("published root", root)] {
        f.eq_text(ctx, span, ENDPOINT_ID, Some(TTS_EP));
        f.eq_text(ctx, span, MODEL_ID, Some(TTS_MODEL_ID));
        f.eq_text(ctx, span, PROJECT_ID, Some(EP_PROJECT));
        f.eq_text(ctx, span, API_KEY_PROJECT_ID, Some(CLIENT_PROJECT));
        f.eq_text(ctx, span, USER_ID, Some(CLIENT_USER));
        f.eq_text(ctx, span, API_KEY_ID, Some(CLIENT_API_KEY));
    }
    f.eq_text("published turn", turn, ENDPOINT_NAME, Some("pub-tts"));
    f.eq_number("published turn", turn, COST, Some(0.0036));
    f.assert_none();
}

/// The other half of the boundary. A caller could name an endpoint by its id instead of an alias
/// (DEG-3), and WaaV resolved ANY id in the voice table: an authenticated key reached every voice
/// deployment whose id it knew, published or not, in any project. Now an id resolves only through
/// the caller's own allowlist (and, for a customer key, the published overlay) — and is then
/// attributed like the alias that reaches it.
#[tokio::test]
async fn an_endpoint_id_outside_the_callers_allowlist_is_not_found() {
    let cap = Capture::install();
    let vendor = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let app = gateway_with(
        vec![
            (TTS_EP, tts_entry(&base(&vendor), None, None)),
            (UNLISTED_EP, tts_entry(&base(&vendor), None, None)),
        ],
        &published_extra(),
    )
    .await;
    let body = |model: &str| json!({"model": model, "input": "hello", "voice": "George", "response_format": "pcm"});

    for bearer in [KEY, CLIENT_KEY] {
        let reply = speech_as(&app, bearer, body(UNLISTED_EP)).await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{bearer} reached an endpoint no allowlist names: {}",
            reply.text()
        );
    }

    // The key's own endpoint by id, and a published one by id for the customer key, still work.
    for bearer in [KEY, CLIENT_KEY] {
        let _ = cap.take().await;
        let reply = speech_as(&app, bearer, body(TTS_EP)).await;
        assert_eq!(reply.status, StatusCode::OK, "{bearer}: {}", reply.text());
        let spans = cap.take().await;
        let turn = only(&spans, "voice.turn");
        let mut f = Findings::default();
        // Named by id, attributed through the entry that reaches it (no longer DEG-3's gap).
        f.eq_text(bearer, turn, MODEL_ID, Some(TTS_MODEL_ID));
        f.eq_text(bearer, turn, PROJECT_ID, Some(EP_PROJECT));
        f.assert_none();
    }
}

/// budgateway extends only a `bud_client_*` key with the overlay: an admin or project key reaches
/// its own project's deployments, not every project's published ones.
#[tokio::test]
async fn the_published_overlay_does_not_extend_a_non_customer_key() {
    let vendor = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let app = gateway_with(
        vec![(TTS_EP, tts_entry(&base(&vendor), None, None))],
        &published_extra(),
    )
    .await;
    let reply = speech(
        &app,
        json!({"model": "pub-tts", "input": "hello", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.text());
}

/// TC-EMIT-01 (STT + translation success), TC-EMIT-02 (STT), TC-EMIT-05 (`second`),
/// TC-EMIT-10 (self-hosted: detected language, no confidence), TC-EMIT-11 (STT),
/// TC-TRACE-01 (success rows).
#[tokio::test]
async fn stt_success_records_the_same_on_transcription_and_translation() {
    let cap = Capture::install();
    let vendor = stt_vendor().await;
    let app = gateway(vec![(
        STT_EP,
        stt_entry(&base(&vendor), Some(pricing("second", 0.0001, 1))),
    )])
    .await;
    let wav = wav_secs(12.5, 16_000);

    let mut f = Findings::default();
    for (route, capability) in [
        ("/v1/audio/transcriptions", "audio_transcription"),
        ("/v1/audio/translations", "audio_translation"),
    ] {
        let reply = upload(
            &app,
            route,
            &[
                ("model", "stt-a"),
                ("response_format", "verbose_json"),
                ("prompt", "cardiology terms"),
            ],
            ("visit.wav", "audio/wav", &wav),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{route}: {}", reply.text());

        let spans = cap.take().await;
        let turn = only(&spans, "voice.turn");
        let ctx = capability;
        f.present(ctx, turn, STT_SUCCESS);
        f.eq_text(ctx, turn, CAPABILITY, Some(capability));
        f.eq_text(ctx, turn, TRANSPORT, Some("http"));
        f.eq_text(ctx, turn, ENDPOINT_ID, Some(STT_EP));
        f.eq_text(ctx, turn, ENDPOINT_NAME, Some("stt-a"));
        f.eq_text(ctx, turn, MODEL_ID, Some(STT_MODEL_ID));
        f.eq_text(ctx, turn, PROJECT_ID, Some(EP_PROJECT));
        f.eq_text(ctx, turn, API_KEY_PROJECT_ID, Some(KEY_PROJECT));
        f.eq_text(ctx, turn, STT_VENDOR, Some("self_hosted"));
        f.eq_text(ctx, turn, STT_MODEL, Some(STT_VENDOR_MODEL));
        f.eq_number(ctx, turn, AUDIO_SECONDS, Some(12.5));
        // 12.5 s / 1 × 0.0001
        f.eq_number(ctx, turn, COST, Some(0.00125));
        f.eq_text(ctx, turn, PRICING_UNIT, Some("second"));
        f.eq_number(ctx, turn, INPUT_AUDIO_BYTES, Some(wav.len() as f64));
        f.eq_text(ctx, turn, AUDIO_FORMAT, Some("wav"));
        f.eq_number(ctx, turn, SAMPLE_RATE, Some(16_000.0));
        f.eq_text(ctx, turn, DETECTED_LANGUAGE, Some("es"));
        f.eq_text(ctx, turn, VENDOR_REQUEST_ID, Some("vendor-req-7"));
        // A self-hosted server reports no result-level confidence; none is invented (DEG-5).
        f.absent(
            ctx,
            turn,
            &[STT_CONFIDENCE, LANGUAGE, ERROR_TYPE, VENDOR_STATUS],
        );
        f.no_empty_strings(ctx, turn);
        f.check(!is_error(turn), || {
            format!("{ctx}: turn status {:?}", turn.status)
        });

        check_root(&mut f, ctx, &spans, route, &reply, Some(STT_VENDOR_MODEL));
        let root = root_of(&spans);
        // FRD §6.8: the form fields plus file metadata — never the audio.
        let req = text(root, REQUEST_BODY).unwrap_or_default();
        let parsed: Json = serde_json::from_str(&req).unwrap_or(Json::Null);
        f.check(
            parsed["model"] == "stt-a"
                && parsed["prompt"] == "cardiology terms"
                && parsed["response_format"] == "verbose_json"
                && parsed["file"]["filename"] == "visit.wav"
                && parsed["file"]["content_type"] == "audio/wav"
                && parsed["file"]["bytes"] == json!(wav.len()),
            || format!("{ctx}: http.request.body {req:?}"),
        );
        f.check(req.len() < 1024, || {
            format!(
                "{ctx}: http.request.body is {} bytes; audio leaked?",
                req.len()
            )
        });
        let resp = text(root, RESPONSE_BODY).unwrap_or_default();
        let returned: Json = serde_json::from_slice(&reply.body).unwrap_or(Json::Null);
        f.check(
            serde_json::from_str::<Json>(&resp).ok() == Some(returned.clone()),
            || format!("{ctx}: http.response.body {resp:?} is not what was returned {returned}"),
        );
    }
    f.assert_none();
}

/// TC-EMIT-05 (`minute`, `request`).
#[tokio::test]
async fn minute_and_request_pricing() {
    let cap = Capture::install();
    let stt = stt_vendor().await;
    let tts = tts_vendor(vec![0u8; 4_800], "audio/pcm").await;
    let app = gateway(vec![
        (
            STT_EP,
            stt_entry(&base(&stt), Some(pricing("minute", 0.0043, 1))),
        ),
        (
            TTS_EP,
            tts_entry(&base(&tts), Some(pricing("request", 0.002, 1)), None),
        ),
    ])
    .await;
    let mut f = Findings::default();

    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a")],
        ("long.wav", "audio/wav", &wav_secs(90.0, 8_000)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    f.eq_number("stt minute", turn, AUDIO_SECONDS, Some(90.0));
    // 90 s / 60 × 0.0043
    f.eq_number("stt minute", turn, COST, Some(0.00645));
    f.eq_text("stt minute", turn, PRICING_UNIT, Some("minute"));

    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    f.eq_number("tts request", turn, COST, Some(0.002));
    f.eq_text("tts request", turn, PRICING_UNIT, Some("request"));
    f.assert_none();
}

/// TC-EMIT-06: no pricing, a unit that does not apply, an unknown output duration, `per_units`
/// = 0 and a malformed block all leave the call unpriced — and none of them drops the endpoint.
#[tokio::test]
async fn unpriced_calls_record_no_cost() {
    let cap = Capture::install();
    let stt = stt_vendor().await;
    let mp3 = {
        let mut b = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
        b.extend(std::iter::repeat_n(0x55u8, 4_000));
        b
    };
    let tts_mp3 = tts_vendor(mp3, "audio/mpeg").await;
    let tts_pcm = tts_vendor(vec![0u8; 4_800], "audio/pcm").await;

    let cases: Vec<(&str, Json, bool)> = vec![
        (
            "tts, no pricing",
            tts_entry(&base(&tts_pcm), None, None),
            true,
        ),
        (
            "tts per second, mp3 (duration unreadable)",
            tts_entry(&base(&tts_mp3), Some(pricing("second", 0.001, 1)), None),
            true,
        ),
        (
            "tts per_units = 0",
            tts_entry(&base(&tts_pcm), Some(pricing("character", 30.0, 0)), None),
            true,
        ),
        (
            "tts malformed pricing",
            tts_entry(
                &base(&tts_pcm),
                Some(json!({"unit": "character", "cost_per_unit": "thirty"})),
                None,
            ),
            true,
        ),
        (
            "stt per character",
            stt_entry(&base(&stt), Some(pricing("character", 1.0, 1))),
            false,
        ),
    ];

    let mut f = Findings::default();
    for (ctx, entry, is_tts) in cases {
        let app = if is_tts {
            gateway(vec![(TTS_EP, entry)]).await
        } else {
            gateway(vec![(STT_EP, entry)]).await
        };
        let reply = if is_tts {
            let format = if ctx.contains("mp3") { "mp3" } else { "pcm" };
            speech(
                &app,
                json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": format}),
            )
            .await
        } else {
            upload(
                &app,
                "/v1/audio/transcriptions",
                &[("model", "stt-a")],
                ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
            )
            .await
        };
        f.check(reply.status == StatusCode::OK, || {
            format!("{ctx}: HTTP {} {}", reply.status, reply.text())
        });
        let spans = cap.take().await;
        let turn = only(&spans, "voice.turn");
        f.absent(ctx, turn, &[COST, PRICING_UNIT]);
        f.present(
            ctx,
            turn,
            if is_tts {
                &[CHARACTERS]
            } else {
                &[AUDIO_SECONDS]
            },
        );
    }
    f.assert_none();
}

/// TC-EMIT-02 variant (DEG-3): a caller that names the endpoint id itself — an id its allowlist
/// reaches — is attributed through the entry that reaches it, exactly as if it had used the alias.
/// (It used to get no model and the KEY's project, because an id resolved without any entry.)
#[tokio::test]
async fn a_raw_endpoint_id_is_attributed_through_the_entry_that_reaches_it() {
    let cap = Capture::install();
    let vendor = tts_vendor(vec![0u8; 4_800], "audio/pcm").await;
    let app = gateway(vec![(TTS_EP, tts_entry(&base(&vendor), None, None))]).await;
    let reply = speech(
        &app,
        json!({"model": TTS_EP, "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    let mut f = Findings::default();
    f.eq_text("raw id", turn, ENDPOINT_ID, Some(TTS_EP));
    f.eq_text("raw id", turn, ENDPOINT_NAME, Some(TTS_EP));
    f.eq_text("raw id", turn, PROJECT_ID, Some(EP_PROJECT));
    f.eq_text("raw id", turn, API_KEY_PROJECT_ID, Some(KEY_PROJECT));
    f.eq_text("raw id", turn, MODEL_ID, Some(TTS_MODEL_ID));
    f.assert_none();
}

// =============================================================================================
// Failure paths
// =============================================================================================

/// Assert one failure's classification on the child and the root (TC-EMIT-03, TC-EMIT-04,
/// TC-TRACE-01 error rows).
fn check_failure(
    f: &mut Findings,
    ctx: &str,
    spans: &[SpanData],
    route: &str,
    reply: &Reply,
    want_class: &str,
    want_vendor_status: Option<u16>,
) {
    let turns: Vec<&SpanData> = spans.iter().filter(|s| s.name == "voice.turn").collect();
    let Some(turn) = turns.first().copied() else {
        f.check(false, || {
            format!(
                "{ctx}: HTTP {} produced no voice.turn (exported {:?})",
                reply.status,
                names(spans)
            )
        });
        return;
    };
    f.check(is_error(turn), || {
        format!("{ctx}: voice.turn status {:?}, want Error", turn.status)
    });
    f.present(ctx, turn, FAILURE);
    f.eq_text(ctx, turn, ERROR_TYPE, Some(want_class));
    f.eq_number(
        ctx,
        turn,
        VENDOR_STATUS,
        want_vendor_status.map(|s| s as f64),
    );
    f.absent(ctx, turn, SUCCESS_ONLY);
    f.no_empty_strings(ctx, turn);
    let vendor_model = text(turn, TTS_MODEL).or(text(turn, STT_MODEL));
    check_root(f, ctx, spans, route, reply, vendor_model.as_deref());
    let root = root_of(spans);
    let captured = text(root, RESPONSE_BODY).unwrap_or_default();
    let returned: Json = serde_json::from_slice(&reply.body).unwrap_or(Json::Null);
    f.check(
        serde_json::from_str::<Json>(&captured).ok() == Some(returned.clone()),
        || {
            format!(
                "{ctx}: the error returned ({returned}) is not http.response.body ({captured:?})"
            )
        },
    );
}

/// ⚑ TC-EMIT-03 (TTS rows), TC-EMIT-04. Each vendor answer is classified from its numeric status,
/// which is carried to the handler instead of being folded into a message.
#[tokio::test]
async fn tts_vendor_failures_are_classified() {
    let cap = Capture::install();
    let vendor = MockServer::start().await;
    let plan =
        json!({"detail": {"status": "subscription_required", "message": "Upgrade your plan"}});
    for (marker, status, body) in [
        (
            "status-429",
            429,
            json!({"detail": "too many concurrent requests"}),
        ),
        ("status-408", 408, json!({"detail": "request timeout"})),
        ("status-401", 401, json!({"detail": "invalid api key"})),
        ("status-402", 402, json!({"detail": "payment required"})),
        ("status-403", 403, plan.clone()),
        ("status-404", 404, json!({"detail": "voice not found"})),
        ("status-500", 500, json!({"detail": "internal"})),
    ] {
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .and(BodyHas(marker))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&vendor)
            .await;
    }
    let app = gateway(vec![(
        TTS_EP,
        tts_entry(
            &base(&vendor),
            Some(pricing("character", 30.0, 1_000_000)),
            None,
        ),
    )])
    .await;

    let mut f = Findings::default();
    for (marker, class, vendor_status) in [
        ("status-429", "rate_limited", 429),
        ("status-408", "vendor_timeout", 408),
        ("status-401", "auth", 401),
        ("status-402", "auth", 402),
        ("status-403", "auth", 403),
        ("status-404", "vendor_rejected", 404),
        ("status-500", "vendor_5xx", 500),
    ] {
        let reply = speech(
            &app,
            json!({"model": "tts-a", "input": format!("{marker} please"), "voice": "George", "response_format": "pcm"}),
        )
        .await;
        f.check(!reply.status.is_success(), || {
            format!("tts {marker}: HTTP {}", reply.status)
        });
        let spans = cap.take().await;
        check_failure(
            &mut f,
            &format!("tts {marker}"),
            &spans,
            "/v1/audio/speech",
            &reply,
            class,
            Some(vendor_status),
        );
    }
    f.assert_none();
}

/// ⚑ TC-EMIT-03 (TTS: connect refused, deployment deadline, no credential, bad override).
#[tokio::test]
async fn tts_failures_without_a_vendor_answer_are_classified() {
    let cap = Capture::install();
    let slow = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0u8; 4_800])
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&slow)
        .await;
    let mut f = Findings::default();

    // Connection refused.
    let app = gateway(vec![(TTS_EP, tts_entry(&closed_port_base(), None, None))]).await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "tts refused",
        &spans,
        "/v1/audio/speech",
        &reply,
        "network",
        None,
    );

    // The deployment's request_timeout (1 s) elapses before the vendor (3 s) answers.
    let app = gateway(vec![(
        TTS_EP,
        tts_entry(
            &base(&slow),
            None,
            Some(json!({"tts": {"request_timeout": 1}})),
        ),
    )])
    .await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "tts deadline",
        &spans,
        "/v1/audio/speech",
        &reply,
        "deadline",
        None,
    );

    // A hosted vendor with no credential: the deployment cannot be served as configured.
    let app = gateway(hosted_entries()).await;
    let reply = speech(
        &app,
        json!({"model": "dg-tts", "input": "Hello.", "response_format": "mp3"}),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "tts no credential",
        &spans,
        "/v1/audio/speech",
        &reply,
        "config",
        None,
    );

    // A per-request override outside the canonical vocabulary: Bud-side validation.
    let app = gateway(vec![(TTS_EP, tts_entry(&base(&slow), None, None))]).await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello.", "voice": "George", "emotion": "zzz-not-an-emotion"}),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "tts bad override",
        &spans,
        "/v1/audio/speech",
        &reply,
        "invalid_request",
        None,
    );
    f.assert_none();
}

/// Release 5 data settings on the upload route. Azure OpenAI keeps audio by account setting, so a
/// deployment asking for no retention is refused before anything is sent, on the passthrough that
/// forwards the file whole (found on pde-ditto: that branch returned before the check). A
/// self-hosted server is the operator's own, so it carries both settings and is served.
#[tokio::test]
async fn the_upload_route_refuses_a_data_setting_the_vendor_cannot_carry() {
    let vendor = stt_vendor().await;
    Mock::given(method("POST"))
        .and(path(
            "/openai/deployments/whisper-deploy/audio/transcriptions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({"text": "from azure"})),
        )
        .mount(&vendor)
        .await;
    let azure = json!({
        "vendor": "azure_openai",
        "api_base": vendor.uri(),
        "endpoints": ["audio_transcription"],
        "model": "whisper-deploy",
        "credential": TEST_CREDENTIAL.trim(),
        "config": {"stt": {"data_retention": "none"}},
    });
    let app = gateway(vec![(STT_EP, azure)]).await;
    let wav = wav_secs(1.0, 16_000);
    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a")],
        ("a.wav", "audio/wav", &wav),
    )
    .await;
    assert_eq!(reply.status, 500, "{}", reply.text());
    assert!(
        reply.text().contains("stt_data_setting_unavailable"),
        "{}",
        reply.text()
    );
    assert!(
        vendor
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "no audio reached the vendor"
    );

    // A self-hosted server carries both: it is the operator's own address.
    let mut hosted = stt_entry(&base(&vendor), None);
    hosted["config"] = json!({"stt": {"data_retention": "none", "data_region": "eu"}});
    let app = gateway(vec![(STT_EP, hosted)]).await;
    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a")],
        ("a.wav", "audio/wav", &wav),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
}

/// ⚑ TC-EMIT-03 (STT rows), TC-EMIT-04.
#[tokio::test]
async fn stt_failures_are_classified() {
    let cap = Capture::install();
    let vendor = MockServer::start().await;
    for (marker, status) in [
        ("m-429", 429),
        ("m-408", 408),
        ("m-401", 401),
        ("m-404", 404),
        ("m-500", 500),
    ] {
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(BodyHas(marker))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(json!({"error": {"message": format!("vendor said {status}")}})),
            )
            .mount(&vendor)
            .await;
    }
    let mut table = hosted_entries();
    table.push((
        STT_EP,
        stt_entry(&base(&vendor), Some(pricing("second", 0.0001, 1))),
    ));
    let app = gateway(table).await;
    let wav = wav_secs(1.0, 16_000);
    let route = "/v1/audio/transcriptions";
    let mut f = Findings::default();

    for (marker, class, status) in [
        ("m-429", "rate_limited", 429),
        ("m-408", "vendor_timeout", 408),
        ("m-401", "auth", 401),
        ("m-404", "vendor_rejected", 404),
        ("m-500", "vendor_5xx", 500),
    ] {
        // The marker rides the prompt, which the self-hosted path forwards verbatim.
        let reply = upload(
            &app,
            route,
            &[("model", "stt-a"), ("prompt", marker)],
            ("a.wav", "audio/wav", &wav),
        )
        .await;
        let spans = cap.take().await;
        check_failure(
            &mut f,
            &format!("stt {marker}"),
            &spans,
            route,
            &reply,
            class,
            Some(status),
        );
    }

    let refused = gateway(vec![(STT_EP, stt_entry(&closed_port_base(), None))]).await;
    let reply = upload(
        &refused,
        route,
        &[("model", "stt-a")],
        ("a.wav", "audio/wav", &wav),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "stt refused",
        &spans,
        route,
        &reply,
        "network",
        None,
    );

    for (ctx, fields, file, class) in [
        (
            "truncated mp3",
            // Decoding comes before the credential check, so no credential is needed.
            vec![("model", "dg-bare")],
            (
                "speech.mp3",
                "audio/mpeg",
                b"\xff\xfbfake mp3 body".to_vec(),
            ),
            "input_decode",
        ),
        (
            "opus upload",
            vec![("model", "dg-bare")],
            ("speech.ogg", "audio/ogg", HELLO_OPUS.to_vec()),
            "input_decode",
        ),
        (
            "self-hosted without api_base",
            vec![("model", "no-base")],
            ("a.wav", "audio/wav", wav.clone()),
            "config",
        ),
        (
            "hosted without credential",
            vec![("model", "dg-bare")],
            ("a.wav", "audio/wav", wav.clone()),
            "config",
        ),
        (
            "language with language_detection",
            vec![
                ("model", "dg-a"),
                ("language", "en"),
                ("language_detection", "true"),
            ],
            ("a.wav", "audio/wav", wav.clone()),
            "invalid_request",
        ),
    ] {
        // Only this case needs a credential that opens; without bud-auth's git-ignored fixture key
        // the endpoint cannot hydrate, so the case is skipped — loudly, never passed.
        if fields.iter().any(|(_, v)| *v == "dg-a") && test_pem().is_none() {
            eprintln!("SKIPPED stt {ctx}: bud-auth/tests/fixtures/test_cred_private.pem is absent");
            continue;
        }
        let reply = upload(&app, route, &fields, (file.0, file.1, &file.2)).await;
        f.check(!reply.status.is_success(), || {
            format!("stt {ctx}: HTTP {} {}", reply.status, reply.text())
        });
        let spans = cap.take().await;
        check_failure(
            &mut f,
            &format!("stt {ctx}"),
            &spans,
            route,
            &reply,
            class,
            None,
        );
    }
    f.assert_none();
}

/// DEG-2: a refusal before the endpoint is resolved is not a call — no `voice.turn` — but it is
/// a failed HTTP request on the SERVER root.
#[tokio::test]
async fn a_refusal_before_resolution_has_a_root_and_no_turn() {
    let cap = Capture::install();
    let app = gateway(vec![]).await;
    let reply = speech(&app, json!({"model": "no-such-model", "input": "Hello."})).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.text());
    let spans = cap.take().await;
    let mut f = Findings::default();
    f.check(!spans.iter().any(|s| s.name == "voice.turn"), || {
        "an unresolved model produced a voice.turn".to_string()
    });
    let root = root_of(&spans);
    f.check(
        root.name == "POST /v1/audio/speech" && root.span_kind == SpanKind::Server,
        || format!("root {:?} {:?}", root.name, root.span_kind),
    );
    f.eq_text("unresolved", root, "error.type", Some("404"));
    f.check(!is_error(root), || {
        "a 404 marked the root ERROR".to_string()
    });
    f.assert_none();
}

// =============================================================================================
// Phase 5 signals
// =============================================================================================

/// TC-EMIT-08: vendor time to first audio, not the whole call.
#[tokio::test]
async fn tts_ttfb_is_the_vendors_first_audio() {
    let cap = Capture::install();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let vendor = axum::Router::new().route(
        "/v1/audio/speech",
        axum::routing::post(|| async {
            let chunks = vec![
                (Duration::from_millis(300), vec![0u8; 4_800]),
                (Duration::from_millis(700), vec![0u8; 43_200]),
            ];
            let stream = futures::stream::iter(chunks).then(|(delay, bytes)| async move {
                tokio::time::sleep(delay).await;
                Ok::<Bytes, std::io::Error>(Bytes::from(bytes))
            });
            axum::response::Response::builder()
                .header("content-type", "audio/pcm")
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, vendor).await;
    });

    let app = gateway(vec![(
        TTS_EP,
        tts_entry(&format!("http://{addr}/v1"), None, None),
    )])
    .await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    let ttfb = number(turn, TTS_TTFB);
    let duration = number(turn, TTS_DURATION);
    assert!(
        ttfb.is_some_and(|t| (300.0..400.0).contains(&t)),
        "tts.ttfb_ms = {ttfb:?}, want [300, 400)"
    );
    assert!(
        duration.is_some_and(|d| d >= 1000.0),
        "tts.duration_ms = {duration:?}, want ≥ 1000"
    );
    assert_eq!(number(turn, OUTPUT_AUDIO_SECONDS), Some(1.0));
}

/// `frames` silent MPEG-1 Layer III frames at 128 kb/s, 44.1 kHz mono: 417 bytes and 1152
/// samples each.
fn mp3_frames(frames: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(frames * 417);
    for _ in 0..frames {
        v.extend_from_slice(&[0xFF, 0xFB, 0x90, 0xC0]);
        v.resize(v.len() + 413, 0);
    }
    v
}

/// TC-EMIT-09: WAV duration from its header; a compressed format's from its container, demuxed
/// but not decoded; an unreadable one stays unknown (DEG-4).
#[tokio::test]
async fn tts_output_duration_for_wav_and_compressed_formats() {
    let cap = Capture::install();
    let wav = tts_vendor(wav_secs(1.5, 24_000), "audio/wav").await;
    let mp3 = tts_vendor(mp3_frames(100), "audio/mpeg").await;
    let opus = tts_vendor(HELLO_OPUS.to_vec(), "audio/ogg").await;
    let junk = tts_vendor(
        {
            let mut b = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
            b.extend(std::iter::repeat_n(0x55u8, 4_000));
            b
        },
        "audio/mpeg",
    )
    .await;
    let mut f = Findings::default();
    for (ctx, vendor, format, want_secs, want_rate) in [
        ("wav", &wav, "wav", Some(1.5), Some(24_000.0)),
        (
            "mp3",
            &mp3,
            "mp3",
            Some(100.0 * 1152.0 / 44_100.0),
            Some(44_100.0),
        ),
        // "Hello from Bud." from ElevenLabs: 88,320 samples at 48 kHz after the pre-skip.
        ("opus", &opus, "opus", Some(1.84), Some(48_000.0)),
        ("unreadable mp3", &junk, "mp3", None, None),
    ] {
        let app = gateway(vec![(TTS_EP, tts_entry(&base(vendor), None, None))]).await;
        let reply = speech(
            &app,
            json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": format}),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{ctx}: {}", reply.text());
        let spans = cap.take().await;
        let turn = only(&spans, "voice.turn");
        f.eq_number(ctx, turn, OUTPUT_AUDIO_SECONDS, want_secs);
        f.eq_number(ctx, turn, SAMPLE_RATE, want_rate);
        f.eq_text(ctx, turn, AUDIO_FORMAT, Some(format));
        let root = root_of(&spans);
        f.eq_number(ctx, root, OUTPUT_AUDIO_SECONDS, want_secs);
    }
    f.assert_none();
}

// =============================================================================================
// Probes and other routes (TC-TRACE-02)
// =============================================================================================

/// TC-TRACE-02: the operability routes export no span but keep their request id; other routes
/// keep today's INTERNAL `request` root (NG-10).
#[tokio::test]
async fn probes_export_no_span_and_other_routes_keep_the_internal_root() {
    let cap = Capture::install();
    let app = gateway(vec![]).await;
    let mut f = Findings::default();
    for (i, probe) in ["/", "/ready", "/livez", "/readyz", "/metrics"]
        .into_iter()
        .enumerate()
    {
        let id = format!("probe-{i}");
        let reply = send(
            &app,
            Request::get(probe)
                .header("x-request-id", &id)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let echoed = reply
            .headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok());
        f.check(echoed == Some(id.as_str()), || {
            format!("{probe}: x-request-id echoed as {echoed:?}")
        });
        let spans = cap.drain().await;
        f.check(spans.is_empty(), || {
            format!("{probe} exported {:?}", names(&spans))
        });
    }

    let reply = send(
        &app,
        Request::get("/voices")
            .header("x-request-id", "voices-1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    f.check(
        reply
            .headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            == Some("voices-1"),
        || "/voices lost its x-request-id".to_string(),
    );
    let spans = cap.take().await;
    let root = root_of(&spans);
    f.check(
        root.name == "request" && root.span_kind == SpanKind::Internal,
        || format!("/voices root is {:?} {:?}", root.name, root.span_kind),
    );
    f.eq_text("/voices", root, "request_id", Some("voices-1"));
    f.assert_none();
}

// =============================================================================================
// Process-wide switches, in child processes (TC-TRACE-05, TC-TRACE-06, TC-TRACE-07)
// =============================================================================================

const CHILD_MARKER: &str = "VOICE_HTTP_SPANS_CHILD";

fn is_child() -> bool {
    std::env::var_os(CHILD_MARKER).is_some()
}

/// Run one ignored test of this binary in a fresh process with `env` set.
fn run_child(test: &str, env: &[(&str, &str)]) {
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            test,
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_MARKER, "1")
        .env_remove("BUD_TRACE_CAPTURE_CONTENT")
        .env_remove("BUD_TRACE_BODY_MAX_BYTES")
        .env_remove("WAAV_HTTP_SERVER_SPAN")
        .envs(env.iter().copied())
        .output()
        .expect("child runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("1 passed"),
        "child `{test}` failed ({:?}):\n{stdout}\n{stderr}",
        out.status
    );
}

#[test]
fn content_capture_is_bounded_redacted_and_never_audio() {
    run_child(
        "child_capture_bounded",
        &[("BUD_TRACE_BODY_MAX_BYTES", "1024")],
    );
}

#[test]
fn capture_off_records_no_bodies_but_every_metric() {
    run_child(
        "child_capture_off",
        &[("BUD_TRACE_CAPTURE_CONTENT", "false")],
    );
}

#[test]
fn the_server_span_switch_restores_the_internal_root() {
    run_child(
        "child_server_span_off",
        &[("WAAV_HTTP_SERVER_SPAN", "false")],
    );
}

/// TC-TRACE-05, with `BUD_TRACE_BODY_MAX_BYTES=1024`.
#[tokio::test]
#[ignore = "run by content_capture_is_bounded_redacted_and_never_audio"]
async fn child_capture_bounded() {
    if !is_child() {
        return;
    }
    let cap = Capture::install();
    let tts = tts_vendor(vec![0u8; 4_800], "audio/pcm").await;
    let stt = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"text": "patient reports chest pain"})),
        )
        .mount(&stt)
        .await;
    let app = gateway(vec![
        (TTS_EP, tts_entry(&base(&tts), None, None)),
        (STT_EP, stt_entry(&base(&stt), None)),
    ])
    .await;
    let mut f = Findings::default();

    let input = sentence(3_000);
    let reply = speech(
        &app,
        json!({"api_key": "sk-caller-secret", "model": "tts-a", "input": input, "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let root = root_of(&spans);
    let body = text(root, REQUEST_BODY).unwrap_or_default();
    let kept = body.strip_suffix("...[truncated]");
    f.check(
        kept.is_some_and(|k| k.len() <= 1024 && k.starts_with('{')),
        || {
            format!(
                "tts request body not truncated at 1024 bytes: {} bytes, ends {:?}",
                body.len(),
                body.get(body.len().saturating_sub(20)..)
            )
        },
    );
    f.check(body.contains("\"input\""), || {
        "tts request body lost `input`".to_string()
    });
    f.check(
        body.contains("\"api_key\":\"***\"") && !body.contains("sk-caller-secret"),
        || format!("api_key not redacted: {body:?}"),
    );
    f.absent("tts", root, &[RESPONSE_BODY]);

    let wav = wav_of_len(204_800, 16_000);
    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a"), ("prompt", "cardiology terms")],
        ("visit.wav", "audio/wav", &wav),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let root = root_of(&spans);
    let req = text(root, REQUEST_BODY).unwrap_or_default();
    let parsed: Json = serde_json::from_str(&req).unwrap_or(Json::Null);
    f.check(
        parsed["file"]["bytes"] == json!(204_800)
            && parsed["file"]["filename"] == "visit.wav"
            && parsed["prompt"] == "cardiology terms"
            && req.len() < 1024,
        || format!("stt request body: {req:?}"),
    );
    let resp = text(root, RESPONSE_BODY).unwrap_or_default();
    f.check(
        serde_json::from_str::<Json>(&resp).ok()
            == serde_json::from_slice::<Json>(&reply.body).ok(),
        || format!("stt response body {resp:?} vs returned {}", reply.text()),
    );
    f.assert_none();
}

/// TC-TRACE-06, with `BUD_TRACE_CAPTURE_CONTENT=false`.
#[tokio::test]
#[ignore = "run by capture_off_records_no_bodies_but_every_metric"]
async fn child_capture_off() {
    if !is_child() {
        return;
    }
    let cap = Capture::install();
    let tts = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let stt = stt_vendor().await;
    let app = gateway(vec![
        (
            TTS_EP,
            tts_entry(
                &base(&tts),
                Some(pricing("character", 30.0, 1_000_000)),
                None,
            ),
        ),
        (
            STT_EP,
            stt_entry(&base(&stt), Some(pricing("second", 0.0001, 1))),
        ),
    ])
    .await;
    let mut f = Findings::default();

    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    f.present("tts, capture off", only(&spans, "voice.turn"), TTS_SUCCESS);
    for s in &spans {
        f.absent("tts, capture off", s, &[REQUEST_BODY, RESPONSE_BODY]);
    }

    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a"), ("response_format", "verbose_json")],
        ("visit.wav", "audio/wav", &wav_secs(2.0, 16_000)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    f.present("stt, capture off", only(&spans, "voice.turn"), STT_SUCCESS);
    for s in &spans {
        f.absent("stt, capture off", s, &[REQUEST_BODY, RESPONSE_BODY]);
    }

    let reply = speech(&app, json!({"model": "no-such-model", "input": "Hello."})).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let spans = cap.take().await;
    for s in &spans {
        f.absent("error, capture off", s, &[REQUEST_BODY, RESPONSE_BODY]);
    }
    f.assert_none();
}

/// TC-TRACE-07, with `WAAV_HTTP_SERVER_SPAN=false`.
#[tokio::test]
#[ignore = "run by the_server_span_switch_restores_the_internal_root"]
async fn child_server_span_off() {
    if !is_child() {
        return;
    }
    let cap = Capture::install();
    let tts = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let app = gateway(vec![(
        TTS_EP,
        tts_entry(
            &base(&tts),
            Some(pricing("character", 30.0, 1_000_000)),
            None,
        ),
    )])
    .await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let mut f = Findings::default();
    let root = root_of(&spans);
    f.check(
        root.name == "request" && root.span_kind == SpanKind::Internal,
        || format!("root is {:?} {:?}", root.name, root.span_kind),
    );
    f.absent(
        "server span off",
        root,
        &["http.route", PROJECT_ID, REQUEST_BODY],
    );
    let turn = only(&spans, "voice.turn");
    f.present("server span off", turn, TTS_SUCCESS);
    f.eq_text("server span off", turn, ENDPOINT_ID, Some(TTS_EP));
    f.assert_none();
}

// =============================================================================================
// Hosted prerecorded vendors, below the handler (TC-EMIT-03 STT 408/429, TC-EMIT-10, TC-EMIT-11)
//
// Their base URLs are compiled in, so the handler cannot be pointed at a mock; the driver it
// calls can, through `endpoint_override`, which the SSRF gate lets reach loopback only with
// `WAAV_ALLOW_LOOPBACK_ENDPOINTS` — set for the child process from its start rather than mutated
// under the other tests of this binary.
// =============================================================================================

#[test]
fn prerecorded_vendors_carry_confidence_request_id_and_status() {
    run_child(
        "child_prerecorded_signals",
        &[("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1")],
    );
}

async fn prerecorded(
    vendor: &str,
    model: &str,
    base: &str,
) -> Result<
    waav_gateway::handlers::transcribe::Transcript,
    waav_gateway::handlers::transcribe::TranscribeFailure,
> {
    let config = waav_gateway::core::stt::STTConfig {
        provider: vendor.to_string(),
        api_key: "vendor-test-key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16_000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: model.to_string(),
    };
    let std = waav_gateway::core::stt::standard::StandardSTTConfig::from_base(config)
        .with_endpoint_override(base);
    let pcm = waav_openai_audio::pcm::PcmAudio {
        samples: vec![0; 16_000],
        sample_rate: 16_000,
    };
    waav_gateway::handlers::transcribe::transcribe_once_standard(vendor, std, &pcm).await
}

#[tokio::test]
#[ignore = "run by prerecorded_vendors_carry_confidence_request_id_and_status"]
async fn child_prerecorded_signals() {
    if !is_child() {
        return;
    }
    let mut f = Findings::default();

    // Deepgram with a confidence, a detected language and a request id in its metadata.
    let dg = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "metadata": {"request_id": "dg-req-1", "duration": 1.0},
            "results": {"channels": [{
                "detected_language": "es",
                "alternatives": [{"transcript": "hola", "confidence": 0.83, "words": []}]
            }]}
        })))
        .mount(&dg)
        .await;
    match prerecorded("deepgram", "nova-3", &dg.uri()).await {
        Ok(t) => {
            f.check(
                t.confidence.is_some_and(|c| (c - 0.83).abs() < 1e-6),
                || format!("deepgram confidence {:?}, want 0.83", t.confidence),
            );
            f.check(t.detected_language.as_deref() == Some("es"), || {
                format!("deepgram detected_language {:?}", t.detected_language)
            });
            f.check(t.vendor_request_id.as_deref() == Some("dg-req-1"), || {
                format!("deepgram vendor_request_id {:?}", t.vendor_request_id)
            });
        }
        Err(e) => f.check(false, || format!("deepgram with confidence failed: {e}")),
    }

    // Deepgram without a confidence: absent, not the parser's 1.0 default (DEG-5).
    let dg_bare = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("dg-request-id", "dg-hdr-2")
                .set_body_json(json!({
                    "results": {"channels": [{"alternatives": [{"transcript": "hola"}]}]}
                })),
        )
        .mount(&dg_bare)
        .await;
    match prerecorded("deepgram", "nova-3", &dg_bare.uri()).await {
        Ok(t) => {
            f.check(t.confidence.is_none(), || {
                format!("deepgram without confidence reported {:?}", t.confidence)
            });
            // No id in the body: the response header's.
            f.check(t.vendor_request_id.as_deref() == Some("dg-hdr-2"), || {
                format!("header request id {:?}", t.vendor_request_id)
            });
        }
        Err(e) => f.check(false, || format!("deepgram without confidence failed: {e}")),
    }

    // ElevenLabs reports a language probability, never a transcript confidence.
    let el = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/speech-to-text"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "text": "hello", "language_code": "en", "language_probability": 0.98, "words": []
        })))
        .mount(&el)
        .await;
    match prerecorded("elevenlabs", "scribe_v1", &el.uri()).await {
        Ok(t) => f.check(t.confidence.is_none(), || {
            format!("elevenlabs reported a confidence {:?}", t.confidence)
        }),
        Err(e) => f.check(false, || format!("elevenlabs failed: {e}")),
    }

    // ⚑ TC-EMIT-03: a 408 and a 429 were the same `ProviderError` and nothing else.
    for (status, class) in [
        (429u16, "rate_limited"),
        (408, "vendor_timeout"),
        (401, "auth"),
    ] {
        let vendor = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"err_msg": "no"})))
            .mount(&vendor)
            .await;
        match prerecorded("deepgram", "nova-3", &vendor.uri()).await {
            Ok(_) => f.check(false, || format!("deepgram {status} succeeded")),
            Err(e) => {
                let failure = e.failure();
                f.check(
                    failure.class.as_str() == class && failure.vendor_status == Some(status),
                    || {
                        format!(
                            "deepgram {status}: class {} status {:?}, want {class} {status}",
                            failure.class, failure.vendor_status
                        )
                    },
                );
            }
        }
    }
    f.assert_none();
}

// =============================================================================================
// The vendor call span (CONTRACTS §1.2a): a CLIENT child of `voice.turn` per vendor request,
// showing what WaaV sent the vendor and what the vendor answered.
// =============================================================================================

const VENDOR_REQUEST_BODY: &str = "gen_ai.request.body";
const VENDOR_RESPONSE_BODY: &str = "gen_ai.response.body";

/// What every vendor span carries, whether or not content is captured.
const VENDOR_SHAPE: &[&str] = &[
    "gen_ai.operation.name",
    "gen_ai.provider.name",
    "gen_ai.request.model",
    "http.request.method",
    "server.address",
    "url.full",
];

/// §1.2a "Never": InferenceFact's discriminators and token usage.
fn never_on_a_vendor_span(key: &str) -> bool {
    key == "gen_ai.inference_id"
        || key.starts_with("gateway_analytics.")
        || key == "bud.prompt_id"
        || key.starts_with("gen_ai.usage.")
}

fn client_spans(spans: &[SpanData]) -> Vec<&SpanData> {
    spans
        .iter()
        .filter(|s| s.span_kind == SpanKind::Client)
        .collect()
}

/// With `VOICE_HTTP_SPANS_DUMP` set, print the exported tree: name, kind, parent and attribute
/// keys, for reading what a case produced.
fn dump_tree(ctx: &str, spans: &[SpanData]) {
    if std::env::var_os("VOICE_HTTP_SPANS_DUMP").is_none() {
        return;
    }
    fn walk(spans: &[SpanData], parent: SpanId, depth: usize) {
        for s in spans.iter().filter(|s| s.parent_span_id == parent) {
            eprintln!(
                "{}{} [{:?}] {:?}\n{}  keys: {:?}",
                "  ".repeat(depth),
                s.name,
                s.span_kind,
                s.status,
                "  ".repeat(depth),
                keys(s)
            );
            walk(spans, s.span_context.span_id(), depth + 1);
        }
    }
    eprintln!("--- {ctx}");
    walk(spans, SpanId::INVALID, 0);
}

fn json_attr(span: &SpanData, key: &str) -> Json {
    text(span, key)
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or(Json::Null)
}

struct WantVendor<'a> {
    name: &'a str,
    operation: &'a str,
    provider: &'a str,
    model: &'a str,
    /// Prefix of `url.full` (a vendor with query parameters is asserted on them separately).
    url: &'a str,
    /// The vendor's HTTP status, or `None` when no response arrived.
    status: Option<u16>,
    /// `error.type` on failure; `None` on success.
    error_type: Option<&'a str>,
}

/// The call's single vendor span, checked against §1.2a. `None` (and a finding) when the call did
/// not export exactly one CLIENT span.
fn check_vendor<'a>(
    f: &mut Findings,
    ctx: &str,
    spans: &'a [SpanData],
    want: &WantVendor<'_>,
) -> Option<&'a SpanData> {
    let clients = client_spans(spans);
    f.check(clients.len() == 1, || {
        format!(
            "{ctx}: want exactly one CLIENT (vendor) span, exported {:?}",
            spans
                .iter()
                .map(|s| format!("{} ({:?})", s.name, s.span_kind))
                .collect::<Vec<_>>()
        )
    });
    let vendor = *clients.first()?;
    let turn = only(spans, "voice.turn");
    f.check(vendor.name == want.name, || {
        format!(
            "{ctx}: vendor span is named {:?}, want {:?}",
            vendor.name, want.name
        )
    });
    f.check(
        vendor.parent_span_id == turn.span_context.span_id()
            && vendor.span_context.trace_id() == turn.span_context.trace_id(),
        || format!("{ctx}: the vendor span is not a child of voice.turn"),
    );
    f.present(ctx, vendor, VENDOR_SHAPE);
    f.eq_text(ctx, vendor, "gen_ai.operation.name", Some(want.operation));
    f.eq_text(ctx, vendor, "gen_ai.provider.name", Some(want.provider));
    f.eq_text(ctx, vendor, "gen_ai.request.model", Some(want.model));
    f.eq_text(ctx, vendor, "http.request.method", Some("POST"));
    f.eq_text(ctx, vendor, "server.address", Some("127.0.0.1"));
    let url = text(vendor, "url.full").unwrap_or_default();
    f.check(url.starts_with(want.url), || {
        format!("{ctx}: url.full = {url:?}, want it to start {:?}", want.url)
    });
    f.eq_number(
        ctx,
        vendor,
        "http.response.status_code",
        want.status.map(f64::from),
    );
    f.eq_text(ctx, vendor, "error.type", want.error_type);
    f.check(is_error(vendor) == want.error_type.is_some(), || {
        format!(
            "{ctx}: vendor span status {:?} (want Error exactly on failure)",
            vendor.status
        )
    });
    for id in SIX_IDS {
        f.check(
            text(vendor, id).is_some() && text(vendor, id) == text(turn, id),
            || {
                format!(
                    "{ctx}: vendor {id} = {:?} but voice.turn has {:?}",
                    text(vendor, id),
                    text(turn, id)
                )
            },
        );
    }
    let leaked: Vec<String> = keys(vendor)
        .into_iter()
        .filter(|k| never_on_a_vendor_span(k))
        .collect();
    f.check(leaked.is_empty(), || {
        format!("{ctx}: the vendor span carries {leaked:?}")
    });
    f.no_empty_strings(ctx, vendor);
    f.check(
        vendor.start_time >= turn.start_time && vendor.end_time <= turn.end_time,
        || format!("{ctx}: the vendor exchange is not inside voice.turn"),
    );
    Some(vendor)
}

/// §1.2a, TTS success: the request body is the JSON the vendor RECEIVED, and the response is the
/// audio described — never stored.
#[tokio::test]
async fn vendor_span_tts_success_shows_the_request_sent_and_describes_the_audio() {
    let cap = Capture::install();
    let vendor = tts_vendor(vec![0u8; 48_000], "audio/pcm").await;
    let app = gateway(vec![(TTS_EP, tts_entry(&base(&vendor), None, None))]).await;
    let input = sentence(120);
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": input, "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let spans = cap.take().await;
    dump_tree("vendor tts success", &spans);
    let mut f = Findings::default();
    let ctx = "vendor tts success";
    let url = format!("{}/v1/audio/speech", vendor.uri());
    let want = WantVendor {
        name: "text_to_speech kokoro-v1",
        operation: "text_to_speech",
        provider: "self_hosted",
        model: TTS_VENDOR_MODEL,
        url: &url,
        status: Some(200),
        error_type: None,
    };
    if let Some(v) = check_vendor(&mut f, ctx, &spans, &want) {
        f.eq_text(ctx, v, "url.full", Some(url.as_str()));
        // The POST: a synthesis may warm the connection with a HEAD first.
        let received = vendor.received_requests().await.expect("wiremock records");
        let posted: Vec<_> = received
            .iter()
            .filter(|r| r.method.as_str() == "POST")
            .collect();
        f.check(posted.len() == 1, || {
            format!("{ctx}: the vendor received {} POSTs", posted.len())
        });
        let sent: Json = posted
            .first()
            .and_then(|r| serde_json::from_slice(&r.body).ok())
            .unwrap_or(Json::Null);
        f.check(sent["input"] == json!(input), || {
            format!("{ctx}: the vendor did not receive the input: {sent}")
        });
        f.check(json_attr(v, VENDOR_REQUEST_BODY) == sent, || {
            format!(
                "{ctx}: gen_ai.request.body {:?} is not what the vendor received ({sent})",
                text(v, VENDOR_REQUEST_BODY)
            )
        });
        let described = json_attr(v, VENDOR_RESPONSE_BODY);
        f.check(
            described["audio"] == "not stored"
                && described["content_type"] == "audio/pcm"
                && described["bytes"] == json!(48_000)
                && described["sample_rate"] == json!(24_000)
                && described["first_byte_ms"].is_number(),
            || format!("{ctx}: gen_ai.response.body {described}"),
        );
        let longest = v
            .attributes
            .iter()
            .map(|kv| kv.value.as_str().len())
            .max()
            .unwrap_or(0);
        f.check(longest < 4_096, || {
            format!("{ctx}: an attribute of {longest} bytes; audio leaked?")
        });
    }
    f.assert_none();
}

/// §1.2a, STT success on both routes: the request is the vendor params and the file's metadata —
/// under 2 KB for a 200 KB upload — and the response is the vendor's own body.
#[tokio::test]
async fn vendor_span_stt_success_shows_params_file_metadata_and_the_vendors_answer() {
    let cap = Capture::install();
    let vendor = stt_vendor().await;
    let app = gateway(vec![(STT_EP, stt_entry(&base(&vendor), None))]).await;
    let wav = wav_of_len(204_800, 16_000);
    let mut f = Findings::default();
    for (route, operation, task, said) in [
        (
            "/v1/audio/transcriptions",
            "transcription",
            "transcribe",
            "hola a todos",
        ),
        (
            "/v1/audio/translations",
            "translation",
            "translate",
            "hello everyone",
        ),
    ] {
        let reply = upload(
            &app,
            route,
            &[
                ("model", "stt-a"),
                ("response_format", "verbose_json"),
                ("prompt", "cardiology terms"),
            ],
            ("visit.wav", "audio/wav", &wav),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{route}: {}", reply.text());
        let spans = cap.take().await;
        dump_tree(operation, &spans);
        let ctx = format!("vendor {operation}");
        let name = format!("{operation} {STT_VENDOR_MODEL}");
        let url = format!("{}{route}", vendor.uri());
        let want = WantVendor {
            name: &name,
            operation,
            provider: "self_hosted",
            model: STT_VENDOR_MODEL,
            url: &url,
            status: Some(200),
            error_type: None,
        };
        let Some(v) = check_vendor(&mut f, &ctx, &spans, &want) else {
            continue;
        };
        f.eq_text(&ctx, v, VENDOR_REQUEST_ID, Some("vendor-req-7"));
        let req = text(v, VENDOR_REQUEST_BODY).unwrap_or_default();
        let sent = json_attr(v, VENDOR_REQUEST_BODY);
        f.check(
            sent["params"]["model"] == STT_VENDOR_MODEL
                && sent["params"]["response_format"] == "verbose_json"
                && sent["params"]["prompt"] == "cardiology terms"
                && sent["file"]["filename"] == "visit.wav"
                && sent["file"]["bytes"] == json!(204_800),
            || format!("{ctx}: gen_ai.request.body {req:?}"),
        );
        f.check(req.len() < 2_048, || {
            format!(
                "{ctx}: gen_ai.request.body is {} bytes; audio leaked?",
                req.len()
            )
        });
        let answered = json!({
            "task": task, "language": "es", "duration": 12.5, "text": said, "segments": [],
        });
        f.check(json_attr(v, VENDOR_RESPONSE_BODY) == answered, || {
            format!(
                "{ctx}: gen_ai.response.body {:?} is not the vendor's answer {answered}",
                text(v, VENDOR_RESPONSE_BODY)
            )
        });
    }
    f.assert_none();
}

/// §1.2a, failure: a vendor 429 is an ERROR span with `error.type` and the status, and the response
/// body is the vendor's error body; a vendor nobody answers for has no status and a `network`
/// error type.
#[tokio::test]
async fn vendor_span_failure_carries_the_status_error_type_and_the_vendors_error_body() {
    let cap = Capture::install();
    let tts = MockServer::start().await;
    let tts_error = json!({"detail": "too many concurrent requests"});
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(429).set_body_json(tts_error.clone()))
        .mount(&tts)
        .await;
    let stt = MockServer::start().await;
    let stt_error = json!({"error": {"message": "vendor said 429"}});
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(stt_error.clone()))
        .mount(&stt)
        .await;
    let app = gateway(vec![
        (TTS_EP, tts_entry(&base(&tts), None, None)),
        (STT_EP, stt_entry(&base(&stt), None)),
    ])
    .await;
    let mut f = Findings::default();

    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert!(!reply.status.is_success(), "{}", reply.text());
    let spans = cap.take().await;
    let url = format!("{}/v1/audio/speech", tts.uri());
    let want = WantVendor {
        name: "text_to_speech kokoro-v1",
        operation: "text_to_speech",
        provider: "self_hosted",
        model: TTS_VENDOR_MODEL,
        url: &url,
        status: Some(429),
        error_type: Some("429"),
    };
    if let Some(v) = check_vendor(&mut f, "vendor tts 429", &spans, &want) {
        f.check(json_attr(v, VENDOR_RESPONSE_BODY) == tts_error, || {
            format!(
                "vendor tts 429: gen_ai.response.body {:?}, want the vendor's {tts_error}",
                text(v, VENDOR_RESPONSE_BODY)
            )
        });
        f.check(
            json_attr(v, VENDOR_REQUEST_BODY)["input"] == "Hello there.",
            || "vendor tts 429: the request sent is not recorded".to_string(),
        );
    }

    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a")],
        ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
    )
    .await;
    assert!(!reply.status.is_success(), "{}", reply.text());
    let spans = cap.take().await;
    let url = format!("{}/v1/audio/transcriptions", stt.uri());
    let name = format!("transcription {STT_VENDOR_MODEL}");
    let want = WantVendor {
        name: &name,
        operation: "transcription",
        provider: "self_hosted",
        model: STT_VENDOR_MODEL,
        url: &url,
        status: Some(429),
        error_type: Some("429"),
    };
    if let Some(v) = check_vendor(&mut f, "vendor stt 429", &spans, &want) {
        f.check(json_attr(v, VENDOR_RESPONSE_BODY) == stt_error, || {
            format!(
                "vendor stt 429: gen_ai.response.body {:?}, want the vendor's {stt_error}",
                text(v, VENDOR_RESPONSE_BODY)
            )
        });
    }

    // Nothing listening: no status, a transport error class.
    let refused_base = closed_port_base();
    let app = gateway(vec![(TTS_EP, tts_entry(&refused_base, None, None))]).await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert!(!reply.status.is_success(), "{}", reply.text());
    let spans = cap.take().await;
    let url = format!("{refused_base}/audio/speech");
    let want = WantVendor {
        name: "text_to_speech kokoro-v1",
        operation: "text_to_speech",
        provider: "self_hosted",
        model: TTS_VENDOR_MODEL,
        url: &url,
        status: None,
        error_type: Some("network"),
    };
    if let Some(v) = check_vendor(&mut f, "vendor tts refused", &spans, &want) {
        f.absent("vendor tts refused", v, &[VENDOR_RESPONSE_BODY]);
    }
    f.assert_none();
}

/// §1.2a + TC-TRACE-06: with content capture off the vendor span still exists with every non-body
/// attribute, and carries no body.
#[test]
fn vendor_span_capture_off_keeps_the_span_and_drops_the_bodies() {
    run_child(
        "child_vendor_capture_off",
        &[("BUD_TRACE_CAPTURE_CONTENT", "false")],
    );
}

#[tokio::test]
#[ignore = "run by vendor_span_capture_off_keeps_the_span_and_drops_the_bodies"]
async fn child_vendor_capture_off() {
    if !is_child() {
        return;
    }
    let cap = Capture::install();
    let tts = tts_vendor(vec![0u8; 4_800], "audio/pcm").await;
    let stt = stt_vendor().await;
    let app = gateway(vec![
        (TTS_EP, tts_entry(&base(&tts), None, None)),
        (STT_EP, stt_entry(&base(&stt), None)),
    ])
    .await;
    let mut f = Findings::default();

    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let url = format!("{}/v1/audio/speech", tts.uri());
    let want = WantVendor {
        name: "text_to_speech kokoro-v1",
        operation: "text_to_speech",
        provider: "self_hosted",
        model: TTS_VENDOR_MODEL,
        url: &url,
        status: Some(200),
        error_type: None,
    };
    if let Some(v) = check_vendor(&mut f, "vendor tts, capture off", &spans, &want) {
        f.absent(
            "vendor tts, capture off",
            v,
            &[VENDOR_REQUEST_BODY, VENDOR_RESPONSE_BODY],
        );
    }

    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a"), ("response_format", "verbose_json")],
        ("visit.wav", "audio/wav", &wav_secs(2.0, 16_000)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let url = format!("{}/v1/audio/transcriptions", stt.uri());
    let name = format!("transcription {STT_VENDOR_MODEL}");
    let want = WantVendor {
        name: &name,
        operation: "transcription",
        provider: "self_hosted",
        model: STT_VENDOR_MODEL,
        url: &url,
        status: Some(200),
        error_type: None,
    };
    if let Some(v) = check_vendor(&mut f, "vendor stt, capture off", &spans, &want) {
        f.eq_text(
            "vendor stt, capture off",
            v,
            VENDOR_REQUEST_ID,
            Some("vendor-req-7"),
        );
        f.absent(
            "vendor stt, capture off",
            v,
            &[VENDOR_REQUEST_BODY, VENDOR_RESPONSE_BODY],
        );
    }
    f.assert_none();
}

/// §1.2a "Never": the deployment's credential reaches the vendor (in a header) and appears in no
/// attribute of any span; no vendor span carries a header or an InferenceFact key.
#[tokio::test]
async fn vendor_spans_carry_no_credential_header_or_inference_fact_key() {
    if test_pem().is_none() {
        eprintln!(
            "SKIPPED vendor credential case: bud-auth/tests/fixtures/test_cred_private.pem is absent"
        );
        return;
    }
    // What `TEST_CREDENTIAL` decrypts to (bud-auth's `credentials::tests::PLAIN`).
    const SECRET: &str = "dg_vendor_key_abc123";
    let cap = Capture::install();
    let tts = tts_vendor(vec![0u8; 4_800], "audio/pcm").await;
    let tts_refusing = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"detail": "invalid api key"})),
        )
        .mount(&tts_refusing)
        .await;
    let stt = stt_vendor().await;
    let with_credential = |mut entry: Json| {
        entry["credential"] = json!(TEST_CREDENTIAL.trim());
        entry
    };
    let mut f = Findings::default();

    for (ctx, entry, is_tts, vendor) in [
        (
            "tts success",
            with_credential(tts_entry(&base(&tts), None, None)),
            true,
            &tts,
        ),
        (
            "tts 401",
            with_credential(tts_entry(&base(&tts_refusing), None, None)),
            true,
            &tts_refusing,
        ),
        (
            "stt success",
            with_credential(stt_entry(&base(&stt), None)),
            false,
            &stt,
        ),
    ] {
        let app = if is_tts {
            gateway(vec![(TTS_EP, entry)]).await
        } else {
            gateway(vec![(STT_EP, entry)]).await
        };
        let before = vendor.received_requests().await.unwrap_or_default().len();
        if is_tts {
            speech(
                &app,
                json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "pcm"}),
            )
            .await;
        } else {
            upload(
                &app,
                "/v1/audio/transcriptions",
                &[("model", "stt-a")],
                ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
            )
            .await;
        }
        let spans = cap.take().await;
        // The credential was configured and used: the vendor got it in a header.
        // The POST: a synthesis may warm the connection with a HEAD first.
        let received = vendor.received_requests().await.unwrap_or_default();
        let auth = received
            .iter()
            .skip(before)
            .find(|r| r.method.as_str() == "POST")
            .and_then(|r| r.headers.get("authorization"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        f.check(auth.as_deref() == Some(&format!("Bearer {SECRET}")), || {
            format!("{ctx}: the vendor was sent authorization {auth:?}")
        });
        let clients = client_spans(&spans);
        f.check(clients.len() == 1, || {
            format!("{ctx}: want one vendor span, exported {:?}", names(&spans))
        });
        for s in &spans {
            for kv in &s.attributes {
                let value = kv.value.as_str();
                f.check(!value.contains(SECRET), || {
                    format!("{ctx}: `{}`.{} carries the credential", s.name, kv.key)
                });
            }
        }
        for v in clients {
            let bad: Vec<String> = keys(v)
                .into_iter()
                .filter(|k| never_on_a_vendor_span(k) || k.contains("header"))
                .collect();
            f.check(bad.is_empty(), || {
                format!("{ctx}: the vendor span carries {bad:?}")
            });
            let bearer: Vec<String> = v
                .attributes
                .iter()
                .filter(|kv| kv.value.as_str().contains("Bearer"))
                .map(|kv| kv.key.as_str().to_string())
                .collect();
            f.check(bearer.is_empty(), || {
                format!("{ctx}: an Authorization value is on {bearer:?}")
            });
        }
    }
    f.assert_none();
}

/// §1.2a for the hosted prerecorded vendors, below the handler (their base URLs are compiled in):
/// the driver inside a turn's vendor scope, pointed at wiremock through `endpoint_override`.
#[test]
fn hosted_prerecorded_vendor_spans() {
    run_child(
        "child_prerecorded_vendor_spans",
        &[("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1")],
    );
}

/// `transcribe_once_standard` as the upload handler runs it: inside `voice.turn`, in the turn's
/// vendor scope, with the six ids recorded.
async fn prerecorded_in_a_turn(
    vendor: &str,
    model: &str,
    base: &str,
    samples: usize,
) -> Result<
    waav_gateway::handlers::transcribe::Transcript,
    waav_gateway::handlers::transcribe::TranscribeFailure,
> {
    use tracing::Instrument;
    use waav_gateway::observability::voice_span::{Root, VoiceSpans};
    let spans = VoiceSpans::open("audio_transcription", &Root::default());
    for (key, value) in [
        (PROJECT_ID, EP_PROJECT),
        (API_KEY_PROJECT_ID, KEY_PROJECT),
        (ENDPOINT_ID, DG_EP),
        (MODEL_ID, STT_MODEL_ID),
        (USER_ID, USER),
        (API_KEY_ID, API_KEY),
    ] {
        spans.record_text(key, Some(value));
    }
    let config = waav_gateway::core::stt::STTConfig {
        provider: vendor.to_string(),
        api_key: "vendor-test-key".to_string(),
        language: "en-US".to_string(),
        sample_rate: 16_000,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: model.to_string(),
    };
    let std = waav_gateway::core::stt::standard::StandardSTTConfig::from_base(config)
        .with_endpoint_override(base);
    let pcm = waav_openai_audio::pcm::PcmAudio {
        samples: vec![0; samples],
        sample_rate: 16_000,
    };
    spans
        .vendor_scope("transcription")
        .run(waav_gateway::handlers::transcribe::transcribe_once_standard(vendor, std, &pcm))
        .instrument(spans.turn().clone())
        .await
}

#[tokio::test]
#[ignore = "run by hosted_prerecorded_vendor_spans"]
async fn child_prerecorded_vendor_spans() {
    if !is_child() {
        return;
    }
    let cap = Capture::install();
    let mut f = Findings::default();
    let mut every_span: Vec<SpanData> = Vec::new();
    // 100,000 samples of 16-bit mono: a 200 KB WAV.
    const SAMPLES: usize = 100_000;
    let wav_bytes = 44 + SAMPLES * 2;

    let dg_answer = json!({
        "metadata": {"request_id": "dg-req-1", "duration": 6.25},
        "results": {"channels": [{"alternatives": [{"transcript": "hola", "confidence": 0.83, "words": []}]}]}
    });
    let dg = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .respond_with(ResponseTemplate::new(200).set_body_json(dg_answer.clone()))
        .mount(&dg)
        .await;
    let result = prerecorded_in_a_turn("deepgram", "nova-3", &dg.uri(), SAMPLES).await;
    f.check(result.is_ok(), || {
        format!(
            "deepgram failed: {:?}",
            result.as_ref().err().map(|e| e.to_string())
        )
    });
    let spans = cap.take().await;
    every_span.extend(spans.iter().cloned());
    let url = format!("{}/v1/listen?", dg.uri());
    let want = WantVendor {
        name: "transcription nova-3",
        operation: "transcription",
        provider: "deepgram",
        model: "nova-3",
        url: &url,
        status: Some(200),
        error_type: None,
    };
    if let Some(v) = check_vendor(&mut f, "deepgram", &spans, &want) {
        let full = text(v, "url.full").unwrap_or_default();
        f.check(full.contains("model=nova-3"), || {
            format!("deepgram: url.full {full:?} lost its query")
        });
        f.eq_text("deepgram", v, VENDOR_REQUEST_ID, Some("dg-req-1"));
        let req = text(v, VENDOR_REQUEST_BODY).unwrap_or_default();
        let sent = json_attr(v, VENDOR_REQUEST_BODY);
        f.check(
            sent["params"]["model"] == "nova-3"
                && sent["params"]["language"] == "en-US"
                && sent["params"]["punctuate"] == "true"
                && sent["file"]["content_type"] == "audio/wav"
                && sent["file"]["bytes"] == json!(wav_bytes),
            || format!("deepgram: gen_ai.request.body {req:?}"),
        );
        f.check(req.len() < 2_048, || {
            format!(
                "deepgram: gen_ai.request.body is {} bytes; audio leaked?",
                req.len()
            )
        });
        f.check(json_attr(v, VENDOR_RESPONSE_BODY) == dg_answer, || {
            format!(
                "deepgram: gen_ai.response.body {:?} is not the vendor's body",
                text(v, VENDOR_RESPONSE_BODY)
            )
        });
    }

    let el_answer =
        json!({"text": "hello", "language_code": "en", "language_probability": 0.98, "words": []});
    let el = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/speech-to-text"))
        .respond_with(ResponseTemplate::new(200).set_body_json(el_answer.clone()))
        .mount(&el)
        .await;
    let result = prerecorded_in_a_turn("elevenlabs", "scribe_v1", &el.uri(), SAMPLES).await;
    f.check(result.is_ok(), || {
        format!(
            "elevenlabs failed: {:?}",
            result.as_ref().err().map(|e| e.to_string())
        )
    });
    let spans = cap.take().await;
    every_span.extend(spans.iter().cloned());
    let url = format!("{}/v1/speech-to-text", el.uri());
    let want = WantVendor {
        name: "transcription scribe_v1",
        operation: "transcription",
        provider: "elevenlabs",
        model: "scribe_v1",
        url: &url,
        status: Some(200),
        error_type: None,
    };
    if let Some(v) = check_vendor(&mut f, "elevenlabs", &spans, &want) {
        let req = text(v, VENDOR_REQUEST_BODY).unwrap_or_default();
        let sent = json_attr(v, VENDOR_REQUEST_BODY);
        f.check(
            sent["params"]["model_id"] == "scribe_v1"
                && sent["file"]["filename"] == "audio.wav"
                && sent["file"]["content_type"] == "audio/wav"
                && sent["file"]["bytes"] == json!(wav_bytes),
            || format!("elevenlabs: gen_ai.request.body {req:?}"),
        );
        f.check(req.len() < 2_048, || {
            format!("elevenlabs: gen_ai.request.body is {} bytes", req.len())
        });
        f.check(json_attr(v, VENDOR_RESPONSE_BODY) == el_answer, || {
            format!(
                "elevenlabs: gen_ai.response.body {:?}",
                text(v, VENDOR_RESPONSE_BODY)
            )
        });
    }

    // The OpenAI and Groq clients, through their endpoint overrides. Both make ONE request on
    // close, so the upload driver must close them at once: before Groq carried the
    // request/response marker, its case sat out the 45 s first-result timeout and came back
    // marked truncated — hence the time bound and the `truncated` check.
    let whisper_answer = json!({"text": "hello", "language": "english", "duration": 6.25});
    for (vendor_id, model, route, request_id) in [
        (
            "openai",
            "whisper-1",
            "/v1/audio/transcriptions",
            "oa-req-9",
        ),
        (
            "groq",
            "whisper-large-v3",
            "/openai/v1/audio/transcriptions",
            "gq-req-7",
        ),
    ] {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-request-id", request_id)
                    .set_body_json(whisper_answer.clone()),
            )
            .mount(&mock)
            .await;
        let started = std::time::Instant::now();
        let result = prerecorded_in_a_turn(vendor_id, model, &mock.uri(), SAMPLES).await;
        let took = started.elapsed();
        f.check(result.is_ok(), || {
            format!(
                "{vendor_id} failed: {:?}",
                result.as_ref().err().map(|e| e.to_string())
            )
        });
        if let Ok(t) = &result {
            f.check(t.text == "hello" && !t.truncated, || {
                format!(
                    "{vendor_id}: transcript {:?}, truncated={}",
                    t.text, t.truncated
                )
            });
        }
        f.check(took < std::time::Duration::from_secs(10), || {
            format!("{vendor_id}: the upload took {took:?} — waited out the first-result timeout")
        });
        let spans = cap.take().await;
        every_span.extend(spans.iter().cloned());
        let url = format!("{}{route}", mock.uri());
        let name = format!("transcription {model}");
        let want = WantVendor {
            name: &name,
            operation: "transcription",
            provider: vendor_id,
            model,
            url: &url,
            status: Some(200),
            error_type: None,
        };
        if let Some(v) = check_vendor(&mut f, vendor_id, &spans, &want) {
            f.eq_text(vendor_id, v, VENDOR_REQUEST_ID, Some(request_id));
            let req = text(v, VENDOR_REQUEST_BODY).unwrap_or_default();
            let sent = json_attr(v, VENDOR_REQUEST_BODY);
            f.check(
                sent["params"]["model"] == model
                    && sent["file"]["filename"] == "audio.wav"
                    && sent["file"]["content_type"] == "audio/wav"
                    && sent["file"]["bytes"] == json!(wav_bytes),
                || format!("{vendor_id}: gen_ai.request.body {req:?}"),
            );
            f.check(req.len() < 2_048, || {
                format!("{vendor_id}: gen_ai.request.body is {} bytes", req.len())
            });
            f.check(json_attr(v, VENDOR_RESPONSE_BODY) == whisper_answer, || {
                format!(
                    "{vendor_id}: gen_ai.response.body {:?}",
                    text(v, VENDOR_RESPONSE_BODY)
                )
            });
        }
    }

    let refused = json!({"err_msg": "slow down"});
    let dg429 = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .respond_with(ResponseTemplate::new(429).set_body_json(refused.clone()))
        .mount(&dg429)
        .await;
    let result = prerecorded_in_a_turn("deepgram", "nova-3", &dg429.uri(), 16_000).await;
    f.check(result.is_err(), || "deepgram 429 succeeded".to_string());
    let spans = cap.take().await;
    every_span.extend(spans.iter().cloned());
    let url = format!("{}/v1/listen?", dg429.uri());
    let want = WantVendor {
        name: "transcription nova-3",
        operation: "transcription",
        provider: "deepgram",
        model: "nova-3",
        url: &url,
        status: Some(429),
        error_type: Some("429"),
    };
    if let Some(v) = check_vendor(&mut f, "deepgram 429", &spans, &want) {
        f.check(json_attr(v, VENDOR_RESPONSE_BODY) == refused, || {
            format!(
                "deepgram 429: gen_ai.response.body {:?}, want the vendor's {refused}",
                text(v, VENDOR_RESPONSE_BODY)
            )
        });
    }

    // The configured key travels in a header only: it is on no span of any of the three calls.
    for s in &every_span {
        for kv in &s.attributes {
            f.check(!kv.value.as_str().contains("vendor-test-key"), || {
                format!("`{}`.{} carries the vendor key", s.name, kv.key)
            });
        }
    }
    f.assert_none();
}

// =============================================================================================
// One value per key: fallbacks, breakers (FRD-022 §6.4-6.5 on the FRD-021 record)
// =============================================================================================

/// Every value `span` exports under `key`, in order. [`text`] reads only the FIRST — which is how a
/// duplicated key hid: `tracing-opentelemetry` appends a second `KeyValue` for a re-recorded field,
/// and ClickHouse's `SpanAttributes['key']` then reads either.
fn values(span: &SpanData, key: &str) -> Vec<String> {
    span.attributes
        .iter()
        .filter(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.as_str().into_owned())
        .collect()
}

/// Keys `span` exports more than once.
fn duplicated(span: &SpanData) -> Vec<String> {
    let mut all: Vec<String> = keys(span);
    all.dedup();
    all.into_iter()
        .filter(|k| values(span, k).len() > 1)
        .collect()
}

/// A self-hosted vendor that fails every speech and transcription request with `status`.
async fn failing_vendor(status: u16) -> MockServer {
    let vendor = MockServer::start().await;
    for route in [
        "/v1/audio/speech",
        "/v1/audio/transcriptions",
        "/v1/audio/translations",
    ] {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(status).set_body_json(json!({"detail": "vendor is down"})),
            )
            .mount(&vendor)
            .await;
    }
    vendor
}

/// A self-hosted STT vendor answering verbose JSON with `language` as given.
async fn stt_vendor_saying(language: &str) -> MockServer {
    let vendor = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_json(json!({
                    "task": "transcribe",
                    "language": language,
                    "duration": 1.0,
                    "text": "hola",
                    "segments": [],
                })),
        )
        .mount(&vendor)
        .await;
    vendor
}

/// The primary TTS deployment, failing, with a fallback of another vendor, model and voice.
fn tts_fallback_table(down: &MockServer, up: &MockServer) -> Vec<(&'static str, Json)> {
    let mut primary = tts_entry(&base(down), None, None);
    primary["voice"] = json!("af_heart");
    primary["fallback_models"] = json!([FB_TTS_EP]);
    let fallback = json!({
        "vendor": "openai_compatible",
        "api_base": base(up),
        "endpoints": ["text_to_speech"],
        "model": "kokoro-v2",
        "voice": "af_bella",
        "pricing": pricing("character", 30.0, 1_000_000),
    });
    vec![(TTS_EP, primary), (FB_TTS_EP, fallback)]
}

fn stt_fallback_table(down: &MockServer, up: &MockServer) -> Vec<(&'static str, Json)> {
    let mut primary = stt_entry(&base(down), None);
    primary["fallback_models"] = json!([FB_STT_EP]);
    let fallback = json!({
        "vendor": "openai_compatible",
        "api_base": base(up),
        "endpoints": ["audio_transcription", "audio_translation"],
        "model": "whisper-v3-turbo",
    });
    vec![(STT_EP, primary), (FB_STT_EP, fallback)]
}

/// A synthesis a fallback served exports ONE value per key — the served deployment's vendor, model
/// and voice — and one GenAI name each on the root. Before, the primary's were recorded when the
/// turn opened and the fallback's again when it served, and both were exported.
#[tokio::test]
async fn a_fallback_served_synthesis_records_each_key_once() {
    let cap = Capture::install();
    let down = failing_vendor(503).await;
    let up = tts_vendor(wav_secs(1.0, 24_000), "audio/wav").await;
    let app = gateway(tts_fallback_table(&down, &up)).await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "response_format": "wav"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(
        reply
            .headers
            .get("x-bud-fallback")
            .map(|v| v.to_str().unwrap()),
        Some("true"),
        "the fallback must have served"
    );
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    let root = root_of(&spans);
    for (key, want) in [
        (TTS_VENDOR, "openai_compatible"),
        (TTS_MODEL, "kokoro-v2"),
        (TTS_VOICE, "af_bella"),
        (SERVED_ENDPOINT_ID, FB_TTS_EP),
        (FALLBACK_FROM, TTS_EP),
        (RETRY_COUNT, "0"),
        (ENDPOINT_ID, TTS_EP),
    ] {
        assert_eq!(values(turn, key), [want], "voice.turn {key}");
    }
    assert_eq!(values(root, GEN_AI_PROVIDER), ["openai_compatible"]);
    assert_eq!(values(root, GEN_AI_MODEL), ["kokoro-v2"]);
    assert_eq!(duplicated(turn), Vec::<String>::new(), "voice.turn");
    assert_eq!(duplicated(root), Vec::<String>::new(), "the root");
    // Billed at the fallback's price (FRD-022 §6.4), once.
    assert_eq!(values(turn, PRICING_UNIT), ["character"]);
}

/// The same for a transcription: the served deployment's vendor and model, once each.
#[tokio::test]
async fn a_fallback_served_transcription_records_each_key_once() {
    let cap = Capture::install();
    let down = failing_vendor(503).await;
    let up = stt_vendor().await;
    let app = gateway(stt_fallback_table(&down, &up)).await;
    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a"), ("response_format", "verbose_json")],
        ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    let root = root_of(&spans);
    for (key, want) in [
        (STT_VENDOR, "openai_compatible"),
        (STT_MODEL, "whisper-v3-turbo"),
        (SERVED_ENDPOINT_ID, FB_STT_EP),
        (FALLBACK_FROM, STT_EP),
        (DETECTED_LANGUAGE, "es"),
    ] {
        assert_eq!(values(turn, key), [want], "voice.turn {key}");
    }
    assert_eq!(values(root, GEN_AI_PROVIDER), ["openai_compatible"]);
    assert_eq!(values(root, GEN_AI_MODEL), ["whisper-v3-turbo"]);
    assert_eq!(duplicated(turn), Vec::<String>::new(), "voice.turn");
    assert_eq!(duplicated(root), Vec::<String>::new(), "the root");
}

/// Every hop failed: the call is attributed to the primary, once, with the primary's failure.
#[tokio::test]
async fn a_chain_that_fails_everywhere_keeps_the_primary_leg_once() {
    let cap = Capture::install();
    let down = failing_vendor(503).await;
    let also_down = failing_vendor(500).await;
    let app = gateway(tts_fallback_table(&down, &also_down)).await;
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "response_format": "wav"}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_GATEWAY, "{}", reply.text());
    let spans = cap.take().await;
    let turn = only(&spans, "voice.turn");
    for (key, want) in [
        (TTS_VENDOR, "self_hosted"),
        (TTS_MODEL, TTS_VENDOR_MODEL),
        (TTS_VOICE, "af_heart"),
        (ERROR_TYPE, "vendor_5xx"),
        (VENDOR_STATUS, "503"),
    ] {
        assert_eq!(values(turn, key), [want], "voice.turn {key}");
    }
    assert!(values(turn, SERVED_ENDPOINT_ID).is_empty());
    assert_eq!(duplicated(turn), Vec::<String>::new(), "voice.turn");
    assert_eq!(
        duplicated(root_of(&spans)),
        Vec::<String>::new(),
        "the root"
    );
}

fn open_breaker(
    policies: &waav_gateway::core::deployment_policy::DeploymentPolicies,
    deployment: &str,
) {
    for _ in 0..5 {
        policies
            .breakers()
            .deployment
            .record_failure(deployment, deployment);
    }
}

/// A call an open circuit breaker refused made no vendor call: `circuit_open`, with no vendor
/// status — not `vendor_5xx`, which blamed the vendor for a request it never received.
#[tokio::test]
async fn an_open_breaker_is_circuit_open_not_a_vendor_failure() {
    let cap = Capture::install();
    let tts = tts_vendor(wav_secs(1.0, 24_000), "audio/wav").await;
    let stt = stt_vendor().await;
    let policies = waav_gateway::core::deployment_policy::DeploymentPolicies::local();
    open_breaker(&policies, TTS_EP);
    open_breaker(&policies, STT_EP);
    let app = gateway_with_policies(
        vec![
            (TTS_EP, tts_entry(&base(&tts), None, None)),
            (STT_EP, stt_entry(&base(&stt), None)),
        ],
        policies,
    )
    .await;

    let mut f = Findings::default();
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "voice": "George", "response_format": "wav"}),
    )
    .await;
    f.check(reply.status == StatusCode::SERVICE_UNAVAILABLE, || {
        format!("tts: HTTP {} {}", reply.status, reply.text())
    });
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "tts breaker open",
        &spans,
        "/v1/audio/speech",
        &reply,
        "circuit_open",
        None,
    );

    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a")],
        ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
    )
    .await;
    f.check(reply.status == StatusCode::SERVICE_UNAVAILABLE, || {
        format!("stt: HTTP {} {}", reply.status, reply.text())
    });
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "stt breaker open",
        &spans,
        "/v1/audio/transcriptions",
        &reply,
        "circuit_open",
        None,
    );
    f.check(
        tts.received_requests().await.unwrap_or_default().is_empty()
            && stt.received_requests().await.unwrap_or_default().is_empty(),
        || "a vendor was called through an open breaker".to_string(),
    );
    f.assert_none();
}

/// The primary's breaker is open and a fallback DID call its vendor, which failed: that real
/// failure is what the call records — its class and the vendor's status — not "circuit open".
#[tokio::test]
async fn behind_an_open_breaker_a_fallbacks_real_failure_is_recorded() {
    let cap = Capture::install();
    let primary_tts = tts_vendor(wav_secs(1.0, 24_000), "audio/wav").await;
    let primary_stt = stt_vendor().await;
    let fallback = failing_vendor(500).await;
    let policies = waav_gateway::core::deployment_policy::DeploymentPolicies::local();
    open_breaker(&policies, TTS_EP);
    open_breaker(&policies, STT_EP);
    let mut table = tts_fallback_table(&primary_tts, &fallback);
    table.extend(stt_fallback_table(&primary_stt, &fallback));
    let app = gateway_with_policies(table, policies).await;

    let mut f = Findings::default();
    let reply = speech(
        &app,
        json!({"model": "tts-a", "input": "Hello there.", "response_format": "wav"}),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "tts fallback 500",
        &spans,
        "/v1/audio/speech",
        &reply,
        "vendor_5xx",
        Some(500),
    );
    let reply = upload(
        &app,
        "/v1/audio/transcriptions",
        &[("model", "stt-a")],
        ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
    )
    .await;
    let spans = cap.take().await;
    check_failure(
        &mut f,
        "stt fallback 500",
        &spans,
        "/v1/audio/transcriptions",
        &reply,
        "vendor_5xx",
        Some(500),
    );
    f.check(
        !fallback
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        || "the fallback was never called".to_string(),
    );
    f.assert_none();
}

/// `bud.voice.detected_language` is one spelling per language, whoever answered: a Whisper-style
/// backend's `Spanish` and ElevenLabs' `spa` are both `es`.
#[tokio::test]
async fn the_detected_language_is_normalized() {
    let cap = Capture::install();
    let mut f = Findings::default();
    for (said, want) in [
        ("Spanish", "es"),
        ("spa", "es"),
        ("es-MX", "es"),
        ("en", "en"),
    ] {
        let vendor = stt_vendor_saying(said).await;
        let app = gateway(vec![(STT_EP, stt_entry(&base(&vendor), None))]).await;
        let reply = upload(
            &app,
            "/v1/audio/transcriptions",
            &[("model", "stt-a"), ("response_format", "verbose_json")],
            ("a.wav", "audio/wav", &wav_secs(1.0, 16_000)),
        )
        .await;
        f.check(reply.status == StatusCode::OK, || {
            format!("{said}: HTTP {} {}", reply.status, reply.text())
        });
        let spans = cap.take().await;
        let turn = only(&spans, "voice.turn");
        f.eq_text(said, turn, DETECTED_LANGUAGE, Some(want));
    }
    f.assert_none();
}
