//! Segmented speech-to-text over a real socket: a `/ws` session on a file-only model, a mock
//! vendor that only takes files, and the gateway's own detector cutting the caller's audio.
//!
//! What it proves, end to end through the real handler: the session reports how it is served
//! (`ready.stt`), each utterance is uploaded once as a 16 kHz WAV with the session's language, the
//! client gets the turn's text and exactly one end-of-turn result per turn, a client commit seals
//! the turn mid-speech, and a request the model cannot serve is refused with a coded error.
//!
//! The build has no Silero model here, so the sessions run on the loudness detector
//! (`WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1`), and the mock vendor listens on loopback
//! (`WAAV_ALLOW_LOOPBACK_ENDPOINTS=1`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Multipart, State};
use axum::middleware;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    middleware::auth::auth_middleware,
    routes,
    state::AppState,
};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One upload as the vendor saw it.
#[derive(Debug, Clone, Default)]
struct Upload {
    fields: Vec<(String, String)>,
    wav_bytes: usize,
    sample_rate: u32,
}

#[derive(Clone, Default)]
struct Vendor {
    uploads: Arc<Mutex<Vec<Upload>>>,
}

async fn transcribe(State(v): State<Vendor>, mut form: Multipart) -> axum::Json<Value> {
    let mut up = Upload::default();
    while let Ok(Some(field)) = form.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            let bytes = field.bytes().await.unwrap_or_default();
            up.wav_bytes = bytes.len();
            if bytes.len() >= 28 && &bytes[..4] == b"RIFF" {
                up.sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
            }
        } else {
            let text = field.text().await.unwrap_or_default();
            up.fields.push((name, text));
        }
    }
    let n = {
        let mut all = v.uploads.lock();
        all.push(up);
        all.len()
    };
    // A file model answers after the upload, like the real ones.
    tokio::time::sleep(Duration::from_millis(150)).await;
    axum::Json(json!({ "text": format!("utterance {n}") }))
}

async fn start_vendor() -> (SocketAddr, Vendor) {
    let vendor = Vendor::default();
    let app = axum::Router::new()
        .route("/v1/audio/transcriptions", axum::routing::post(transcribe))
        .with_state(vendor.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, vendor)
}

fn set_env() {
    // SAFETY: every test in this binary sets the same values before its gateway starts.
    unsafe {
        std::env::set_var("WAAV_SEGMENTED_STT", "on");
        std::env::set_var("WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR", "1");
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
}

async fn start_gateway() -> SocketAddr {
    set_env();
    let app_state = AppState::new(server_config()).await;
    let ws_routes = routes::ws::create_ws_router().layer(middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));
    let app = routes::api::create_api_router()
        .merge(ws_routes)
        .with_state(app_state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn server_config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        livekit_api_key: Some("test_key".to_string()),
        livekit_api_secret: Some("test_key".to_string()),
        port: 0, // Let the OS assign a port
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        deepgram_api_key: Some("test_key".to_string()),
        elevenlabs_api_key: Some("test_key".to_string()),
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

fn config(vendor: SocketAddr, mode: &str) -> Value {
    json!({
        "type": "config",
        "audio": true,
        "stt_config": {
            "provider": "openai",
            "model": "gpt-transcribe",
            "api_key": "test-key",
            "language": "en",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true,
            "encoding": "linear16",
            "transcription_mode": mode,
            "features": { "vad_events": true },
            "extras": { "base_url": format!("http://{vendor}") }
        },
        "tts_config": {
            "provider": "openai",
            "api_key": "test-key",
            "model": "tts-1",
            "voice_id": "alloy",
            "audio_format": "linear16",
            "sample_rate": 24000,
            "connection_timeout": 1,
            "request_timeout": 5
        }
    })
}

async fn connect(gateway: SocketAddr) -> Ws {
    let (ws, _) = connect_async(format!("ws://{gateway}/ws")).await.unwrap();
    ws
}

/// The next JSON message, skipping binary frames; `None` after `wait`.
async fn next_json(ws: &mut Ws, wait: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(left, ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(&t).ok(),
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

/// Messages until one matches, with everything seen before it.
async fn until(
    ws: &mut Ws,
    wait: Duration,
    want: impl Fn(&Value) -> bool,
) -> (Option<Value>, Vec<Value>) {
    let deadline = tokio::time::Instant::now() + wait;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match next_json(ws, left).await {
            Some(m) if want(&m) => return (Some(m), seen),
            Some(m) => seen.push(m),
            None => return (None, seen),
        }
    }
}

/// `ms` of 16 kHz PCM: a 440 Hz tone at a speaking level, or silence.
fn pcm(ms: u64, loud: bool) -> Vec<u8> {
    let n = (16 * ms) as usize;
    (0..n)
        .flat_map(|i| {
            let v = if loud {
                ((i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 8_000.0) as i16
            } else {
                0
            };
            v.to_le_bytes()
        })
        .collect()
}

/// Send `ms` of audio in 20 ms frames at twice real time.
async fn speak(ws: &mut Ws, ms: u64, loud: bool) {
    for _ in 0..ms / 20 {
        ws.send(Message::Binary(pcm(20, loud).into()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn ready_session(gateway: SocketAddr, vendor: SocketAddr) -> (Ws, Value) {
    let mut ws = connect(gateway).await;
    ws.send(Message::Text(
        config(vendor, "segmented").to_string().into(),
    ))
    .await
    .unwrap();
    let (ready, seen) = until(&mut ws, Duration::from_secs(10), |m| {
        m["type"] == "ready" || m["type"] == "error"
    })
    .await;
    let ready = ready.unwrap_or_else(|| panic!("no ready: {seen:?}"));
    assert_eq!(ready["type"], "ready", "{ready} after {seen:?}");
    (ws, ready)
}

fn finals(msgs: &[Value]) -> Vec<&Value> {
    msgs.iter()
        .filter(|m| m["type"] == "stt_result" && m["is_speech_final"] == true)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_segmented_session_yields_one_final_per_turn_from_file_uploads() {
    let (vendor_addr, vendor) = start_vendor().await;
    let gateway = start_gateway().await;
    let (mut ws, ready) = ready_session(gateway, vendor_addr).await;

    let stt = &ready["stt"];
    assert_eq!(stt["transcription_mode"], "segmented", "{ready}");
    assert_eq!(stt["requested_mode"], "segmented");
    assert_eq!(stt["requested_mode_source"], "request");
    assert_eq!(stt["endpointing"], "gateway");
    assert_eq!(stt["interim_results"], "per_segment");
    assert_eq!(stt["provider"], "openai");
    assert_eq!(stt["model"], "gpt-transcribe");

    // Two caller turns, each a utterance followed by a silence the gateway ends the turn on.
    let mut seen = Vec::new();
    for _ in 0..2 {
        speak(&mut ws, 300, false).await;
        speak(&mut ws, 1_200, true).await;
        speak(&mut ws, 2_400, false).await;
        let (fin, before) = until(&mut ws, Duration::from_secs(8), |m| {
            m["type"] == "stt_result" && m["is_speech_final"] == true
        })
        .await;
        seen.extend(before);
        seen.push(fin.unwrap_or_else(|| panic!("no final: {seen:#?}")));
    }

    let fins = finals(&seen);
    assert_eq!(fins.len(), 2, "one end-of-turn result per turn: {seen:#?}");
    assert_eq!(fins[0]["transcript"], "utterance 1");
    assert_eq!(fins[1]["transcript"], "utterance 2");
    assert!(fins.iter().all(|f| f["is_final"] == true));
    // Never a per-segment final that is not also the turn's end.
    assert!(
        !seen.iter().any(|m| m["type"] == "stt_result"
            && m["is_final"] == true
            && m["is_speech_final"] != true),
        "{seen:#?}"
    );
    // The detector's speech events, in order, for the first turn.
    let events: Vec<&str> = seen
        .iter()
        .filter(|m| m["type"] == "vad_event")
        .filter_map(|m| m["event"].as_str())
        .collect();
    let first = |e: &str| events.iter().position(|x| *x == e);
    assert!(first("speech_start").is_some(), "{events:?}");
    assert!(first("speech_start") < first("speech_end"), "{events:?}");
    assert!(
        first("turn_end").is_some() && first("turn_closed").is_some(),
        "{events:?}"
    );

    let uploads = vendor.uploads.lock().clone();
    assert_eq!(uploads.len(), 2, "one upload per utterance: {uploads:?}");
    for up in &uploads {
        let field = |k: &str| {
            up.fields
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(field("model"), Some("gpt-transcribe"));
        assert_eq!(field("languages[]"), Some("en"), "{up:?}");
        assert_eq!(
            field("language"),
            None,
            "never both language fields: {up:?}"
        );
        assert_eq!(up.sample_rate, 16_000);
        // About the utterance plus its padding, not the whole call.
        let seconds = (up.wav_bytes - 44) as f64 / 32_000.0;
        assert!((1.0..3.5).contains(&seconds), "{seconds} s uploaded");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_audio_end_seals_the_turn_mid_speech_and_its_final_follows() {
    let (vendor_addr, vendor) = start_vendor().await;
    let gateway = start_gateway().await;
    let (mut ws, _) = ready_session(gateway, vendor_addr).await;

    speak(&mut ws, 300, false).await;
    speak(&mut ws, 1_000, true).await;
    // The client commits while the caller is still speaking: no pause has ended the turn.
    ws.send(Message::Text(
        json!({"type": "audio_end"}).to_string().into(),
    ))
    .await
    .unwrap();
    let (fin, seen) = until(&mut ws, Duration::from_secs(8), |m| {
        m["type"] == "stt_result" && m["is_speech_final"] == true
    })
    .await;
    let fin = fin.unwrap_or_else(|| panic!("no final after audio_end: {seen:#?}"));
    assert_eq!(fin["transcript"], "utterance 1");
    assert_eq!(vendor.uploads.lock().len(), 1);

    // Nothing more for that turn.
    let (more, _) = until(&mut ws, Duration::from_millis(1_500), |m| {
        m["type"] == "stt_result"
    })
    .await;
    assert!(
        more.is_none(),
        "a second result for a sealed turn: {more:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_asking_to_stream_a_file_only_model_is_a_coded_refusal_and_the_socket_stays_usable() {
    let (vendor_addr, _vendor) = start_vendor().await;
    let gateway = start_gateway().await;
    let mut ws = connect(gateway).await;
    ws.send(Message::Text(
        config(vendor_addr, "streaming").to_string().into(),
    ))
    .await
    .unwrap();
    let (err, seen) = until(&mut ws, Duration::from_secs(10), |m| m["type"] == "error").await;
    let err = err.unwrap_or_else(|| panic!("no refusal: {seen:?}"));
    assert_eq!(err["code"], "stt_not_streaming", "{err}");
    assert_eq!(err["recoverable"], true);
    assert!(err["details"]["provider"] == "openai", "{err}");
    assert!(
        !seen.iter().any(|m| m["type"] == "ready"),
        "a refused session is not ready: {seen:?}"
    );

    // A corrected config on the same socket is served.
    ws.send(Message::Text(
        config(vendor_addr, "segmented").to_string().into(),
    ))
    .await
    .unwrap();
    let (ready, seen) = until(&mut ws, Duration::from_secs(10), |m| m["type"] == "ready").await;
    assert!(ready.is_some(), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ws_a_client_named_private_base_is_refused_without_the_escape_hatch() {
    // The same check, at plan time, with the hatch off: a private address the client named is
    // never dialled. The hatch is process-wide here, so the check is exercised directly.
    assert!(
        waav_segmented_stt::transcriber::http::check_untrusted_base("http://127.0.0.1:9/v1", false)
            .is_err()
    );
    assert!(
        waav_segmented_stt::transcriber::http::check_untrusted_base(
            "http://169.254.169.254/",
            false
        )
        .is_err()
    );
}
