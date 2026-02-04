//! SIMD Performance Benchmarks for Audio Processing
//!
//! These benchmarks measure the performance of SIMD-optimized audio processing
//! operations compared to baseline implementations.
//!
//! Run with: cargo bench --features smart-turn,silero-vad -- simd
//!
//! ## Benchmark Categories
//!
//! - PCM conversion: i16 <-> f32 conversion
//! - Energy calculation: RMS and peak detection
//! - Mel spectrogram: filterbank multiplication, power spectrum
//! - Whisper normalization: log + threshold + scale operations

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::f32::consts::PI;
use std::time::Duration;
use waav_gateway::utils::simd_ops;

// =============================================================================
// Test data generators
// =============================================================================

/// Generate test PCM data (16-bit little-endian samples)
fn generate_pcm_data(sample_count: usize) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(sample_count * 2);
    for i in 0..sample_count {
        // Generate sine wave with some harmonic content
        let t = i as f32 / 16000.0;
        let sample = (2.0 * PI * 440.0 * t).sin() * 0.5
            + (2.0 * PI * 880.0 * t).sin() * 0.25
            + (2.0 * PI * 1320.0 * t).sin() * 0.125;
        let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        pcm.extend_from_slice(&sample_i16.to_le_bytes());
    }
    pcm
}

/// Generate test f32 audio samples
fn generate_float_samples(sample_count: usize) -> Vec<f32> {
    (0..sample_count)
        .map(|i| {
            let t = i as f32 / 16000.0;
            (2.0 * PI * 440.0 * t).sin() * 0.5
        })
        .collect()
}

/// Generate mel filterbank (80 filters x 201 bins)
fn generate_mel_filterbank() -> Vec<Vec<f32>> {
    (0..80)
        .map(|mel_idx| {
            (0..201)
                .map(|bin_idx| {
                    // Triangular filter response
                    let center = 10 + mel_idx * 2;
                    let dist = (bin_idx as i32 - center as i32).abs() as f32;
                    (1.0 - dist / 10.0).max(0.0)
                })
                .collect()
        })
        .collect()
}

/// Generate power spectrum (201 bins)
fn generate_power_spectrum() -> Vec<f32> {
    (0..201)
        .map(|i| {
            // Simulate speech-like spectrum (higher energy at lower frequencies)
            let freq_bin = i as f32 / 201.0;
            (1.0 - freq_bin).powi(2) * 0.01
        })
        .collect()
}

/// Scalar implementation for comparison
mod scalar {
    use super::*;

    pub fn pcm_to_float(pcm: &[u8]) -> Vec<f32> {
        const PCM_TO_FLOAT_SCALE: f32 = 1.0 / 32768.0;
        let sample_count = pcm.len() / 2;
        let mut output = Vec::with_capacity(sample_count);
        for chunk in pcm.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as f32;
            output.push((sample * PCM_TO_FLOAT_SCALE).clamp(-1.0, 1.0));
        }
        output
    }

    pub fn float_to_pcm(samples: &[f32]) -> Vec<u8> {
        const FLOAT_TO_PCM_SCALE_POS: f32 = 32767.0;
        const FLOAT_TO_PCM_SCALE_NEG: f32 = 32768.0;
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

    pub fn rms_peak(samples: &[f32]) -> (f32, f32) {
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

    pub fn mel_filter_dot(filter: &[f32], spectrum: &[f32]) -> f32 {
        filter.iter().zip(spectrum.iter()).map(|(f, s)| f * s).sum()
    }

    pub fn apply_window(audio: &[f32], window: &[f32], output: &mut [f32]) {
        for i in 0..audio.len() {
            output[i] = audio[i] * window[i];
        }
    }

    pub fn power_spectrum(fft_re: &[f32], fft_im: &[f32]) -> Vec<f32> {
        fft_re
            .iter()
            .zip(fft_im.iter())
            .map(|(re, im)| re * re + im * im)
            .collect()
    }

    pub fn whisper_norm_log_max(data: &mut [f32]) -> f32 {
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

    pub fn whisper_norm_threshold_scale(data: &mut [f32], max_val: f32) {
        let threshold = max_val - 8.0;
        for val in data.iter_mut() {
            *val = ((*val).max(threshold) + 4.0) / 4.0;
        }
    }
}

// =============================================================================
// PCM Conversion Benchmarks
// =============================================================================

fn bench_pcm_to_float(c: &mut Criterion) {
    let mut group = c.benchmark_group("pcm_to_float");
    group.measurement_time(Duration::from_secs(5));

    // Test different buffer sizes representing typical audio chunks
    let sizes = [
        (512, "512_samples_32ms"),     // VAD chunk size
        (1600, "1600_samples_100ms"),  // 100ms chunk
        (16000, "16000_samples_1s"),   // 1 second
        (128000, "128000_samples_8s"), // 8 seconds (smart-turn window)
    ];

    for (sample_count, name) in sizes {
        let pcm = generate_pcm_data(sample_count);

        group.throughput(Throughput::Bytes(pcm.len() as u64));

        group.bench_with_input(BenchmarkId::new("scalar", name), &pcm, |b, pcm| {
            b.iter(|| scalar::pcm_to_float(black_box(pcm)));
        });

        group.bench_with_input(BenchmarkId::new("simd", name), &pcm, |b, pcm| {
            b.iter(|| simd_ops::pcm_to_float_simd(black_box(pcm)));
        });
    }

    group.finish();
}

fn bench_float_to_pcm(c: &mut Criterion) {
    let mut group = c.benchmark_group("float_to_pcm");
    group.measurement_time(Duration::from_secs(5));

    let sizes = [
        (512, "512_samples_32ms"),
        (1600, "1600_samples_100ms"),
        (16000, "16000_samples_1s"),
        (128000, "128000_samples_8s"),
    ];

    for (sample_count, name) in sizes {
        let samples = generate_float_samples(sample_count);

        group.throughput(Throughput::Elements(sample_count as u64));

        group.bench_with_input(BenchmarkId::new("scalar", name), &samples, |b, samples| {
            b.iter(|| scalar::float_to_pcm(black_box(samples)));
        });

        group.bench_with_input(BenchmarkId::new("simd", name), &samples, |b, samples| {
            b.iter(|| simd_ops::float_to_pcm_simd(black_box(samples)));
        });
    }

    group.finish();
}

// =============================================================================
// Energy Calculation Benchmarks
// =============================================================================

fn bench_rms_peak(c: &mut Criterion) {
    let mut group = c.benchmark_group("rms_peak");
    group.measurement_time(Duration::from_secs(5));

    let sizes = [
        (512, "512_samples"),
        (1600, "1600_samples"),
        (16000, "16000_samples"),
    ];

    for (sample_count, name) in sizes {
        let samples = generate_float_samples(sample_count);

        group.throughput(Throughput::Elements(sample_count as u64));

        group.bench_with_input(BenchmarkId::new("scalar", name), &samples, |b, samples| {
            b.iter(|| scalar::rms_peak(black_box(samples)));
        });

        group.bench_with_input(BenchmarkId::new("simd", name), &samples, |b, samples| {
            b.iter(|| simd_ops::rms_peak_simd(black_box(samples)));
        });
    }

    group.finish();
}

// =============================================================================
// Mel Spectrogram Benchmarks
// =============================================================================

fn bench_mel_filter_dot(c: &mut Criterion) {
    let mut group = c.benchmark_group("mel_filter_dot");
    group.measurement_time(Duration::from_secs(5));

    let filterbank = generate_mel_filterbank();
    let spectrum = generate_power_spectrum();

    // Benchmark single filter dot product (201 bins)
    group.throughput(Throughput::Elements(201));

    group.bench_function("scalar_single_filter", |b| {
        b.iter(|| scalar::mel_filter_dot(black_box(&filterbank[40]), black_box(&spectrum)));
    });

    group.bench_function("simd_single_filter", |b| {
        b.iter(|| simd_ops::mel_filter_dot_simd(black_box(&filterbank[40]), black_box(&spectrum)));
    });

    // Benchmark full filterbank (80 filters x 201 bins = 16,080 ops)
    group.throughput(Throughput::Elements(80 * 201));

    group.bench_function("scalar_full_filterbank", |b| {
        b.iter(|| {
            let mut energies = [0.0f32; 80];
            for (i, filter) in filterbank.iter().enumerate() {
                energies[i] = scalar::mel_filter_dot(black_box(filter), black_box(&spectrum));
            }
            energies
        });
    });

    group.bench_function("simd_full_filterbank", |b| {
        b.iter(|| {
            let mut energies = [0.0f32; 80];
            for (i, filter) in filterbank.iter().enumerate() {
                energies[i] =
                    simd_ops::mel_filter_dot_simd(black_box(filter), black_box(&spectrum));
            }
            energies
        });
    });

    group.finish();
}

fn bench_apply_window(c: &mut Criterion) {
    let mut group = c.benchmark_group("apply_window");
    group.measurement_time(Duration::from_secs(5));

    // N_FFT = 400 for Whisper
    let audio = generate_float_samples(400);
    let window: Vec<f32> = (0..400)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / 400.0).cos()))
        .collect();
    let mut output_scalar = vec![0.0f32; 400];
    let mut output_simd = vec![0.0f32; 400];

    group.throughput(Throughput::Elements(400));

    group.bench_function("scalar", |b| {
        b.iter(|| {
            scalar::apply_window(black_box(&audio), black_box(&window), &mut output_scalar);
        });
    });

    group.bench_function("simd", |b| {
        b.iter(|| {
            simd_ops::apply_window_simd(black_box(&audio), black_box(&window), &mut output_simd);
        });
    });

    group.finish();
}

fn bench_power_spectrum(c: &mut Criterion) {
    let mut group = c.benchmark_group("power_spectrum");
    group.measurement_time(Duration::from_secs(5));

    // N_FFT/2+1 = 201 bins for Whisper
    let fft_re: Vec<f32> = (0..201).map(|i| (i as f32 * 0.1).cos()).collect();
    let fft_im: Vec<f32> = (0..201).map(|i| (i as f32 * 0.1).sin()).collect();

    group.throughput(Throughput::Elements(201));

    group.bench_function("scalar", |b| {
        b.iter(|| scalar::power_spectrum(black_box(&fft_re), black_box(&fft_im)));
    });

    group.bench_function("simd", |b| {
        b.iter(|| simd_ops::power_spectrum_simd(black_box(&fft_re), black_box(&fft_im)));
    });

    group.finish();
}

// =============================================================================
// Whisper Normalization Benchmarks
// =============================================================================

fn bench_whisper_normalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("whisper_normalization");
    group.measurement_time(Duration::from_secs(5));

    // 80 mels x 800 frames = 64,000 values (8 second mel spectrogram)
    let mel_size = 80 * 800;

    group.throughput(Throughput::Elements(mel_size as u64));

    // Benchmark log + max pass
    group.bench_function("scalar_log_max", |b| {
        let mut data: Vec<f32> = (0..mel_size).map(|i| (i as f32 + 1.0) / 1000.0).collect();
        b.iter(|| {
            let _ = scalar::whisper_norm_log_max(black_box(&mut data));
        });
    });

    group.bench_function("simd_log_max", |b| {
        let mut data: Vec<f32> = (0..mel_size).map(|i| (i as f32 + 1.0) / 1000.0).collect();
        b.iter(|| {
            let _ = simd_ops::whisper_norm_log_max_simd(black_box(&mut data));
        });
    });

    // Benchmark threshold + scale pass
    group.bench_function("scalar_threshold_scale", |b| {
        let mut data: Vec<f32> = (0..mel_size)
            .map(|i| (i as f32 / mel_size as f32) - 0.5)
            .collect();
        let max_val = 0.5;
        b.iter(|| {
            scalar::whisper_norm_threshold_scale(black_box(&mut data), black_box(max_val));
        });
    });

    group.bench_function("simd_threshold_scale", |b| {
        let mut data: Vec<f32> = (0..mel_size)
            .map(|i| (i as f32 / mel_size as f32) - 0.5)
            .collect();
        let max_val = 0.5;
        b.iter(|| {
            simd_ops::whisper_norm_threshold_scale_simd(black_box(&mut data), black_box(max_val));
        });
    });

    group.finish();
}

// =============================================================================
// End-to-End Pipeline Benchmarks
// =============================================================================

fn bench_e2e_mel_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("e2e_mel_frame");
    group.measurement_time(Duration::from_secs(5));

    // Single mel frame processing (400 samples -> 80 mel bins)
    let audio = generate_float_samples(400);
    let window: Vec<f32> = (0..400)
        .map(|n| 0.5 * (1.0 - (2.0 * PI * n as f32 / 400.0).cos()))
        .collect();
    let filterbank = generate_mel_filterbank();

    group.throughput(Throughput::Elements(1)); // 1 frame

    group.bench_function("scalar", |b| {
        b.iter(|| {
            // Window application
            let mut windowed = vec![0.0f32; 400];
            scalar::apply_window(&audio, &window, &mut windowed);

            // Simulated power spectrum (in practice this is FFT output)
            let fft_re: Vec<f32> = windowed.iter().take(201).copied().collect();
            let fft_im: Vec<f32> = windowed.iter().skip(200).take(201).map(|_| 0.0).collect();
            let power = scalar::power_spectrum(&fft_re, &fft_im);

            // Mel filterbank
            let mut mel_energies = [0.0f32; 80];
            for (i, filter) in filterbank.iter().enumerate() {
                mel_energies[i] = scalar::mel_filter_dot(filter, &power);
            }

            mel_energies
        });
    });

    group.bench_function("simd", |b| {
        b.iter(|| {
            // Window application
            let mut windowed = vec![0.0f32; 400];
            simd_ops::apply_window_simd(&audio, &window, &mut windowed);

            // Simulated power spectrum
            let fft_re: Vec<f32> = windowed.iter().take(201).copied().collect();
            let fft_im: Vec<f32> = windowed.iter().skip(200).take(201).map(|_| 0.0).collect();
            let power = simd_ops::power_spectrum_simd(&fft_re, &fft_im);

            // Mel filterbank
            let mut mel_energies = [0.0f32; 80];
            for (i, filter) in filterbank.iter().enumerate() {
                mel_energies[i] = simd_ops::mel_filter_dot_simd(filter, &power);
            }

            mel_energies
        });
    });

    group.finish();
}

// =============================================================================
// Aligned Buffer Benchmarks
// =============================================================================

fn bench_aligned_buffer(c: &mut Criterion) {
    let mut group = c.benchmark_group("aligned_buffer");
    group.measurement_time(Duration::from_secs(5));

    let sizes = [512, 1024, 4096, 16384];

    for size in sizes {
        let data = generate_float_samples(size);

        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::new("vec_copy", size), &data, |b, data| {
            let mut dst = vec![0.0f32; size];
            b.iter(|| {
                dst.copy_from_slice(black_box(data));
            });
        });

        group.bench_with_input(BenchmarkId::new("aligned_copy", size), &data, |b, data| {
            let mut dst = simd_ops::AlignedBuffer::<f32>::new(size);
            b.iter(|| {
                dst.copy_from_slice(black_box(data));
            });
        });
    }

    group.finish();
}

criterion_group!(
    simd_benches,
    bench_pcm_to_float,
    bench_float_to_pcm,
    bench_rms_peak,
    bench_mel_filter_dot,
    bench_apply_window,
    bench_power_spectrum,
    bench_whisper_normalization,
    bench_e2e_mel_frame,
    bench_aligned_buffer,
);

criterion_main!(simd_benches);
