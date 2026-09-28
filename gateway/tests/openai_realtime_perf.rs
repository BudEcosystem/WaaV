//! FRD-023 TC-PERF-01 — the relay's added latency per audio frame, in process and exact.
//!
//! One client streams 24 kHz PCM16 audio as `input_audio_buffer.append` events at 20 ms frames
//! (960 bytes → 1280 base64 characters, real-time pacing). An echo vendor answers every append
//! at once with a `response.output_audio.delta` carrying the same audio, so both directions carry
//! a frame every 20 ms, as in a live conversation. The client times each frame from send to echo:
//!
//! * **direct** — client ↔ vendor;
//! * **relay** — client ↔ WaaV `/v1/realtime` (Bud mode, in-memory control plane) ↔ vendor.
//!
//! Direct and relay phases alternate (`PERF_ROUNDS`) so drift on a shared host hits both. The added
//! latency is the relay's percentile minus the direct path's at the same percentile; the FRD budget
//! is p99 < 5 ms (R-2). Every frame must come back (FR-EVT-4: audio is never dropped).
//!
//! Serving is `main.rs`'s (`axum::serve` on `waav_gateway::server::nodelay_listener`,
//! `connection_limit_middleware` in front) and the production `Timings` (ping 20 s, revalidate 30 s), with the process-global
//! Prometheus recorder installed as `AppState::new` does in production.
//!
//! Run explicitly (about two minutes):
//!
//! ```text
//! cargo test --release --no-default-features --features dag-routing,turn-ensemble,noise-filter,openapi \
//!     --test openai_realtime_perf -- --ignored --nocapture
//! ```
//!
//! Knobs: `PERF_FRAMES` (measured frames per path, default 3000 = 60 s of audio), `PERF_WARMUP`
//! (discarded frames per phase, default 50), `PERF_ROUNDS` (default 3), `PERF_BACKGROUND` (extra
//! sessions streaming alongside the measured one on the same path, default 0), `PERF_BUDGET_MS`
//! (default 5). Where the shell environment does not reach the test binary (a builder container),
//! cargo's `--config 'env.PERF_FRAMES="600"'` sets them.
//!
//! First run (2026-09-28, before `server::nodelay_listener`): added p50 19.5 / p99 20.4 ms — Nagle on
//! the accepted client socket held each relayed frame for one frame interval.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value as Json, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use waav_gateway::config::{DAGTimeoutsConfig, PluginConfig, ServerConfig};
use waav_gateway::handlers::openai_realtime::{RealtimeRuntime, Timings};

// =============================================================================================
// Fixtures (as `openai_realtime_relay.rs`)
// =============================================================================================

const KEY: &str = "bud_realtime_perf_test_key";
const PROJECT: &str = "5b0c7e1d-0000-4000-8000-00000000ef01";
const USER: &str = "5b0c7e1d-0000-4000-8000-00000000ef02";
const API_KEY_ID: &str = "5b0c7e1d-0000-4000-8000-00000000ef03";
const MODEL_ID: &str = "5b0c7e1d-0000-4000-8000-00000000ef04";
const ENDPOINT_ID: &str = "f0f0f0f0-0000-4000-8000-00000000ef05";
const ALIAS: &str = "rt-perf";
const VENDOR_MODEL: &str = "gpt-realtime-2.1";

/// bud-auth's fixture ciphertext; the plaintext is its `PLAIN`.
const TEST_CREDENTIAL: &str = include_str!("../../bud-auth/tests/fixtures/test_cred_encrypted.hex");

/// 24 kHz mono PCM16 at 20 ms: 480 samples.
const SAMPLES_PER_FRAME: usize = 24_000 / 50;
const FRAME_INTERVAL: Duration = Duration::from_millis(20);

fn test_pem() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../bud-auth/tests/fixtures/test_cred_private.pem"
    ))
    .expect("bud-auth's fixture key (git-ignored *.pem) must be present locally")
}

/// The loopback escape hatch, so the vendor on 127.0.0.1 passes SSRF validation.
fn allow_loopback() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") });
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// One 20 ms frame of a 300 Hz tone, base64 (the payload size a real client sends).
fn audio_frame_b64() -> String {
    let mut pcm = Vec::with_capacity(SAMPLES_PER_FRAME * 2);
    for i in 0..SAMPLES_PER_FRAME {
        let s = (6000.0 * (2.0 * std::f64::consts::PI * 300.0 * i as f64 / 24_000.0).sin()) as i16;
        pcm.extend_from_slice(&s.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(pcm)
}

// =============================================================================================
// The echo vendor (OpenAI Realtime GA)
// =============================================================================================

/// `session.created` on connect, `session.update` → `session.updated`, and every
/// `input_audio_buffer.append` echoed at once as a `response.output_audio.delta` with the same
/// audio and `event_id` `echo_<seq>`.
async fn start_echo_vendor() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            // A vendor's edge answers small frames without Nagle delay.
            let _ = stream.set_nodelay(true);
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                let created = json!({"type": "session.created", "event_id": "evt_v0",
                    "session": {"id": "sess_vendor_perf", "object": "realtime.session", "type": "realtime", "model": VENDOR_MODEL}});
                if ws
                    .send(Message::Text(created.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
                while let Some(Ok(msg)) = ws.next().await {
                    let Message::Text(t) = msg else { continue };
                    let v: Json = serde_json::from_str(t.as_str()).unwrap_or(Json::Null);
                    let out = match v["type"].as_str().unwrap_or_default() {
                        "session.update" => json!({"type": "session.updated", "event_id": "evt_vu",
                            "session": {"id": "sess_vendor_perf", "model": VENDOR_MODEL, "type": v["session"]["type"]}}),
                        "input_audio_buffer.append" => {
                            let seq = v["event_id"]
                                .as_str()
                                .and_then(|e| e.strip_prefix("perf_"))
                                .unwrap_or("x");
                            json!({"type": "response.output_audio.delta", "event_id": format!("echo_{seq}"),
                                "response_id": "resp_perf", "item_id": "item_perf", "output_index": 0,
                                "content_index": 0, "delta": v["audio"]})
                        }
                        _ => continue,
                    };
                    if ws
                        .send(Message::Text(out.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    addr
}

// =============================================================================================
// The gateway (Bud mode over an in-memory control plane, served as main.rs serves it)
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

/// A realtime deployment on the vendor, configured as the live harness configures one.
fn rt_entry(vendor: SocketAddr) -> Json {
    json!({
        "vendor": "openai",
        "api_base": format!("http://{vendor}/v1"),
        "credential": TEST_CREDENTIAL.trim(),
        "endpoints": ["realtime_session"],
        "model": VENDOR_MODEL,
        "pricing": {"unit": "token", "per_units": 1000000, "currency": "USD",
            "rates": {"input_text": 4.0, "input_audio": 32.0, "input_image": 5.0,
                      "cached_input_text": 0.4, "cached_input_audio": 0.4, "cached_input_image": 0.5,
                      "output_text": 24.0, "output_audio": 64.0, "transcription_per_minute": 0.003}},
        "config": {"realtime": {
            "defaults": {"voice": "marin", "instructions": "You are the TC-PERF-01 agent.",
                         "turn_detection": {"type": "server_vad"}},
            "limits": {"max_session_seconds": 3600, "idle_timeout_seconds": 300}
        }}
    })
}

async fn start_gateway(vendor: SocketAddr) -> SocketAddr {
    allow_loopback();
    let store = Arc::new(bud_auth::MemoryStore::new());
    store.set(
        &format!("api_key:{}", bud_auth::hash_api_key(KEY)),
        &json!({
            ALIAS: {"endpoint_id": ENDPOINT_ID, "model_id": MODEL_ID, "project_id": PROJECT, "kind": "model"},
            "__metadata__": {"api_key_id": API_KEY_ID, "user_id": USER, "api_key_project_id": PROJECT}
        })
        .to_string(),
    );
    store.set(
        &format!("voice_table:{ENDPOINT_ID}"),
        &json!({ ENDPOINT_ID: rt_entry(vendor) }).to_string(),
    );
    let plane = Arc::new(bud_auth::BudPlane::with_decryptor(
        store.clone() as Arc<dyn bud_auth::ControlPlaneStore>,
        None,
        bud_auth::CredentialDecryptor::from_pem(&test_pem()).unwrap(),
    ));
    plane.boot().await.unwrap();

    let mut state = waav_gateway::state::AppState::new(config()).await;
    {
        let s = Arc::get_mut(&mut state).expect("unshared");
        s.bud_mode = Some(waav_gateway::auth::bud_mode::BudMode::for_plane(plane.clone()).unwrap());
        s.policies = Some(waav_gateway::core::deployment_policy::DeploymentPolicies::local());
        s.realtime = Arc::new(RealtimeRuntime {
            timings: Timings::default(),
            client_secret_keys: None,
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
    let service = app.into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        // Served exactly as main.rs serves it (the production listener, TCP_NODELAY on accept).
        axum::serve(waav_gateway::server::nodelay_listener(listener), service)
            .await
            .unwrap();
        drop(plane);
    });
    addr
}

// =============================================================================================
// The client
// =============================================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Path {
    Direct,
    Relay,
}

struct Phase {
    rtts: Vec<Duration>,
    sent: usize,
    received: usize,
}

/// One session: connect, configure, stream `warmup + frames` appends at 20 ms, time each echo.
async fn stream_session(
    path: Path,
    vendor: SocketAddr,
    gateway: SocketAddr,
    warmup: usize,
    frames: usize,
    audio: Arc<String>,
) -> Phase {
    let url = match path {
        Path::Direct => format!("ws://{vendor}/v1/realtime?model={VENDOR_MODEL}"),
        Path::Relay => format!("ws://{gateway}/v1/realtime?model={ALIAS}"),
    };
    let mut req = url.into_client_request().unwrap();
    req.headers_mut()
        .insert("authorization", format!("Bearer {KEY}").parse().unwrap());
    // Browsers and asyncio clients set TCP_NODELAY on their sockets.
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async_with_config(req, None, true),
    )
    .await
    .expect("handshake within 15 s")
    .unwrap_or_else(|e| panic!("{path:?} connect failed: {e}"));
    let (mut sink, mut stream) = ws.split();

    // session.created, then the client's own session.update and its answer.
    let mut seen_created = false;
    while !seen_created {
        match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Json = serde_json::from_str(t.as_str()).unwrap();
                seen_created = v["type"] == "session.created";
            }
            Ok(Some(Ok(_))) => {}
            other => panic!("{path:?}: no session.created: {other:?}"),
        }
    }
    let update = json!({"type": "session.update", "event_id": "perf_cfg",
        "session": {"type": "realtime", "audio": {"input": {"turn_detection": null}}}});
    sink.send(Message::Text(update.to_string().into()))
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Json = serde_json::from_str(t.as_str()).unwrap();
                if v["type"] == "session.updated" {
                    break;
                }
                assert_ne!(v["type"], "error", "{path:?}: {v}");
            }
            Ok(Some(Ok(_))) => {}
            other => panic!("{path:?}: no session.updated: {other:?}"),
        }
    }

    let total = warmup + frames;
    let sent_at: Arc<Mutex<Vec<Option<Instant>>>> = Arc::new(Mutex::new(vec![None; total]));
    let sender = {
        let sent_at = sent_at.clone();
        let audio = audio.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(FRAME_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            for seq in 0..total {
                tick.tick().await;
                let frame = format!(
                    r#"{{"type":"input_audio_buffer.append","event_id":"perf_{seq}","audio":"{audio}"}}"#
                );
                sent_at.lock().unwrap()[seq] = Some(Instant::now());
                if sink.send(Message::Text(frame.into())).await.is_err() {
                    return (sink, seq);
                }
            }
            (sink, total)
        })
    };

    let mut rtts = Vec::with_capacity(frames);
    let mut received = 0usize;
    let deadline =
        tokio::time::Instant::now() + FRAME_INTERVAL * total as u32 + Duration::from_secs(10);
    while received < total {
        match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let now = Instant::now();
                let v: Json = serde_json::from_str(t.as_str()).unwrap();
                if v["type"] != "response.output_audio.delta" {
                    assert_ne!(v["type"], "error", "{path:?}: {v}");
                    continue;
                }
                let Some(seq) = v["event_id"]
                    .as_str()
                    .and_then(|e| e.strip_prefix("echo_"))
                    .and_then(|s| s.parse::<usize>().ok())
                else {
                    continue;
                };
                received += 1;
                let sent = sent_at.lock().unwrap()[seq].expect("echo before send");
                if seq >= warmup {
                    rtts.push(now - sent);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("{path:?}: read error after {received} frames: {e}"),
            Ok(None) => panic!("{path:?}: closed after {received} frames"),
            Err(_) => break,
        }
    }
    let (mut sink, sent) = sender.await.unwrap();
    let _ = sink.send(Message::Close(None)).await;
    let _ = sink.close().await;
    Phase {
        rtts,
        sent,
        received,
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

const PCTS: [f64; 5] = [50.0, 90.0, 99.0, 99.9, 100.0];

fn row(label: &str, sorted: &[Duration]) -> String {
    let mean = sorted.iter().map(|d| d.as_secs_f64()).sum::<f64>() / sorted.len().max(1) as f64;
    let cells: Vec<String> = PCTS
        .iter()
        .map(|p| format!("{:>8.3}", ms(percentile(sorted, *p))))
        .collect();
    format!("{label:<8}{}{:>9.3}", cells.join(""), mean * 1e3)
}

// =============================================================================================
// TC-PERF-01
// =============================================================================================

/// TC-PERF-01 — added latency per 20 ms frame of 24 kHz audio, client → vendor → client, relay
/// vs. a direct connection to the same vendor: p99 added < 5 ms, no frame lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "performance: run explicitly with --ignored --nocapture (about two minutes)"]
async fn tc_perf_01_relay_added_latency_per_frame() {
    let frames = env_usize("PERF_FRAMES", 3000);
    let warmup = env_usize("PERF_WARMUP", 50);
    let rounds = env_usize("PERF_ROUNDS", 3).max(1);
    let background = env_usize("PERF_BACKGROUND", 0);
    let budget_ms = env_usize("PERF_BUDGET_MS", 5) as f64;
    let per_round = frames.div_ceil(rounds);

    let vendor = start_echo_vendor().await;
    let gateway = start_gateway(vendor).await;
    let audio = Arc::new(audio_frame_b64());
    assert_eq!(
        audio.len(),
        1280,
        "20 ms of 24 kHz PCM16 is 1280 base64 characters"
    );

    let mut all: [(Path, Vec<Duration>, usize, usize); 2] = [
        (Path::Direct, Vec::new(), 0, 0),
        (Path::Relay, Vec::new(), 0, 0),
    ];
    for round in 0..rounds {
        for slot in all.iter_mut() {
            let path = slot.0;
            // Background sessions stream on the same path for the whole phase (plus slack).
            let bg: Vec<_> = (0..background)
                .map(|_| {
                    tokio::spawn(stream_session(
                        path,
                        vendor,
                        gateway,
                        0,
                        per_round + warmup + 100,
                        audio.clone(),
                    ))
                })
                .collect();
            let phase =
                stream_session(path, vendor, gateway, warmup, per_round, audio.clone()).await;
            for h in bg {
                let b = h.await.unwrap();
                assert_eq!(
                    b.received, b.sent,
                    "{path:?} background session lost frames"
                );
            }
            eprintln!(
                "round {}/{rounds} {path:?}: {} frames, p50 {:.3} ms, p99 {:.3} ms",
                round + 1,
                phase.received,
                ms(percentile(&sorted(&phase.rtts), 50.0)),
                ms(percentile(&sorted(&phase.rtts), 99.0)),
            );
            slot.1.extend(phase.rtts);
            slot.2 += phase.sent;
            slot.3 += phase.received;
        }
    }

    let direct = sorted(&all[0].1);
    let relay = sorted(&all[1].1);
    let header = PCTS
        .iter()
        .map(|p| {
            if *p == 100.0 {
                format!("{:>8}", "max")
            } else {
                format!("{:>8}", format!("p{p}"))
            }
        })
        .collect::<String>();
    let added: Vec<String> = PCTS
        .iter()
        .map(|p| {
            format!(
                "{:>8.3}",
                ms(percentile(&relay, *p)) - ms(percentile(&direct, *p))
            )
        })
        .collect();
    let added_p50 = ms(percentile(&relay, 50.0)) - ms(percentile(&direct, 50.0));
    let added_p99 = ms(percentile(&relay, 99.0)) - ms(percentile(&direct, 99.0));
    eprintln!(
        "\nTC-PERF-01  round trip per 20 ms frame (24 kHz PCM16, 1280 B base64), {} measured frames per path \
         ({rounds} rounds, {warmup} warm-up frames per phase, {background} background sessions), {} build, \
         gateway accept as main.rs (TCP_NODELAY on accept)\n\
         ms      {header}     mean\n{}\n{}\nadded   {}\n\
         => added latency p50 {added_p50:.3} ms, p99 {added_p99:.3} ms (budget p99 < {budget_ms} ms)\n",
        relay.len(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        row("direct", &direct),
        row("relay", &relay),
        added.join(""),
    );

    for (path, rtts, sent, received) in &all {
        assert_eq!(received, sent, "{path:?}: frames lost");
        assert_eq!(rtts.len(), per_round * rounds, "{path:?}: measured frames");
    }
    assert!(
        added_p99 < budget_ms,
        "TC-PERF-01: relay p99 adds {added_p99:.3} ms over the direct path (budget {budget_ms} ms)"
    );
}

fn sorted(v: &[Duration]) -> Vec<Duration> {
    let mut s = v.to_vec();
    s.sort_unstable();
    s
}
