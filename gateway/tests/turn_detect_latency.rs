//! Per-frame TURN-DETECTION latency: silero-VAD + smart-turn ONNX inference cost.
//!
//! These run on the audio hot path (~50–100 fps/session) and gate how fast WaaV
//! can DECIDE the user stopped talking — the budget that precedes `stt_final`
//! (see LATENCY_ANALYSIS.md §6/§7). Inference must stay well under one frame
//! (~10–20 ms) to run continuously without adding turn-detection lag.
//!
//! Models are read from the local cache (`~/.cache/waav/models/*.onnx`); no
//! network. Run:
//!   CUDA_HOME=/tmp/nocuda ORT_DYLIB_PATH=…/libonnxruntime.so \
//!     cargo test --features smart-turn,silero-vad --test turn_detect_latency -- --nocapture --test-threads=1
#![cfg(all(feature = "smart-turn", feature = "silero-vad"))]

use std::time::Instant;

use anyhow::Result;
use waav_gateway::core::silero_vad::{SileroVAD, SileroVADConfig};
use waav_gateway::core::smart_turn::{
    MelExtractor, MelExtractorConfig, SmartTurnDetector, SmartTurnDetectorConfig,
};

const SR: usize = 16_000;

/// Speech-ish signal (fundamental + harmonics). Content is irrelevant to the
/// fixed-shape ONNX inference cost we are timing.
fn synth(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            0.30 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
                + 0.15 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()
                + 0.08 * (2.0 * std::f32::consts::PI * 450.0 * t).sin()
        })
        .collect()
}

fn pctl(v: &mut [u64], p: f64) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() as f64 * p) as usize).min(v.len() - 1)]
}

#[tokio::test]
async fn smart_turn_inference_latency() -> Result<()> {
    let audio = synth(SR * 2); // 2 s window
    let mut detector = SmartTurnDetector::new(SmartTurnDetectorConfig::default()).await?;

    let mut mel_us = Vec::new();
    let mut infer_us = Vec::new();
    let mut total_us = Vec::new();
    let iters = 40usize;
    for i in 0..iters {
        let t0 = Instant::now();
        let mut mel = MelExtractor::new(MelExtractorConfig::default())?;
        mel.process(&audio)?;
        let frames = mel.get_mel_2d_padded(800);
        let mel = t0.elapsed().as_micros() as u64;
        let r = detector.predict(&frames).await?;
        let total = t0.elapsed().as_micros() as u64;
        if i >= 5 {
            // discard warmup (first ONNX runs allocate arenas / JIT)
            mel_us.push(mel);
            infer_us.push(r.inference_time_us);
            total_us.push(total);
        }
    }
    let n = infer_us.len();
    println!("\n=== SMART-TURN per-inference latency (n={n}, 2s/800-frame window) ===");
    println!(
        "  mel extraction : p50={}us p90={}us p99={}us",
        pctl(&mut mel_us.clone(), 0.5),
        pctl(&mut mel_us.clone(), 0.9),
        pctl(&mut mel_us.clone(), 0.99)
    );
    println!(
        "  onnx inference : p50={}us p90={}us p99={}us",
        pctl(&mut infer_us.clone(), 0.5),
        pctl(&mut infer_us.clone(), 0.9),
        pctl(&mut infer_us.clone(), 0.99)
    );
    println!(
        "  mel+inference  : p50={}us p90={}us p99={}us  ({:.2}ms p50)",
        pctl(&mut total_us.clone(), 0.5),
        pctl(&mut total_us.clone(), 0.9),
        pctl(&mut total_us.clone(), 0.99),
        pctl(&mut total_us.clone(), 0.5) as f64 / 1000.0
    );
    assert!(!infer_us.is_empty());
    Ok(())
}

#[tokio::test]
async fn silero_vad_inference_latency() -> Result<()> {
    let cfg = SileroVADConfig {
        model_path: Some("/home/bud/.cache/waav/models/silero_vad.onnx".into()),
        ..Default::default()
    };
    let mut vad = SileroVAD::new(cfg).await?;

    // Silero processes 512-sample (32 ms) frames at 16 kHz.
    let audio = synth(SR); // 1 s → ~31 frames
    let mut per_frame = Vec::new();
    let mut i = 0usize;
    for frame in audio.chunks(512) {
        if frame.len() < 512 {
            break;
        }
        let t = Instant::now();
        let _ = vad.process(frame)?;
        let us = t.elapsed().as_micros() as u64;
        if i >= 5 {
            per_frame.push(us);
        }
        i += 1;
    }
    let n = per_frame.len();
    println!("\n=== SILERO-VAD per-frame (512-sample / 32ms) inference latency (n={n}) ===");
    println!(
        "  vad.process    : p50={}us p90={}us p99={}us  ({:.3}ms p50)",
        pctl(&mut per_frame.clone(), 0.5),
        pctl(&mut per_frame.clone(), 0.9),
        pctl(&mut per_frame.clone(), 0.99),
        pctl(&mut per_frame.clone(), 0.5) as f64 / 1000.0
    );
    assert!(!per_frame.is_empty());
    Ok(())
}
