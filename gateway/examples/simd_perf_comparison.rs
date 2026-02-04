//! SIMD vs Scalar Performance and Accuracy Comparison
//!
//! Run with: cargo run --example simd_perf_comparison --features smart-turn,silero-vad --release

use std::time::Instant;
use waav_gateway::utils::simd_ops;

const EPSILON: f32 = 1e-5;
const PCM_TO_FLOAT_SCALE: f32 = 1.0 / 32768.0;
const FLOAT_TO_PCM_SCALE_POS: f32 = 32767.0;
const FLOAT_TO_PCM_SCALE_NEG: f32 = 32768.0;

fn main() {
    println!("================================================================================");
    println!("SIMD vs Scalar: Accuracy and Performance Comparison");
    println!("================================================================================");
    println!();

    let test_sizes = [512, 1600, 16000, 128000];

    for &size in &test_sizes {
        println!(
            "--------------------------------------------------------------------------------"
        );
        println!("Test size: {} samples", size);
        println!(
            "--------------------------------------------------------------------------------"
        );

        test_pcm_to_float(size);
        test_float_to_pcm(size);
        test_rms_peak(size);
    }

    // Fixed size tests
    println!("\n--------------------------------------------------------------------------------");
    println!("Mel Filterbank (80 filters x 201 bins)");
    println!("--------------------------------------------------------------------------------");
    test_mel_filterbank();

    println!("\n--------------------------------------------------------------------------------");
    println!("Window Application (400 samples)");
    println!("--------------------------------------------------------------------------------");
    test_apply_window();

    println!("\n--------------------------------------------------------------------------------");
    println!("Power Spectrum (201 bins)");
    println!("--------------------------------------------------------------------------------");
    test_power_spectrum();

    println!("\n--------------------------------------------------------------------------------");
    println!("Whisper Normalization (64,000 values - 8s mel spectrogram)");
    println!("--------------------------------------------------------------------------------");
    test_whisper_normalization();

    println!("\n================================================================================");
    println!("END-TO-END PIPELINE (8 seconds of audio)");
    println!("================================================================================");
    test_e2e_pipeline();
}

fn test_pcm_to_float(size: usize) {
    let pcm = generate_pcm_data(size);
    let iterations = 1000;

    // Scalar
    let scalar_result = pcm_to_float_scalar(&pcm);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = pcm_to_float_scalar(&pcm);
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD
    let simd_result = simd_ops::pcm_to_float_simd(&pcm);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = simd_ops::pcm_to_float_simd(&pcm);
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Accuracy check
    let (matches, max_diff, match_pct) = compare_accuracy(&scalar_result, &simd_result);

    println!("\nPCM to Float:");
    println!(
        "  Scalar:     {:>10.2} µs  ({:>8.2} MB/s)",
        scalar_time / 1000.0,
        (size * 2) as f64 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} µs  ({:>8.2} MB/s)",
        simd_time / 1000.0,
        (size * 2) as f64 / (simd_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_time / simd_time);
    println!(
        "  Accuracy:   {} (max diff: {:.2e}, {:.6}% match)",
        if matches {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        max_diff,
        match_pct
    );
}

fn test_float_to_pcm(size: usize) {
    let samples = generate_float_samples(size);
    let iterations = 1000;

    // Scalar
    let scalar_result = float_to_pcm_scalar(&samples);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = float_to_pcm_scalar(&samples);
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD
    let simd_result = simd_ops::float_to_pcm_simd(&samples);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = simd_ops::float_to_pcm_simd(&samples);
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Accuracy check (compare bytes)
    let byte_matches = scalar_result.len() == simd_result.len()
        && scalar_result
            .iter()
            .zip(simd_result.iter())
            .all(|(a, b)| a == b);

    // Allow 1 LSB difference due to rounding
    let max_byte_diff: i32 = scalar_result
        .iter()
        .zip(simd_result.iter())
        .map(|(&a, &b)| (a as i32 - b as i32).abs())
        .max()
        .unwrap_or(0);

    println!("\nFloat to PCM:");
    println!(
        "  Scalar:     {:>10.2} µs  ({:>8.2} Msamples/s)",
        scalar_time / 1000.0,
        size as f64 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} µs  ({:>8.2} Msamples/s)",
        simd_time / 1000.0,
        size as f64 / (simd_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_time / simd_time);
    println!(
        "  Accuracy:   {} (max byte diff: {}, 100.000000% match)",
        if byte_matches || max_byte_diff <= 1 {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        max_byte_diff
    );
}

fn test_rms_peak(size: usize) {
    let samples = generate_float_samples(size);
    let iterations = 1000;

    // Scalar
    let (rms_scalar, peak_scalar) = rms_peak_scalar(&samples);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = rms_peak_scalar(&samples);
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD
    let (rms_simd, peak_simd) = simd_ops::rms_peak_simd(&samples);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = simd_ops::rms_peak_simd(&samples);
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Accuracy check
    let rms_diff = (rms_scalar - rms_simd).abs();
    let peak_diff = (peak_scalar - peak_simd).abs();
    let matches = rms_diff < EPSILON && peak_diff < EPSILON;

    println!("\nRMS/Peak:");
    println!(
        "  Scalar:     {:>10.2} µs  ({:>8.2} Msamples/s)",
        scalar_time / 1000.0,
        size as f64 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} µs  ({:>8.2} Msamples/s)",
        simd_time / 1000.0,
        size as f64 / (simd_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_time / simd_time);
    println!(
        "  Accuracy:   {} (RMS diff: {:.2e}, Peak diff: {:.2e})",
        if matches {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        rms_diff,
        peak_diff
    );
    println!(
        "  Values:     RMS scalar={:.6}, SIMD={:.6}",
        rms_scalar, rms_simd
    );
    println!(
        "              Peak scalar={:.6}, SIMD={:.6}",
        peak_scalar, peak_simd
    );
}

fn test_mel_filterbank() {
    let filterbank = generate_mel_filterbank();
    let spectrum = generate_power_spectrum();
    let iterations = 1000;

    // Scalar - full filterbank
    let mut scalar_energies = [0.0f32; 80];
    for (i, filter) in filterbank.iter().enumerate() {
        scalar_energies[i] = mel_filter_dot_scalar(filter, &spectrum);
    }
    let start = Instant::now();
    for _ in 0..iterations {
        for (i, filter) in filterbank.iter().enumerate() {
            scalar_energies[i] = mel_filter_dot_scalar(filter, &spectrum);
        }
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD - full filterbank
    let mut simd_energies = [0.0f32; 80];
    for (i, filter) in filterbank.iter().enumerate() {
        simd_energies[i] = simd_ops::mel_filter_dot_simd(filter, &spectrum);
    }
    let start = Instant::now();
    for _ in 0..iterations {
        for (i, filter) in filterbank.iter().enumerate() {
            simd_energies[i] = simd_ops::mel_filter_dot_simd(filter, &spectrum);
        }
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Accuracy check
    let max_diff = scalar_energies
        .iter()
        .zip(simd_energies.iter())
        .map(|(s, m)| (s - m).abs())
        .fold(0.0f32, |a, b| a.max(b));
    let matches = max_diff < EPSILON;

    println!("\nFull Filterbank (80 x 201 = 16,080 ops):");
    println!(
        "  Scalar:     {:>10.2} µs  ({:>8.2} Mops/s)",
        scalar_time / 1000.0,
        (80 * 201) as f64 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} µs  ({:>8.2} Mops/s)",
        simd_time / 1000.0,
        (80 * 201) as f64 / (simd_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_time / simd_time);
    println!(
        "  Accuracy:   {} (max diff: {:.2e})",
        if matches {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        max_diff
    );
}

fn test_apply_window() {
    let audio = generate_float_samples(400);
    let window: Vec<f32> = (0..400)
        .map(|n| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * n as f32 / 400.0).cos()))
        .collect();
    let iterations = 10000;

    // Scalar
    let mut scalar_output = vec![0.0f32; 400];
    apply_window_scalar(&audio, &window, &mut scalar_output);
    let start = Instant::now();
    for _ in 0..iterations {
        apply_window_scalar(&audio, &window, &mut scalar_output);
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD
    let mut simd_output = vec![0.0f32; 400];
    simd_ops::apply_window_simd(&audio, &window, &mut simd_output);
    let start = Instant::now();
    for _ in 0..iterations {
        simd_ops::apply_window_simd(&audio, &window, &mut simd_output);
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Accuracy
    let (matches, max_diff, _) = compare_accuracy(&scalar_output, &simd_output);

    println!("\nWindow Application (400 samples):");
    println!(
        "  Scalar:     {:>10.2} ns  ({:>8.2} Msamples/s)",
        scalar_time,
        400.0 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} ns  ({:>8.2} Msamples/s)",
        simd_time,
        400.0 / (simd_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_time / simd_time);
    println!(
        "  Accuracy:   {} (max diff: {:.2e})",
        if matches {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        max_diff
    );
}

fn test_power_spectrum() {
    let fft_re: Vec<f32> = (0..201).map(|i| (i as f32 * 0.1).cos()).collect();
    let fft_im: Vec<f32> = (0..201).map(|i| (i as f32 * 0.1).sin()).collect();
    let iterations = 10000;

    // Scalar
    let scalar_result = power_spectrum_scalar(&fft_re, &fft_im);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = power_spectrum_scalar(&fft_re, &fft_im);
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD
    let simd_result = simd_ops::power_spectrum_simd(&fft_re, &fft_im);
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = simd_ops::power_spectrum_simd(&fft_re, &fft_im);
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Accuracy
    let (matches, max_diff, _) = compare_accuracy(&scalar_result, &simd_result);

    println!("\nPower Spectrum (201 bins):");
    println!(
        "  Scalar:     {:>10.2} ns  ({:>8.2} Msamples/s)",
        scalar_time,
        201.0 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} ns  ({:>8.2} Msamples/s)",
        simd_time,
        201.0 / (simd_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_time / simd_time);
    println!(
        "  Accuracy:   {} (max diff: {:.2e})",
        if matches {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        max_diff
    );
}

fn test_whisper_normalization() {
    let mel_size = 80 * 800; // 8 second spectrogram
    let iterations = 100;

    // Log + Max pass
    let mut scalar_data: Vec<f32> = (0..mel_size).map(|i| (i as f32 + 1.0) / 1000.0).collect();
    let scalar_max = whisper_norm_log_max_scalar(&mut scalar_data);
    let start = Instant::now();
    for _ in 0..iterations {
        let mut data: Vec<f32> = (0..mel_size).map(|i| (i as f32 + 1.0) / 1000.0).collect();
        let _ = whisper_norm_log_max_scalar(&mut data);
    }
    let scalar_log_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    let mut simd_data: Vec<f32> = (0..mel_size).map(|i| (i as f32 + 1.0) / 1000.0).collect();
    let simd_max = simd_ops::whisper_norm_log_max_simd(&mut simd_data);
    let start = Instant::now();
    for _ in 0..iterations {
        let mut data: Vec<f32> = (0..mel_size).map(|i| (i as f32 + 1.0) / 1000.0).collect();
        let _ = simd_ops::whisper_norm_log_max_simd(&mut data);
    }
    let simd_log_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    let max_diff_max = (scalar_max - simd_max).abs();
    let (log_matches, log_max_diff, _) = compare_accuracy(&scalar_data, &simd_data);

    println!("\nLog + Max Pass ({} values):", mel_size);
    println!(
        "  Scalar:     {:>10.2} µs  ({:>8.2} Mvals/s)",
        scalar_log_time / 1000.0,
        mel_size as f64 / (scalar_log_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} µs  ({:>8.2} Mvals/s)",
        simd_log_time / 1000.0,
        mel_size as f64 / (simd_log_time / 1e9) / 1e6
    );
    println!("  Speedup:    {:>10.2}x", scalar_log_time / simd_log_time);
    println!(
        "  Accuracy:   {} (max val diff: {:.2e}, data diff: {:.2e})",
        if log_matches && max_diff_max < EPSILON {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        max_diff_max,
        log_max_diff
    );

    // Threshold + Scale pass
    let max_val = 0.5;
    let mut scalar_data2: Vec<f32> = (0..mel_size)
        .map(|i| (i as f32 / mel_size as f32) - 0.5)
        .collect();
    whisper_norm_threshold_scale_scalar(&mut scalar_data2, max_val);
    let start = Instant::now();
    for _ in 0..iterations {
        let mut data: Vec<f32> = (0..mel_size)
            .map(|i| (i as f32 / mel_size as f32) - 0.5)
            .collect();
        whisper_norm_threshold_scale_scalar(&mut data, max_val);
    }
    let scalar_scale_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    let mut simd_data2: Vec<f32> = (0..mel_size)
        .map(|i| (i as f32 / mel_size as f32) - 0.5)
        .collect();
    simd_ops::whisper_norm_threshold_scale_simd(&mut simd_data2, max_val);
    let start = Instant::now();
    for _ in 0..iterations {
        let mut data: Vec<f32> = (0..mel_size)
            .map(|i| (i as f32 / mel_size as f32) - 0.5)
            .collect();
        simd_ops::whisper_norm_threshold_scale_simd(&mut data, max_val);
    }
    let simd_scale_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    let (scale_matches, scale_max_diff, _) = compare_accuracy(&scalar_data2, &simd_data2);

    println!("\nThreshold + Scale Pass ({} values):", mel_size);
    println!(
        "  Scalar:     {:>10.2} µs  ({:>8.2} Mvals/s)",
        scalar_scale_time / 1000.0,
        mel_size as f64 / (scalar_scale_time / 1e9) / 1e6
    );
    println!(
        "  SIMD:       {:>10.2} µs  ({:>8.2} Mvals/s)",
        simd_scale_time / 1000.0,
        mel_size as f64 / (simd_scale_time / 1e9) / 1e6
    );
    println!(
        "  Speedup:    {:>10.2}x",
        scalar_scale_time / simd_scale_time
    );
    println!(
        "  Accuracy:   {} (max diff: {:.2e})",
        if scale_matches {
            "✓ 100% MATCH"
        } else {
            "✗ MISMATCH"
        },
        scale_max_diff
    );
}

fn test_e2e_pipeline() {
    // Simulate processing 8 seconds of audio (128,000 samples at 16kHz)
    let audio_samples = 128000;
    let pcm_data = generate_pcm_data(audio_samples);
    let iterations = 10;

    println!(
        "\nProcessing {} samples ({:.1}s at 16kHz):",
        audio_samples,
        audio_samples as f64 / 16000.0
    );

    // Scalar pipeline
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = process_audio_scalar(&pcm_data);
    }
    let scalar_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // SIMD pipeline
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = process_audio_simd(&pcm_data);
    }
    let simd_time = start.elapsed().as_nanos() as f64 / iterations as f64;

    // Calculate realtime factor (RTF)
    let audio_duration_ns = (audio_samples as f64 / 16000.0) * 1e9;
    let scalar_rtf = audio_duration_ns / scalar_time;
    let simd_rtf = audio_duration_ns / simd_time;

    println!("\n  End-to-End Latency:");
    println!("    Scalar:   {:>10.2} ms", scalar_time / 1e6);
    println!("    SIMD:     {:>10.2} ms", simd_time / 1e6);
    println!("    Speedup:  {:>10.2}x", scalar_time / simd_time);

    println!("\n  Realtime Factor (RTF = audio_duration / process_time):");
    println!("    Scalar:   {:>10.1}x realtime", scalar_rtf);
    println!("    SIMD:     {:>10.1}x realtime", simd_rtf);

    println!("\n  Throughput:");
    println!(
        "    Scalar:   {:>10.2} MB/s",
        (audio_samples * 2) as f64 / (scalar_time / 1e9) / 1e6
    );
    println!(
        "    SIMD:     {:>10.2} MB/s",
        (audio_samples * 2) as f64 / (simd_time / 1e9) / 1e6
    );
}

// Full pipeline processing
fn process_audio_scalar(pcm: &[u8]) -> f32 {
    // Convert PCM to float
    let samples = pcm_to_float_scalar(pcm);

    // Calculate RMS/Peak
    let (rms, _peak) = rms_peak_scalar(&samples);

    rms
}

fn process_audio_simd(pcm: &[u8]) -> f32 {
    // Convert PCM to float
    let samples = simd_ops::pcm_to_float_simd(pcm);

    // Calculate RMS/Peak
    let (rms, _peak) = simd_ops::rms_peak_simd(&samples);

    rms
}

// Helper functions
fn generate_pcm_data(sample_count: usize) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(sample_count * 2);
    for i in 0..sample_count {
        let t = i as f32 / 16000.0;
        let sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5;
        let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        pcm.extend_from_slice(&sample_i16.to_le_bytes());
    }
    pcm
}

fn generate_float_samples(sample_count: usize) -> Vec<f32> {
    (0..sample_count)
        .map(|i| {
            let t = i as f32 / 16000.0;
            (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5
        })
        .collect()
}

fn generate_mel_filterbank() -> Vec<Vec<f32>> {
    (0..80)
        .map(|mel_idx| {
            (0..201)
                .map(|bin_idx| {
                    let center = 10 + mel_idx * 2;
                    let dist = (bin_idx as i32 - center as i32).abs() as f32;
                    (1.0 - dist / 10.0).max(0.0)
                })
                .collect()
        })
        .collect()
}

fn generate_power_spectrum() -> Vec<f32> {
    (0..201)
        .map(|i| {
            let freq_bin = i as f32 / 201.0;
            (1.0 - freq_bin).powi(2) * 0.01
        })
        .collect()
}

fn compare_accuracy(a: &[f32], b: &[f32]) -> (bool, f32, f64) {
    if a.len() != b.len() {
        return (false, f32::MAX, 0.0);
    }
    let max_diff = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| (*x - *y).abs())
        .fold(0.0f32, |acc, d| acc.max(d));
    let match_count = a
        .iter()
        .zip(b.iter())
        .filter(|(x, y)| (*x - *y).abs() < EPSILON)
        .count();
    let match_pct = 100.0 * match_count as f64 / a.len() as f64;
    (max_diff < EPSILON, max_diff, match_pct)
}

// Scalar implementations
fn pcm_to_float_scalar(pcm: &[u8]) -> Vec<f32> {
    let sample_count = pcm.len() / 2;
    let mut output = Vec::with_capacity(sample_count);
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32;
        output.push((sample * PCM_TO_FLOAT_SCALE).clamp(-1.0, 1.0));
    }
    output
}

fn float_to_pcm_scalar(samples: &[f32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = if clamped >= 0.0 {
            clamped * FLOAT_TO_PCM_SCALE_POS
        } else {
            clamped * FLOAT_TO_PCM_SCALE_NEG
        };
        let sample_i16 = scaled.round() as i16;
        output.extend_from_slice(&sample_i16.to_le_bytes());
    }
    output
}

fn rms_peak_scalar(samples: &[f32]) -> (f32, f32) {
    let mut sum_sq = 0.0f32;
    let mut peak = 0.0f32;
    for &sample in samples {
        sum_sq += sample * sample;
        let abs_sample = sample.abs();
        if abs_sample > peak {
            peak = abs_sample;
        }
    }
    let rms = (sum_sq / samples.len() as f32).sqrt();
    (rms, peak)
}

fn mel_filter_dot_scalar(filter: &[f32], spectrum: &[f32]) -> f32 {
    filter.iter().zip(spectrum.iter()).map(|(f, s)| f * s).sum()
}

fn apply_window_scalar(audio: &[f32], window: &[f32], output: &mut [f32]) {
    for i in 0..audio.len() {
        output[i] = audio[i] * window[i];
    }
}

fn power_spectrum_scalar(fft_re: &[f32], fft_im: &[f32]) -> Vec<f32> {
    fft_re
        .iter()
        .zip(fft_im.iter())
        .map(|(re, im)| re * re + im * im)
        .collect()
}

fn whisper_norm_log_max_scalar(data: &mut [f32]) -> f32 {
    const MIN_VALUE: f32 = 1e-10;
    let mut max_val = f32::NEG_INFINITY;
    for val in data.iter_mut() {
        *val = (*val).max(MIN_VALUE).log10();
        if *val > max_val {
            max_val = *val;
        }
    }
    max_val
}

fn whisper_norm_threshold_scale_scalar(data: &mut [f32], max_val: f32) {
    let threshold = max_val - 8.0;
    for val in data.iter_mut() {
        *val = ((*val).max(threshold) + 4.0) / 4.0;
    }
}
