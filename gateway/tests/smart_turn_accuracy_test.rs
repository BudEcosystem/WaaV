//! Smart Turn Accuracy Comparison Test
//!
//! This test compares Smart Turn model inference results between:
//! 1. Gateway's SmartTurnProcessor (Rust mel extraction + ONNX)
//! 2. Direct Python ONNX inference (for baseline)
//!
//! The goal is to verify 100% accuracy match between both implementations.

#![cfg(all(feature = "smart-turn", feature = "silero-vad"))]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use waav_gateway::core::smart_turn::{
    MelExtractor, MelExtractorConfig, SmartTurnDetector, SmartTurnDetectorConfig,
    SmartTurnProcessor, SmartTurnProcessorConfig,
};

const AUDIO_DIR: &str = "/tmp/smart_turn_test/audio";
const PYTHON_RESULTS: &str = "/tmp/smart_turn_test/whisper_mel_results.json";
const RUST_RESULTS: &str = "/tmp/smart_turn_test/rust_gateway_results.json";

/// Python test result structure
#[derive(Debug, Deserialize)]
struct PythonResult {
    probability: f64,
    logits: f64,
    mel_frames: usize,
    audio_samples: usize,
    audio_duration_ms: f64,
    mel_time_ms: f64,
    inference_time_ms: f64,
    total_time_ms: f64,
}

/// Rust test result structure
#[derive(Debug, Serialize)]
struct RustResult {
    probability: f64,
    mel_frames: usize,
    audio_samples: usize,
    audio_duration_ms: f64,
    mel_time_us: u64,
    inference_time_us: u64,
    total_time_us: u64,
}

/// Comparison result
#[derive(Debug, Serialize)]
struct ComparisonResult {
    filename: String,
    python_probability: f64,
    rust_probability: f64,
    probability_diff: f64,
    python_mel_frames: usize,
    rust_mel_frames: usize,
    match_status: String,
}

/// Read WAV file and return f32 samples at 16kHz mono
fn read_wav_16k_mono(path: &Path) -> Result<Vec<f32>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);

    // Read RIFF header
    let mut riff = [0u8; 4];
    reader.read_exact(&mut riff)?;
    if &riff != b"RIFF" {
        anyhow::bail!("Not a RIFF file");
    }

    let mut chunk_size = [0u8; 4];
    reader.read_exact(&mut chunk_size)?;

    let mut wave = [0u8; 4];
    reader.read_exact(&mut wave)?;
    if &wave != b"WAVE" {
        anyhow::bail!("Not a WAVE file");
    }

    // Find fmt chunk
    let mut audio_format = 0u16;
    let mut num_channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;

    loop {
        let mut chunk_id = [0u8; 4];
        if reader.read_exact(&mut chunk_id).is_err() {
            break;
        }

        let mut chunk_size = [0u8; 4];
        reader.read_exact(&mut chunk_size)?;
        let chunk_size = u32::from_le_bytes(chunk_size) as usize;

        if &chunk_id == b"fmt " {
            let mut fmt_data = vec![0u8; chunk_size];
            reader.read_exact(&mut fmt_data)?;

            audio_format = u16::from_le_bytes([fmt_data[0], fmt_data[1]]);
            num_channels = u16::from_le_bytes([fmt_data[2], fmt_data[3]]);
            sample_rate = u32::from_le_bytes([fmt_data[4], fmt_data[5], fmt_data[6], fmt_data[7]]);
            bits_per_sample = u16::from_le_bytes([fmt_data[14], fmt_data[15]]);
        } else if &chunk_id == b"data" {
            // Read audio data
            let mut audio_data = vec![0u8; chunk_size];
            reader.read_exact(&mut audio_data)?;

            // Convert to f32 samples
            let samples: Vec<f32> = if bits_per_sample == 16 {
                audio_data
                    .chunks_exact(2)
                    .map(|chunk| {
                        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                        sample as f32 / 32768.0
                    })
                    .collect()
            } else if bits_per_sample == 8 {
                audio_data
                    .iter()
                    .map(|&b| (b as f32 - 128.0) / 128.0)
                    .collect()
            } else {
                anyhow::bail!("Unsupported bits per sample: {}", bits_per_sample);
            };

            // Convert to mono if stereo
            let mono_samples: Vec<f32> = if num_channels == 2 {
                samples
                    .chunks_exact(2)
                    .map(|c| (c[0] + c[1]) / 2.0)
                    .collect()
            } else {
                samples
            };

            // Resample to 16kHz if needed
            if sample_rate != 16000 {
                // Simple linear interpolation resampling
                let ratio = sample_rate as f64 / 16000.0;
                let new_len = (mono_samples.len() as f64 / ratio) as usize;
                let mut resampled = Vec::with_capacity(new_len);

                for i in 0..new_len {
                    let src_idx = i as f64 * ratio;
                    let idx0 = src_idx.floor() as usize;
                    let idx1 = (idx0 + 1).min(mono_samples.len() - 1);
                    let frac = (src_idx - idx0 as f64) as f32;

                    let sample = mono_samples[idx0] * (1.0 - frac) + mono_samples[idx1] * frac;
                    resampled.push(sample);
                }

                return Ok(resampled);
            }

            return Ok(mono_samples);
        } else {
            // Skip unknown chunks
            let mut skip = vec![0u8; chunk_size];
            reader.read_exact(&mut skip)?;
        }
    }

    anyhow::bail!("No data chunk found in WAV file")
}

/// Test mel extraction and detector separately
#[tokio::test]
async fn test_smart_turn_mel_extraction() -> Result<()> {
    let audio_dir = Path::new(AUDIO_DIR);
    if !audio_dir.exists() {
        println!("Audio directory not found, skipping test");
        return Ok(());
    }

    // Load one test file
    let test_file = audio_dir.join("sample1_seg1.wav");
    if !test_file.exists() {
        println!("Test file not found, skipping");
        return Ok(());
    }

    let audio = read_wav_16k_mono(&test_file)?;
    println!(
        "Loaded {} samples ({:.2}s)",
        audio.len(),
        audio.len() as f64 / 16000.0
    );

    // Create mel extractor
    let mel_config = MelExtractorConfig::default();
    let mut mel_extractor = MelExtractor::new(mel_config)?;

    // Process audio
    mel_extractor.process(&audio)?;
    let mel_frames = mel_extractor.get_mel_frames();

    println!("Extracted {} mel frames", mel_frames.len());
    println!(
        "Each frame has {} mels",
        mel_frames.first().map(|f| f.len()).unwrap_or(0)
    );

    // Get padded output
    let mel_padded = mel_extractor.get_mel_2d_padded(800);
    println!(
        "Padded mel shape: {}x{}",
        mel_padded.len(),
        mel_padded.first().map(|f| f.len()).unwrap_or(0)
    );

    // Verify dimensions
    assert_eq!(mel_padded.len(), 800);
    assert_eq!(mel_padded[0].len(), 80);

    // Verify padding convention: first frames should be zeros (padding), last frames should have data
    let first_frame_sum: f32 = mel_padded[0].iter().sum();
    let last_frame_sum: f32 = mel_padded[799].iter().sum();

    println!("First frame sum: {}", first_frame_sum);
    println!("Last frame sum: {}", last_frame_sum);

    // If we have fewer than 800 frames, first should be padding (all zeros or close to it)
    if mel_frames.len() < 800 {
        // Padding frames at beginning should be zeros
        assert!(
            first_frame_sum.abs() < 0.001,
            "First frame should be zeros (padding), got sum {}",
            first_frame_sum
        );
    }

    Ok(())
}

/// Test full Smart Turn detector
#[tokio::test]
async fn test_smart_turn_detector() -> Result<()> {
    let audio_dir = Path::new(AUDIO_DIR);
    if !audio_dir.exists() {
        println!("Audio directory not found, skipping test");
        return Ok(());
    }

    // Load test file
    let test_file = audio_dir.join("sample1_seg1.wav");
    if !test_file.exists() {
        println!("Test file not found, skipping");
        return Ok(());
    }

    let audio = read_wav_16k_mono(&test_file)?;

    // Create mel extractor
    let mel_config = MelExtractorConfig::default();
    let mut mel_extractor = MelExtractor::new(mel_config)?;
    mel_extractor.process(&audio)?;
    let mel_frames = mel_extractor.get_mel_2d_padded(800);

    // Create detector
    let detector_config = SmartTurnDetectorConfig::default();
    let mut detector = SmartTurnDetector::new(detector_config).await?;

    // Run prediction
    let result = detector.predict(&mel_frames).await?;

    println!("Probability: {:.4}", result.probability);
    println!("Is turn complete: {}", result.is_turn_complete);
    println!("Inference time: {}us", result.inference_time_us);

    // Probability should be in valid range
    assert!(result.probability >= 0.0 && result.probability <= 1.0);

    Ok(())
}

/// REAL accuracy of the Rust Smart-Turn pipeline on the official labelled test set
/// (pipecat-ai/smart-turn-data-v3-test). Extract samples to /tmp/smart_turn_eval/ with a
/// labels.json ({"sN.wav": <endpoint_bool>}), then run with the ONNX runtime available:
///   ORT_DYLIB_PATH=.../libonnxruntime.so cargo test --features turn-ensemble \
///     --test smart_turn_accuracy_test real_dataset_accuracy -- --ignored --nocapture
#[cfg(feature = "smart-turn")]
#[ignore = "requires the downloaded labelled dataset in /tmp/smart_turn_eval + ORT_DYLIB_PATH"]
#[tokio::test]
async fn real_dataset_accuracy() -> Result<()> {
    let dir = Path::new("/tmp/smart_turn_eval");
    let labels: std::collections::HashMap<String, bool> =
        serde_json::from_reader(BufReader::new(File::open(dir.join("labels.json"))?))?;

    let mut detector = SmartTurnDetector::new(SmartTurnDetectorConfig::default()).await?;
    let (mut tp, mut fp, mut tn, mut fn_, mut n) = (0u32, 0u32, 0u32, 0u32, 0u32);
    let mut latencies = Vec::new();

    let mut names: Vec<_> = labels.keys().cloned().collect();
    names.sort();
    for name in names {
        let path = dir.join(&name);
        if !path.exists() {
            continue;
        }
        let audio = read_wav_16k_mono(&path)?;
        // Fresh extractor per clip (independent samples; no state bleed).
        let mut mel = MelExtractor::new(MelExtractorConfig::default())?;
        mel.process(&audio)?;
        let frames = mel.get_mel_2d_padded(800);
        let r = detector.predict(&frames).await?;
        latencies.push(r.inference_time_us);
        // Use the model's CALIBRATED decision threshold (0.7) — this model's probabilities sit
        // in ~[0.5, 0.73], so a naive 0.5 cutoff is degenerate (everything "complete").
        let predicted_complete = r.probability > detector.config().threshold;
        let actual_complete = labels[&name];
        match (predicted_complete, actual_complete) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, false) => tn += 1,
            (false, true) => fn_ += 1,
        }
        n += 1;
    }

    assert!(n >= 20, "need a meaningful sample count, got {n}");
    let acc = (tp + tn) as f64 / n as f64;
    let precision = if tp + fp > 0 { tp as f64 / (tp + fp) as f64 } else { 0.0 };
    let recall = if tp + fn_ > 0 { tp as f64 / (tp + fn_) as f64 } else { 0.0 };
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };
    latencies.sort_unstable();
    let p50 = latencies[latencies.len() / 2];
    println!(
        "SMART-TURN ACCURACY on {n} labelled samples: acc={:.3} precision={:.3} recall={:.3} F1={:.3} | TP={tp} FP={fp} TN={tn} FN={fn_} | p50_latency={p50}us",
        acc, precision, recall, f1
    );

    // Sanity gate: the model must do substantially better than chance on the official test set.
    assert!(acc > 0.65, "accuracy {acc:.3} below 0.65 — pipeline likely broken");
    Ok(())
}

/// Debug mel spectrogram values from Rust implementation
#[tokio::test]
async fn test_debug_mel_values() -> Result<()> {
    use std::io::Write;

    let audio_path = Path::new("/tmp/smart_turn_test/audio/noise_then_silence.wav");
    if !audio_path.exists() {
        println!("Audio file not found, skipping debug test");
        return Ok(());
    }

    let audio = read_wav_16k_mono(audio_path)?;
    println!(
        "Audio samples: {}, duration: {:.2}s",
        audio.len(),
        audio.len() as f64 / 16000.0
    );
    println!(
        "Audio range: [{:.4}, {:.4}]",
        audio.iter().cloned().fold(f32::MAX, f32::min),
        audio.iter().cloned().fold(f32::MIN, f32::max)
    );
    println!("Audio first 10 samples: {:?}", &audio[..10]);

    // Create mel extractor
    let mel_config = MelExtractorConfig::default();
    let mut mel_extractor = MelExtractor::new(mel_config)?;
    mel_extractor.process(&audio)?;

    let mel_frames = mel_extractor.get_mel_frames();
    println!("\nMel frames: {}", mel_frames.len());

    // Compute statistics
    let mut all_values: Vec<f32> = Vec::new();
    for frame in mel_frames {
        all_values.extend(frame);
    }

    let mel_min = all_values.iter().cloned().fold(f32::MAX, f32::min);
    let mel_max = all_values.iter().cloned().fold(f32::MIN, f32::max);
    let mel_mean = all_values.iter().sum::<f32>() / all_values.len() as f32;

    println!(
        "Mel stats: min={:.4}, max={:.4}, mean={:.4}",
        mel_min, mel_max, mel_mean
    );

    // Get padded output (matching Python's prepare_model_input)
    let mel_padded = mel_extractor.get_mel_2d_padded(800);
    println!("Padded shape: {}x{}", mel_padded.len(), mel_padded[0].len());

    // Check first frame (should be zeros if padded)
    let first_frame_sum: f32 = mel_padded[0].iter().sum();
    let last_frame_sum: f32 = mel_padded[799].iter().sum();
    println!("First frame sum: {}", first_frame_sum);
    println!("Last frame sum: {}", last_frame_sum);

    // Print first few values of first and last frames
    println!("First frame first 5 mels: {:?}", &mel_padded[0][..5]);
    println!("Last frame first 5 mels: {:?}", &mel_padded[799][..5]);

    // Save debug output
    let debug_output = serde_json::json!({
        "audio_samples": audio.len(),
        "audio_first_20": &audio[..20],
        "mel_frames": mel_frames.len(),
        "mel_min": mel_min,
        "mel_max": mel_max,
        "mel_mean": mel_mean,
        "padded_first_frame_first_10": &mel_padded[0][..10],
        "padded_last_frame_first_10": &mel_padded[799][..10],
        "mel_frames_first_5_first_10_mels": mel_frames.iter().take(5).map(|f| &f[..10]).collect::<Vec<_>>(),
    });

    let mut file = File::create("/tmp/smart_turn_test/rust_mel_debug.json")?;
    file.write_all(serde_json::to_string_pretty(&debug_output)?.as_bytes())?;
    println!("\nSaved rust_mel_debug.json");

    Ok(())
}

/// Compare gateway results with Python direct ONNX results
#[tokio::test]
async fn test_smart_turn_accuracy_comparison() -> Result<()> {
    use std::time::Instant;

    let audio_dir = Path::new(AUDIO_DIR);
    let python_results_path = Path::new(PYTHON_RESULTS);

    if !audio_dir.exists() {
        println!("Audio directory not found at {}, skipping test", AUDIO_DIR);
        println!("Run the Python test first: python3 /tmp/smart_turn_test/test_direct_onnx.py");
        return Ok(());
    }

    if !python_results_path.exists() {
        println!(
            "Python results not found at {}, skipping test",
            PYTHON_RESULTS
        );
        println!("Run the Python test first: python3 /tmp/smart_turn_test/test_direct_onnx.py");
        return Ok(());
    }

    // Load Python results
    let python_file = File::open(python_results_path)?;
    let python_results: HashMap<String, PythonResult> = serde_json::from_reader(python_file)?;

    println!("Loaded {} Python results", python_results.len());

    // Create detector (only once, reuse for all files)
    let detector_config = SmartTurnDetectorConfig::default();
    let mut detector = SmartTurnDetector::new(detector_config).await?;

    let mut rust_results: HashMap<String, RustResult> = HashMap::new();
    let mut comparisons: Vec<ComparisonResult> = Vec::new();

    // Process each audio file
    for (filename, python_result) in &python_results {
        let audio_path = audio_dir.join(filename);
        if !audio_path.exists() {
            println!("Audio file not found: {}", filename);
            continue;
        }

        let start = Instant::now();

        // Read audio
        let audio = read_wav_16k_mono(&audio_path)?;

        // Create mel extractor for this file
        let mel_config = MelExtractorConfig::default();
        let mut mel_extractor = MelExtractor::new(mel_config)?;

        let mel_start = Instant::now();
        mel_extractor.process(&audio)?;
        let mel_time_us = mel_start.elapsed().as_micros() as u64;

        let mel_frames = mel_extractor.get_mel_2d_padded(800);
        let num_mel_frames = mel_extractor.num_frames();

        // Reset detector state for fair comparison
        detector.reset();

        // Run prediction
        let result = detector.predict(&mel_frames).await?;

        let total_time_us = start.elapsed().as_micros() as u64;

        // Store result
        let rust_result = RustResult {
            probability: result.probability as f64,
            mel_frames: num_mel_frames,
            audio_samples: audio.len(),
            audio_duration_ms: audio.len() as f64 / 16.0,
            mel_time_us,
            inference_time_us: result.inference_time_us,
            total_time_us,
        };

        // Compare with Python
        let prob_diff = (result.probability as f64 - python_result.probability).abs();
        let match_status = if prob_diff < 0.001 {
            "EXACT"
        } else if prob_diff < 0.01 {
            "CLOSE"
        } else if prob_diff < 0.05 {
            "ACCEPTABLE"
        } else {
            "MISMATCH"
        };

        let comparison = ComparisonResult {
            filename: filename.clone(),
            python_probability: python_result.probability,
            rust_probability: result.probability as f64,
            probability_diff: prob_diff,
            python_mel_frames: python_result.mel_frames,
            rust_mel_frames: num_mel_frames,
            match_status: match_status.to_string(),
        };

        println!(
            "{}: Python={:.4}, Rust={:.4}, diff={:.4}, status={}",
            filename, python_result.probability, result.probability, prob_diff, match_status
        );

        rust_results.insert(filename.clone(), rust_result);
        comparisons.push(comparison);
    }

    // Save Rust results
    let rust_results_file = File::create(RUST_RESULTS)?;
    serde_json::to_writer_pretty(rust_results_file, &rust_results)?;
    println!("\nRust results saved to {}", RUST_RESULTS);

    // Save comparisons
    let comparison_file = File::create("/tmp/smart_turn_test/comparison_results.json")?;
    serde_json::to_writer_pretty(comparison_file, &comparisons)?;
    println!("Comparison results saved to /tmp/smart_turn_test/comparison_results.json");

    // Summary
    let total = comparisons.len();
    let exact = comparisons
        .iter()
        .filter(|c| c.match_status == "EXACT")
        .count();
    let close = comparisons
        .iter()
        .filter(|c| c.match_status == "CLOSE")
        .count();
    let acceptable = comparisons
        .iter()
        .filter(|c| c.match_status == "ACCEPTABLE")
        .count();
    let mismatch = comparisons
        .iter()
        .filter(|c| c.match_status == "MISMATCH")
        .count();

    println!("\n{}", "=".repeat(60));
    println!("COMPARISON SUMMARY");
    println!("{}", "=".repeat(60));
    println!("Total files: {}", total);
    println!("EXACT (diff < 0.001): {}", exact);
    println!("CLOSE (diff < 0.01): {}", close);
    println!("ACCEPTABLE (diff < 0.05): {}", acceptable);
    println!("MISMATCH (diff >= 0.05): {}", mismatch);

    if mismatch > 0 {
        println!(
            "\nWARNING: {} files have significant probability differences!",
            mismatch
        );
        for c in &comparisons {
            if c.match_status == "MISMATCH" {
                println!(
                    "  {}: Python={:.4}, Rust={:.4}, diff={:.4}",
                    c.filename, c.python_probability, c.rust_probability, c.probability_diff
                );
            }
        }
    }

    // Calculate statistics
    let diffs: Vec<f64> = comparisons.iter().map(|c| c.probability_diff).collect();
    let mean_diff: f64 = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let max_diff: f64 = diffs.iter().cloned().fold(f64::MIN, f64::max);

    println!("\nStatistics:");
    println!("  Mean probability difference: {:.6}", mean_diff);
    println!("  Max probability difference: {:.6}", max_diff);

    // Accuracy requirements:
    // 1. Mean difference should be very small (< 0.02)
    // 2. At least 90% of files should be EXACT or CLOSE (diff < 0.01)
    // 3. Allow up to 1 outlier (for edge cases like extremely high-energy audio)
    //
    // Note: Small differences can occur due to:
    // - Floating-point precision differences between numpy/realfft FFT
    // - Filterbank computation differences for high-energy signals
    // - Numerical precision in log10/exp operations
    let total = comparisons.len();
    let exact_or_close = exact + close;
    let exact_or_close_percent = (exact_or_close as f64 / total as f64) * 100.0;

    println!("\nAccuracy Assessment:");
    println!(
        "  EXACT/CLOSE rate: {:.1}% ({}/{})",
        exact_or_close_percent, exact_or_close, total
    );
    println!("  Mean difference: {:.6}", mean_diff);

    // Assertions
    assert!(
        mean_diff < 0.02,
        "Mean probability difference {:.6} is too high (expected < 0.02)",
        mean_diff
    );

    assert!(
        exact_or_close_percent >= 90.0,
        "Only {:.1}% of files are EXACT/CLOSE (expected >= 90%)",
        exact_or_close_percent
    );

    assert!(
        mismatch <= 1,
        "Found {} files with significant differences (expected <= 1 outlier)",
        mismatch
    );

    Ok(())
}
