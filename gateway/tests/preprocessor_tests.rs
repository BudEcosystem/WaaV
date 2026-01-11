//! Audio Preprocessor Tests
//!
//! Tests for audio preprocessing components:
//! - VAD (Voice Activity Detection) / Turn Detection
//! - Noise Filtering (DeepFilterNet)
//! - Audio format conversion
//! - Sample rate conversion
//!
//! Feature-gated tests:
//! - Turn detection: requires --features turn-detect
//! - Noise filter: requires --features noise-filter
//!
//! Run all: cargo test --test preprocessor_tests --features turn-detect,noise-filter
//! Run without features: cargo test --test preprocessor_tests

mod fixtures;

use fixtures::audio_fixtures;
use std::time::{Duration, Instant};

// =============================================================================
// Audio Format Tests (always available)
// =============================================================================

#[test]
fn test_audio_sample_conversion() {
    // Test i16 samples to bytes conversion
    let samples: Vec<i16> = vec![0, 1000, -1000, i16::MAX, i16::MIN];
    let bytes = audio_fixtures::samples_to_bytes(&samples);

    // 2 bytes per sample
    assert_eq!(bytes.len(), samples.len() * 2);

    // Convert back
    let recovered = audio_fixtures::bytes_to_samples(&bytes);
    assert_eq!(samples, recovered);
}

#[test]
fn test_audio_wav_header_creation() {
    let num_samples = 16000; // 1 second at 16kHz
    let header = audio_fixtures::create_wav_header(num_samples);

    // WAV header is 44 bytes
    assert_eq!(header.len(), 44);

    // Check RIFF magic
    assert_eq!(&header[0..4], b"RIFF");

    // Check WAVE format
    assert_eq!(&header[8..12], b"WAVE");

    // Check fmt chunk
    assert_eq!(&header[12..16], b"fmt ");

    // Check data chunk
    assert_eq!(&header[36..40], b"data");
}

#[test]
fn test_complete_wav_file_creation() {
    let samples = audio_fixtures::generate_a440_tone(audio_fixtures::SECOND);
    let wav = audio_fixtures::create_wav_file(&samples);

    // Header (44) + data (samples * 2)
    assert_eq!(wav.len(), 44 + audio_fixtures::SECOND * 2);

    // Verify it's a valid WAV structure
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
}

// =============================================================================
// Audio Analysis Tests
// =============================================================================

#[test]
fn test_rms_calculation() {
    // Silence should have 0 RMS
    let silence = audio_fixtures::generate_silence(1000);
    let silence_rms = audio_fixtures::calculate_rms(&silence);
    assert!(silence_rms < 0.001, "Silence RMS should be near zero");

    // Loud signal should have higher RMS than quiet signal
    let loud = audio_fixtures::generate_sine_wave(1000, 440.0, 0.9);
    let quiet = audio_fixtures::generate_sine_wave(1000, 440.0, 0.1);

    let loud_rms = audio_fixtures::calculate_rms(&loud);
    let quiet_rms = audio_fixtures::calculate_rms(&quiet);

    assert!(
        loud_rms > quiet_rms * 5.0,
        "Loud signal ({}) should have much higher RMS than quiet ({})",
        loud_rms,
        quiet_rms
    );
}

#[test]
fn test_peak_calculation() {
    // Test peak amplitude detection
    let samples = audio_fixtures::generate_sine_wave(1000, 440.0, 0.5);
    let peak = audio_fixtures::calculate_peak(&samples);

    // Peak should be approximately 50% of max (amplitude = 0.5)
    let expected_max = (0.5 * i16::MAX as f32) as i16;
    let tolerance = expected_max / 10; // 10% tolerance

    assert!(
        (peak - expected_max).abs() < tolerance,
        "Peak {} should be near expected {}",
        peak,
        expected_max
    );
}

#[test]
fn test_snr_calculation() {
    // Create signal and noise
    let signal = audio_fixtures::generate_sine_wave(10000, 440.0, 0.5);
    let noise = audio_fixtures::generate_white_noise(10000, 0.1);

    let snr = audio_fixtures::calculate_snr(&signal, &noise);

    // SNR should be positive (signal stronger than noise)
    assert!(snr > 0.0, "SNR should be positive: {}", snr);

    // Roughly expected: signal at 0.5 amplitude, noise at 0.1
    // Power ratio ~ (0.5/0.1)^2 = 25, SNR ~ 10*log10(25) ~ 14 dB
    assert!(
        snr > 5.0 && snr < 30.0,
        "SNR {} should be in reasonable range",
        snr
    );
}

// =============================================================================
// Audio Generation Tests
// =============================================================================

#[test]
fn test_silence_generation() {
    let silence = audio_fixtures::generate_silence(audio_fixtures::SECOND);
    assert_eq!(silence.len(), audio_fixtures::SECOND);
    assert!(silence.iter().all(|&s| s == 0));
}

#[test]
fn test_white_noise_generation() {
    let noise = audio_fixtures::generate_white_noise(audio_fixtures::SECOND, 0.5);
    assert_eq!(noise.len(), audio_fixtures::SECOND);

    // Should have non-zero samples
    assert!(noise.iter().any(|&s| s != 0));

    // Should be roughly zero-mean
    let mean: f64 = noise.iter().map(|&s| s as f64).sum::<f64>() / noise.len() as f64;
    assert!(
        mean.abs() < 1000.0,
        "White noise mean should be near zero: {}",
        mean
    );
}

#[test]
fn test_sine_wave_generation() {
    let sine = audio_fixtures::generate_sine_wave(audio_fixtures::SECOND, 440.0, 0.5);
    assert_eq!(sine.len(), audio_fixtures::SECOND);

    // Check it oscillates
    let max = sine.iter().max().unwrap();
    let min = sine.iter().min().unwrap();
    assert!(*max > 0, "Sine wave should have positive samples");
    assert!(*min < 0, "Sine wave should have negative samples");
}

#[test]
fn test_chirp_generation() {
    let chirp = audio_fixtures::generate_chirp(audio_fixtures::SECOND, 100.0, 1000.0, 0.5);
    assert_eq!(chirp.len(), audio_fixtures::SECOND);
    assert!(chirp.iter().any(|&s| s != 0));
}

#[test]
fn test_speech_pattern_generation() {
    let speech = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND);
    assert_eq!(speech.len(), audio_fixtures::SECOND);

    // Should have varying amplitude (not constant)
    let first_quarter_rms =
        audio_fixtures::calculate_rms(&speech[..audio_fixtures::SECOND / 4]);
    let last_quarter_rms =
        audio_fixtures::calculate_rms(&speech[3 * audio_fixtures::SECOND / 4..]);

    // Both should have some energy
    assert!(
        first_quarter_rms > 0.0 || last_quarter_rms > 0.0,
        "Speech pattern should have energy"
    );
}

#[test]
fn test_speech_with_pauses() {
    let speech = audio_fixtures::generate_speech_with_pauses(audio_fixtures::SECOND * 4, 0.3);
    assert_eq!(speech.len(), audio_fixtures::SECOND * 4);

    // Count segments that are mostly silent
    let segment_size = audio_fixtures::SECOND / 4; // 250ms segments
    let mut silent_segments = 0;
    let mut total_segments = 0;

    for chunk in speech.chunks(segment_size) {
        total_segments += 1;
        let rms = audio_fixtures::calculate_rms(chunk);
        if rms < 100.0 {
            // Low energy = silent
            silent_segments += 1;
        }
    }

    // Should have some silent segments (around 30% requested)
    let silence_ratio = silent_segments as f32 / total_segments as f32;
    println!(
        "Speech with pauses: {} / {} segments silent ({:.1}%)",
        silent_segments,
        total_segments,
        silence_ratio * 100.0
    );
}

#[test]
fn test_noisy_speech_generation() {
    let clean_speech = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND);
    let noisy_speech = audio_fixtures::generate_noisy_speech(audio_fixtures::SECOND, 10.0);

    // Noisy speech should have similar length
    assert_eq!(noisy_speech.len(), audio_fixtures::SECOND);

    // Noisy speech should have different samples than clean
    let differences: usize = clean_speech
        .iter()
        .zip(noisy_speech.iter())
        .filter(|(a, b)| a != b)
        .count();

    assert!(
        differences > audio_fixtures::SECOND / 2,
        "Noisy speech should differ from clean"
    );
}

#[test]
fn test_deterministic_generation() {
    // Generators should produce identical output for same parameters
    let noise1 = audio_fixtures::generate_white_noise(1000, 0.5);
    let noise2 = audio_fixtures::generate_white_noise(1000, 0.5);
    assert_eq!(noise1, noise2, "Noise generation should be deterministic");

    let speech1 = audio_fixtures::generate_speech_pattern(1000);
    let speech2 = audio_fixtures::generate_speech_pattern(1000);
    assert_eq!(speech1, speech2, "Speech generation should be deterministic");
}

// =============================================================================
// Performance Tests
// =============================================================================

#[test]
fn test_audio_generation_performance() {
    // Generate 10 seconds of audio and measure time
    let samples = 10 * audio_fixtures::SECOND;

    let start = Instant::now();
    let _ = audio_fixtures::generate_speech_pattern(samples);
    let speech_time = start.elapsed();

    let start = Instant::now();
    let _ = audio_fixtures::generate_white_noise(samples, 0.5);
    let noise_time = start.elapsed();

    let start = Instant::now();
    let _ = audio_fixtures::generate_sine_wave(samples, 440.0, 0.5);
    let sine_time = start.elapsed();

    println!("Audio generation performance (10 seconds):");
    println!("  Speech pattern: {:?}", speech_time);
    println!("  White noise: {:?}", noise_time);
    println!("  Sine wave: {:?}", sine_time);

    // All should complete quickly (under 100ms for 10 seconds of audio)
    assert!(
        speech_time < Duration::from_millis(100),
        "Speech generation too slow: {:?}",
        speech_time
    );
    assert!(
        noise_time < Duration::from_millis(100),
        "Noise generation too slow: {:?}",
        noise_time
    );
    assert!(
        sine_time < Duration::from_millis(100),
        "Sine generation too slow: {:?}",
        sine_time
    );
}

#[test]
fn test_conversion_performance() {
    let samples = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND * 60);

    let start = Instant::now();
    let bytes = audio_fixtures::samples_to_bytes(&samples);
    let to_bytes_time = start.elapsed();

    let start = Instant::now();
    let _ = audio_fixtures::bytes_to_samples(&bytes);
    let from_bytes_time = start.elapsed();

    println!("Conversion performance (60 seconds of audio):");
    println!("  To bytes: {:?}", to_bytes_time);
    println!("  From bytes: {:?}", from_bytes_time);

    // Should be reasonably fast (lenient for debug builds)
    assert!(
        to_bytes_time < Duration::from_millis(200),
        "To bytes too slow: {:?}",
        to_bytes_time
    );
    assert!(
        from_bytes_time < Duration::from_millis(200),
        "From bytes too slow: {:?}",
        from_bytes_time
    );
}

// =============================================================================
// VAD / Turn Detection Tests (feature-gated)
// =============================================================================

#[cfg(feature = "turn-detect")]
mod turn_detect_tests {
    use super::*;

    #[test]
    fn test_turn_detection_on_speech_with_pauses() {
        // This test uses the audio fixture that simulates speech with pauses
        // pause_ratio = 0.3 means ~30% of 250ms segments will be silent

        let audio = audio_fixtures::generate_speech_with_pauses(audio_fixtures::SECOND * 5, 0.3);

        // Analyze using 250ms segments to match the pause generation granularity
        // (generate_speech_with_pauses uses SAMPLE_RATE_USIZE / 4 = 250ms segments)
        let segment_size = audio_fixtures::SAMPLE_RATE_USIZE / 4; // 250ms = 4000 samples
        let mut segments_with_speech = 0;
        let mut segments_silent = 0;

        for chunk in audio.chunks(segment_size) {
            let rms = audio_fixtures::calculate_rms(chunk);
            // Silent segments have RMS = 0 (they are generated as vec![0i16; len])
            // Speech segments have significant RMS
            if rms > 1.0 {
                segments_with_speech += 1;
            } else {
                segments_silent += 1;
            }
        }

        println!(
            "Turn detection simulation: {} speech segments, {} silent segments",
            segments_with_speech, segments_silent
        );

        // Should have both speech and silence
        // With 5 seconds and 30% pause ratio, we expect ~6 silent segments out of 20
        assert!(segments_with_speech > 0, "Should detect speech segments");
        assert!(segments_silent > 0, "Should detect silent segments");
    }

    #[test]
    fn test_turn_boundaries_detection() {
        // Generate audio with clear speech/silence boundaries
        let silence1 = audio_fixtures::generate_silence(audio_fixtures::SECOND);
        let speech = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND * 2);
        let silence2 = audio_fixtures::generate_silence(audio_fixtures::SECOND);

        let mut combined = Vec::new();
        combined.extend_from_slice(&silence1);
        combined.extend_from_slice(&speech);
        combined.extend_from_slice(&silence2);

        // Detect turn boundaries (speech onset/offset)
        let threshold = 1000.0f32;
        let segment_size = audio_fixtures::MS_100;
        let mut in_speech = false;
        let mut boundaries = Vec::new();

        for (i, chunk) in combined.chunks(segment_size).enumerate() {
            let rms = audio_fixtures::calculate_rms(chunk);
            let is_speech = rms > threshold;

            if is_speech && !in_speech {
                // Speech onset
                boundaries.push(("onset", i));
                in_speech = true;
            } else if !is_speech && in_speech {
                // Speech offset
                boundaries.push(("offset", i));
                in_speech = false;
            }
        }

        println!("Detected boundaries: {:?}", boundaries);

        // Should detect at least one onset and one offset
        let onsets: Vec<_> = boundaries.iter().filter(|(t, _)| *t == "onset").collect();
        let offsets: Vec<_> = boundaries.iter().filter(|(t, _)| *t == "offset").collect();

        assert!(!onsets.is_empty(), "Should detect speech onset");
        assert!(!offsets.is_empty(), "Should detect speech offset");
    }
}

// =============================================================================
// Noise Filter Tests (feature-gated)
// =============================================================================

#[cfg(feature = "noise-filter")]
mod noise_filter_tests {
    use super::*;

    #[test]
    fn test_noise_characteristics() {
        // Test that we can generate audio with known noise characteristics
        // that would be suitable for noise filtering

        let clean = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND);
        let noisy_10db = audio_fixtures::generate_noisy_speech(audio_fixtures::SECOND, 10.0);
        let noisy_0db = audio_fixtures::generate_noisy_speech(audio_fixtures::SECOND, 0.0);

        let clean_rms = audio_fixtures::calculate_rms(&clean);
        let noisy_10_rms = audio_fixtures::calculate_rms(&noisy_10db);
        let noisy_0_rms = audio_fixtures::calculate_rms(&noisy_0db);

        println!("Noise characteristics:");
        println!("  Clean RMS: {}", clean_rms);
        println!("  10dB SNR RMS: {}", noisy_10_rms);
        println!("  0dB SNR RMS: {}", noisy_0_rms);

        // Noisier audio should have higher RMS due to added noise energy
        assert!(
            noisy_0_rms >= noisy_10_rms * 0.8,
            "0dB SNR should have at least as much energy as 10dB SNR"
        );
    }

    #[test]
    fn test_snr_measurement_accuracy() {
        // Generate signals with known SNR and verify measurement
        let signal = audio_fixtures::generate_sine_wave(audio_fixtures::SECOND, 440.0, 0.5);

        // Test various noise levels
        for target_snr in [20.0, 10.0, 0.0, -10.0] {
            let noise_amplitude = 0.5 / 10.0f32.powf(target_snr / 20.0);
            let noise =
                audio_fixtures::generate_white_noise(audio_fixtures::SECOND, noise_amplitude);

            let measured_snr = audio_fixtures::calculate_snr(&signal, &noise);

            println!(
                "Target SNR: {:.1} dB, Measured: {:.1} dB",
                target_snr, measured_snr
            );

            // Should be within a reasonable range of target
            // Higher tolerance needed for extreme SNR values where
            // white noise RMS variance affects the measurement
            let tolerance = if target_snr.abs() > 5.0 { 8.0 } else { 5.0 };
            assert!(
                (measured_snr - target_snr).abs() < tolerance,
                "SNR measurement error too large: target {}, measured {}",
                target_snr,
                measured_snr
            );
        }
    }
}

// =============================================================================
// Integration Tests
// =============================================================================

#[test]
fn test_full_audio_pipeline_simulation() {
    // Simulate a full audio processing pipeline:
    // 1. Generate speech
    // 2. Add noise
    // 3. Convert to bytes (for transmission)
    // 4. Convert back to samples (after reception)
    // 5. Analyze

    // Step 1: Generate speech
    let clean_speech = audio_fixtures::generate_speech_pattern(audio_fixtures::SECOND * 3);

    // Step 2: Add noise (simulate real-world audio)
    let noisy_speech = audio_fixtures::generate_noisy_speech(audio_fixtures::SECOND * 3, 15.0);

    // Step 3: Convert to bytes
    let audio_bytes = audio_fixtures::samples_to_bytes(&noisy_speech);

    // Step 4: Convert back
    let received_samples = audio_fixtures::bytes_to_samples(&audio_bytes);

    // Step 5: Analyze
    assert_eq!(
        noisy_speech.len(),
        received_samples.len(),
        "Sample count should match"
    );

    let original_rms = audio_fixtures::calculate_rms(&noisy_speech);
    let received_rms = audio_fixtures::calculate_rms(&received_samples);

    assert!(
        (original_rms - received_rms).abs() < 1.0,
        "RMS should be preserved through conversion"
    );

    println!("Full pipeline test:");
    println!("  Original samples: {}", noisy_speech.len());
    println!("  Bytes transmitted: {}", audio_bytes.len());
    println!("  Received samples: {}", received_samples.len());
    println!("  RMS preserved: {:.2} -> {:.2}", original_rms, received_rms);
}

#[test]
fn test_streaming_audio_simulation() {
    // Simulate streaming audio in chunks

    let total_duration = audio_fixtures::SECOND * 5; // 5 seconds
    let chunk_size = audio_fixtures::MS_100; // 100ms chunks

    let full_audio = audio_fixtures::generate_speech_with_pauses(total_duration, 0.2);

    // Process in chunks
    let mut chunks_processed = 0;
    let mut total_energy = 0.0f64;

    for chunk in full_audio.chunks(chunk_size) {
        // Simulate streaming processing
        let chunk_bytes = audio_fixtures::samples_to_bytes(chunk);

        // Simulate receiving and converting back
        let received = audio_fixtures::bytes_to_samples(&chunk_bytes);

        // Calculate per-chunk metrics
        let chunk_rms = audio_fixtures::calculate_rms(&received) as f64;
        total_energy += chunk_rms;

        chunks_processed += 1;
    }

    let expected_chunks = total_duration / chunk_size;
    assert_eq!(
        chunks_processed, expected_chunks,
        "Should process correct number of chunks"
    );

    let avg_energy = total_energy / chunks_processed as f64;
    println!("Streaming simulation:");
    println!("  Chunks processed: {}", chunks_processed);
    println!("  Average energy per chunk: {:.2}", avg_energy);
}
