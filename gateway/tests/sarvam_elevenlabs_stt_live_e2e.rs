//! LIVE STT e2e for two providers I have keys for but had only exercised on other surfaces:
//! **Sarvam STT** (Saarika, `wss://api.sarvam.ai/speech-to-text/ws`) and **ElevenLabs STT**
//! (Scribe realtime). Drives WaaV's real provider clients through the standardized keystone
//! (`create_stt_standard`) against the live streaming APIs: stream English PCM, collect the
//! transcript. English speech is synthesized via Deepgram Aura so the test is self-contained.
//!
//! `#[ignore]` + key-gated. Run:
//!   SARVAM_API_KEY=… ELEVENLABS_API_KEY=… DEEPGRAM_API_KEY=… \
//!     cargo test --features dag-routing --test sarvam_elevenlabs_stt_live_e2e -- --ignored --nocapture --test-threads=1

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use waav_gateway::core::stt::standard::{StandardSTTConfig, create_stt_standard};
use waav_gateway::core::stt::{BaseSTT, STTConfig, STTResult};

const SAMPLE_RATE: u32 = 16000;
const SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";

fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn key(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("SKIP: {var} not set");
            None
        }
    }
}

/// Synthesize clean linear16 English PCM via a direct Deepgram Aura call (self-contained STT input).
async fn synth_pcm(text: &str, deepgram_key: &str) -> Vec<u8> {
    let url = format!(
        "https://api.deepgram.com/v1/speak?model=aura-asteria-en&encoding=linear16&container=none&sample_rate={SAMPLE_RATE}"
    );
    let resp = reqwest::Client::new()
        .post(url)
        .header("Authorization", format!("Token {deepgram_key}"))
        .json(&serde_json::json!({ "text": text }))
        .send()
        .await
        .expect("deepgram /v1/speak");
    assert!(
        resp.status().is_success(),
        "deepgram speak: {}",
        resp.status()
    );
    resp.bytes().await.unwrap().to_vec()
}

/// Drive any `BaseSTT` streaming provider: stream `pcm` ~real-time, return the best (longest) final
/// transcript and any provider error that fired.
async fn transcribe(stt: &mut dyn BaseSTT, pcm: &[u8]) -> (String, Option<String>) {
    let best = Arc::new(tokio::sync::Mutex::new(String::new()));
    let b2 = best.clone();
    stt.on_result(Arc::new(move |r: STTResult| {
        let b = b2.clone();
        Box::pin(async move {
            let t = r.transcript.trim().to_string();
            if !t.is_empty() {
                let mut g = b.lock().await;
                if t.len() > g.len() {
                    *g = t;
                }
            }
        })
    }))
    .await
    .unwrap();

    let err = Arc::new(tokio::sync::Mutex::new(Option::<String>::None));
    let e2 = err.clone();
    stt.on_error(Arc::new(move |e| {
        let err = e2.clone();
        Box::pin(async move {
            *err.lock().await = Some(format!("{e:?}"));
        })
    }))
    .await
    .unwrap();

    stt.connect().await.expect("connect STT");

    let bytes_per_50ms = (SAMPLE_RATE as usize * 2 / 20).max(2);
    for chunk in pcm.chunks(bytes_per_50ms) {
        if stt.send_audio(Bytes::copy_from_slice(chunk)).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    // Trailing silence to trigger endpointing/finalization.
    let _ = stt
        .send_audio(Bytes::from(vec![0u8; SAMPLE_RATE as usize * 2]))
        .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    stt.disconnect()
        .await
        .expect("disconnect live STT provider");

    let transcript = best.lock().await.clone();
    let error = err.lock().await.clone();
    (transcript, error)
}

fn std_cfg(provider: &str, key: String, model: &str, language: &str) -> StandardSTTConfig {
    StandardSTTConfig::from_base(STTConfig {
        provider: provider.to_string(),
        api_key: key,
        language: language.to_string(),
        sample_rate: SAMPLE_RATE,
        channels: 1,
        punctuation: true,
        encoding: "linear16".to_string(),
        model: model.to_string(),
    })
}

#[tokio::test]
#[ignore = "Requires SARVAM_API_KEY + DEEPGRAM_API_KEY; real billed Sarvam STT calls"]
async fn sarvam_stt_transcribes_live() {
    ensure_crypto();
    let (Some(sarvam), Some(dg)) = (key("SARVAM_API_KEY"), key("DEEPGRAM_API_KEY")) else {
        return;
    };

    let pcm = synth_pcm(SENTENCE, &dg).await;
    assert!(pcm.len() > 8000, "synth audio too short: {} B", pcm.len());

    let mut stt = create_stt_standard("sarvam", std_cfg("sarvam", sarvam, "saarika:v2.5", "en-IN"))
        .expect("create_stt_standard sarvam");
    let (transcript, error) = transcribe(stt.as_mut(), &pcm).await;
    println!("Sarvam STT (saarika:v2.5) transcript: {transcript:?}  error: {error:?}");
    assert!(
        !transcript.trim().is_empty(),
        "Sarvam STT returned no transcript (error: {error:?})"
    );
}

#[tokio::test]
#[ignore = "Requires ELEVENLABS_API_KEY + DEEPGRAM_API_KEY; real billed ElevenLabs STT calls"]
async fn elevenlabs_stt_transcribes_live() {
    ensure_crypto();
    let (Some(el), Some(dg)) = (key("ELEVENLABS_API_KEY"), key("DEEPGRAM_API_KEY")) else {
        return;
    };

    let pcm = synth_pcm(SENTENCE, &dg).await;
    assert!(pcm.len() > 8000, "synth audio too short: {} B", pcm.len());

    let mut stt = create_stt_standard(
        "elevenlabs",
        std_cfg("elevenlabs", el, "scribe_v2_realtime", "en"),
    )
    .expect("create_stt_standard elevenlabs");
    let (transcript, error) = transcribe(stt.as_mut(), &pcm).await;
    println!("ElevenLabs STT (scribe_v2_realtime) transcript: {transcript:?}  error: {error:?}");
    assert!(
        !transcript.trim().is_empty(),
        "ElevenLabs STT returned no transcript (error: {error:?})"
    );
}
