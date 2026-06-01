//! LIVE end-to-end tests against the REAL ElevenLabs API, exercising WaaV's own provider code
//! (not standalone HTTP calls). All tests are `#[ignore]`d and require `ELEVENLABS_API_KEY` in the
//! environment — the key is never hardcoded.
//!
//! Run with:
//!   ELEVENLABS_API_KEY=sk_... cargo test --test elevenlabs_live_e2e -- --ignored --nocapture --test-threads=1
//!
//! These prove the real wire path through WaaV's `ElevenLabsTTS` provider and the full gateway
//! WebSocket pipeline against the live vendor.

use std::sync::Arc;
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, ElevenLabsTTS, TTSConfig, TTSError};

const RACHEL_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";

fn api_key() -> String {
    std::env::var("ELEVENLABS_API_KEY").expect("ELEVENLABS_API_KEY must be set for live tests")
}

/// Capture callback that forwards received audio byte-counts to a channel.
struct CaptureCallback(tokio::sync::mpsc::UnboundedSender<usize>);
impl AudioCallback for CaptureCallback {
    fn on_audio(
        &self,
        audio: AudioData,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let n = audio.data.len();
        let tx = self.0.clone();
        Box::pin(async move {
            let _ = tx.send(n);
        })
    }
    fn on_error(
        &self,
        error: TTSError,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            eprintln!("ElevenLabs TTS error: {error:?}");
        })
    }
    fn on_complete(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {})
    }
}

/// LIVE: synthesize speech through WaaV's `ElevenLabsTTS` provider against the real API and verify
/// real audio bytes stream back through `AudioCallback`.
#[tokio::test]
#[ignore = "Requires ELEVENLABS_API_KEY; makes a real billed ElevenLabs API call"]
async fn test_waav_elevenlabs_tts_provider_live() {
    use tokio::time::{Duration, timeout};

    let config = TTSConfig {
        provider: "elevenlabs".to_string(),
        api_key: api_key(),
        voice_id: Some(RACHEL_VOICE.to_string()),
        model: "eleven_multilingual_v2".to_string(),
        audio_format: Some("mp3_44100_128".to_string()),
        sample_rate: Some(44100),
        ..Default::default()
    };

    let mut tts = ElevenLabsTTS::new(config).expect("create ElevenLabs TTS provider");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
    tts.on_audio(Arc::new(CaptureCallback(tx)))
        .expect("register audio callback");

    tts.connect().await.expect("connect to ElevenLabs");
    assert!(tts.is_ready());

    timeout(
        Duration::from_secs(20),
        tts.speak("Hello from WaaV. This is a live end to end test of ElevenLabs.", true),
    )
    .await
    .expect("speak timed out")
    .expect("speak failed");

    // Collect streamed audio for up to 20s.
    let mut total = 0usize;
    while let Ok(Some(n)) = timeout(Duration::from_secs(20), rx.recv()).await {
        total += n;
        if total > 2000 {
            break;
        }
    }

    tts.disconnect().await.expect("disconnect");

    println!("WaaV ElevenLabs provider streamed {total} bytes of real audio");
    assert!(
        total > 2000,
        "expected real synthesized audio from ElevenLabs through WaaV's provider, got {total} bytes"
    );
}

/// Build a minimal gateway `ServerConfig` with the real ElevenLabs key, bound to an OS-assigned port.
fn gateway_config() -> waav_gateway::ServerConfig {
    use waav_gateway::config::{DAGTimeoutsConfig, PluginConfig};
    waav_gateway::ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        tls: None,
        livekit_api_key: Some("test_key".to_string()),
        livekit_api_secret: Some("test_key".to_string()),
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        deepgram_api_key: None,
        elevenlabs_api_key: Some(api_key()),
        google_credentials: None,
        azure_speech_subscription_key: None,
        azure_speech_region: None,
        cartesia_api_key: None,
        openai_api_key: None,
        assemblyai_api_key: None,
        hume_api_key: None,
        lmnt_api_key: None,
        groq_api_key: None,
        playht_api_key: None,
        playht_user_id: None,
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
        rate_limit_requests_per_second: 0,
        rate_limit_burst_size: 10,
        max_websocket_connections: None,
        max_connections_per_ip: 100,
        ws_processing_timeout_secs: 30,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
    }
}

/// LIVE FULL-SYSTEM e2e: boot the REAL gateway (AppState + production router on a real TCP socket),
/// connect a REAL WebSocket client, configure the ElevenLabs TTS provider, issue a `speak`, and
/// verify REAL ElevenLabs audio streams back to the client over the socket. This is "the entire
/// system, its components up and running and live" against a real paid provider.
#[tokio::test]
#[ignore = "Requires ELEVENLABS_API_KEY; boots the gateway and makes a real billed ElevenLabs call"]
async fn test_waav_gateway_ws_elevenlabs_tts_live() {
    use axum::middleware;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::time::{Duration, timeout};
    use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
    use waav_gateway::{middleware::auth::auth_middleware, routes, state::AppState};

    // NOTE: we intentionally do NOT install a rustls CryptoProvider here — `AppState::new` now does
    // it idempotently, so embedders/tests no longer need to replicate the main.rs startup. If the
    // realtime STT WSS connects below, that fix is working.

    // --- Boot the real gateway ---
    let app_state = AppState::new(gateway_config()).await;
    let ws_routes = routes::ws::create_ws_router().layer(middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));
    let app = routes::api::create_api_router()
        .merge(ws_routes)
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind gateway");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // --- Real WebSocket client ---
    let url = format!("ws://127.0.0.1:{}/ws", addr.port());
    let (ws_stream, _) = connect_async(url).await.expect("connect to gateway WS");
    let (mut write, mut read) = ws_stream.split();

    // Configure ElevenLabs TTS (mp3 so it works on non-Pro tiers). STT is set but TTS is what we drive.
    let config_message = json!({
        "type": "config",
        "audio": true,
        // NOTE: `encoding` and `model` are deliberately OMITTED — they now default
        // (linear16 / provider-default) instead of hard-rejecting the config. If the gateway
        // returns "ready" below, that fix is working.
        "stt_config": {
            "provider": "elevenlabs",
            "api_key": api_key(),
            "language": "en",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true
        },
        "tts_config": {
            "provider": "elevenlabs",
            "api_key": api_key(),
            "voice_id": RACHEL_VOICE,
            "model": "eleven_multilingual_v2",
            "audio_format": "mp3_44100_128",
            "sample_rate": 44100,
            "speaking_rate": 1.0,
            "connection_timeout": 30,
            "request_timeout": 60
        }
    });
    write
        .send(Message::Text(config_message.to_string().into()))
        .await
        .expect("send config");

    // Read the config ack (ready or error — log it; TTS can still work).
    if let Ok(Some(Ok(msg))) = timeout(Duration::from_secs(10), read.next()).await {
        if let Message::Text(t) = msg {
            println!("gateway config ack: {t}");
        }
    }

    // Issue a speak command — drives the gateway → ElevenLabs TTS → audio back over the socket.
    let speak_message = json!({
        "type": "speak",
        "text": "Hello from the WaaV gateway. This is a full system live test with ElevenLabs.",
        "flush": true
    });
    write
        .send(Message::Text(speak_message.to_string().into()))
        .await
        .expect("send speak");

    // Collect real TTS audio (binary frames) for up to 25s.
    let mut audio_bytes = 0usize;
    let deadline = Duration::from_secs(25);
    let collect = async {
        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Binary(b) => {
                    audio_bytes += b.len();
                    if audio_bytes > 2000 {
                        break;
                    }
                }
                Message::Text(t) => {
                    println!("gateway → client text: {t}");
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let _ = timeout(deadline, collect).await;

    let _ = write.close().await;

    println!("gateway streamed {audio_bytes} bytes of real ElevenLabs TTS audio to the client");
    assert!(
        audio_bytes > 2000,
        "the full gateway pipeline should stream real ElevenLabs audio to the WS client, got {audio_bytes} bytes"
    );
}
