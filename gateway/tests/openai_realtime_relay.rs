//! FRD-023 `/v1/realtime` end to end, in process: a Bud-mode gateway over an in-memory control
//! plane, a mock OpenAI-GA vendor, and real WebSocket clients (TC-HS, TC-UP, TC-EVT, TC-LIFE,
//! TC-MET, TC-EK, TC-SEC-07).
//!
//! Each test uses its own deployment names and ids, so tests run in parallel against their own
//! gateway and vendor; spans are captured per test on the test's own (current-thread) runtime.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use opentelemetry::trace::{SpanId, TracerProvider as _};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use serde_json::{Value as Json, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing_subscriber::layer::SubscriberExt;

use waav_gateway::config::{DAGTimeoutsConfig, PluginConfig, ServerConfig};
use waav_gateway::handlers::openai_realtime::{RealtimeRuntime, Timings};
use waav_gateway::state::AppState;

// =============================================================================================
// Fixtures
// =============================================================================================

const KEY: &str = "bud_realtime_relay_test_key";
const OTHER_KEY: &str = "bud_realtime_relay_other_project_key";
const PROJECT: &str = "5b0c7e1d-0000-4000-8000-00000000aa01";
const OTHER_PROJECT: &str = "5b0c7e1d-0000-4000-8000-00000000aa02";
const USER: &str = "5b0c7e1d-0000-4000-8000-00000000bb01";
const API_KEY_ID: &str = "5b0c7e1d-0000-4000-8000-00000000cc01";
const MODEL_ID: &str = "5b0c7e1d-0000-4000-8000-00000000dd01";

/// bud-auth's fixture ciphertext; the plaintext is its `PLAIN`.
const TEST_CREDENTIAL: &str = include_str!("../../bud-auth/tests/fixtures/test_cred_encrypted.hex");
const VENDOR_KEY: &str = "dg_vendor_key_abc123";

const JWKS: &str = include_str!("../../bud-auth/tests/fixtures/test_jwks.json");
const JWT_ISSUER: &str = "https://auth.test/realms/bud";

fn test_pem() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bud-auth/tests/fixtures/test_cred_private.pem"
    ))
    .expect("bud-auth's fixture key (git-ignored *.pem) must be present locally")
}

fn jwt_private_pem() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bud-auth/tests/fixtures/test_rsa_private.pem"
    ))
    .expect("bud-auth's JWT fixture key must be present locally")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn jwt(sub: &str, exp_in: i64) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("test-key-1".into());
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(jwt_private_pem().as_bytes()).unwrap();
    let now = now() as i64;
    jsonwebtoken::encode(
        &header,
        &json!({"iss": JWT_ISSUER, "sub": sub, "azp": "bud-playground", "exp": now + exp_in, "iat": now}),
        &key,
    )
    .unwrap()
}

/// The loopback escape hatch, so the mock vendor on 127.0.0.1 passes SSRF validation. Set once,
/// before any test builds a gateway; the SSRF refusal itself is covered by lib tests (TC-UP-04).
fn allow_loopback() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") });
}

// =============================================================================================
// Span capture (per test, thread-local)
// =============================================================================================

#[derive(Debug, Clone, Default)]
struct Exported(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Exported {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

struct Capture {
    exported: Exported,
    _guard: tracing::subscriber::DefaultGuard,
    _provider: SdkTracerProvider,
    logs: Arc<Mutex<Vec<u8>>>,
}

#[derive(Clone)]
struct LogSink(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn install() -> Self {
        let exported = Exported::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exported.clone())
            .build();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = LogSink(logs.clone());
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("realtime-relay")))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(move || sink.clone())
                    .with_ansi(false),
            );
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            exported,
            _guard: guard,
            _provider: provider,
            logs,
        }
    }

    fn spans(&self) -> Vec<SpanData> {
        self.exported.0.lock().unwrap().clone()
    }

    fn logs(&self) -> String {
        String::from_utf8_lossy(&self.logs.lock().unwrap()).into_owned()
    }

    async fn wait_for(&self, name: &str, n: usize) -> Vec<SpanData> {
        for _ in 0..300 {
            let found: Vec<SpanData> = self
                .spans()
                .into_iter()
                .filter(|s| s.name == name)
                .collect();
            if found.len() >= n {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "expected {n} `{name}` spans, got {:?}",
            self.spans()
                .iter()
                .map(|s| s.name.to_string())
                .collect::<Vec<_>>()
        );
    }
}

fn attr(span: &SpanData, key: &str) -> Option<opentelemetry::Value> {
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
        opentelemetry::Value::I64(i) => Some(i as f64),
        opentelemetry::Value::F64(f) => Some(f),
        opentelemetry::Value::String(s) => s.as_str().parse().ok(),
        _ => None,
    }
}

// =============================================================================================
// The mock vendor (OpenAI Realtime GA)
// =============================================================================================

#[derive(Clone)]
struct Behaviour {
    /// Complete the WebSocket handshake at all (TC-UP-07: a vendor that never answers).
    accept: bool,
    /// Answer a `session.update` with `session.updated` (TC-EVT-15 turns it off).
    answer_updates: bool,
    /// Send `rate_limits.updated` after each response (TC-EVT-12).
    rate_limits: bool,
    /// Audio deltas per response.
    audio_deltas: usize,
    /// Size of each audio delta's base64 payload.
    delta_bytes: usize,
    /// Stop reading (and therefore ponging) after this many client frames (TC-LIFE-03).
    go_silent_after: Option<usize>,
    /// Answer `input_audio_buffer.commit` with a completed transcription carrying this usage.
    transcription_usage: Option<Json>,
    usage: Json,
    /// Speak like xAI (TC-XL-06): bootstrap with `conversation.created` instead of
    /// `session.created`, answer a commit with CUMULATIVE
    /// `conversation.item.input_audio_transcription.updated` events, never `rate_limits.updated`.
    xai: bool,
    /// Close the socket with this code after this many client frames (the vendor ending the
    /// session itself).
    close_after: Option<(usize, u16)>,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            accept: true,
            answer_updates: true,
            rate_limits: false,
            audio_deltas: 2,
            delta_bytes: 16,
            go_silent_after: None,
            transcription_usage: None,
            usage: documented_usage(),
            xai: false,
            close_after: None,
        }
    }
}

fn documented_usage() -> Json {
    json!({
        "total_tokens": 253, "input_tokens": 132, "output_tokens": 121,
        "input_token_details": {"text_tokens": 119, "audio_tokens": 13, "image_tokens": 0,
            "cached_tokens": 64, "cached_tokens_details": {"text_tokens": 64, "audio_tokens": 0, "image_tokens": 0}},
        "output_token_details": {"text_tokens": 30, "audio_tokens": 91}
    })
}

#[derive(Default)]
struct VendorLog {
    /// `(path?query, lower-cased headers)` per upgrade.
    upgrades: Vec<(String, HashMap<String, String>)>,
    /// Every client frame the vendor received, in order.
    frames: Vec<Json>,
    connections: usize,
}

#[derive(Clone)]
struct MockVendor {
    addr: SocketAddr,
    log: Arc<Mutex<VendorLog>>,
}

impl MockVendor {
    async fn start(b: Behaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(VendorLog::default()));
        let shared = log.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                shared.lock().unwrap().connections += 1;
                let b = b.clone();
                let log = shared.clone();
                tokio::spawn(async move {
                    if !b.accept {
                        // Hold the TCP connection and never answer the upgrade.
                        tokio::time::sleep(Duration::from_secs(120)).await;
                        drop(stream);
                        return;
                    }
                    let captured = log.clone();
                    let callback = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                         resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        let headers = req
                            .headers()
                            .iter()
                            .map(|(k, v)| (k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        let pq = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
                        captured.lock().unwrap().upgrades.push((pq, headers));
                        Ok(resp)
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await
                    else {
                        return;
                    };
                    let model = log
                        .lock()
                        .unwrap()
                        .upgrades
                        .last()
                        .and_then(|(pq, _)| pq.split("model=").nth(1).map(str::to_string))
                        .unwrap_or_default();
                    let created = if b.xai {
                        json!({"type": "conversation.created", "event_id": "evt_x0",
                            "conversation": {"id": "conv_xai_1", "object": "realtime.conversation"}})
                    } else {
                        json!({"type": "session.created", "event_id": "evt_v0",
                            "session": {"id": "sess_vendor_1", "object": "realtime.session", "type": "realtime", "model": model}})
                    };
                    if ws
                        .send(Message::Text(created.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let mut received = 0usize;
                    let mut response_n = 0usize;
                    while let Some(Ok(msg)) = ws.next().await {
                        let Message::Text(t) = msg else { continue };
                        let v: Json = serde_json::from_str(t.as_str()).unwrap_or(Json::Null);
                        log.lock().unwrap().frames.push(v.clone());
                        received += 1;
                        if b.go_silent_after.is_some_and(|n| received >= n) {
                            // Stop reading: no more pongs, no more answers.
                            tokio::time::sleep(Duration::from_secs(120)).await;
                            return;
                        }
                        if let Some((n, code)) = b.close_after
                            && received >= n
                        {
                            use tokio_tungstenite::tungstenite::protocol::CloseFrame;
                            let _ = ws
                                .close(Some(CloseFrame {
                                    code: code.into(),
                                    reason: "session over".into(),
                                }))
                                .await;
                            // Drain until the peer's close answer, as a real server does.
                            while let Some(Ok(_)) = ws.next().await {}
                            return;
                        }
                        let kind = v["type"].as_str().unwrap_or_default().to_string();
                        let mut out: Vec<Json> = Vec::new();
                        match kind.as_str() {
                            "session.update" if b.answer_updates => {
                                out.push(json!({"type": "session.updated", "event_id": "evt_vu",
                                    "session": {"id": "sess_vendor_1", "model": model, "type": v["session"]["type"]}}));
                            }
                            "response.create" => {
                                response_n += 1;
                                let rid = format!("resp_{response_n}");
                                out.push(
                                    json!({"type": "response.created", "response": {"id": rid}}),
                                );
                                for i in 0..b.audio_deltas {
                                    out.push(json!({"type": "response.output_audio.delta", "response_id": rid,
                                        "item_id": "item_1", "delta": format!("{:0>width$}", i, width = b.delta_bytes)}));
                                }
                                if b.rate_limits {
                                    out.push(json!({"type": "rate_limits.updated", "rate_limits": [{"name": "tokens", "remaining": 1}]}));
                                }
                                out.push(json!({"type": "response.done", "event_id": format!("evt_done_{response_n}"),
                                    "response": {"id": rid, "status": "completed", "usage": b.usage,
                                        "output": [{"type": "message", "content": [{"type": "output_audio", "transcript": "hello there"}]}]}}));
                            }
                            "input_audio_buffer.commit" if b.xai => {
                                for partial in ["hel", "hello", "hello world"] {
                                    out.push(json!({"type": "conversation.item.input_audio_transcription.updated",
                                        "item_id": "item_x", "content_index": 0, "transcript": partial}));
                                }
                                out.push(json!({"type": "conversation.item.input_audio_transcription.completed",
                                    "item_id": "item_x", "content_index": 0, "transcript": "hello world"}));
                            }
                            "input_audio_buffer.commit" => {
                                if let Some(u) = &b.transcription_usage {
                                    out.push(json!({"type": "conversation.item.input_audio_transcription.completed",
                                        "item_id": "item_u", "content_index": 0, "transcript": "hi", "usage": u}));
                                }
                            }
                            _ => {}
                        }
                        for o in out {
                            if ws.send(Message::Text(o.to_string().into())).await.is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        Self { addr, log }
    }

    fn base(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn frames(&self) -> Vec<Json> {
        self.log.lock().unwrap().frames.clone()
    }

    fn frames_of(&self, kind: &str) -> Vec<Json> {
        self.frames()
            .into_iter()
            .filter(|f| f["type"] == kind)
            .collect()
    }

    fn upgrades(&self) -> Vec<(String, HashMap<String, String>)> {
        self.log.lock().unwrap().upgrades.clone()
    }

    fn connections(&self) -> usize {
        self.log.lock().unwrap().connections
    }
}

// =============================================================================================
// The gateway
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
        openai_api_key: Some("sk-process-canary".to_string()),
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
        max_connections_per_ip: 1000,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        realtime_endpoint_overrides: Default::default(),
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
        aliases: Default::default(),
    }
}

fn fast_timings() -> Timings {
    Timings {
        ping: Duration::from_millis(300),
        max_missed_pongs: 3,
        revalidate: Duration::from_millis(300),
        connect: Duration::from_millis(1500),
        hold: Duration::from_millis(700),
        slow_client: Duration::from_millis(600),
        max_session: Duration::from_secs(3600),
        default_idle: Duration::from_secs(300),
        warn_before: Duration::from_secs(1),
        segment: Duration::from_secs(1),
        upstream_send: Duration::from_secs(2),
        connection_cap: None,
    }
}

/// A realtime `voice_table` entry pointed at the mock vendor.
fn rt_entry(vendor: &MockVendor, extra: Json) -> Json {
    let mut e = json!({
        "vendor": "openai",
        "api_base": vendor.base(),
        "credential": TEST_CREDENTIAL.trim(),
        "endpoints": ["realtime_session"],
        "model": "gpt-realtime-2.1",
        "pricing": {"unit": "token", "per_units": 1000000, "currency": "USD",
            "rates": {"input_text": 4.0, "input_audio": 32.0, "input_image": 5.0,
                      "cached_input_text": 0.4, "cached_input_audio": 0.4, "cached_input_image": 0.5,
                      "output_text": 24.0, "output_audio": 64.0, "transcription_per_minute": 0.003}}
    });
    if let Some(obj) = extra.as_object() {
        for (k, v) in obj {
            e[k] = v.clone();
        }
    }
    e
}

struct Gateway {
    addr: SocketAddr,
    store: Arc<bud_auth::MemoryStore>,
    plane: Arc<bud_auth::BudPlane>,
    state: Arc<AppState>,
}

struct Setup {
    /// `(alias, endpoint id, entry)` reachable by KEY in PROJECT.
    endpoints: Vec<(String, String, Json)>,
    /// Endpoints in the table that only OTHER_KEY reaches.
    other_endpoints: Vec<(String, String, Json)>,
    timings: Timings,
    client_secret_keys: Option<&'static str>,
    jwt_subjects: Vec<&'static str>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
            other_endpoints: Vec::new(),
            timings: fast_timings(),
            client_secret_keys: Some("k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="),
            jwt_subjects: Vec::new(),
        }
    }
}

struct StaticJwks;
#[async_trait::async_trait]
impl bud_auth::JwksSource for StaticJwks {
    async fn fetch(&self) -> Result<String, String> {
        Ok(JWKS.to_string())
    }
}

async fn gateway(setup: Setup) -> Gateway {
    allow_loopback();
    if std::env::var("RUST_LOG").is_ok() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let store = Arc::new(bud_auth::MemoryStore::new());
    let blob = |eps: &[(String, String, Json)], project: &str| {
        let mut m = serde_json::Map::new();
        for (alias, id, _) in eps {
            m.insert(alias.clone(), json!({"endpoint_id": id, "model_id": MODEL_ID, "project_id": project, "kind": "model"}));
        }
        m.insert(
            "__metadata__".into(),
            json!({"api_key_id": API_KEY_ID, "user_id": USER, "api_key_project_id": project}),
        );
        Json::Object(m).to_string()
    };
    store.set(
        &format!("api_key:{}", bud_auth::hash_api_key(KEY)),
        &blob(&setup.endpoints, PROJECT),
    );
    store.set(
        &format!("api_key:{}", bud_auth::hash_api_key(OTHER_KEY)),
        &blob(&setup.other_endpoints, OTHER_PROJECT),
    );
    for (_, id, entry) in setup.endpoints.iter().chain(setup.other_endpoints.iter()) {
        store.set(
            &format!("voice_table:{id}"),
            &json!({ id.as_str(): entry }).to_string(),
        );
    }
    // JWT subjects reach the KEY project's deployments.
    let mut project_models = serde_json::Map::new();
    for (alias, id, _) in &setup.endpoints {
        project_models.insert(
            alias.clone(),
            json!({"endpoint_id": id, "model_id": MODEL_ID, "project_id": PROJECT}),
        );
    }
    store.set(
        &format!("project_models:{PROJECT}"),
        &Json::Object(project_models).to_string(),
    );
    for sub in &setup.jwt_subjects {
        store.set(
            &format!("user_projects:{sub}"),
            &json!({"user_id": USER, "projects": [PROJECT]}).to_string(),
        );
    }

    let jwt_cfg = bud_auth::JwtConfig::from_lookup(|k| match k {
        "OIDC_ISSUER" => Some(JWT_ISSUER.into()),
        "OIDC_ALLOWED_CLIENTS" => Some("bud-playground".into()),
        "OIDC_AUTHZ_TTL_SECS" => Some("300".into()),
        _ => None,
    })
    .unwrap();
    let verifier = Arc::new(bud_auth::JwtVerifier::new(jwt_cfg, Arc::new(StaticJwks)));
    verifier.prime().await.unwrap();
    let plane = Arc::new(bud_auth::BudPlane::with_decryptor(
        store.clone() as Arc<dyn bud_auth::ControlPlaneStore>,
        Some(verifier),
        bud_auth::CredentialDecryptor::from_pem(&test_pem()).unwrap(),
    ));
    plane.boot().await.unwrap();

    let mut state = AppState::new(config()).await;
    {
        let s = Arc::get_mut(&mut state).expect("unshared");
        s.bud_mode = Some(waav_gateway::auth::bud_mode::BudMode::for_plane(plane.clone()).unwrap());
        s.policies = Some(waav_gateway::core::deployment_policy::DeploymentPolicies::local());
        s.realtime = Arc::new(RealtimeRuntime {
            timings: setup.timings,
            client_secret_keys: setup
                .client_secret_keys
                .map(|k| waav_gateway::auth::ephemeral::ClientSecretKeys::parse(k).unwrap()),
            bedrock_http_client: None,
        });
    }
    let app = waav_gateway::routes::openai_realtime::create_openai_realtime_router()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            waav_gateway::middleware::connection_limit_middleware,
        ))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Gateway {
        addr,
        store,
        plane,
        state,
    }
}

fn ep(alias: &str, id: &str, entry: Json) -> (String, String, Json) {
    (alias.to_string(), id.to_string(), entry)
}

// =============================================================================================
// The client
// =============================================================================================

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Connected {
    ws: Client,
    protocol: Option<String>,
    extensions: Option<String>,
}

enum Auth<'a> {
    Bearer(&'a str),
    ApiKey(&'a str),
    Subprotocol(&'a str),
    None,
}

async fn connect_with(
    gw: &Gateway,
    query: &str,
    auth: Auth<'_>,
    extra_headers: &[(&str, &str)],
    extra_protocols: &[&str],
) -> Result<Connected, (u16, Json, HashMap<String, String>)> {
    let mut req = format!("ws://{}/v1/realtime?{query}", gw.addr)
        .into_client_request()
        .unwrap();
    let mut protocols: Vec<String> = Vec::new();
    match auth {
        Auth::Bearer(k) => {
            req.headers_mut()
                .insert("authorization", format!("Bearer {k}").parse().unwrap());
        }
        Auth::ApiKey(k) => {
            req.headers_mut().insert("api-key", k.parse().unwrap());
        }
        Auth::Subprotocol(k) => {
            protocols.push("realtime".into());
            protocols.push(format!("openai-insecure-api-key.{k}"));
        }
        Auth::None => {}
    }
    protocols.extend(extra_protocols.iter().map(|p| p.to_string()));
    if !protocols.is_empty() {
        req.headers_mut().insert(
            "sec-websocket-protocol",
            protocols.join(", ").parse().unwrap(),
        );
    }
    for (k, v) in extra_headers {
        req.headers_mut().insert(
            axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    let connected = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(req),
    )
    .await
    .expect("the handshake did not complete within 15 s");
    match connected {
        Ok((ws, resp)) => Ok(Connected {
            protocol: resp
                .headers()
                .get("sec-websocket-protocol")
                .map(|v| v.to_str().unwrap().to_string()),
            extensions: resp
                .headers()
                .get("sec-websocket-extensions")
                .map(|v| v.to_str().unwrap().to_string()),
            ws,
        }),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            let status = resp.status().as_u16();
            let headers = resp
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            let body = resp
                .body()
                .as_ref()
                .and_then(|b| serde_json::from_slice(b).ok())
                .unwrap_or(Json::Null);
            Err((status, body, headers))
        }
        Err(e) => panic!("unexpected connect error: {e}"),
    }
}

async fn connect(gw: &Gateway, model: &str) -> Client {
    connect_with(gw, &format!("model={model}"), Auth::Bearer(KEY), &[], &[])
        .await
        .unwrap_or_else(|(s, b, _)| panic!("connect refused {s}: {b}"))
        .ws
}

async fn next_json(ws: &mut Client) -> Json {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).unwrap(),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            other => panic!("expected a text event, got {other:?}"),
        }
    }
}

async fn until_type(ws: &mut Client, kind: &str) -> Json {
    for _ in 0..200 {
        let v = next_json(ws).await;
        if v["type"] == kind {
            return v;
        }
    }
    panic!("no {kind}");
}

/// Read until the close frame; returns (error codes seen, close code).
async fn until_close(ws: &mut Client) -> (Vec<String>, Option<u16>) {
    let mut errors = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Json = serde_json::from_str(t.as_str()).unwrap();
                if v["type"] == "error" {
                    errors.push(v["error"]["code"].as_str().unwrap_or_default().to_string());
                }
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                return (errors, frame.map(|f| u16::from(f.code)));
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => return (errors, None),
            Err(_) => panic!("no close within 10 s; errors so far {errors:?}"),
        }
    }
}

/// Wait while still reading, as a live client does: the reads are what answer the server's
/// pings. A test that merely sleeps looks like a dead client and is closed (`client_timeout`).
async fn keep_alive(ws: &mut Client, dur: Duration) {
    let deadline = tokio::time::Instant::now() + dur;
    while tokio::time::Instant::now() < deadline {
        let _ = tokio::time::timeout_at(deadline, ws.next()).await;
    }
}

async fn send(ws: &mut Client, v: Json) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

// =============================================================================================
// Handshake — TC-HS
// =============================================================================================

const EP_HS: &str = "a1a1a1a1-0000-4000-8000-000000000001";

async fn hs_gateway() -> (MockVendor, Gateway) {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep("rt", EP_HS, rt_entry(&vendor, json!({})))],
        jwt_subjects: vec!["sub-hs"],
        ..Default::default()
    })
    .await;
    (vendor, gw)
}

#[tokio::test]
async fn tc_hs_01_02_bearer_and_api_key_header() {
    let (_v, gw) = hs_gateway().await;
    let mut a = connect_with(&gw, "model=rt", Auth::Bearer(KEY), &[], &[])
        .await
        .unwrap();
    assert_eq!(
        until_type(&mut a.ws, "session.created").await["session"]["model"],
        "rt"
    );
    let mut b = connect_with(&gw, "model=rt", Auth::ApiKey(KEY), &[], &[])
        .await
        .unwrap();
    until_type(&mut b.ws, "session.created").await;
}

/// TC-HS-03 🔒 — `realtime` selected, the credential never echoed.
#[tokio::test]
async fn tc_hs_03_subprotocol_auth_selects_realtime_and_never_echoes_the_key() {
    let (_v, gw) = hs_gateway().await;
    let c = connect_with(&gw, "model=rt", Auth::Subprotocol(KEY), &[], &[])
        .await
        .unwrap();
    assert_eq!(c.protocol.as_deref(), Some("realtime"));
}

#[tokio::test]
async fn tc_hs_04_extra_subprotocols_are_tolerated() {
    let (_v, gw) = hs_gateway().await;
    let c = connect_with(
        &gw,
        "model=rt",
        Auth::Subprotocol(KEY),
        &[],
        &["openai-agents-sdk.v0.18", "openai-project.p"],
    )
    .await
    .unwrap();
    assert_eq!(c.protocol.as_deref(), Some("realtime"));
}

#[tokio::test]
async fn tc_hs_05_a_keycloak_jwt_through_the_subprotocol() {
    let (_v, gw) = hs_gateway().await;
    let token = jwt("sub-hs", 300);
    let mut c = connect_with(&gw, "model=rt", Auth::Subprotocol(&token), &[], &[])
        .await
        .unwrap();
    until_type(&mut c.ws, "session.created").await;
}

#[tokio::test]
async fn tc_hs_06_to_09_pre_upgrade_refusals_are_openai_envelopes() {
    let (_v, gw) = hs_gateway().await;
    for (query, auth_key, headers, protocols, status, code) in [
        ("", Some(KEY), vec![], vec![], 400, "model_required"),
        (
            "token=x&model=rt",
            None,
            vec![],
            vec![],
            400,
            "use_subprotocol",
        ),
        (
            "model=rt",
            Some(KEY),
            vec![("openai-beta", "realtime=v1")],
            vec![],
            400,
            "beta_api_shape_disabled",
        ),
        (
            "model=rt",
            Some(KEY),
            vec![],
            vec!["openai-beta.realtime-v1"],
            400,
            "beta_api_shape_disabled",
        ),
        (
            "model=rt&call_id=rtc_x",
            Some(KEY),
            vec![],
            vec![],
            400,
            "unsupported_parameter",
        ),
        (
            "model=rt",
            Some("bud_not_a_key"),
            vec![],
            vec![],
            401,
            "invalid_api_key",
        ),
    ] {
        let auth = auth_key.map(Auth::Bearer).unwrap_or(Auth::None);
        let Err((s, body, _)) = connect_with(&gw, query, auth, &headers, &protocols).await else {
            panic!("{query} {code}: upgraded");
        };
        assert_eq!(s, status, "{code}: {body}");
        assert_eq!(body["error"]["code"], code, "{body}");
        assert!(body["error"]["message"].is_string());
    }
}

#[tokio::test]
async fn tc_hs_10_intent_is_ignored() {
    let (_v, gw) = hs_gateway().await;
    let mut c = connect_with(
        &gw,
        "model=rt&intent=transcription",
        Auth::Bearer(KEY),
        &[],
        &[],
    )
    .await
    .unwrap();
    until_type(&mut c.ws, "session.created").await;
}

#[tokio::test]
async fn tc_hs_11_permessage_deflate_is_never_negotiated() {
    let (_v, gw) = hs_gateway().await;
    let c = connect_with(
        &gw,
        "model=rt",
        Auth::Bearer(KEY),
        &[(
            "sec-websocket-extensions",
            "permessage-deflate; client_max_window_bits",
        )],
        &[],
    )
    .await
    .unwrap();
    assert!(c.extensions.is_none(), "{:?}", c.extensions);
}

#[tokio::test]
async fn tc_hs_13_to_16_resolution() {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let tts = json!({"vendor": "openai", "endpoints": ["text_to_speech"], "model": "tts-1"});
    let gw = gateway(Setup {
        endpoints: vec![
            ep("rt", EP_HS, rt_entry(&vendor, json!({}))),
            ep("tts", "a1a1a1a1-0000-4000-8000-0000000000f1", tts),
        ],
        other_endpoints: vec![ep(
            "theirs",
            "a1a1a1a1-0000-4000-8000-0000000000f2",
            rt_entry(&vendor, json!({})),
        )],
        ..Default::default()
    })
    .await;
    // TC-HS-13 unknown, TC-HS-14 another project's (by alias and by id), TC-HS-15 wrong capability.
    for model in [
        "nope",
        "theirs",
        "a1a1a1a1-0000-4000-8000-0000000000f2",
        "tts",
    ] {
        let Err((s, body, _)) =
            connect_with(&gw, &format!("model={model}"), Auth::Bearer(KEY), &[], &[]).await
        else {
            panic!("{model} upgraded");
        };
        assert_eq!(s, 404, "{model}: {body}");
        assert_eq!(body["error"]["code"], "model_not_found");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("realtime_session")
        );
    }
    // TC-HS-16 the raw endpoint UUID the key reaches.
    let mut c = connect(&gw, EP_HS).await;
    until_type(&mut c, "session.created").await;
}

// =============================================================================================
// Upstream — TC-UP
// =============================================================================================

/// TC-UP-01 / TC-UP-05 — URL, auth header, no beta header; the model is the deployment's.
#[tokio::test]
async fn tc_up_01_05_the_vendor_request() {
    let (vendor, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, json!({"type": "session.update", "session": {"type": "realtime", "model": "gpt-4o-realtime-preview"}})).await;
    until_type(&mut c, "session.updated").await;

    let (pq, headers) = vendor.upgrades()[0].clone();
    assert_eq!(pq, "/v1/realtime?model=gpt-realtime-2.1");
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some(&*format!("Bearer {VENDOR_KEY}"))
    );
    assert!(!headers.contains_key("openai-beta"));
    assert!(
        !headers.values().any(|v| v.contains(KEY)),
        "the Bud key reached the vendor"
    );
    let update = &vendor.frames_of("session.update")[0];
    assert!(update["session"].get("model").is_none(), "{update}");
}

/// TC-UP-07 — a vendor that never answers: `upstream_error` + 1011 within the deadline, and the
/// admission is released (a second session is admitted under `max_concurrent: 1`).
#[tokio::test]
async fn tc_up_07_connect_deadline_releases_the_admission() {
    let vendor = MockVendor::start(Behaviour {
        accept: false,
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000071",
            rt_entry(&vendor, json!({"max_concurrent": 1})),
        )],
        ..Default::default()
    })
    .await;
    let started = std::time::Instant::now();
    let mut c = connect(&gw, "rt").await;
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(code, Some(1011));
    assert_eq!(errors, vec!["upstream_error"]);
    assert!(started.elapsed() < Duration::from_secs(5));
    // The slot came back.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        connect_with(&gw, "model=rt", Auth::Bearer(KEY), &[], &[])
            .await
            .is_ok()
    );
}

/// TC-UP-06 — an open breaker refuses before connecting.
#[tokio::test]
async fn tc_up_06_an_open_breaker_refuses_with_503() {
    let (vendor, gw) = hs_gateway().await;
    let vkey = waav_gateway::core::deployment_policy::vendor_key("openai", Some(&vendor.base()));
    gw.state
        .policies
        .as_ref()
        .unwrap()
        .breakers()
        .vendor
        .open_for(&vkey, Duration::from_secs(30));
    let Err((s, body, headers)) = connect_with(&gw, "model=rt", Auth::Bearer(KEY), &[], &[]).await
    else {
        panic!("upgraded through an open breaker");
    };
    assert_eq!(s, 503);
    assert_eq!(body["error"]["code"], "circuit_open");
    assert!(headers.contains_key("retry-after"));
    assert_eq!(vendor.connections(), 0, "no connect attempt");
}

// =============================================================================================
// Event policy — TC-EVT
// =============================================================================================

async fn policy_gateway(extra: Json) -> (MockVendor, Gateway) {
    let vendor = MockVendor::start(Behaviour {
        rate_limits: true,
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-0000000000e1",
            rt_entry(&vendor, extra),
        )],
        ..Default::default()
    })
    .await;
    (vendor, gw)
}

/// TC-EVT-02 / 04 / 05 / 12 / 13 through a real session.
#[tokio::test]
async fn tc_evt_policy_through_a_live_session() {
    let (vendor, gw) = policy_gateway(json!({})).await;
    let mut c = connect(&gw, "rt").await;
    let created = until_type(&mut c, "session.created").await;
    assert_eq!(created["session"]["model"], "rt", "TC-EVT-13");

    // TC-EVT-02 🔒: MCP refused, session continues.
    send(&mut c, json!({"type": "session.update", "event_id": "c_mcp", "session": {"tools": [{"type": "mcp", "server_url": "https://x"}]}})).await;
    let e = until_type(&mut c, "error").await;
    assert_eq!(e["error"]["code"], "event_not_allowed");
    assert_eq!(e["error"]["param"], "session.tools.mcp");
    assert_eq!(e["error"]["event_id"], "c_mcp");

    // TC-EVT-04 🔒: stored prompt refused.
    send(
        &mut c,
        json!({"type": "session.update", "session": {"prompt": {"id": "pmpt_x"}}}),
    )
    .await;
    assert_eq!(
        until_type(&mut c, "error").await["error"]["param"],
        "session.prompt"
    );

    // TC-EVT-05: tracing stripped, forwarded.
    send(
        &mut c,
        json!({"type": "session.update", "session": {"type": "realtime", "tracing": "auto"}}),
    )
    .await;
    until_type(&mut c, "session.updated").await;

    // TC-EVT-12 🔒: the vendor's rate_limits.updated never arrives.
    send(&mut c, json!({"type": "response.create"})).await;
    loop {
        let v = next_json(&mut c).await;
        assert_ne!(v["type"], "rate_limits.updated", "TC-EVT-12");
        if v["type"] == "response.done" {
            break;
        }
    }
    let updates = vendor.frames_of("session.update");
    assert_eq!(
        updates.len(),
        1,
        "refused frames never reached the vendor: {updates:?}"
    );
    assert!(updates[0]["session"].get("tracing").is_none());
}

/// TC-EVT-14 — the vendor receives the defaults update BEFORE the client's, and the client's
/// voice wins (it arrives later).
#[tokio::test]
async fn tc_evt_14_defaults_first_then_the_client() {
    let (vendor, gw) = policy_gateway(json!({"config": {"realtime": {"session_type": "realtime",
        "defaults": {"voice": "marin", "instructions": "be brief"}}}}))
    .await;
    let mut c = connect(&gw, "rt").await;
    // Sent immediately — before the vendor has even said session.created.
    send(&mut c, json!({"type": "session.update", "session": {"type": "realtime", "audio": {"output": {"voice": "cedar"}}}})).await;
    until_type(&mut c, "session.created").await;
    for _ in 0..2 {
        until_type(&mut c, "session.updated").await;
    }
    let updates = vendor.frames_of("session.update");
    assert_eq!(updates.len(), 2);
    assert!(
        updates[0]["event_id"]
            .as_str()
            .unwrap()
            .starts_with("evt_bud_defaults_"),
        "{updates:?}"
    );
    assert_eq!(updates[0]["session"]["audio"]["output"]["voice"], "marin");
    assert_eq!(updates[0]["session"]["instructions"], "be brief");
    assert_eq!(updates[1]["session"]["audio"]["output"]["voice"], "cedar");
}

/// TC-EVT-15 — the vendor never answers the defaults update: `upstream_error` after the hold.
#[tokio::test]
async fn tc_evt_15_hold_timeout() {
    let vendor = MockVendor::start(Behaviour {
        answer_updates: false,
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-0000000000e5",
            rt_entry(
                &vendor,
                json!({"config": {"realtime": {"defaults": {"voice": "marin"}}}}),
            ),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(code, Some(1011));
    assert_eq!(errors.last().map(String::as_str), Some("upstream_error"));
}

/// TC-EVT-16 — a client that stops reading: 1011 `client_too_slow`, and every delta sent before
/// the close arrives in order.
#[tokio::test]
async fn tc_evt_16_slow_client_is_closed_without_silent_drops() {
    let vendor = MockVendor::start(Behaviour {
        audio_deltas: 2000,
        delta_bytes: 4096,
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-0000000000e6",
            rt_entry(&vendor, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, json!({"type": "response.create"})).await;
    // Stop reading for longer than the slow-client window.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let mut last = -1i64;
    let mut close = None;
    let mut errors = Vec::new();
    while let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(10), c.next()).await {
        match msg {
            Message::Text(t) => {
                let v: Json = serde_json::from_str(t.as_str()).unwrap();
                if v["type"] == "response.output_audio.delta" {
                    let i: i64 = v["delta"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches('0')
                        .parse()
                        .unwrap_or(0);
                    assert!(
                        i == last + 1 || (last == -1 && i == 0),
                        "delta {i} after {last}: out of order or dropped"
                    );
                    last = i;
                } else if v["type"] == "error" {
                    errors.push(v["error"]["code"].as_str().unwrap().to_string());
                }
            }
            Message::Close(f) => {
                close = f.map(|f| u16::from(f.code));
                break;
            }
            _ => {}
        }
    }
    assert_eq!(close, Some(1011));
    assert_eq!(errors, vec!["client_too_slow"]);
    assert!(last >= 0, "some deltas were delivered in order");
}

// =============================================================================================
// Lifecycle — TC-LIFE
// =============================================================================================

/// TC-LIFE-01 — pings on both legs.
#[tokio::test]
async fn tc_life_01_server_pings() {
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    let mut pings = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Ping(_)))) =
            tokio::time::timeout(Duration::from_millis(500), c.next()).await
        {
            pings += 1;
        }
    }
    assert!(pings >= 2, "{pings} pings in 2 s at a 300 ms interval");
}

/// TC-LIFE-02 — a client that stops answering pongs is closed with 1011 after 3 missed.
#[tokio::test]
async fn tc_life_02_dead_client() {
    let cap = Capture::install();
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    // Stop reading: no pongs go back.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    assert_eq!(
        text(&session, "bud.voice.session.end_reason").as_deref(),
        Some("client_timeout")
    );
    assert_eq!(
        number(&session, "bud.voice.session.close_code"),
        Some(1011.0)
    );
}

/// TC-LIFE-03 — a vendor that stops answering pongs: `upstream_error` + 1011.
#[tokio::test]
async fn tc_life_03_dead_vendor() {
    let vendor = MockVendor::start(Behaviour {
        go_silent_after: Some(1),
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000103",
            rt_entry(&vendor, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, json!({"type": "input_audio_buffer.clear"})).await;
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(code, Some(1011));
    assert!(errors.contains(&"upstream_error".to_string()), "{errors:?}");
}

/// A vendor that closes its socket NORMALLY ends the session: `vendor_close` with the vendor's
/// code relayed, no `error` event, and a session span that is not ERROR. An abnormal close is still
/// `upstream_error` + 1011, ERROR.
#[tokio::test]
async fn a_vendors_normal_close_is_vendor_close_not_an_upstream_error() {
    let cap = Capture::install();
    for (n, (code, want_reason, want_client_code, want_errors, want_error_status)) in [
        (
            1000u16,
            "vendor_close",
            1000u16,
            Vec::<String>::new(),
            false,
        ),
        (1001, "vendor_close", 1001, vec![], false),
        (
            1011,
            "upstream_error",
            1011,
            vec!["upstream_error".to_string()],
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let vendor = MockVendor::start(Behaviour {
            close_after: Some((1, code)),
            ..Default::default()
        })
        .await;
        let gw = gateway(Setup {
            endpoints: vec![ep(
                "rt",
                &format!("a1a1a1a1-0000-4000-8000-00000000015{n}"),
                rt_entry(&vendor, json!({})),
            )],
            ..Default::default()
        })
        .await;
        let mut c = connect(&gw, "rt").await;
        until_type(&mut c, "session.created").await;
        send(&mut c, json!({"type": "input_audio_buffer.clear"})).await;
        let (errors, client_code) = until_close(&mut c).await;
        assert_eq!(errors, want_errors, "vendor close {code}");
        assert_eq!(client_code, Some(want_client_code), "vendor close {code}");
        let session = cap.wait_for("voice.session", n + 1).await.remove(n);
        assert_eq!(
            text(&session, "bud.voice.session.end_reason").as_deref(),
            Some(want_reason),
            "vendor close {code}"
        );
        assert_eq!(
            number(&session, "bud.voice.session.close_code"),
            Some(f64::from(want_client_code)),
            "vendor close {code}"
        );
        assert_eq!(
            matches!(session.status, opentelemetry::trace::Status::Error { .. }),
            want_error_status,
            "vendor close {code}: {:?}",
            session.status
        );
    }
}

/// TC-LIFE-05 — maximum length: a warning, then `session_expired` + 1000.
#[tokio::test]
async fn tc_life_05_maximum_length() {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000105",
            rt_entry(
                &vendor,
                json!({"config": {"realtime": {"limits": {"max_session_seconds": 2}}}}),
            ),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors, vec!["session_expiring", "session_expired"]);
    assert_eq!(code, Some(1000));
}

/// TC-LIFE-04 — idle.
#[tokio::test]
async fn tc_life_04_idle() {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000104",
            rt_entry(
                &vendor,
                json!({"config": {"realtime": {"limits": {"idle_timeout_seconds": 1}}}}),
            ),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors.last().map(String::as_str), Some("session_expired"));
    assert_eq!(code, Some(1000));
}

/// TC-LIFE-06 🔒 — the key is revoked mid-session: `session_revoked` + 1008 by the next timer.
#[tokio::test]
async fn tc_life_06_revoked_key_closes_on_the_timer() {
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    let key = format!("api_key:{}", bud_auth::hash_api_key(KEY));
    gw.store.remove(&key);
    gw.plane
        .on_key_event(&key, bud_auth::KeyEvent::Del)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors, vec!["session_revoked"]);
    assert_eq!(code, Some(1008));
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// TC-LIFE-07 🔒 — revoked, then `response.create` at once: refused before forwarding.
#[tokio::test]
async fn tc_life_07_revoked_key_closes_on_response_create() {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000107",
            rt_entry(&vendor, json!({})),
        )],
        timings: Timings {
            revalidate: Duration::from_secs(3600),
            ..fast_timings()
        },
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    let key = format!("api_key:{}", bud_auth::hash_api_key(KEY));
    gw.store.remove(&key);
    gw.plane
        .on_key_event(&key, bud_auth::KeyEvent::Del)
        .await
        .unwrap();
    send(&mut c, json!({"type": "response.create"})).await;
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors, vec!["session_revoked"]);
    assert_eq!(code, Some(1008));
    assert!(
        vendor.frames_of("response.create").is_empty(),
        "forwarded after revocation"
    );
}

/// TC-LIFE-08 — a JWT user removed from the project: closed by the next timer (Q-7 eviction).
#[tokio::test]
async fn tc_life_08_jwt_user_removed_from_the_project() {
    let (_v, gw) = hs_gateway().await;
    let token = jwt("sub-hs", 300);
    let mut c = connect_with(&gw, "model=rt", Auth::Subprotocol(&token), &[], &[])
        .await
        .unwrap()
        .ws;
    until_type(&mut c, "session.created").await;
    gw.store.set(
        "user_projects:sub-hs",
        &json!({"user_id": USER, "projects": []}).to_string(),
    );
    gw.plane
        .on_key_event("user_projects:sub-hs", bud_auth::KeyEvent::Set)
        .await
        .unwrap();
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors, vec!["session_revoked"]);
    assert_eq!(code, Some(1008));
}

/// TC-LIFE-08 (second half) — JWT EXPIRY alone does not end a started session (D-17).
#[tokio::test]
async fn tc_life_08_jwt_expiry_does_not_close_a_started_session() {
    let (_v, gw) = hs_gateway().await;
    let token = jwt("sub-hs", 2);
    let mut c = connect_with(&gw, "model=rt", Auth::Subprotocol(&token), &[], &[])
        .await
        .unwrap()
        .ws;
    until_type(&mut c, "session.created").await;
    keep_alive(&mut c, Duration::from_secs(3)).await;
    send(&mut c, json!({"type": "response.create"})).await;
    until_type(&mut c, "response.done").await;
}

/// TC-LIFE-09 — the endpoint is unpublished mid-session.
#[tokio::test]
async fn tc_life_09_unpublished_endpoint() {
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    let key = format!("voice_table:{EP_HS}");
    gw.store.remove(&key);
    gw.plane
        .on_key_event(&key, bud_auth::KeyEvent::Del)
        .await
        .unwrap();
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors, vec!["session_revoked"]);
    assert_eq!(code, Some(1008));
}

/// TC-LIFE-10 🔒 — one concurrency slot held per session and released at close.
#[tokio::test]
async fn tc_life_10_concurrency_held_and_released() {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000110",
            rt_entry(&vendor, json!({"max_concurrent": 1})),
        )],
        ..Default::default()
    })
    .await;
    let mut first = connect(&gw, "rt").await;
    until_type(&mut first, "session.created").await;
    let Err((s, body, headers)) = connect_with(&gw, "model=rt", Auth::Bearer(KEY), &[], &[]).await
    else {
        panic!("a second session was admitted under max_concurrent: 1");
    };
    assert_eq!(s, 429);
    assert_eq!(body["error"]["code"], "concurrency_limit_exceeded");
    assert!(headers.contains_key("retry-after"));
    first.close(None).await.unwrap();
    let _ = until_close(&mut first).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut again = connect(&gw, "rt").await;
    until_type(&mut again, "session.created").await;
}

/// TC-LIFE-11 — drain: `server_shutdown` then 1012.
#[tokio::test]
async fn tc_life_11_drain() {
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    gw.state.shutdown.cancel();
    let (errors, code) = until_close(&mut c).await;
    assert_eq!(errors, vec!["server_shutdown"]);
    assert_eq!(code, Some(1012));
}

/// TC-LIFE-12 — the FRD-022 connection slot returns to zero after sessions and refusals.
#[tokio::test]
async fn tc_life_12_connection_slots_return_to_zero() {
    let (_v, gw) = hs_gateway().await;
    for _ in 0..20 {
        let mut c = connect(&gw, "rt").await;
        until_type(&mut c, "session.created").await;
        c.close(None).await.unwrap();
        let _ = until_close(&mut c).await;
        let _ = connect_with(&gw, "model=nope", Auth::Bearer(KEY), &[], &[]).await;
    }
    for _ in 0..100 {
        if gw.state.ws_connection_count() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} connection slots leaked", gw.state.ws_connection_count());
}

// =============================================================================================
// Metering — TC-MET
// =============================================================================================

/// TC-MET-01 / 02 🔒 / 03 🔒 / 07 — three responses, three root turns with their own traces, each
/// linked to the session; the session span's totals are the sum of its turns.
#[tokio::test]
async fn tc_met_01_02_03_07_per_response_turns_and_the_session_record() {
    let cap = Capture::install();
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    for _ in 0..3 {
        send(&mut c, json!({"type": "response.create"})).await;
        until_type(&mut c, "response.done").await;
    }
    c.close(None).await.unwrap();
    let _ = until_close(&mut c).await;

    let turns = cap.wait_for("voice.turn", 3).await;
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    let mut traces = std::collections::HashSet::new();
    let mut indices = Vec::new();
    for t in &turns {
        assert_eq!(
            t.parent_span_id,
            SpanId::INVALID,
            "TC-MET-02: a turn has no parent"
        );
        assert!(
            traces.insert(t.span_context.trace_id()),
            "TC-MET-02: turns share a trace"
        );
        assert_ne!(t.span_context.trace_id(), session.span_context.trace_id());
        assert!(
            t.links
                .links
                .iter()
                .any(|l| l.span_context.span_id() == session.span_context.span_id()),
            "TC-MET-02: no link to the session span"
        );
        assert_eq!(
            text(t, "bud.voice.capability").as_deref(),
            Some("realtime_session")
        );
        assert_eq!(text(t, "bud.voice.transport").as_deref(), Some("websocket"));
        assert_eq!(
            text(t, "bud.voice.rt.component").as_deref(),
            Some("response")
        );
        assert_eq!(text(t, "bud.endpoint_id").as_deref(), Some(EP_HS));
        assert_eq!(text(t, "bud.voice.endpoint_name").as_deref(), Some("rt"));
        assert_eq!(text(t, "bud.project_id").as_deref(), Some(PROJECT));
        assert_eq!(text(t, "bud.model_id").as_deref(), Some(MODEL_ID));
        assert_eq!(text(t, "bud.api_key_id").as_deref(), Some(API_KEY_ID));
        assert_eq!(text(t, "bud.user_id").as_deref(), Some(USER));
        assert_eq!(
            text(t, "bud.voice.vendor_session_id").as_deref(),
            Some("sess_vendor_1")
        );
        assert_eq!(text(t, "bud.voice.rt.vendor").as_deref(), Some("openai"));
        assert_eq!(
            text(t, "bud.voice.rt.model").as_deref(),
            Some("gpt-realtime-2.1")
        );
        assert_eq!(number(t, "bud.voice.rt.input_text_tokens"), Some(119.0));
        assert_eq!(number(t, "bud.voice.rt.cached_text_tokens"), Some(64.0));
        assert_eq!(number(t, "bud.voice.rt.output_audio_tokens"), Some(91.0));
        let cost = number(t, "bud.voice.cost").expect("TC-MET-03: priced");
        assert!((cost - 0.0072056).abs() < 1e-10, "TC-MET-03 cost {cost}");
        assert_eq!(text(t, "bud.voice.pricing_unit").as_deref(), Some("token"));
        indices.push(number(t, "bud.voice.turn_index").unwrap() as u64);
    }
    indices.sort_unstable();
    assert_eq!(indices, vec![0, 1, 2], "TC-MET-01");
    let session_ids: std::collections::HashSet<_> = turns
        .iter()
        .map(|t| text(t, "bud.voice.session_id").unwrap())
        .collect();
    assert_eq!(session_ids.len(), 1);

    // TC-MET-07
    assert_eq!(number(&session, "bud.voice.session.turns"), Some(3.0));
    assert_eq!(
        text(&session, "bud.voice.session.end_reason").as_deref(),
        Some("client_close")
    );
    assert_eq!(
        number(&session, "bud.voice.rt.input_text_tokens"),
        Some(357.0)
    );
    let total = number(&session, "bud.voice.cost").unwrap();
    assert!((total - 3.0 * 0.0072056).abs() < 1e-9, "{total}");
    assert!(number(&session, "bud.voice.session.duration_ms").unwrap() > 0.0);
    assert_eq!(
        text(&session, "bud.voice.rt.session_type").as_deref(),
        Some("realtime")
    );
}

/// TC-MET-06 — an input transcription is its own priced turn.
#[tokio::test]
async fn tc_met_06_transcription_turn() {
    let cap = Capture::install();
    let vendor = MockVendor::start(Behaviour {
        transcription_usage: Some(json!({"type": "duration", "seconds": 12})),
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000206",
            rt_entry(&vendor, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, json!({"type": "input_audio_buffer.commit"})).await;
    until_type(
        &mut c,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    let t = cap.wait_for("voice.turn", 1).await.remove(0);
    assert_eq!(
        text(&t, "bud.voice.rt.component").as_deref(),
        Some("input_transcription")
    );
    assert_eq!(number(&t, "bud.voice.billed_seconds"), Some(12.0));
    assert!((number(&t, "bud.voice.cost").unwrap() - 0.0006).abs() < 1e-12);
}

/// TC-MET-08 — the client's TCP dies after two responses: both turns are exported, and the
/// session record says how it ended.
#[tokio::test]
async fn tc_met_08_drop_mid_session_keeps_the_turns() {
    let cap = Capture::install();
    let (_v, gw) = hs_gateway().await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    for _ in 0..2 {
        send(&mut c, json!({"type": "response.create"})).await;
        until_type(&mut c, "response.done").await;
    }
    drop(c);
    let turns = cap.wait_for("voice.turn", 2).await;
    assert_eq!(turns.len(), 2);
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    assert_eq!(
        text(&session, "bud.voice.session.end_reason").as_deref(),
        Some("client_close")
    );
}

/// TC-XL-07 (duration billing) — a minute price bills 1 s segments here (shortened), with the
/// final partial one.
#[tokio::test]
async fn duration_priced_sessions_bill_segments() {
    let cap = Capture::install();
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000207",
            rt_entry(
                &vendor,
                json!({"pricing": {"unit": "minute", "cost_per_unit": 0.06, "per_units": 1}}),
            ),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "rt").await;
    until_type(&mut c, "session.created").await;
    keep_alive(&mut c, Duration::from_millis(2500)).await;
    c.close(None).await.unwrap();
    let _ = until_close(&mut c).await;
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    let segments: Vec<SpanData> = cap
        .spans()
        .into_iter()
        .filter(|s| {
            s.name == "voice.turn"
                && text(s, "bud.voice.rt.component").as_deref() == Some("duration_segment")
        })
        .collect();
    assert_eq!(segments.len(), 3, "1 s + 1 s + the partial remainder");
    let billed: f64 = segments
        .iter()
        .map(|s| number(s, "bud.voice.billed_seconds").unwrap())
        .sum();
    assert!((billed - number(&session, "bud.voice.billed_seconds").unwrap()).abs() < 1e-9);
    assert!(billed > 2.4 && billed < 3.5, "{billed}");
    for s in &segments {
        assert_eq!(text(s, "bud.voice.pricing_unit").as_deref(), Some("minute"));
    }
}

// =============================================================================================
// xAI — TC-XL-06 (relayed, not translated)
// =============================================================================================

/// TC-XL-06 — an xAI deployment is RELAYED: its upstream is the xAI GA URL with the vendor model
/// and Bearer; xAI's `conversation.created` bootstrap starts the session (the client gets a
/// GA `session.created`, the deployment defaults are applied); cumulative
/// `…input_audio_transcription.updated` events reach the client verbatim and in order; and no
/// `rate_limits.updated` is needed for anything.
#[tokio::test]
async fn tc_xl_06_xai_relay_quirks() {
    let cap = Capture::install();
    let vendor = MockVendor::start(Behaviour {
        xai: true,
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "grok-rt",
            "a1a1a1a1-0000-4000-8000-0000000000a6",
            rt_entry(
                &vendor,
                json!({"vendor": "grok", "model": "grok-voice-2",
                       "config": {"realtime": {"defaults": {"voice": "ara"}}}}),
            ),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "grok-rt").await;
    let created = until_type(&mut c, "session.created").await;
    assert_eq!(created["session"]["model"], "grok-rt");
    // The defaults went to xAI once its session existed.
    for _ in 0..50 {
        if !vendor.frames_of("session.update").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let defaults = &vendor.frames_of("session.update")[0];
    assert_eq!(defaults["session"]["audio"]["output"]["voice"], "ara");

    send(&mut c, json!({"type": "input_audio_buffer.commit"})).await;
    let mut partials = Vec::new();
    loop {
        let v = next_json(&mut c).await;
        match v["type"].as_str() {
            Some("conversation.item.input_audio_transcription.updated") => {
                partials.push(v["transcript"].as_str().unwrap().to_string())
            }
            Some("conversation.item.input_audio_transcription.completed") => break,
            _ => {}
        }
    }
    assert_eq!(partials, vec!["hel", "hello", "hello world"]);

    send(&mut c, json!({"type": "response.create"})).await;
    until_type(&mut c, "response.done").await;
    // No `rate_limits.updated` ever arrives, and the session is fine without it.
    keep_alive(&mut c, Duration::from_millis(700)).await;
    send(&mut c, json!({"type": "response.create"})).await;
    until_type(&mut c, "response.done").await;

    let (pq, headers) = vendor.upgrades()[0].clone();
    assert_eq!(pq, "/v1/realtime?model=grok-voice-2");
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some(&*format!("Bearer {VENDOR_KEY}"))
    );
    // Two responses (and the unpriced transcription turn xAI reported without usage).
    let turns = cap.wait_for("voice.turn", 3).await;
    let responses: Vec<&SpanData> = turns
        .iter()
        .filter(|t| text(t, "bud.voice.rt.component").as_deref() == Some("response"))
        .collect();
    assert_eq!(responses.len(), 2);
    for t in responses {
        assert_eq!(text(t, "bud.voice.rt.vendor").as_deref(), Some("grok"));
        assert_eq!(
            text(t, "bud.voice.vendor_session_id").as_deref(),
            Some("conv_xai_1")
        );
        assert!(number(t, "bud.voice.cost").is_some());
    }
}

/// CONTRACTS C7 — an xAI deployment priced PER MINUTE (xAI bills its Voice Agent API by the
/// minute) is billed by duration segments; its responses carry tokens and no cost.
#[tokio::test]
async fn xai_priced_per_minute_bills_duration_segments() {
    let cap = Capture::install();
    let vendor = MockVendor::start(Behaviour {
        xai: true,
        ..Default::default()
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "grok-min",
            "a1a1a1a1-0000-4000-8000-0000000000a7",
            rt_entry(
                &vendor,
                json!({"vendor": "grok", "model": "grok-voice-2",
                       "pricing": {"unit": "minute", "cost_per_unit": 0.08, "per_units": 1}}),
            ),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "grok-min").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, json!({"type": "response.create"})).await;
    until_type(&mut c, "response.done").await;
    keep_alive(&mut c, Duration::from_millis(1500)).await;
    c.close(None).await.unwrap();
    let _ = until_close(&mut c).await;
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    let turns: Vec<SpanData> = cap
        .spans()
        .into_iter()
        .filter(|s| s.name == "voice.turn")
        .collect();
    let segments: Vec<&SpanData> = turns
        .iter()
        .filter(|t| text(t, "bud.voice.rt.component").as_deref() == Some("duration_segment"))
        .collect();
    assert!(segments.len() >= 2, "a full segment and the remainder");
    for s in &segments {
        let secs = number(s, "bud.voice.billed_seconds").unwrap();
        assert!((number(s, "bud.voice.cost").unwrap() - secs / 60.0 * 0.08).abs() < 1e-12);
    }
    let response = turns
        .iter()
        .find(|t| text(t, "bud.voice.rt.component").as_deref() == Some("response"))
        .expect("the response is still recorded");
    assert_eq!(
        number(response, "bud.voice.cost"),
        None,
        "a minute price bills no tokens"
    );
    assert!(number(&session, "bud.voice.billed_seconds").unwrap() > 1.5);
}

// =============================================================================================
// Credentials never logged — TC-SEC-07
// =============================================================================================

/// TC-SEC-07 🔒 — the Bud key and the vendor key appear in no log line and no span.
#[tokio::test]
async fn tc_sec_07_credentials_never_reach_logs_or_spans() {
    let cap = Capture::install();
    let (_v, gw) = hs_gateway().await;
    for auth in [Auth::Bearer(KEY), Auth::ApiKey(KEY), Auth::Subprotocol(KEY)] {
        let mut c = connect_with(&gw, "model=rt", auth, &[], &[])
            .await
            .unwrap()
            .ws;
        until_type(&mut c, "session.created").await;
        send(&mut c, json!({"type": "response.create"})).await;
        until_type(&mut c, "response.done").await;
        c.close(None).await.unwrap();
        let _ = until_close(&mut c).await;
    }
    cap.wait_for("voice.session", 3).await;
    let logs = cap.logs();
    assert!(!logs.is_empty(), "the log capture is live");
    for secret in [KEY, VENDOR_KEY, "sk-process-canary"] {
        assert!(!logs.contains(secret), "{secret} appeared in a log line");
        for s in cap.spans() {
            for kv in s.attributes.iter() {
                assert!(
                    !kv.value.as_str().contains(secret),
                    "{secret} on span {}",
                    s.name
                );
            }
        }
    }
}

// =============================================================================================
// Client secrets — TC-EK
// =============================================================================================

async fn mint(gw: &Gateway, bearer: &str, body: Json) -> (u16, Json) {
    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/realtime/client_secrets", gw.addr))
        .bearer_auth(bearer)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Json::Null))
}

async fn ek_gateway() -> (MockVendor, Gateway) {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![
            ep(
                "rt",
                "a1a1a1a1-0000-4000-8000-000000000301",
                rt_entry(&vendor, json!({})),
            ),
            ep(
                "rt2",
                "a1a1a1a1-0000-4000-8000-000000000302",
                rt_entry(&vendor, json!({})),
            ),
        ],
        other_endpoints: vec![ep(
            "theirs",
            "a1a1a1a1-0000-4000-8000-000000000303",
            rt_entry(&vendor, json!({})),
        )],
        jwt_subjects: vec!["sub-ek"],
        ..Default::default()
    })
    .await;
    (vendor, gw)
}

/// TC-EK-01 / 05 — mint, then connect with the secret; turns are attributed to the parent.
#[tokio::test]
async fn tc_ek_01_05_mint_and_connect() {
    let cap = Capture::install();
    let (_v, gw) = ek_gateway().await;
    let before = now();
    let (s, body) = mint(&gw, KEY, json!({"session": {"model": "rt"}})).await;
    assert_eq!(s, 200, "{body}");
    let value = body["value"].as_str().unwrap().to_string();
    assert!(value.starts_with("ek_bud_"));
    let exp = body["expires_at"].as_u64().unwrap();
    assert!(
        (before + 600..=now() + 600).contains(&exp),
        "default TTL 600: {exp}"
    );
    assert_eq!(body["session"]["model"], "rt");

    let c = connect_with(&gw, "model=rt", Auth::Subprotocol(&value), &[], &[])
        .await
        .unwrap();
    assert_eq!(c.protocol.as_deref(), Some("realtime"), "TC-EK-15");
    let mut ws = c.ws;
    until_type(&mut ws, "session.created").await;
    send(&mut ws, json!({"type": "response.create"})).await;
    until_type(&mut ws, "response.done").await;
    let t = cap.wait_for("voice.turn", 1).await.remove(0);
    assert_eq!(text(&t, "bud.api_key_id").as_deref(), Some(API_KEY_ID));
    assert_eq!(text(&t, "bud.project_id").as_deref(), Some(PROJECT));
}

#[tokio::test]
async fn tc_ek_02_03_04_08_mint_and_connect_refusals() {
    let (_v, gw) = ek_gateway().await;
    for (body, status) in [
        (
            json!({"expires_after": {"anchor": "created_at", "seconds": 5}, "session": {"model": "rt"}}),
            400,
        ),
        (
            json!({"expires_after": {"anchor": "created_at", "seconds": 7201}, "session": {"model": "rt"}}),
            400,
        ),
        (json!({"session": {}}), 400),
        (json!({"session": {"model": "theirs"}}), 403),
    ] {
        let (s, b) = mint(&gw, KEY, body.clone()).await;
        assert_eq!(s, status, "{body} → {b}");
    }
    let (_, ok) = mint(&gw, KEY, json!({"session": {"model": "rt"}})).await;
    let secret = ok["value"].as_str().unwrap().to_string();
    // TC-EK-04: no chaining.
    let (s, _) = mint(&gw, &secret, json!({"session": {"model": "rt"}})).await;
    assert_eq!(s, 401);
    // TC-EK-08: a secret for `rt` cannot open `rt2`.
    let Err((s, body, _)) =
        connect_with(&gw, "model=rt2", Auth::Subprotocol(&secret), &[], &[]).await
    else {
        panic!("a secret opened a deployment it was not minted for");
    };
    assert_eq!(s, 403);
    assert_eq!(body["error"]["code"], "model_mismatch");
}

/// TC-EK-09 🔒 / TC-EK-14 🔒 — the parent key revoked: connect refused, a live session closed.
#[tokio::test]
async fn tc_ek_09_14_parent_revocation() {
    let (_v, gw) = ek_gateway().await;
    let (_, ok) = mint(&gw, KEY, json!({"session": {"model": "rt"}})).await;
    let secret = ok["value"].as_str().unwrap().to_string();
    let mut live = connect_with(&gw, "model=rt", Auth::Subprotocol(&secret), &[], &[])
        .await
        .unwrap()
        .ws;
    until_type(&mut live, "session.created").await;

    // TC-EK-14: the parent is exactly the snapshot key — deleting `api_key:{hash_api_key(K)}` is
    // what revokes the secret.
    let key = format!("api_key:{}", bud_auth::hash_api_key(KEY));
    gw.store.remove(&key);
    gw.plane
        .on_key_event(&key, bud_auth::KeyEvent::Del)
        .await
        .unwrap();

    let (errors, code) = until_close(&mut live).await;
    assert_eq!(errors, vec!["session_revoked"]);
    assert_eq!(code, Some(1008));
    let Err((s, _, _)) = connect_with(&gw, "model=rt", Auth::Subprotocol(&secret), &[], &[]).await
    else {
        panic!("a secret of a revoked parent connected");
    };
    assert_eq!(s, 401);
}

/// TC-EK-12 🔒 — a JWT parent caps the lifetime; under 10 s left, minting is refused.
#[tokio::test]
async fn tc_ek_12_jwt_parent_caps_the_lifetime() {
    let (_v, gw) = ek_gateway().await;
    let token = jwt("sub-ek", 120);
    let (s, body) = mint(&gw, &token, json!({"expires_after": {"anchor": "created_at", "seconds": 600}, "session": {"model": "rt"}})).await;
    assert_eq!(s, 200, "{body}");
    let exp = body["expires_at"].as_u64().unwrap();
    assert!(
        exp <= now() + 121 && exp >= now() + 110,
        "capped at the JWT's exp: {exp}"
    );

    let short = jwt("sub-ek", 5);
    let (s, body) = mint(&gw, &short, json!({"session": {"model": "rt"}})).await;
    assert_eq!(s, 401);
    assert_eq!(body["error"]["code"], "credential_expiring");
}

/// TC-EK-17 — a JWT parent removed from the project: connect refused.
#[tokio::test]
async fn tc_ek_17_jwt_parent_revoked() {
    let (_v, gw) = ek_gateway().await;
    let (_, ok) = mint(
        &gw,
        &jwt("sub-ek", 300),
        json!({"session": {"model": "rt"}}),
    )
    .await;
    let secret = ok["value"].as_str().unwrap().to_string();
    gw.store.set(
        "user_projects:sub-ek",
        &json!({"user_id": USER, "projects": []}).to_string(),
    );
    gw.plane
        .on_key_event("user_projects:sub-ek", bud_auth::KeyEvent::Set)
        .await
        .unwrap();
    let Err((s, _, _)) = connect_with(&gw, "model=rt", Auth::Subprotocol(&secret), &[], &[]).await
    else {
        panic!("connected on a revoked JWT parent");
    };
    assert_eq!(s, 401);
}

/// No keys configured: the mint route answers 501.
#[tokio::test]
async fn client_secrets_unconfigured_is_501() {
    let vendor = MockVendor::start(Behaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "rt",
            "a1a1a1a1-0000-4000-8000-000000000304",
            rt_entry(&vendor, json!({})),
        )],
        client_secret_keys: None,
        ..Default::default()
    })
    .await;
    let (s, body) = mint(&gw, KEY, json!({"session": {"model": "rt"}})).await;
    assert_eq!(s, 501);
    assert_eq!(body["error"]["code"], "client_secrets_not_configured");
}
