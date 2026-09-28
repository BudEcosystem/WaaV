//! FRD-023 RT7 end to end, in process: `/v1/realtime` speaking OpenAI Realtime GA to real
//! WebSocket clients while WaaV's native providers speak each vendor's own protocol to an
//! in-process MOCK of that protocol (TC-XL-01…05, TC-XL-07).
//!
//! * Gemini Live — a WebSocket mock of `BidiGenerateContent` (`setup` / `setupComplete`,
//!   `realtimeInput`, `clientContent`, `serverContent` + `usageMetadata`, `goAway`, session
//!   resumption).
//! * Nova 2 Sonic — an aws-smithy `HttpConnector` that speaks the Bedrock
//!   `InvokeModelWithBidirectionalStream` HTTP event stream: it decodes the SDK's SIGNED input
//!   event-stream frames and answers with output frames, so the SigV4 request, the event-stream
//!   framing and the Nova JSON events are all exercised (the precedent: the Transcribe mock in
//!   `mock_endpoint_e2e.rs`).
//! * The per-minute agents (Deepgram Voice Agent, ElevenLabs Agents, Hume EVI) — WebSocket mocks
//!   that open the vendor's session and record what they receive.
//!
//! No real vendor is called and no vendor key exists.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use serde_json::{Value as Json, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing_subscriber::layer::SubscriberExt;

use waav_gateway::config::{DAGTimeoutsConfig, PluginConfig, ServerConfig};
use waav_gateway::handlers::openai_realtime::{RealtimeRuntime, Timings};
use waav_gateway::state::AppState;

// =============================================================================================
// Fixtures
// =============================================================================================

const KEY: &str = "bud_realtime_translate_test_key";
const PROJECT: &str = "5b0c7e1d-0000-4000-8000-00000000ab01";
const USER: &str = "5b0c7e1d-0000-4000-8000-00000000bc01";
const API_KEY_ID: &str = "5b0c7e1d-0000-4000-8000-00000000cd01";
const MODEL_ID: &str = "5b0c7e1d-0000-4000-8000-00000000de01";

/// bud-auth's fixture ciphertext; the plaintext is `VENDOR_KEY`.
const TEST_CREDENTIAL: &str = include_str!("../../bud-auth/tests/fixtures/test_cred_encrypted.hex");
const VENDOR_KEY: &str = "dg_vendor_key_abc123";

fn test_pem() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bud-auth/tests/fixtures/test_cred_private.pem"
    ))
    .expect("bud-auth's fixture key (git-ignored *.pem) must be present locally")
}

/// Encrypt a credential exactly as budapp does (RSA-OAEP-SHA-256 blocks, hex).
fn encrypt_like_budapp(plain: &str) -> String {
    use rsa::pkcs8::DecodePrivateKey;
    let pem = test_pem();
    let key = rsa::RsaPrivateKey::from_pkcs8_pem(&pem)
        .or_else(|_| {
            use rsa::pkcs1::DecodeRsaPrivateKey;
            rsa::RsaPrivateKey::from_pkcs1_pem(&pem)
        })
        .expect("the fixture key loads");
    let public = key.to_public_key();
    let mut out = Vec::new();
    for chunk in plain
        .as_bytes()
        .chunks(rsa::traits::PublicKeyParts::size(&key) - 66)
    {
        out.extend(
            public
                .encrypt(
                    &mut rsa::rand_core::OsRng,
                    rsa::Oaep::new::<sha2::Sha256>(),
                    chunk,
                )
                .unwrap(),
        );
    }
    hex::encode(out)
}

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
}

impl Capture {
    fn install() -> Self {
        let exported = Exported::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exported.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::filter::LevelFilter::DEBUG)
            .with(
                tracing_opentelemetry::layer().with_tracer(provider.tracer("realtime-translate")),
            );
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            exported,
            _guard: guard,
            _provider: provider,
        }
    }

    fn spans(&self) -> Vec<SpanData> {
        self.exported.0.lock().unwrap().clone()
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
        panic!("expected {n} `{name}` spans, got {}", self.spans().len());
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
        deepgram_api_key: Some("dg-process-canary".to_string()),
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
        gemini_api_key: Some("gemini-process-canary".to_string()),
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
        aws_access_key_id: Some("AKIDPROCESSCANARY".to_string()),
        aws_secret_access_key: Some("process-canary".to_string()),
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
        connect: Duration::from_millis(3000),
        hold: Duration::from_millis(1500),
        slow_client: Duration::from_millis(600),
        max_session: Duration::from_secs(3600),
        default_idle: Duration::from_secs(300),
        warn_before: Duration::from_secs(1),
        segment: Duration::from_secs(1),
        upstream_send: Duration::from_secs(2),
        connection_cap: None,
    }
}

/// Gemini-shaped token rates (CONTRACTS C7), per 1 000 000 tokens.
fn token_pricing() -> Json {
    json!({"unit": "token", "per_units": 1000000, "currency": "USD",
        "rates": {"input_text": 0.5, "input_audio": 3.0, "cached_input_text": 0.05,
                  "cached_input_audio": 0.3, "output_text": 2.0, "output_audio": 12.0}})
}

struct Gateway {
    addr: SocketAddr,
}

struct Setup {
    endpoints: Vec<(String, String, Json)>,
    timings: Timings,
    bedrock: Option<aws_smithy_runtime_api::client::http::SharedHttpClient>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
            timings: fast_timings(),
            bedrock: None,
        }
    }
}

async fn gateway(setup: Setup) -> Gateway {
    allow_loopback();
    let store = Arc::new(bud_auth::MemoryStore::new());
    let mut m = serde_json::Map::new();
    for (alias, id, _) in &setup.endpoints {
        m.insert(
            alias.clone(),
            json!({"endpoint_id": id, "model_id": MODEL_ID, "project_id": PROJECT, "kind": "model"}),
        );
    }
    m.insert(
        "__metadata__".into(),
        json!({"api_key_id": API_KEY_ID, "user_id": USER, "api_key_project_id": PROJECT}),
    );
    store.set(
        &format!("api_key:{}", bud_auth::hash_api_key(KEY)),
        &Json::Object(m).to_string(),
    );
    for (_, id, entry) in &setup.endpoints {
        store.set(
            &format!("voice_table:{id}"),
            &json!({ id.as_str(): entry }).to_string(),
        );
    }
    let plane = Arc::new(bud_auth::BudPlane::with_decryptor(
        store.clone() as Arc<dyn bud_auth::ControlPlaneStore>,
        None,
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
            client_secret_keys: None,
            bedrock_http_client: setup.bedrock,
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
    Gateway { addr }
}

fn ep(alias: &str, id: &str, entry: Json) -> (String, String, Json) {
    (alias.to_string(), id.to_string(), entry)
}

// =============================================================================================
// The client
// =============================================================================================

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(gw: &Gateway, model: &str) -> Client {
    let mut req = format!("ws://{}/v1/realtime?model={model}", gw.addr)
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {KEY}").parse().unwrap());
    match tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(req),
    )
    .await
    .expect("the handshake did not complete within 15 s")
    {
        Ok((ws, _)) => ws,
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            let body = resp
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned());
            panic!("connect refused {}: {body:?}", resp.status())
        }
        Err(e) => panic!("connect failed: {e}"),
    }
}

async fn send(ws: &mut Client, v: Json) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// The next text event; a close or a silent 5 s fails the test with what was seen.
async fn next_json(ws: &mut Client) -> Json {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(t.as_str()).unwrap(),
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
            other => panic!("expected a text event, got {other:?}"),
        }
    }
}

/// Read until `kind`; every event read on the way is returned too.
async fn until_type(ws: &mut Client, kind: &str) -> (Json, Vec<Json>) {
    let mut seen = Vec::new();
    for _ in 0..500 {
        let v = next_json(ws).await;
        if std::env::var("RT7_DEBUG").is_ok() {
            eprintln!("<- {v}");
        }
        if v["type"] == kind {
            return (v, seen);
        }
        seen.push(v);
    }
    panic!(
        "no {kind}; saw {:?}",
        seen.iter().map(|e| e["type"].clone()).collect::<Vec<_>>()
    );
}

/// Wait while still reading (the reads answer the server's pings); returns what arrived.
async fn keep_alive(ws: &mut Client, dur: Duration) -> Vec<Json> {
    let deadline = tokio::time::Instant::now() + dur;
    let mut seen = Vec::new();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => seen.push(serde_json::from_str(t.as_str()).unwrap()),
            Ok(Some(Ok(Message::Close(f)))) => panic!("the session closed: {f:?}; saw {seen:?}"),
            Ok(None) => panic!("the session ended; saw {seen:?}"),
            _ => {}
        }
    }
    seen
}

async fn close_and_drain(ws: &mut Client) {
    let _ = ws.close(None).await;
    for _ in 0..500 {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => return,
            _ => {}
        }
    }
}

fn user_text(text: &str) -> Json {
    json!({"type": "conversation.item.create", "item": {"type": "message", "role": "user",
        "content": [{"type": "input_text", "text": text}]}})
}

// =============================================================================================
// Gemini Live mock (BidiGenerateContent over WebSocket)
// =============================================================================================

/// 20 ms of 24 kHz PCM16 with a recognisable byte pattern.
fn gemini_out_pcm() -> Vec<u8> {
    (0..960u32).map(|i| (i % 251) as u8).collect()
}

#[derive(Clone, Default)]
struct GeminiBehaviour {
    /// After the first turn on the first connection: `goAway`, then keep the socket open (the
    /// gateway must reconnect on its own, with the resumption handle).
    go_away_after_first_turn: bool,
}

#[derive(Default)]
struct GeminiLog {
    /// Per connection: the URL it was opened with.
    urls: Vec<String>,
    /// Per connection: every `setup` it received.
    setups: Vec<Vec<Json>>,
    /// Every `realtimeInput.mediaChunks[]`: (connection, mime, decoded bytes).
    audio: Vec<(usize, String, usize)>,
    /// Every `clientContent` text: (connection, text).
    texts: Vec<(usize, String)>,
}

#[derive(Clone)]
struct GeminiMock {
    addr: SocketAddr,
    log: Arc<Mutex<GeminiLog>>,
}

impl GeminiMock {
    async fn start(b: GeminiBehaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(GeminiLog::default()));
        let shared = log.clone();
        tokio::spawn(async move {
            let mut n = 0usize;
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let conn = n;
                n += 1;
                let log = shared.clone();
                let b = b.clone();
                tokio::spawn(async move {
                    let captured = log.clone();
                    let callback = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                         resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        let mut l = captured.lock().unwrap();
                        l.urls.push(req.uri().to_string());
                        l.setups.push(Vec::new());
                        Ok(resp)
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await
                    else {
                        return;
                    };
                    let mut turns = 0usize;
                    while let Some(Ok(msg)) = ws.next().await {
                        let Message::Text(t) = msg else { continue };
                        let v: Json = serde_json::from_str(t.as_str()).unwrap_or(Json::Null);
                        let mut out: Vec<Json> = Vec::new();
                        if let Some(setup) = v.get("setup") {
                            log.lock().unwrap().setups[conn].push(setup.clone());
                            out.push(json!({"setupComplete": {}}));
                            out.push(json!({"sessionResumptionUpdate": {
                                "newHandle": format!("handle-{conn}"), "resumable": true}}));
                        } else if let Some(ri) = v.get("realtimeInput") {
                            for c in ri["mediaChunks"].as_array().into_iter().flatten() {
                                let bytes = BASE64_STANDARD
                                    .decode(c["data"].as_str().unwrap_or(""))
                                    .map(|b| b.len())
                                    .unwrap_or(0);
                                log.lock().unwrap().audio.push((
                                    conn,
                                    c["mimeType"].as_str().unwrap_or("").to_string(),
                                    bytes,
                                ));
                            }
                        } else if let Some(cc) = v.get("clientContent") {
                            let said = cc["turns"][0]["parts"][0]["text"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            log.lock().unwrap().texts.push((conn, said));
                            turns += 1;
                            out.push(json!({"serverContent": {
                                "modelTurn": {"parts": [{"inlineData": {
                                    "mimeType": "audio/pcm;rate=24000",
                                    "data": BASE64_STANDARD.encode(gemini_out_pcm())}}]},
                                "outputTranscription": {"text": "Hello"}}}));
                            out.push(json!({"serverContent": {
                                "outputTranscription": {"text": " there"}, "turnComplete": true},
                                "usageMetadata": {
                                    "promptTokenCount": 150, "cachedContentTokenCount": 40,
                                    "responseTokenCount": 60, "totalTokenCount": 210,
                                    "promptTokensDetails": [{"modality": "TEXT", "tokenCount": 100},
                                                            {"modality": "AUDIO", "tokenCount": 50}],
                                    "cacheTokensDetails": [{"modality": "TEXT", "tokenCount": 40}],
                                    "responseTokensDetails": [{"modality": "AUDIO", "tokenCount": 50},
                                                              {"modality": "TEXT", "tokenCount": 10}]}}));
                            if b.go_away_after_first_turn && conn == 0 && turns == 1 {
                                out.push(json!({"goAway": {"timeLeft": "5s"}}));
                            }
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
        format!("http://{}/ws/gemini", self.addr)
    }

    fn setups(&self) -> Vec<Vec<Json>> {
        self.log.lock().unwrap().setups.clone()
    }
}

fn gemini_entry(mock: &GeminiMock, extra: Json) -> Json {
    let mut e = json!({
        "vendor": "gemini",
        "api_base": mock.base(),
        "credential": TEST_CREDENTIAL.trim(),
        "endpoints": ["realtime_session"],
        "model": "gemini-3.8-live",
        "pricing": token_pricing(),
        "config": {"realtime": {"defaults": {"voice": "Puck", "instructions": "deployment prompt"},
                                "limits": {"max_session_seconds": 900}}}
    });
    for (k, v) in extra.as_object().into_iter().flatten() {
        e[k] = v.clone();
    }
    e
}

/// TC-XL-01 🔒 — the Gemini setup is built from the client's first `session.update` (over the
/// deployment defaults) and sent ONCE; a later change of voice or tools is refused with
/// `event_not_allowed` naming the field; the session continues and no second setup is sent.
/// The deployment's key authenticates the vendor leg, never the process's.
#[tokio::test]
async fn tc_xl_01_gemini_setup_from_the_ga_session_update() {
    let mock = GeminiMock::start(GeminiBehaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "live",
            "c7c7c7c7-0000-4000-8000-000000000101",
            gemini_entry(&mock, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "live").await;
    let (created, _) = until_type(&mut c, "session.created").await;
    assert_eq!(created["session"]["model"], "live");
    assert_eq!(
        created["session"]["audio"]["output"]["voice"], "Puck",
        "the deployment's default"
    );

    let tool = json!({"type": "function", "name": "get_weather", "description": "d",
        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}});
    send(
        &mut c,
        json!({"type": "session.update", "event_id": "c1", "session": {
        "type": "realtime", "model": "gpt-realtime", "instructions": "client prompt",
        "audio": {"output": {"voice": "Kore"}}, "tools": [tool]}}),
    )
    .await;
    let (updated, _) = until_type(&mut c, "session.updated").await;
    assert_eq!(updated["session"]["audio"]["output"]["voice"], "Kore");
    assert_eq!(
        updated["session"]["model"], "live",
        "the client's `model` is never used"
    );

    let setups = mock.setups();
    assert_eq!(setups.len(), 1, "one connection");
    assert_eq!(setups[0].len(), 1, "one setup frame");
    let setup = &setups[0][0];
    assert_eq!(setup["model"], "models/gemini-3.8-live");
    assert_eq!(
        setup["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
        "Kore"
    );
    assert_eq!(
        setup["systemInstruction"]["parts"][0]["text"],
        "client prompt"
    );
    assert_eq!(
        setup["tools"][0]["functionDeclarations"][0]["name"],
        "get_weather"
    );
    let url = mock.log.lock().unwrap().urls[0].clone();
    assert!(
        url.contains(&format!("key={VENDOR_KEY}")),
        "the deployment's key: {url}"
    );
    assert!(!url.contains("gemini-process-canary"));

    // A later voice change: refused by name, not silently ignored.
    send(
        &mut c,
        json!({"type": "session.update", "event_id": "c2", "session": {
        "audio": {"output": {"voice": "Charon"}}}}),
    )
    .await;
    let (e, _) = until_type(&mut c, "error").await;
    assert_eq!(e["error"]["code"], "event_not_allowed");
    assert_eq!(e["error"]["param"], "session.audio.output.voice");
    assert_eq!(e["error"]["event_id"], "c2");
    // A later tools change: the same.
    send(
        &mut c,
        json!({"type": "session.update", "session": {"tools": []}}),
    )
    .await;
    let (e, _) = until_type(&mut c, "error").await;
    assert_eq!(e["error"]["param"], "session.tools");
    // `conversation.item.truncate` has no Gemini equivalent (FRD §5.7).
    send(&mut c, json!({"type": "conversation.item.truncate", "item_id": "i", "content_index": 0, "audio_end_ms": 5})).await;
    let (e, _) = until_type(&mut c, "error").await;
    assert_eq!(e["error"]["param"], "conversation.item.truncate");

    // The session is still alive and still set up once.
    send(&mut c, user_text("hi")).await;
    until_type(&mut c, "response.done").await;
    assert_eq!(mock.setups()[0].len(), 1, "no second setup");
    close_and_drain(&mut c).await;
}

/// TC-XL-02 — 24 kHz client audio reaches Gemini resampled to 16 kHz (declared as such); the
/// vendor's 24 kHz output reaches the client unchanged as `response.output_audio.delta`.
#[tokio::test]
async fn tc_xl_02_gemini_audio_rates() {
    let mock = GeminiMock::start(GeminiBehaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "live",
            "c7c7c7c7-0000-4000-8000-000000000102",
            gemini_entry(&mock, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "live").await;
    until_type(&mut c, "session.created").await;
    // 1 s of a tone at 24 kHz, in 20 ms appends (the first one sets the session up).
    let pcm: Vec<u8> = (0..24_000)
        .flat_map(|i| {
            ((f32::sin(i as f32 * 440.0 * std::f32::consts::TAU / 24_000.0) * 8000.0) as i16)
                .to_le_bytes()
        })
        .collect();
    for chunk in pcm.chunks(960) {
        send(
            &mut c,
            json!({"type": "input_audio_buffer.append",
            "audio": BASE64_STANDARD.encode(chunk)}),
        )
        .await;
    }
    send(&mut c, json!({"type": "input_audio_buffer.commit"})).await;
    until_type(&mut c, "input_audio_buffer.committed").await;
    let mut received = 0usize;
    for _ in 0..100 {
        let audio = mock.log.lock().unwrap().audio.clone();
        assert!(
            audio
                .iter()
                .all(|(_, mime, _)| mime == "audio/pcm;rate=16000"),
            "every chunk declared 16 kHz"
        );
        received = audio.iter().map(|(_, _, n)| n).sum();
        if received >= 31_000 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // 1 s at 16 kHz PCM16 = 32 000 bytes (± the resampler's one chunk).
    assert!(
        (31_000..=33_000).contains(&received),
        "{received} bytes at 16 kHz"
    );

    send(&mut c, user_text("say something")).await;
    let (_, seen) = until_type(&mut c, "response.done").await;
    let audio: Vec<u8> = seen
        .iter()
        .filter(|e| e["type"] == "response.output_audio.delta")
        .flat_map(|e| {
            BASE64_STANDARD
                .decode(e["delta"].as_str().unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(
        audio,
        gemini_out_pcm(),
        "24 kHz output passes through unchanged"
    );
    let order: Vec<String> = seen
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_string())
        .filter(|t| t.starts_with("response."))
        .collect();
    assert_eq!(order.first().map(String::as_str), Some("response.created"));
    close_and_drain(&mut c).await;
}

/// TC-XL-03 🔒 — Gemini's `usageMetadata` becomes the `usage` of the `response.done` it closes
/// and exactly one priced `voice.turn` per response (cached text a subset of input text).
#[tokio::test]
async fn tc_xl_03_gemini_usage_is_the_response_usage_and_a_priced_turn() {
    let cap = Capture::install();
    let mock = GeminiMock::start(GeminiBehaviour::default()).await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "live",
            "c7c7c7c7-0000-4000-8000-000000000103",
            gemini_entry(&mock, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "live").await;
    until_type(&mut c, "session.created").await;
    for i in 0..2 {
        send(&mut c, user_text(&format!("turn {i}"))).await;
        let (done, seen) = until_type(&mut c, "response.done").await;
        let usage = &done["response"]["usage"];
        assert_eq!(usage["input_tokens"], 150);
        assert_eq!(usage["output_tokens"], 60);
        assert_eq!(usage["input_token_details"]["text_tokens"], 100);
        assert_eq!(usage["input_token_details"]["audio_tokens"], 50);
        assert_eq!(
            usage["input_token_details"]["cached_tokens_details"]["text_tokens"],
            40
        );
        assert_eq!(usage["output_token_details"]["audio_tokens"], 50);
        let transcript: String = seen
            .iter()
            .filter(|e| e["type"] == "response.output_audio_transcript.delta")
            .map(|e| e["delta"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(transcript, "Hello there");
    }
    close_and_drain(&mut c).await;

    let turns = cap.wait_for("voice.turn", 2).await;
    assert_eq!(turns.len(), 2, "one billed record per response, never two");
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    // (100 − 40)·0.5 + 40·0.05 + 50·3 + 10·2 + 50·12 = 30 + 2 + 150 + 20 + 600 = 802 per 10⁶.
    for t in &turns {
        assert_eq!(
            text(t, "bud.voice.rt.component").as_deref(),
            Some("response")
        );
        assert_eq!(text(t, "bud.voice.rt.vendor").as_deref(), Some("gemini"));
        assert_eq!(
            text(t, "bud.voice.rt.model").as_deref(),
            Some("gemini-3.8-live")
        );
        assert_eq!(
            text(t, "bud.voice.rt.response_status").as_deref(),
            Some("completed")
        );
        assert_eq!(number(t, "bud.voice.rt.input_text_tokens"), Some(100.0));
        assert_eq!(number(t, "bud.voice.rt.cached_text_tokens"), Some(40.0));
        assert_eq!(number(t, "bud.voice.rt.output_audio_tokens"), Some(50.0));
        let cost = number(t, "bud.voice.cost").expect("priced");
        assert!((cost - 802.0e-6).abs() < 1e-12, "cost {cost}");
        assert_eq!(text(t, "bud.voice.pricing_unit").as_deref(), Some("token"));
        assert_eq!(
            text(t, "bud.endpoint_id").as_deref(),
            Some("c7c7c7c7-0000-4000-8000-000000000103")
        );
        assert_eq!(text(t, "bud.api_key_id").as_deref(), Some(API_KEY_ID));
    }
    assert_eq!(number(&session, "bud.voice.session.turns"), Some(2.0));
    assert!((number(&session, "bud.voice.cost").unwrap() - 2.0 * 802.0e-6).abs() < 1e-12);
}

/// TC-XL-04 — Gemini's `goAway`: WaaV reconnects on its own with the resumption handle the
/// vendor issued, and the client sees no close and no error — the next turn simply works.
#[tokio::test]
async fn tc_xl_04_gemini_go_away_is_a_transparent_resumption() {
    let mock = GeminiMock::start(GeminiBehaviour {
        go_away_after_first_turn: true,
    })
    .await;
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "live",
            "c7c7c7c7-0000-4000-8000-000000000104",
            gemini_entry(&mock, json!({})),
        )],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "live").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, user_text("first")).await;
    until_type(&mut c, "response.done").await;
    // The goAway follows the turn; the gateway replaces the connection meanwhile.
    let seen = keep_alive(&mut c, Duration::from_millis(800)).await;
    assert!(
        seen.iter().all(|e| e["type"] != "error"),
        "the client is told nothing: {seen:?}"
    );
    let setups = mock.setups();
    assert_eq!(setups.len(), 2, "a second connection");
    assert_eq!(
        setups[1][0]["sessionResumption"]["handle"], "handle-0",
        "resumed with the handle the vendor issued"
    );
    assert!(setups[0][0]["sessionResumption"].get("handle").is_none());

    send(&mut c, user_text("second")).await;
    until_type(&mut c, "response.done").await;
    let texts = mock.log.lock().unwrap().texts.clone();
    assert_eq!(
        texts,
        vec![(0, "first".to_string()), (1, "second".to_string())]
    );
    close_and_drain(&mut c).await;
}

// =============================================================================================
// Nova 2 Sonic mock: the Bedrock bidirectional event stream, behind an aws-smithy connector
// =============================================================================================

mod bedrock {
    use super::*;
    use aws_smithy_eventstream::frame::{
        DecodedFrame, MessageFrameDecoder, read_message_from, write_message_to,
    };
    use aws_smithy_runtime_api::client::http::{
        HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings, SharedHttpClient,
        SharedHttpConnector,
    };
    use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
    use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
    use aws_smithy_runtime_api::http::{Response, StatusCode};
    use aws_smithy_types::body::SdkBody;
    use aws_smithy_types::event_stream::{Header, HeaderValue, Message as EsMessage};
    use http_body_util::BodyExt;

    /// One stream's output half, fed by the mock.
    struct ChanBody(mpsc::UnboundedReceiver<Bytes>);

    impl http_body::Body for ChanBody {
        type Data = Bytes;
        type Error = std::convert::Infallible;
        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Bytes>, Self::Error>>> {
            self.0
                .poll_recv(cx)
                .map(|o| o.map(|b| Ok(http_body::Frame::data(b))))
        }
    }

    #[derive(Default)]
    pub struct NovaLog {
        /// Per stream: (uri, Authorization header).
        pub calls: Vec<(String, String)>,
        /// Per stream: the Nova events received, in order.
        pub events: Vec<Vec<Json>>,
    }

    /// The Nova events a stream answers a user text turn with: a speculative and a final
    /// assistant transcript, one audio block, then a usage report whose `delta` is this turn's
    /// and whose `total` is the running sum ACROSS streams — a meter that read totals would bill
    /// twice.
    fn answer(stream: usize, turn: usize, total_turns: u64) -> Vec<Json> {
        let content = format!("ct-{stream}-{turn}");
        vec![
            json!({"event": {"completionStart": {"completionId": format!("cmp-{stream}-{turn}")}}}),
            json!({"event": {"contentStart": {"type": "TEXT", "role": "ASSISTANT",
                "contentId": format!("tx-{stream}-{turn}"),
                "additionalModelFields": "{\"generationStage\":\"SPECULATIVE\"}"}}}),
            json!({"event": {"textOutput": {"content": "Sure."}}}),
            json!({"event": {"contentStart": {"type": "TEXT", "role": "ASSISTANT",
                "contentId": format!("txf-{stream}-{turn}"),
                "additionalModelFields": "{\"generationStage\":\"FINAL\"}"}}}),
            json!({"event": {"textOutput": {"content": "Sure."}}}),
            json!({"event": {"contentStart": {"type": "AUDIO", "role": "ASSISTANT", "contentId": content}}}),
            json!({"event": {"audioOutput": {"contentId": content,
                "content": BASE64_STANDARD.encode(vec![7u8; 480])}}}),
            json!({"event": {"contentEnd": {"contentId": content, "stopReason": "END_TURN"}}}),
            json!({"event": {"usageEvent": {"completionId": "c", "details": {
                "delta": {"input": {"speechTokens": 40, "textTokens": 3},
                          "output": {"speechTokens": 50, "textTokens": 7}},
                "total": {"input": {"speechTokens": 40 * total_turns, "textTokens": 3 * total_turns},
                          "output": {"speechTokens": 50 * total_turns, "textTokens": 7 * total_turns}}}}}}),
            json!({"event": {"completionEnd": {"completionId": format!("cmp-{stream}-{turn}")}}}),
        ]
    }

    fn output_frame(event: &Json) -> Bytes {
        let payload = json!({"bytes": BASE64_STANDARD.encode(event.to_string())}).to_string();
        let msg = EsMessage::new(payload.into_bytes())
            .add_header(Header::new(
                ":message-type",
                HeaderValue::String("event".into()),
            ))
            .add_header(Header::new(
                ":event-type",
                HeaderValue::String("chunk".into()),
            ))
            .add_header(Header::new(
                ":content-type",
                HeaderValue::String("application/json".into()),
            ));
        let mut out = Vec::new();
        write_message_to(&msg, &mut out).unwrap();
        Bytes::from(out)
    }

    /// The Nova event inside one input frame: SigV4 wraps each event in an outer message whose
    /// payload is the inner `chunk` message, whose payload is `{"bytes": base64(event json)}`.
    fn input_event(outer: &EsMessage) -> Option<Json> {
        let signed = outer
            .headers()
            .iter()
            .any(|h| h.name().as_str() == ":chunk-signature");
        let inner = if signed {
            if outer.payload().is_empty() {
                return None; // the closing empty signed frame
            }
            read_message_from(outer.payload().as_ref()).ok()?
        } else {
            outer.clone()
        };
        let body: Json = serde_json::from_slice(inner.payload()).ok()?;
        let bytes = BASE64_STANDARD.decode(body["bytes"].as_str()?).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    #[derive(Clone, Default)]
    pub struct NovaMock {
        pub log: Arc<Mutex<NovaLog>>,
        turns: Arc<Mutex<u64>>,
    }

    impl std::fmt::Debug for NovaMock {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("NovaMock")
        }
    }

    impl NovaMock {
        pub fn client(&self) -> SharedHttpClient {
            SharedHttpClient::new(self.clone())
        }
    }

    impl HttpConnector for NovaMock {
        fn call(&self, req: HttpRequest) -> HttpConnectorFuture {
            let stream = {
                let mut log = self.log.lock().unwrap();
                log.calls.push((
                    req.uri().to_string(),
                    req.headers()
                        .get("authorization")
                        .unwrap_or_default()
                        .to_string(),
                ));
                log.events.push(Vec::new());
                log.calls.len() - 1
            };
            let (tx, rx) = mpsc::unbounded_channel::<Bytes>();
            let log = self.log.clone();
            let turns = self.turns.clone();
            let mut body = req.into_body();
            tokio::spawn(async move {
                let mut buf = BytesMut::new();
                let mut decoder = MessageFrameDecoder::new();
                let mut turn = 0usize;
                let mut user_text_open = false;
                while let Some(Ok(frame)) = body.frame().await {
                    let Ok(data) = frame.into_data() else {
                        continue;
                    };
                    buf.extend_from_slice(&data);
                    while let Ok(DecodedFrame::Complete(outer)) = decoder.decode_frame(&mut buf) {
                        let Some(ev) = input_event(&outer) else {
                            continue;
                        };
                        log.lock().unwrap().events[stream].push(ev.clone());
                        let e = &ev["event"];
                        if e["contentStart"]["role"] == "USER"
                            && e["contentStart"]["type"] == "TEXT"
                        {
                            user_text_open = true;
                        }
                        if e.get("textInput").is_some() && user_text_open {
                            user_text_open = false;
                            turn += 1;
                            let total = {
                                let mut t = turns.lock().unwrap();
                                *t += 1;
                                *t
                            };
                            for out in answer(stream, turn, total) {
                                if tx.send(output_frame(&out)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            });
            HttpConnectorFuture::new(async move {
                let mut resp = Response::new(
                    StatusCode::try_from(200u16).unwrap(),
                    SdkBody::from_body_1_x(ChanBody(rx)),
                );
                resp.headers_mut()
                    .insert("content-type", "application/vnd.amazon.eventstream");
                Ok(resp)
            })
        }
    }

    impl HttpClient for NovaMock {
        fn http_connector(
            &self,
            _s: &HttpConnectorSettings,
            _c: &RuntimeComponents,
        ) -> SharedHttpConnector {
            SharedHttpConnector::new(self.clone())
        }
    }
}

fn nova_entry() -> Json {
    json!({
        "vendor": "nova_sonic",
        "credential": encrypt_like_budapp(
            r#"{"access_key_id":"AKIDNOVADEPLOYMENT","secret_access_key":"nova-deployment-secret"}"#,
        ),
        "endpoints": ["realtime_session"],
        "model": "amazon.nova-2-sonic-v1:0",
        "provider_params": {"region": "eu-north-1"},
        "pricing": {"unit": "token", "per_units": 1000000, "currency": "USD",
            "rates": {"input_text": 0.06, "input_audio": 3.4, "output_text": 0.24, "output_audio": 13.6}},
        "config": {"realtime": {"defaults": {"voice": "tiffany", "instructions": "be helpful"}}}
    })
}

/// TC-XL-05 🔒 — Nova 2 Sonic: the Bedrock stream is SigV4-signed with the DEPLOYMENT's key
/// pair in its region (never the gateway's AWS identity); the connection cap (shortened here
/// from 8 minutes) is met by a reconnect INSIDE the translator — a new stream with the session's
/// history, invisible to the client — and every `usageEvent` is metered exactly once, from its
/// `delta`, even though the second stream's `total` counts both.
#[tokio::test]
async fn tc_xl_05_nova_sonic_cap_reconnect_and_usage_metered_once() {
    let cap = Capture::install();
    let nova = bedrock::NovaMock::default();
    let gw = gateway(Setup {
        endpoints: vec![ep(
            "sonic",
            "c7c7c7c7-0000-4000-8000-000000000105",
            nova_entry(),
        )],
        timings: Timings {
            connection_cap: Some(Duration::from_millis(1200)),
            ..fast_timings()
        },
        bedrock: Some(nova.client()),
    })
    .await;
    let mut c = connect(&gw, "sonic").await;
    until_type(&mut c, "session.created").await;
    send(&mut c, user_text("first")).await;
    let (done, seen) = until_type(&mut c, "response.done").await;
    assert_eq!(
        done["response"]["usage"]["input_token_details"]["audio_tokens"],
        40
    );
    assert_eq!(
        done["response"]["usage"]["output_token_details"]["text_tokens"],
        7
    );
    let transcript: String = seen
        .iter()
        .filter(|e| e["type"] == "response.output_audio_transcript.delta")
        .map(|e| e["delta"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        transcript, "Sure.",
        "the FINAL block restates the SPECULATIVE one"
    );
    assert!(
        seen.iter()
            .any(|e| e["type"] == "response.output_audio.delta")
    );

    // Past the cap: a second stream, and the client notices nothing.
    let seen = keep_alive(&mut c, Duration::from_millis(1600)).await;
    assert!(seen.iter().all(|e| e["type"] != "error"), "{seen:?}");
    send(&mut c, user_text("second")).await;
    let (done, _) = until_type(&mut c, "response.done").await;
    assert_eq!(
        done["response"]["usage"]["input_token_details"]["audio_tokens"], 40,
        "the delta, not the total"
    );
    close_and_drain(&mut c).await;

    let (calls, events) = {
        let log = nova.log.lock().unwrap();
        (log.calls.clone(), log.events.clone())
    };
    assert!(
        calls.len() >= 2,
        "the cap replaced the stream: {} streams",
        calls.len()
    );
    for (uri, auth) in &calls {
        assert!(
            uri.starts_with("https://bedrock-runtime.eu-north-1.amazonaws.com/"),
            "{uri}"
        );
        assert!(uri.contains("amazon.nova-2-sonic-v1"), "{uri}");
        assert!(auth.contains("Credential=AKIDNOVADEPLOYMENT/"), "{auth}");
        assert!(
            !auth.contains("AKIDPROCESSCANARY"),
            "never the gateway's identity"
        );
    }
    // Every stream opens a Nova session with the deployment's voice and prompt.
    for events in events.iter().filter(|e| !e.is_empty()) {
        assert!(
            events[0]["event"].get("sessionStart").is_some(),
            "{:?}",
            events[0]
        );
        let prompt_start = events
            .iter()
            .find(|e| e["event"].get("promptStart").is_some())
            .unwrap();
        assert_eq!(
            prompt_start["event"]["promptStart"]["audioOutputConfiguration"]["voiceId"],
            "tiffany"
        );
    }
    // The second stream carries the conversation so far.
    let second = &events[1];
    assert!(
        second
            .iter()
            .any(|e| e["event"]["textInput"]["content"] == "Sure."),
        "history replayed on the new stream: {second:?}"
    );

    // The session record is written at close, after every turn.
    let session = cap.wait_for("voice.session", 1).await.remove(0);
    let turns: Vec<SpanData> = cap
        .spans()
        .into_iter()
        .filter(|s| s.name == "voice.turn")
        .collect();
    assert_eq!(
        turns.len(),
        2,
        "two usage reports, two billed records — not three, not four"
    );
    assert_eq!(number(&session, "bud.voice.session.turns"), Some(2.0));
    assert_eq!(
        number(&session, "bud.voice.rt.input_audio_tokens"),
        Some(80.0)
    );
    for t in &turns {
        assert_eq!(
            text(t, "bud.voice.rt.vendor").as_deref(),
            Some("nova_sonic")
        );
        assert_eq!(number(t, "bud.voice.rt.input_audio_tokens"), Some(40.0));
        assert_eq!(number(t, "bud.voice.rt.output_audio_tokens"), Some(50.0));
        // (3·0.06 + 40·3.4 + 7·0.24 + 50·13.6) / 10⁶
        let want = (3.0 * 0.06 + 40.0 * 3.4 + 7.0 * 0.24 + 50.0 * 13.6) / 1e6;
        assert!((number(t, "bud.voice.cost").unwrap() - want).abs() < 1e-12);
    }
}

/// FRD-023 RT7.2 🔒 — a Nova deployment without its AWS key pair is refused before the
/// upgrade, although the gateway process holds AWS keys of its own.
#[tokio::test]
async fn nova_without_a_key_pair_is_refused_before_the_upgrade() {
    let mut entry = nova_entry();
    entry["credential"] = json!(TEST_CREDENTIAL.trim()); // a plain key, not a pair
    let nova = bedrock::NovaMock::default();
    let gw = gateway(Setup {
        endpoints: vec![ep("sonic", "c7c7c7c7-0000-4000-8000-000000000106", entry)],
        bedrock: Some(nova.client()),
        ..Default::default()
    })
    .await;
    let mut req = format!("ws://{}/v1/realtime?model=sonic", gw.addr)
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {KEY}").parse().unwrap());
    let Err(tokio_tungstenite::tungstenite::Error::Http(resp)) =
        tokio_tungstenite::connect_async(req).await
    else {
        panic!("upgraded without the deployment's key pair");
    };
    assert_eq!(resp.status().as_u16(), 502);
    let body: Json = serde_json::from_slice(resp.body().as_ref().unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "deployment_misconfigured");
    assert!(nova.log.lock().unwrap().calls.is_empty(), "no Bedrock call");
}

// =============================================================================================
// The per-minute agents: Deepgram Voice Agent, ElevenLabs Agents, Hume EVI
// =============================================================================================

#[derive(Clone)]
struct AgentMock {
    addr: SocketAddr,
    /// (path?query, lower-cased headers) per upgrade.
    upgrades: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
}

impl AgentMock {
    async fn start(hello: Json) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let upgrades = Arc::new(Mutex::new(Vec::new()));
        let shared = upgrades.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let hello = hello.clone();
                let captured = shared.clone();
                tokio::spawn(async move {
                    let callback = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                         resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        let headers = req
                            .headers()
                            .iter()
                            .map(|(k, v)| (k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("").to_string()))
                            .collect();
                        let pq = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
                        captured.lock().unwrap().push((pq, headers));
                        Ok(resp)
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await
                    else {
                        return;
                    };
                    let _ = ws.send(Message::Text(hello.to_string().into())).await;
                    while let Some(Ok(_)) = ws.next().await {}
                });
            }
        });
        Self { addr, upgrades }
    }

    fn base(&self) -> String {
        format!("http://{}/agent", self.addr)
    }
}

/// TC-XL-07 🔒 — a per-minute vendor with NO duration price still meters its time: the vendor
/// bills by the minute whatever Bud charges, so the segments are recorded with their seconds and
/// marked unpriced (`duration`) — never a session that used 72 s of vendor time and left no record.
#[tokio::test]
async fn tc_xl_07_an_unpriced_per_minute_vendor_still_meters_its_time() {
    let cap = Capture::install();
    let mock = AgentMock::start(json!({"type": "Welcome", "request_id": "dg-req-2"})).await;
    let entry = json!({
        "vendor": "deepgram_voice_agent",
        "api_base": mock.base(),
        "credential": TEST_CREDENTIAL.trim(),
        "endpoints": ["realtime_session"],
        "model": "gpt-4o-mini"
    });
    let id = "c7c7c7c7-0000-4000-8000-000000000180";
    let gw = gateway(Setup {
        endpoints: vec![ep("agent", id, entry)],
        ..Default::default()
    })
    .await;
    let mut c = connect(&gw, "agent").await;
    until_type(&mut c, "session.created").await;
    send(
        &mut c,
        json!({"type": "session.update", "session": {"instructions": "hello"}}),
    )
    .await;
    until_type(&mut c, "session.updated").await;
    keep_alive(&mut c, Duration::from_millis(2500)).await;
    close_and_drain(&mut c).await;

    let sessions = cap.wait_for("voice.session", 1).await;
    let session = sessions
        .iter()
        .find(|s| text(s, "bud.endpoint_id").as_deref() == Some(id))
        .unwrap();
    let segments: Vec<SpanData> = cap
        .spans()
        .into_iter()
        .filter(|s| s.name == "voice.turn" && text(s, "bud.endpoint_id").as_deref() == Some(id))
        .collect();
    assert!(
        segments.len() >= 2,
        "segments are recorded without a price: {}",
        segments.len()
    );
    for s in &segments {
        assert_eq!(
            text(s, "bud.voice.rt.component").as_deref(),
            Some("duration_segment")
        );
        assert!(number(s, "bud.voice.billed_seconds").unwrap() > 0.0);
        assert!(number(s, "bud.voice.cost").is_none(), "unpriced, never $0");
        assert_eq!(
            text(s, "bud.voice.unpriced_components").as_deref(),
            Some("duration")
        );
    }
    let total: f64 = segments
        .iter()
        .map(|s| number(s, "bud.voice.billed_seconds").unwrap())
        .sum();
    assert!(total > 2.3 && total < 3.6, "{total}");
    assert!((total - number(session, "bud.voice.billed_seconds").unwrap()).abs() < 1e-9);
}

/// TC-XL-07 🔒 — a per-minute vendor bills DURATION SEGMENTS from the moment its connection
/// opens: full segments while the session lives and the partial remainder at close (segments
/// shortened to 1 s here, so ~2.5 s bills 1 + 1 + ~0.5; the 60 + 60 + 30 arithmetic is the
/// metering unit test), each priced per minute and none as tokens — for all three vendors.
#[tokio::test]
async fn tc_xl_07_per_minute_vendors_bill_duration_segments() {
    let cap = Capture::install();
    let cases = [
        (
            "deepgram_voice_agent",
            json!({"type": "Welcome", "request_id": "dg-req-1"}),
            json!({"model": "gpt-4o-mini"}),
        ),
        (
            "elevenlabs_convai",
            json!({"type": "conversation_initiation_metadata",
                   "conversation_initiation_metadata_event": {"conversation_id": "conv_1"}}),
            json!({"model": "agent_abc"}),
        ),
        (
            "hume_evi",
            json!({"type": "chat_metadata", "chat_id": "chat_1", "chat_group_id": "g_1", "request_id": "r_1"}),
            json!({}),
        ),
    ];
    for (i, (vendor, hello, extra)) in cases.into_iter().enumerate() {
        let mock = AgentMock::start(hello).await;
        let mut entry = json!({
            "vendor": vendor,
            "api_base": mock.base(),
            "credential": TEST_CREDENTIAL.trim(),
            "endpoints": ["realtime_session"],
            "pricing": {"unit": "minute", "cost_per_unit": 0.08, "per_units": 1, "currency": "USD"}
        });
        for (k, v) in extra.as_object().into_iter().flatten() {
            entry[k] = v.clone();
        }
        let id = format!("c7c7c7c7-0000-4000-8000-00000000017{i}");
        let gw = gateway(Setup {
            endpoints: vec![ep("agent", &id, entry)],
            ..Default::default()
        })
        .await;
        let mut c = connect(&gw, "agent").await;
        until_type(&mut c, "session.created").await;
        send(
            &mut c,
            json!({"type": "session.update", "session": {"instructions": "hello"}}),
        )
        .await;
        until_type(&mut c, "session.updated").await;
        keep_alive(&mut c, Duration::from_millis(2500)).await;
        close_and_drain(&mut c).await;

        let sessions = cap.wait_for("voice.session", i + 1).await;
        let session = sessions
            .iter()
            .find(|s| text(s, "bud.endpoint_id").as_deref() == Some(id.as_str()))
            .unwrap();
        let segments: Vec<SpanData> = cap
            .spans()
            .into_iter()
            .filter(|s| {
                s.name == "voice.turn" && text(s, "bud.endpoint_id").as_deref() == Some(id.as_str())
            })
            .collect();
        assert!(!segments.is_empty(), "{vendor}: no segment");
        for s in &segments {
            assert_eq!(
                text(s, "bud.voice.rt.component").as_deref(),
                Some("duration_segment"),
                "{vendor}: a per-minute vendor bills no token turns"
            );
            assert_eq!(text(s, "bud.voice.pricing_unit").as_deref(), Some("minute"));
            let secs = number(s, "bud.voice.billed_seconds").unwrap();
            assert!(
                secs <= 1.0 + 1e-9,
                "{vendor}: a segment is at most the segment length"
            );
            let cost = number(s, "bud.voice.cost").unwrap();
            assert!((cost - secs / 60.0 * 0.08).abs() < 1e-12, "{vendor}");
            assert_eq!(text(s, "bud.voice.rt.vendor").as_deref(), Some(vendor));
        }
        let billed: Vec<f64> = segments
            .iter()
            .map(|s| number(s, "bud.voice.billed_seconds").unwrap())
            .collect();
        let full = billed.iter().filter(|s| (**s - 1.0).abs() < 1e-9).count();
        assert!(full >= 2, "{vendor}: full segments while live: {billed:?}");
        let total: f64 = billed.iter().sum();
        assert!(total > 2.3 && total < 3.6, "{vendor}: {billed:?}");
        assert!((total - number(session, "bud.voice.billed_seconds").unwrap()).abs() < 1e-9);
        // The deployment's key reached the vendor.
        let (pq, headers) = mock.upgrades.lock().unwrap()[0].clone();
        let carried = pq.contains(VENDOR_KEY) || headers.values().any(|v| v.contains(VENDOR_KEY));
        assert!(carried, "{vendor}: the deployment's key is used");
        assert!(
            !headers.values().any(|v| v.contains("dg-process-canary")),
            "{vendor}: never the process key"
        );
    }
}
