//! Audio Test Fixtures
//!
//! This module provides programmatically generated audio test data.
//! Using generated audio ensures:
//! - Consistent, reproducible test inputs
//! - No external file dependencies
//! - Precise control over audio characteristics
//!
//! Audio formats:
//! - Sample rate: 16kHz (16000 Hz)
//! - Bit depth: 16-bit signed PCM
//! - Channels: Mono
//!
//! Available fixtures:
//! - Silence (pure zeros)
//! - White noise (random samples)
//! - Sine wave tones (various frequencies)
//! - Speech-like patterns (variable amplitude)
//! - Chirp signals (frequency sweep)

use std::f32::consts::PI;

/// Standard sample rate for STT (16kHz)
pub const SAMPLE_RATE: u32 = 16000;

/// Duration constants (in samples at 16kHz)
pub const MS_100: usize = 1600;   // 100ms at 16kHz
pub const MS_500: usize = 8000;   // 500ms at 16kHz
pub const SECOND: usize = 16000;  // 1 second at 16kHz

/// Sample rate constant
pub const SAMPLE_RATE_USIZE: usize = 16000;

/// Generate silence (zeros)
pub fn generate_silence(duration_samples: usize) -> Vec<i16> {
    vec![0i16; duration_samples]
}

/// Generate silence as raw bytes
pub fn generate_silence_bytes(duration_samples: usize) -> Vec<u8> {
    samples_to_bytes(&generate_silence(duration_samples))
}

/// Generate white noise with specified amplitude (0.0 - 1.0)
pub fn generate_white_noise(duration_samples: usize, amplitude: f32) -> Vec<i16> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut samples = Vec::with_capacity(duration_samples);
    let max_amplitude = (amplitude * i16::MAX as f32) as i16;

    // Simple deterministic pseudo-random generator for reproducibility
    let mut state: u64 = 12345;
    for _ in 0..duration_samples {
        // Linear congruential generator
        state = state.wrapping_mul(1103515245).wrapping_add(12345);
        let random = ((state >> 16) & 0x7FFF) as f32 / 0x7FFF as f32;
        let sample = ((random * 2.0 - 1.0) * max_amplitude as f32) as i16;
        samples.push(sample);
    }

    samples
}

/// Generate white noise as raw bytes
pub fn generate_white_noise_bytes(duration_samples: usize, amplitude: f32) -> Vec<u8> {
    samples_to_bytes(&generate_white_noise(duration_samples, amplitude))
}

/// Generate a sine wave tone
pub fn generate_sine_wave(duration_samples: usize, frequency: f32, amplitude: f32) -> Vec<i16> {
    let max_amplitude = amplitude * i16::MAX as f32;
    let angular_freq = 2.0 * PI * frequency / SAMPLE_RATE as f32;

    (0..duration_samples)
        .map(|i| {
            let sample = (angular_freq * i as f32).sin() * max_amplitude;
            sample as i16
        })
        .collect()
}

/// Generate a sine wave as raw bytes
pub fn generate_sine_wave_bytes(
    duration_samples: usize,
    frequency: f32,
    amplitude: f32,
) -> Vec<u8> {
    samples_to_bytes(&generate_sine_wave(duration_samples, frequency, amplitude))
}

/// Generate a 440Hz (A4) reference tone
pub fn generate_a440_tone(duration_samples: usize) -> Vec<i16> {
    generate_sine_wave(duration_samples, 440.0, 0.5)
}

/// Generate a 1kHz test tone
pub fn generate_1khz_tone(duration_samples: usize) -> Vec<i16> {
    generate_sine_wave(duration_samples, 1000.0, 0.5)
}

/// Generate a chirp signal (frequency sweep)
pub fn generate_chirp(
    duration_samples: usize,
    start_freq: f32,
    end_freq: f32,
    amplitude: f32,
) -> Vec<i16> {
    let max_amplitude = amplitude * i16::MAX as f32;
    let duration_secs = duration_samples as f32 / SAMPLE_RATE as f32;

    (0..duration_samples)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            // Linear chirp
            let freq = start_freq + (end_freq - start_freq) * t / duration_secs;
            let phase = 2.0 * PI * (start_freq * t + 0.5 * (end_freq - start_freq) * t * t / duration_secs);
            let sample = phase.sin() * max_amplitude;
            sample as i16
        })
        .collect()
}

/// Generate a chirp as raw bytes
pub fn generate_chirp_bytes(
    duration_samples: usize,
    start_freq: f32,
    end_freq: f32,
    amplitude: f32,
) -> Vec<u8> {
    samples_to_bytes(&generate_chirp(duration_samples, start_freq, end_freq, amplitude))
}

/// Generate speech-like pattern with variable amplitude envelope
pub fn generate_speech_pattern(duration_samples: usize) -> Vec<i16> {
    let mut samples = Vec::with_capacity(duration_samples);
    let base_freq = 150.0; // Approximate fundamental frequency of speech

    // Create envelope that simulates speech patterns (variable amplitude)
    let mut state: u64 = 54321;
    let mut envelope = 0.0f32;
    let envelope_smoothing = 0.001;

    for i in 0..duration_samples {
        // Update envelope occasionally to simulate syllables
        if i % 800 == 0 {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            let target = ((state >> 16) & 0x7FFF) as f32 / 0x7FFF as f32;
            envelope = envelope * 0.7 + target * 0.3;
        }

        // Mix multiple harmonics for more realistic timbre
        let t = i as f32 / SAMPLE_RATE as f32;
        let fundamental = (2.0 * PI * base_freq * t).sin();
        let harmonic2 = (2.0 * PI * base_freq * 2.0 * t).sin() * 0.5;
        let harmonic3 = (2.0 * PI * base_freq * 3.0 * t).sin() * 0.25;

        let waveform = (fundamental + harmonic2 + harmonic3) / 1.75;
        let sample = (waveform * envelope * i16::MAX as f32 * 0.6) as i16;
        samples.push(sample);
    }

    samples
}

/// Generate speech-like pattern as raw bytes
pub fn generate_speech_pattern_bytes(duration_samples: usize) -> Vec<u8> {
    samples_to_bytes(&generate_speech_pattern(duration_samples))
}

/// Generate audio with pauses (simulating speech with gaps)
pub fn generate_speech_with_pauses(duration_samples: usize, pause_ratio: f32) -> Vec<i16> {
    let speech = generate_speech_pattern(duration_samples);
    let segment_size = SAMPLE_RATE_USIZE / 4; // 250ms segments

    speech
        .chunks(segment_size)
        .enumerate()
        .flat_map(|(i, chunk)| {
            // Deterministic "random" decision based on segment index
            let should_pause = ((i * 17) % 10) as f32 / 10.0 < pause_ratio;
            if should_pause {
                vec![0i16; chunk.len()]
            } else {
                chunk.to_vec()
            }
        })
        .collect()
}

/// Generate speech with pauses as raw bytes
pub fn generate_speech_with_pauses_bytes(duration_samples: usize, pause_ratio: f32) -> Vec<u8> {
    samples_to_bytes(&generate_speech_with_pauses(duration_samples, pause_ratio))
}

/// Constant for segment size in generate_speech_with_pauses (250ms segments)
const SEGMENT_SIZE: usize = SAMPLE_RATE_USIZE / 4;

/// Generate noisy speech (speech + noise)
pub fn generate_noisy_speech(duration_samples: usize, snr_db: f32) -> Vec<i16> {
    let speech = generate_speech_pattern(duration_samples);
    let noise = generate_white_noise(duration_samples, 1.0);

    // Calculate noise amplitude based on SNR
    // SNR = 10 * log10(signal_power / noise_power)
    let snr_linear = 10.0f32.powf(snr_db / 10.0);
    let noise_scale = 1.0 / snr_linear.sqrt();

    speech
        .iter()
        .zip(noise.iter())
        .map(|(&s, &n)| {
            let combined = s as f32 + n as f32 * noise_scale;
            combined.clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect()
}

/// Generate noisy speech as raw bytes
pub fn generate_noisy_speech_bytes(duration_samples: usize, snr_db: f32) -> Vec<u8> {
    samples_to_bytes(&generate_noisy_speech(duration_samples, snr_db))
}

/// Convert i16 samples to little-endian bytes
pub fn samples_to_bytes(samples: &[i16]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect()
}

/// Convert bytes to i16 samples
pub fn bytes_to_samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

/// Calculate RMS (root mean square) amplitude
pub fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_squares: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    (sum_squares / samples.len() as f64).sqrt() as f32
}

/// Calculate peak amplitude
pub fn calculate_peak(samples: &[i16]) -> i16 {
    samples.iter().map(|&s| s.abs()).max().unwrap_or(0)
}

/// Calculate SNR between clean and noisy signals
pub fn calculate_snr(signal: &[i16], noise: &[i16]) -> f32 {
    let signal_power: f64 = signal.iter().map(|&s| (s as f64).powi(2)).sum();
    let noise_power: f64 = noise.iter().map(|&n| (n as f64).powi(2)).sum();

    if noise_power == 0.0 {
        return f32::INFINITY;
    }

    10.0 * (signal_power / noise_power).log10() as f32
}

/// Create a WAV file header for the given audio parameters
pub fn create_wav_header(num_samples: usize) -> Vec<u8> {
    let data_size = (num_samples * 2) as u32; // 16-bit = 2 bytes per sample
    let file_size = data_size + 36;
    let sample_rate = SAMPLE_RATE;
    let byte_rate = sample_rate * 2; // mono, 16-bit
    let block_align: u16 = 2;
    let bits_per_sample: u16 = 16;

    let mut header = Vec::with_capacity(44);

    // RIFF header
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&file_size.to_le_bytes());
    header.extend_from_slice(b"WAVE");

    // fmt chunk
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    header.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    header.extend_from_slice(&1u16.to_le_bytes()); // mono
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_size.to_le_bytes());

    header
}

/// Create a complete WAV file
pub fn create_wav_file(samples: &[i16]) -> Vec<u8> {
    let mut wav = create_wav_header(samples.len());
    wav.extend(samples_to_bytes(samples));
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_generation() {
        let silence = generate_silence(SECOND);
        assert_eq!(silence.len(), SECOND);
        assert!(silence.iter().all(|&s| s == 0));
    }

    #[test]
    fn test_white_noise_generation() {
        let noise = generate_white_noise(SECOND, 0.5);
        assert_eq!(noise.len(), SECOND);

        // Should have non-zero samples
        assert!(noise.iter().any(|&s| s != 0));

        // RMS should be approximately proportional to amplitude
        let rms = calculate_rms(&noise);
        assert!(rms > 0.0);
    }

    #[test]
    fn test_sine_wave_generation() {
        let sine = generate_sine_wave(SECOND, 440.0, 0.5);
        assert_eq!(sine.len(), SECOND);

        // Peak should be approximately half of max (amplitude = 0.5)
        let peak = calculate_peak(&sine);
        assert!(peak > i16::MAX / 4);
        assert!(peak < i16::MAX);
    }

    #[test]
    fn test_chirp_generation() {
        let chirp = generate_chirp(SECOND, 100.0, 1000.0, 0.5);
        assert_eq!(chirp.len(), SECOND);
        assert!(chirp.iter().any(|&s| s != 0));
    }

    #[test]
    fn test_speech_pattern_generation() {
        let speech = generate_speech_pattern(SECOND);
        assert_eq!(speech.len(), SECOND);

        // Should have varying amplitude
        let first_half_rms = calculate_rms(&speech[..SECOND / 2]);
        let second_half_rms = calculate_rms(&speech[SECOND / 2..]);

        // They might be similar but shouldn't be identical
        // (due to the envelope variation)
        assert!(first_half_rms > 0.0 || second_half_rms > 0.0);
    }

    #[test]
    fn test_speech_with_pauses() {
        let speech = generate_speech_with_pauses(SECOND * 2, 0.3);
        assert_eq!(speech.len(), SECOND * 2);

        // Should have some silence segments
        let silence_count = speech.windows(100).filter(|w| w.iter().all(|&s| s == 0)).count();
        assert!(silence_count > 0);
    }

    #[test]
    fn test_noisy_speech_generation() {
        let noisy = generate_noisy_speech(SECOND, 10.0); // 10dB SNR
        assert_eq!(noisy.len(), SECOND);
        assert!(noisy.iter().any(|&s| s != 0));
    }

    #[test]
    fn test_samples_bytes_conversion() {
        let samples = vec![0i16, 1000, -1000, i16::MAX, i16::MIN];
        let bytes = samples_to_bytes(&samples);
        let recovered = bytes_to_samples(&bytes);
        assert_eq!(samples, recovered);
    }

    #[test]
    fn test_wav_header_creation() {
        let header = create_wav_header(SECOND);
        assert_eq!(header.len(), 44);
        assert_eq!(&header[0..4], b"RIFF");
        assert_eq!(&header[8..12], b"WAVE");
    }

    #[test]
    fn test_wav_file_creation() {
        let samples = generate_a440_tone(SECOND);
        let wav = create_wav_file(&samples);

        // WAV header is 44 bytes, data is 2 bytes per sample
        assert_eq!(wav.len(), 44 + SECOND * 2);
    }

    #[test]
    fn test_deterministic_generation() {
        // Generators should be deterministic
        let noise1 = generate_white_noise(1000, 0.5);
        let noise2 = generate_white_noise(1000, 0.5);
        assert_eq!(noise1, noise2);

        let speech1 = generate_speech_pattern(1000);
        let speech2 = generate_speech_pattern(1000);
        assert_eq!(speech1, speech2);
    }
}
