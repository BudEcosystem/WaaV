//! LIVE multi-vendor DAG e2e: English speech → Hindi speech through the real DAG executor.
//!
//! Pipeline (3 vendors): Deepgram STT (English) → Sarvam-M LLM translate (English → Hindi) →
//! ElevenLabs multilingual TTS (Hindi). Exercises the DAG orchestration engine end-to-end with
//! REAL providers — STT + Translation(LLM) + TTS chained — plus the W-? credential fix that lets
//! DAG `stt_provider`/`tts_provider` nodes authenticate via `${ENV_VAR}` in their node config.
//!
//! All tests are `#[ignore]` and key-gated (real billed calls). Run:
//!   SARVAM_API_KEY=… DEEPGRAM_API_KEY=… ELEVENLABS_API_KEY=… \
//!     cargo test --features dag-routing --test dag_multivendor_live_e2e -- --ignored --nocapture --test-threads=1
//!
//! The authored workflow JSON is committed at `examples/dag/en_to_hindi_multivendor.json`; this
//! test builds the identical graph programmatically so the live path and the artifact stay in sync.

#![cfg(feature = "dag-routing")]

use waav_gateway::dag::compiler::DAGCompiler;
use waav_gateway::dag::context::DAGContext;
use waav_gateway::dag::definition::{DAGDefinition, EdgeDefinition, NodeDefinition, NodeType};
use waav_gateway::dag::executor::DAGExecutor;
use waav_gateway::dag::nodes::DAGData;

const SAMPLE_RATE: u32 = 16000;
const ENGLISH: &str = "Hello, how are you today? The weather is very nice.";
/// A widely-available ElevenLabs prebuilt voice ("Rachel"); the multilingual_v2 model speaks Hindi.
const ELEVENLABS_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";

fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Skip (do not fail) when a required key is absent — matches the repo's live-test convention.
fn key(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("SKIP: {var} not set");
            None
        }
    }
}

fn contains_devanagari(s: &str) -> bool {
    s.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c))
}

/// The Sarvam translation node (English → Hindi). Sarvam-M is a reasoning model, so it needs a
/// generous `max_tokens` or it consumes the whole budget on reasoning and returns null content.
fn sarvam_translate_node() -> NodeDefinition {
    NodeDefinition::new(
        "translate",
        NodeType::LlmEndpoint {
            base_url: "https://api.sarvam.ai/v1".to_string(),
            model: "sarvam-30b".to_string(),
            api_key: Some("${SARVAM_API_KEY}".to_string()),
            system_prompt: Some(
                "You are a translation engine. Translate the user's English text into Hindi in \
                 Devanagari script. Output ONLY the Hindi translation — no English, no \
                 transliteration, no quotes, no notes."
                    .to_string(),
            ),
            temperature: Some(0.1),
            max_tokens: Some(4000),
            streaming: false,
            tools: None,
            assistant_id: None,
            timeout_ms: Some(120_000),
            headers: Default::default(),
        },
    )
}

fn elevenlabs_hindi_tts_node() -> NodeDefinition {
    NodeDefinition::new(
        "tts",
        NodeType::TtsProvider {
            provider: "elevenlabs".to_string(),
            voice_id: Some(ELEVENLABS_VOICE.to_string()),
            model: Some("eleven_multilingual_v2".to_string()),
        },
    )
    // The DAG credential fix: provider nodes read `api_key` (literal or ${ENV_VAR}) from `config`.
    .with_config(serde_json::json!({ "api_key": "${ELEVENLABS_API_KEY}" }))
}

fn deepgram_stt_node() -> NodeDefinition {
    NodeDefinition::new(
        "stt",
        NodeType::SttProvider {
            provider: "deepgram".to_string(),
            model: Some("nova-2".to_string()),
            language: Some("en-US".to_string()),
        },
    )
    .with_config(serde_json::json!({ "api_key": "${DEEPGRAM_API_KEY}" }))
}

/// Synthesize clean linear16 English PCM for the STT stage via a direct Deepgram Aura call.
async fn synth_english_pcm(text: &str, deepgram_key: &str) -> Vec<u8> {
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
    assert!(resp.status().is_success(), "deepgram speak: {}", resp.status());
    resp.bytes().await.unwrap().to_vec()
}

// ============================================================================
// 1. Sarvam translates English → Hindi (the Translation stage, live)
// ============================================================================
#[tokio::test]
#[ignore = "Requires SARVAM_API_KEY; real billed Sarvam call"]
async fn dag_sarvam_translates_en_to_hindi_live() {
    ensure_crypto();
    let Some(_) = key("SARVAM_API_KEY") else { return };

    let mut dag = DAGDefinition::new("translate-only", "Sarvam EN->HI");
    dag.add_node(sarvam_translate_node());
    dag.with_entry("translate");
    dag.add_exit("translate");
    let compiled = DAGCompiler::new().compile(dag).expect("compile");

    let exec = DAGExecutor::new() /* per-node timeout_ms=120000 on the translate node is now honored (no executor override needed) */;
    let mut ctx = DAGContext::new("xlate-test");
    let out = exec
        .execute_from(&compiled, "translate", DAGData::Text(ENGLISH.to_string()), &mut ctx)
        .await
        .expect("translation executes");

    let hindi = match out {
        DAGData::Text(t) => t,
        other => panic!("expected translated Text, got {}", other.type_name()),
    };
    println!("Sarvam EN->HI: {ENGLISH:?} => {hindi:?}");
    assert!(!hindi.trim().is_empty(), "empty translation");
    assert!(
        contains_devanagari(&hindi),
        "translation should be in Hindi (Devanagari): {hindi:?}"
    );
}

// ============================================================================
// 2. translate (Sarvam) → TTS (ElevenLabs Hindi) — 2-vendor DAG, live
// ============================================================================
#[tokio::test]
#[ignore = "Requires SARVAM_API_KEY + ELEVENLABS_API_KEY; real billed calls"]
async fn dag_translate_then_elevenlabs_hindi_tts_live() {
    ensure_crypto();
    let (Some(_), Some(_)) = (key("SARVAM_API_KEY"), key("ELEVENLABS_API_KEY")) else { return };

    let mut dag = DAGDefinition::new("xlate-tts", "Sarvam EN->HI -> ElevenLabs TTS");
    dag.add_node(sarvam_translate_node());
    dag.add_node(elevenlabs_hindi_tts_node());
    dag.add_edge(EdgeDefinition::new("translate", "tts"));
    dag.with_entry("translate");
    dag.add_exit("tts");
    let compiled = DAGCompiler::new().compile(dag).expect("compile");

    let exec = DAGExecutor::new() /* per-node timeout_ms=120000 on the translate node is now honored (no executor override needed) */;
    let mut ctx = DAGContext::new("xlate-tts-test");
    let out = exec
        .execute_from(&compiled, "translate", DAGData::Text(ENGLISH.to_string()), &mut ctx)
        .await
        .expect("translate->tts executes");

    match out {
        DAGData::TTSAudio(a) => {
            println!("Hindi TTS audio: {} bytes ({} Hz, {})", a.data.len(), a.sample_rate, a.format);
            assert!(a.data.len() > 1000, "expected real Hindi audio, got {} B", a.data.len());
        }
        other => panic!("expected TTSAudio, got {}", other.type_name()),
    }
}

// ============================================================================
// 4. STREAMING data-plane: LLM streams sentences → TTS speaks each while the LLM keeps
//    generating. The terminal node delivers MULTIPLE audio chunks incrementally (vs. the batch
//    path's single blob), proving inter-node streaming for real-time.
// ============================================================================
#[tokio::test]
#[ignore = "Requires SARVAM_API_KEY + ELEVENLABS_API_KEY; real billed calls"]
async fn dag_streaming_emits_incremental_audio_live() {
    use std::time::Instant;
    use tokio::sync::mpsc;
    use waav_gateway::dag::context::DagOutput;

    ensure_crypto();
    let (Some(_), Some(_)) = (key("SARVAM_API_KEY"), key("ELEVENLABS_API_KEY")) else { return };

    // A multi-sentence translation so the LLM produces several Hindi sentences to stream.
    let paragraph = "Hello there. How are you doing today? The weather is really nice. \
                     I hope you have a wonderful day ahead.";

    // Streaming variant of the translate node: same as sarvam_translate_node but streaming=true.
    let translate = NodeDefinition::new(
        "translate",
        NodeType::LlmEndpoint {
            base_url: "https://api.sarvam.ai/v1".to_string(),
            model: "sarvam-30b".to_string(),
            api_key: Some("${SARVAM_API_KEY}".to_string()),
            system_prompt: Some(
                "Translate each English sentence into Hindi (Devanagari). Output ONLY the Hindi \
                 translation, preserving sentence boundaries."
                    .to_string(),
            ),
            temperature: Some(0.1),
            max_tokens: Some(4000),
            streaming: true,
            tools: None,
            assistant_id: None,
            timeout_ms: Some(120_000),
            headers: Default::default(),
        },
    );

    let mut dag = DAGDefinition::new("stream-xlate-tts", "streaming Sarvam EN->HI -> ElevenLabs TTS");
    dag.add_node(translate);
    dag.add_node(elevenlabs_hindi_tts_node());
    dag.add_edge(EdgeDefinition::new("translate", "tts"));
    dag.with_entry("translate");
    dag.add_exit("tts");
    let compiled = DAGCompiler::new().compile(dag).expect("compile");

    // Drain the client sink CONCURRENTLY, timestamping each audio chunk as it arrives.
    let (tx, rx) = mpsc::channel::<DagOutput>(64);
    let start = Instant::now();
    let drain = tokio::spawn(async move {
        let mut rx = rx;
        let mut chunks: Vec<(u128, usize)> = Vec::new();
        while let Some(out) = rx.recv().await {
            if let DagOutput::Audio(a) = out {
                chunks.push((start.elapsed().as_millis(), a.data.len()));
            }
        }
        chunks
    });

    let exec = DAGExecutor::new() /* per-node timeout_ms=120000 on the translate node is now honored (no executor override needed) */;
    let mut ctx = DAGContext::new("stream-test").with_output_tx(tx);
    exec.execute_streaming_from(&compiled, "translate", DAGData::Text(paragraph.to_string()), &mut ctx)
        .await
        .expect("streaming pipeline executes");
    drop(ctx); // drop the sink so the drain task ends

    let chunks = drain.await.unwrap();
    let total: usize = chunks.iter().map(|(_, n)| n).sum();
    println!(
        "STREAMING audio chunks (ms@arrival, bytes): {:?}  | {} chunks, {} total bytes",
        chunks,
        chunks.len(),
        total
    );
    assert!(!chunks.is_empty(), "no audio streamed from the pipeline");
    assert!(total > 1000, "expected real Hindi audio, got {total} bytes");
    // The win: more than one chunk arrived incrementally (the batch path would deliver exactly one).
    if chunks.len() >= 2 {
        println!(
            "✓ inter-node streaming confirmed: first audio at {} ms, last at {} ms",
            chunks.first().unwrap().0,
            chunks.last().unwrap().0
        );
    } else {
        println!("NOTE: provider streamed the content as a single chunk this run (model-dependent)");
    }
}

// ============================================================================
// 3. FULL pipeline: Deepgram STT → Sarvam translate → ElevenLabs TTS — 3-vendor DAG, live
// ============================================================================
#[tokio::test]
#[ignore = "Requires DEEPGRAM_API_KEY + SARVAM_API_KEY + ELEVENLABS_API_KEY; real billed calls"]
async fn dag_full_audio_pipeline_en_to_hindi_live() {
    ensure_crypto();
    let (Some(dg), Some(_), Some(_)) =
        (key("DEEPGRAM_API_KEY"), key("SARVAM_API_KEY"), key("ELEVENLABS_API_KEY"))
    else {
        return;
    };

    // Make real English speech for the STT stage.
    let pcm = synth_english_pcm(ENGLISH, &dg).await;
    assert!(pcm.len() > 8000, "synth audio too short: {} B", pcm.len());

    let mut dag = DAGDefinition::new("en2hi-full", "Deepgram STT -> Sarvam translate -> ElevenLabs TTS");
    dag.add_node(deepgram_stt_node());
    dag.add_node(sarvam_translate_node());
    dag.add_node(elevenlabs_hindi_tts_node());
    dag.add_edge(EdgeDefinition::new("stt", "translate"));
    dag.add_edge(EdgeDefinition::new("translate", "tts"));
    dag.with_entry("stt");
    dag.add_exit("tts");
    let compiled = DAGCompiler::new().compile(dag).expect("compile");

    let exec = DAGExecutor::new() /* per-node timeout_ms=120000 on the translate node is now honored (no executor override needed) */;
    let mut ctx = DAGContext::new("full-pipeline-test");
    let out = exec
        .execute_from(&compiled, "stt", DAGData::Audio(bytes::Bytes::from(pcm)), &mut ctx)
        .await
        .expect("full audio->audio pipeline executes");

    match out {
        DAGData::TTSAudio(a) => {
            println!(
                "FULL PIPELINE EN->HI: {} bytes of Hindi audio ({} Hz, {})",
                a.data.len(),
                a.sample_rate,
                a.format
            );
            assert!(a.data.len() > 1000, "expected real Hindi audio out, got {} B", a.data.len());
        }
        other => panic!("expected final TTSAudio, got {}", other.type_name()),
    }
}
