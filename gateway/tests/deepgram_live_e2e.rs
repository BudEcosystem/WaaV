//! COMPREHENSIVE LIVE Deepgram e2e through WaaV's own providers + the full gateway.
//!
//! Covers Deepgram STT (streaming, multiple models + every feature WaaV exposes) and Deepgram TTS
//! (Aura voices), exercising the *actual* WaaV provider code and the full gateway WebSocket
//! pipeline against the real Deepgram API. All tests are `#[ignore]`d and require `DEEPGRAM_API_KEY`
//! in the environment — the key is never hardcoded.
//!
//! Run with:
//!   DEEPGRAM_API_KEY=… cargo test --test deepgram_live_e2e -- --ignored --nocapture --test-threads=1

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use waav_gateway::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
use waav_gateway::core::stt::{BaseSTT, DeepgramSTT, STTConfig, STTResult};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, DeepgramTTS, TTSConfig, TTSError};

const SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";
const SAMPLE_RATE: u32 = 24000;

fn api_key() -> String {
    std::env::var("DEEPGRAM_API_KEY").expect("DEEPGRAM_API_KEY must be set for live tests")
}

fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Synthesize clean linear16 PCM for STT input via a direct Deepgram `/v1/speak` call (Aura).
async fn synth_pcm(text: &str, sample_rate: u32) -> Vec<u8> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.deepgram.com/v1/speak?model=aura-asteria-en&encoding=linear16&container=none&sample_rate={sample_rate}"
    );
    let resp = client
        .post(url)
        .header("Authorization", format!("Token {}", api_key()))
        .json(&serde_json::json!({ "text": text }))
        .send()
        .await
        .expect("deepgram /v1/speak request");
    assert!(
        resp.status().is_success(),
        "deepgram speak failed: {}",
        resp.status()
    );
    resp.bytes().await.unwrap().to_vec()
}

/// Drive WaaV's `DeepgramSTT` against the real streaming API: stream `pcm`, return the best
/// (longest) final transcript and whether any provider error fired.
async fn transcribe_via_waav(
    std_cfg: StandardSTTConfig,
    pcm: &[u8],
    sample_rate: u32,
) -> (String, Option<String>) {
    ensure_crypto();
    let mut stt = DeepgramSTT::new_standard(&std_cfg).expect("create DeepgramSTT");

    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let best2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let best = best2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                let mut g = best.lock().await;
                if t.len() > g.len() {
                    *g = t;
                }
            }
        })
    }))
    .await
    .unwrap();

    let err = Arc::new(tokio::sync::Mutex::new(Option::<String>::None));
    let err2 = err.clone();
    stt.on_error(Arc::new(move |e| {
        let err = err2.clone();
        Box::pin(async move {
            *err.lock().await = Some(format!("{e:?}"));
        })
    }))
    .await
    .unwrap();

    stt.connect().await.expect("connect DeepgramSTT");

    // Stream in ~50 ms chunks at roughly real time so the streaming endpoint behaves naturally.
    // Don't unwrap: if Deepgram rejects a parameter it closes the stream, send fails, and we want
    // the captured error (not a panic) to surface the real cause.
    let bytes_per_50ms = (sample_rate as usize * 2 / 20).max(2);
    for chunk in pcm.chunks(bytes_per_50ms) {
        if stt.send_audio(Bytes::copy_from_slice(chunk)).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    // Trailing silence (1 s) to trigger endpointing/finalization.
    let _ = stt
        .send_audio(Bytes::from(vec![0u8; sample_rate as usize * 2]))
        .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    stt.disconnect().await.ok();

    let transcript = best.lock().await.clone();
    let error = err.lock().await.clone();
    (transcript, error)
}

fn base_stt(model: &str) -> STTConfig {
    STTConfig {
        provider: "deepgram".to_string(),
        api_key: api_key(),
        language: "en".to_string(),
        sample_rate: SAMPLE_RATE,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: model.to_string(),
    }
}

// =============================================================================
// A) Deepgram TTS (Aura) through WaaV's provider
// =============================================================================

#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram TTS calls"]
async fn test_deepgram_tts_aura_voices_live() {
    struct Cap(tokio::sync::mpsc::UnboundedSender<usize>);
    impl AudioCallback for Cap {
        fn on_audio(
            &self,
            a: AudioData,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            let n = a.data.len();
            let tx = self.0.clone();
            Box::pin(async move {
                let _ = tx.send(n);
            })
        }
        fn on_error(
            &self,
            _e: TTSError,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {})
        }
        fn on_complete(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {})
        }
    }

    // A representative spread of Aura voices (different genders/timbres).
    for voice in ["aura-asteria-en", "aura-luna-en", "aura-orion-en", "aura-arcas-en"] {
        let config = TTSConfig {
            provider: "deepgram".to_string(),
            api_key: api_key(),
            voice_id: Some(voice.to_string()),
            model: voice.to_string(),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(SAMPLE_RATE),
            ..Default::default()
        };
        let mut tts = DeepgramTTS::new(config).expect("create DeepgramTTS");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
        tts.on_audio(Arc::new(Cap(tx))).unwrap();
        tts.connect().await.expect("connect DeepgramTTS");
        tokio::time::timeout(Duration::from_secs(15), tts.speak(SENTENCE, true))
            .await
            .expect("speak timed out")
            .expect("speak failed");

        let mut total = 0usize;
        while let Ok(Some(n)) = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
            total += n;
            if total > 2000 {
                break;
            }
        }
        tts.disconnect().await.ok();
        println!("Aura {voice}: {total} bytes of real audio");
        assert!(total > 2000, "voice {voice} produced too little audio: {total} B");
    }
}

// =============================================================================
// B) Deepgram STT round-trip (Aura → WaaV DeepgramSTT) — multiple models
// =============================================================================

async fn roundtrip_model(model: &str) {
    let pcm = synth_pcm(SENTENCE, SAMPLE_RATE).await;
    assert!(pcm.len() > 8000, "synthesized audio too short: {} B", pcm.len());

    let std_cfg = StandardSTTConfig {
        base: base_stt(model),
        features: SttFeatures {
            interim_results: Some(true),
            smart_format: Some(true),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let (transcript, error) = transcribe_via_waav(std_cfg, &pcm, SAMPLE_RATE).await;
    println!("[{model}] transcript: {transcript:?}  error: {error:?}");
    assert!(error.is_none(), "[{model}] provider error: {error:?}");
    let lower = transcript.to_lowercase();
    assert!(
        lower.contains("fox") || lower.contains("dog") || lower.contains("quick"),
        "[{model}] expected recognizable words from '{SENTENCE}', got {transcript:?}"
    );
}

#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram STT+TTS calls"]
async fn test_deepgram_stt_roundtrip_nova2_live() {
    roundtrip_model("nova-2").await;
}

#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram STT+TTS calls"]
async fn test_deepgram_stt_roundtrip_nova3_live() {
    roundtrip_model("nova-3").await;
}

// =============================================================================
// C) Deepgram STT with EVERY feature WaaV exposes — Deepgram must accept it live
// =============================================================================

#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram STT+TTS calls"]
async fn test_deepgram_stt_all_features_live() {
    let pcm = synth_pcm("My name is John Smith and my number is 555 123 4567.", SAMPLE_RATE).await;

    // Turn on the full advertised feature surface at once. If Deepgram rejected any single
    // parameter it would close the stream with a 4xx → on_error fires and the transcript is empty.
    let std_cfg = StandardSTTConfig {
        base: base_stt("nova-3"),
        features: SttFeatures {
            interim_results: Some(true),
            diarization: Some(true),
            smart_format: Some(true),
            profanity_filter: Some(true),
            filler_words: Some(true),
            vad_events: Some(true),
            endpointing_ms: Some(300),
            utterance_end_ms: Some(1000), // exercises the re-enabled param (fix)
            keyterms: Some(vec!["John Smith".to_string()]),
            redaction: Some(vec!["pci".to_string(), "ssn".to_string()]),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let (transcript, error) = transcribe_via_waav(std_cfg, &pcm, SAMPLE_RATE).await;
    println!("[all-features] transcript: {transcript:?}  error: {error:?}");
    assert!(
        error.is_none(),
        "Deepgram rejected the full feature set (a param is wrong): {error:?}"
    );
    assert!(
        !transcript.is_empty(),
        "expected a transcript with all features enabled, got empty"
    );
}

// =============================================================================
// C2) NEW FEATURES (feature-vocabulary expansion): numerals + multichannel — Deepgram
//     must ACCEPT both on the live streaming endpoint and numerals must change the
//     transcript (spoken numbers → digits).
// =============================================================================

/// LIVE: `numerals=true` must be accepted by Deepgram's streaming endpoint AND must convert
/// spoken numbers to numeric digits in the transcript. We synthesize speech that says a number
/// out loud, transcribe it once WITHOUT numerals and once WITH numerals through WaaV's own
/// `DeepgramSTT` (the standardized `SttFeatures.numerals` → `from_standard` → wire path), and
/// assert the numerals-on transcript actually contains digit characters.
#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram STT+TTS calls"]
async fn test_deepgram_numerals_live() {
    // Aura clearly speaks digits as words; numerals=true should render them as digits.
    let spoken = "I have twenty three apples and forty seven oranges.";
    let pcm = synth_pcm(spoken, SAMPLE_RATE).await;
    assert!(pcm.len() > 8000, "synthesized audio too short: {} B", pcm.len());

    // Baseline: numerals OFF — Deepgram returns the number words (or at least no forced digits).
    let off_cfg = StandardSTTConfig {
        base: base_stt("nova-3"),
        features: SttFeatures {
            smart_format: Some(true),
            numerals: Some(false),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let (t_off, e_off) = transcribe_via_waav(off_cfg, &pcm, SAMPLE_RATE).await;
    println!("[numerals=off] transcript: {t_off:?}  error: {e_off:?}");
    assert!(e_off.is_none(), "[numerals=off] provider error: {e_off:?}");

    // Feature ON: numerals=true — Deepgram MUST accept the param (no error / non-empty transcript)
    // and MUST emit digits for the spoken numbers.
    let on_cfg = StandardSTTConfig {
        base: base_stt("nova-3"),
        features: SttFeatures {
            smart_format: Some(true),
            numerals: Some(true),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let (t_on, e_on) = transcribe_via_waav(on_cfg, &pcm, SAMPLE_RATE).await;
    println!("[numerals=on ] transcript: {t_on:?}  error: {e_on:?}");
    assert!(
        e_on.is_none(),
        "Deepgram rejected numerals=true on the streaming endpoint (a wire param is wrong): {e_on:?}"
    );
    assert!(
        !t_on.is_empty(),
        "expected a transcript with numerals=true, got empty (param likely rejected the stream)"
    );
    // The transcript must reflect numerals: digit characters present.
    assert!(
        t_on.chars().any(|c| c.is_ascii_digit()),
        "numerals=true should render spoken numbers as digits, got {t_on:?}"
    );
}

/// LIVE: `multichannel=true` must be accepted by Deepgram's streaming endpoint. Our synthesized
/// audio is single-channel, so we cannot assert per-channel separation, but we CAN assert the
/// param is accepted on the wire (no provider error / a non-empty transcript) — i.e. that
/// `SttFeatures.multichannel` → `from_standard` → `&multichannel=true` does not break the stream.
#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram STT+TTS calls"]
async fn test_deepgram_multichannel_live() {
    let pcm = synth_pcm(SENTENCE, SAMPLE_RATE).await;
    assert!(pcm.len() > 8000, "synthesized audio too short: {} B", pcm.len());

    let std_cfg = StandardSTTConfig {
        base: base_stt("nova-3"),
        features: SttFeatures {
            smart_format: Some(true),
            multichannel: Some(true),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let (transcript, error) = transcribe_via_waav(std_cfg, &pcm, SAMPLE_RATE).await;
    println!("[multichannel=on] transcript: {transcript:?}  error: {error:?}");
    assert!(
        error.is_none(),
        "Deepgram rejected multichannel=true on the streaming endpoint: {error:?}"
    );
    assert!(
        !transcript.is_empty(),
        "expected a transcript with multichannel=true, got empty (param likely rejected the stream)"
    );
    let lower = transcript.to_lowercase();
    assert!(
        lower.contains("fox") || lower.contains("dog") || lower.contains("quick"),
        "[multichannel] expected recognizable words, got {transcript:?}"
    );
}

/// LIVE: numerals + multichannel TOGETHER on one stream — both new params must be accepted at once
/// (this is the exact combined wire string `&numerals=true&multichannel=true`).
#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram STT+TTS calls"]
async fn test_deepgram_numerals_and_multichannel_combined_live() {
    let spoken = "Call me at five five five one two three four.";
    let pcm = synth_pcm(spoken, SAMPLE_RATE).await;

    let std_cfg = StandardSTTConfig {
        base: base_stt("nova-3"),
        features: SttFeatures {
            smart_format: Some(true),
            numerals: Some(true),
            multichannel: Some(true),
            ..Default::default()
        },
        extras: ProviderExtras::default(),
    };
    let (transcript, error) = transcribe_via_waav(std_cfg, &pcm, SAMPLE_RATE).await;
    println!("[numerals+multichannel] transcript: {transcript:?}  error: {error:?}");
    assert!(
        error.is_none(),
        "Deepgram rejected numerals+multichannel together: {error:?}"
    );
    assert!(
        !transcript.is_empty(),
        "expected a transcript with numerals+multichannel, got empty"
    );
    assert!(
        transcript.chars().any(|c| c.is_ascii_digit()),
        "numerals=true should render the spoken phone number as digits, got {transcript:?}"
    );
}

// =============================================================================
// D) FULL gateway, live: WS client → gateway → Deepgram STT (transcript) + TTS (audio)
// =============================================================================

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
        deepgram_api_key: Some(api_key()),
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

#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; boots the gateway + real billed Deepgram calls"]
async fn test_deepgram_gateway_ws_live() {
    use axum::middleware;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::time::timeout;
    use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
    use waav_gateway::{middleware::auth::auth_middleware, routes, state::AppState};

    // Pre-synthesize the audio we'll stream into the gateway (Aura → linear16 PCM).
    let pcm = synth_pcm(SENTENCE, SAMPLE_RATE).await;

    // Boot the real gateway (AppState installs the rustls CryptoProvider).
    let app_state = AppState::new(gateway_config()).await;
    let ws_routes = routes::ws::create_ws_router()
        .layer(middleware::from_fn_with_state(app_state.clone(), auth_middleware));
    let app = routes::api::create_api_router()
        .merge(ws_routes)
        .with_state(app_state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind gateway");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(150)).await;

    let url = format!("ws://127.0.0.1:{}/ws", addr.port());
    let (ws, _) = connect_async(url).await.expect("connect gateway WS");
    let (mut write, mut read) = ws.split();

    // Configure Deepgram for BOTH STT (nova-3) and TTS (Aura).
    let config_message = json!({
        "type": "config",
        "audio": true,
        "stt_config": {
            "provider": "deepgram",
            "api_key": api_key(),
            "language": "en",
            "sample_rate": SAMPLE_RATE,
            "channels": 1,
            "punctuation": true,
            "encoding": "linear16",
            "model": "nova-3"
        },
        "tts_config": {
            "provider": "deepgram",
            "api_key": api_key(),
            "voice_id": "aura-asteria-en",
            "model": "aura-asteria-en",
            "audio_format": "linear16",
            "sample_rate": SAMPLE_RATE
        }
    });
    write
        .send(Message::Text(config_message.to_string().into()))
        .await
        .unwrap();
    if let Ok(Some(Ok(Message::Text(t)))) = timeout(Duration::from_secs(10), read.next()).await {
        println!("gateway config ack: {t}");
    }

    // Stream the synthesized speech in as binary audio frames (real-time-ish pacing).
    let bytes_per_50ms = (SAMPLE_RATE as usize * 2 / 20).max(2);
    for chunk in pcm.chunks(bytes_per_50ms) {
        write
            .send(Message::Binary(Bytes::copy_from_slice(chunk)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    // Trailing silence to trigger finalization.
    write
        .send(Message::Binary(Bytes::from(vec![0u8; SAMPLE_RATE as usize * 2])))
        .await
        .unwrap();

    // Also exercise TTS through the gateway.
    write
        .send(Message::Text(
            json!({"type": "speak", "text": "Gateway speaking via Deepgram Aura.", "flush": true})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    // Collect: STT transcript (stt_result text) + TTS audio (binary frames).
    let mut transcript = String::new();
    let mut audio_bytes = 0usize;
    let collect = async {
        while let Some(Ok(msg)) = read.next().await {
            match msg {
                Message::Text(t) => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
                        && v.get("type").and_then(|x| x.as_str()) == Some("stt_result")
                            && let Some(tr) = v.get("transcript").and_then(|x| x.as_str())
                                && tr.len() > transcript.len() {
                                    transcript = tr.to_string();
                                }
                }
                Message::Binary(b) => audio_bytes += b.len(),
                Message::Close(_) => break,
                _ => {}
            }
            if !transcript.is_empty() && audio_bytes > 2000 {
                break;
            }
        }
    };
    let _ = timeout(Duration::from_secs(15), collect).await;
    let _ = write.close().await;

    println!("gateway STT transcript: {transcript:?}");
    println!("gateway TTS audio bytes: {audio_bytes}");
    let lower = transcript.to_lowercase();
    assert!(
        lower.contains("fox") || lower.contains("dog") || lower.contains("quick"),
        "gateway STT should return a real Deepgram transcript, got {transcript:?}"
    );
    assert!(
        audio_bytes > 2000,
        "gateway should stream real Deepgram Aura audio to the client, got {audio_bytes} B"
    );
}

/// C-G5 pt3 live gate: real Deepgram Aura chunks (their real format/rate
/// stamps) through the STANDARDIZED egress seam — exactly what the session
/// handler does when a client configures `client_playback_rate`.
#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY; real billed Deepgram TTS call"]
async fn test_egress_client_playback_rate_live() {
    use waav_gateway::core::audio::{StreamResampler, egress_to_client_rate};

    struct Cap(tokio::sync::mpsc::UnboundedSender<AudioData>);
    impl AudioCallback for Cap {
        fn on_audio(
            &self,
            a: AudioData,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            let tx = self.0.clone();
            Box::pin(async move {
                let _ = tx.send(a);
            })
        }
        fn on_error(
            &self,
            e: TTSError,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move { eprintln!("[egress] live TTS error: {e:?}") })
        }
        fn on_complete(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            Box::pin(async move {})
        }
    }

    let config = TTSConfig {
        provider: "deepgram".to_string(),
        api_key: api_key(),
        voice_id: Some("aura-asteria-en".to_string()),
        model: "aura-asteria-en".to_string(),
        audio_format: Some("linear16".to_string()),
        sample_rate: Some(SAMPLE_RATE),
        ..Default::default()
    };
    let mut tts = DeepgramTTS::new(config).expect("create DeepgramTTS");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AudioData>();
    tts.on_audio(Arc::new(Cap(tx))).unwrap();
    tts.connect().await.expect("connect DeepgramTTS");
    tokio::time::timeout(Duration::from_secs(15), tts.speak(SENTENCE, true))
        .await
        .expect("speak timed out")
        .expect("speak failed");

    // Stream every REAL chunk through the egress seam at 2x the provider
    // rate — the same per-chunk call the session handler makes.
    let target = SAMPLE_RATE * 2;
    let mut resampler = StreamResampler::new();
    let mut in_bytes = 0usize;
    let mut out_bytes = 0usize;
    let mut chunks = 0usize;
    while let Ok(Some(a)) =
        tokio::time::timeout(Duration::from_secs(10), rx.recv()).await
    {
        in_bytes += a.data.len();
        chunks += 1;
        let converted = egress_to_client_rate(
            &mut resampler,
            &a.data,
            &a.format,
            a.sample_rate,
            SAMPLE_RATE,
            target,
        )
        .expect("PCM at half the client rate must be converted");
        out_bytes += converted.len();
        if in_bytes > 48_000 {
            break;
        }
    }
    tts.disconnect().await.ok();

    println!(
        "[egress] {chunks} live chunks: {in_bytes} B @{SAMPLE_RATE} -> {out_bytes} B @{target}"
    );
    assert!(chunks >= 2, "expected a streamed (multi-chunk) synthesis, got {chunks}");
    assert!(in_bytes > 8_000, "synthesis too short: {in_bytes} B");
    // 2x rate => ~2x bytes; the resampler may still hold < one chunk (1024
    // frames = 4096 output bytes) of tail.
    let expected = in_bytes * 2;
    assert!(
        out_bytes + 8_192 >= expected && out_bytes <= expected + 8_192,
        "expected ~{expected} B at the client rate, got {out_bytes} B"
    );

    // Identity contract live: a client at the provider rate costs nothing.
    let mut id = StreamResampler::new();
    assert!(
        egress_to_client_rate(&mut id, &[0u8; 4096], "linear16", SAMPLE_RATE, SAMPLE_RATE, SAMPLE_RATE)
            .is_none(),
        "identity must be zero-work"
    );
}
