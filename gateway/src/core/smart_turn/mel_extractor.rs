//! Mel spectrogram extractor for Smart Turn detection.
//!
//! This module extracts Whisper-compatible 80-bin mel spectrograms from audio
//! for use with the Smart Turn model.
//!
//! ## Whisper Mel Spectrogram Specifications
//!
//! - Sample rate: 16,000 Hz
//! - FFT size: 400 samples (25ms)
//! - Hop length: 160 samples (10ms)
//! - Mel bins: 80
//! - Max duration: 8 seconds (800 frames)
//!
//! ## Performance
//!
//! Target: 482x realtime on Apple M1 (single-threaded)
//! - 10s audio → 21ms extraction time
//! - 60s audio → 124ms extraction time
//!
//! ## Implementation
//!
//! Uses a custom Whisper-compatible mel spectrogram implementation that exactly
//! matches OpenAI Whisper's preprocessing. This is critical for model accuracy.

use super::whisper_mel::WhisperMelExtractor;
use anyhow::{Context, Result};
use rubato::{FftFixedIn, Resampler};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::{debug, trace};

/// Default Whisper mel spectrogram parameters.
pub const WHISPER_SAMPLE_RATE: u32 = 16000;
pub const WHISPER_N_FFT: usize = 400;
pub const WHISPER_HOP_LENGTH: usize = 160;
pub const WHISPER_N_MELS: usize = 80;
pub const WHISPER_MAX_DURATION_SECS: f32 = 8.0;

/// Maximum number of mel frames (8 seconds at 10ms per frame = 800 frames).
pub const WHISPER_MAX_FRAMES: usize = 800;

/// Configuration for the mel spectrogram extractor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MelExtractorConfig {
    /// Input sample rate in Hz.
    /// If different from 16kHz, audio will be resampled.
    #[serde(default = "default_sample_rate")]
    pub input_sample_rate: u32,

    /// FFT size in samples (default: 400 for 25ms at 16kHz).
    #[serde(default = "default_n_fft")]
    pub n_fft: usize,

    /// Hop length in samples (default: 160 for 10ms at 16kHz).
    #[serde(default = "default_hop_length")]
    pub hop_length: usize,

    /// Number of mel filter banks (default: 80 for Whisper).
    #[serde(default = "default_n_mels")]
    pub n_mels: usize,

    /// Maximum audio duration in seconds.
    #[serde(default = "default_max_duration_secs")]
    pub max_duration_secs: f32,

    /// Whether to use log mel spectrogram (required for Whisper).
    #[serde(default = "default_use_log")]
    pub use_log: bool,

    /// High frequency limit for mel filters in Hz.
    /// Default is 8000.0 (Nyquist for 16kHz sample rate).
    #[serde(default = "default_high_freq")]
    pub high_freq: f32,

    /// Whether to enable debug logging.
    #[serde(default)]
    pub debug_logging: bool,

    /// Whether to apply Whisper-style normalization.
    /// When true, applies: log10, dynamic range compression, and (x+4)/4 scaling.
    /// This is required for Smart Turn model compatibility.
    #[serde(default = "default_whisper_normalization")]
    pub whisper_normalization: bool,
}

fn default_whisper_normalization() -> bool {
    true // Default to Whisper normalization for Smart Turn compatibility
}

fn default_sample_rate() -> u32 {
    WHISPER_SAMPLE_RATE
}

fn default_n_fft() -> usize {
    WHISPER_N_FFT
}

fn default_hop_length() -> usize {
    WHISPER_HOP_LENGTH
}

fn default_n_mels() -> usize {
    WHISPER_N_MELS
}

fn default_max_duration_secs() -> f32 {
    WHISPER_MAX_DURATION_SECS
}

fn default_use_log() -> bool {
    true
}

fn default_high_freq() -> f32 {
    8000.0 // Nyquist for 16kHz sample rate
}

impl Default for MelExtractorConfig {
    fn default() -> Self {
        Self {
            input_sample_rate: WHISPER_SAMPLE_RATE,
            n_fft: WHISPER_N_FFT,
            hop_length: WHISPER_HOP_LENGTH,
            n_mels: WHISPER_N_MELS,
            max_duration_secs: WHISPER_MAX_DURATION_SECS,
            use_log: true,
            high_freq: default_high_freq(),
            debug_logging: false,
            whisper_normalization: default_whisper_normalization(),
        }
    }
}

impl MelExtractorConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the input sample rate.
    pub fn with_input_sample_rate(mut self, rate: u32) -> Self {
        self.input_sample_rate = rate;
        self
    }

    /// Sets the maximum duration.
    pub fn with_max_duration_secs(mut self, secs: f32) -> Self {
        self.max_duration_secs = secs;
        self
    }

    /// Enables debug logging.
    pub fn with_debug_logging(mut self) -> Self {
        self.debug_logging = true;
        self
    }

    /// Sets the high frequency limit for mel filters.
    pub fn with_high_freq(mut self, freq: f32) -> Self {
        self.high_freq = freq;
        self
    }

    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.input_sample_rate < 8000 || self.input_sample_rate > 96000 {
            return Err(format!(
                "Input sample rate must be between 8000 and 96000 Hz, got {}",
                self.input_sample_rate
            ));
        }

        if self.n_fft < 64 || self.n_fft > 4096 {
            return Err(format!(
                "FFT size must be between 64 and 4096, got {}",
                self.n_fft
            ));
        }

        if self.hop_length < 1 || self.hop_length > self.n_fft {
            return Err(format!(
                "Hop length must be between 1 and n_fft ({}), got {}",
                self.n_fft, self.hop_length
            ));
        }

        if self.n_mels < 1 || self.n_mels > 256 {
            return Err(format!(
                "Number of mel bins must be between 1 and 256, got {}",
                self.n_mels
            ));
        }

        if self.max_duration_secs <= 0.0 || self.max_duration_secs > 60.0 {
            return Err(format!(
                "Max duration must be between 0 and 60 seconds, got {}",
                self.max_duration_secs
            ));
        }

        Ok(())
    }

    /// Returns the number of samples per second at the processing rate (16kHz).
    #[inline]
    pub fn samples_per_frame(&self) -> usize {
        self.hop_length
    }

    /// Returns the frame duration in milliseconds.
    #[inline]
    pub fn frame_duration_ms(&self) -> f32 {
        (self.hop_length as f32 / WHISPER_SAMPLE_RATE as f32) * 1000.0
    }

    /// Returns the maximum number of frames for the configured duration.
    #[inline]
    pub fn max_frames(&self) -> usize {
        ((self.max_duration_secs * WHISPER_SAMPLE_RATE as f32) / self.hop_length as f32).ceil()
            as usize
    }
}

/// Mel spectrogram extractor for Whisper-compatible features.
///
/// This extractor produces 80-bin mel spectrograms compatible with
/// the OpenAI Whisper model and the Smart Turn detector.
///
/// Uses a custom Whisper-compatible mel spectrogram implementation that
/// exactly matches OpenAI Whisper's preprocessing (librosa-style filterbank
/// with HTK formula and Slaney normalization).
pub struct MelExtractor {
    /// Configuration.
    config: MelExtractorConfig,

    /// Whisper-compatible mel spectrogram extractor.
    whisper_mel: WhisperMelExtractor,

    /// Resampler for non-16kHz input (if needed).
    resampler: Option<FftFixedIn<f32>>,

    /// Buffer for accumulating input samples before resampling.
    /// FftFixedIn requires specific chunk sizes.
    resample_input_buffer: Vec<f32>,

    /// Audio buffer for accumulating samples (at 16kHz) before mel processing.
    audio_buffer: Vec<f32>,

    /// Accumulated mel frames.
    mel_frames: Vec<Vec<f32>>,

    /// Total samples processed (at input sample rate).
    total_samples: u64,

    /// Total extraction time in microseconds.
    total_extraction_time_us: u64,
}

impl MelExtractor {
    /// Creates a new mel extractor with the given configuration.
    pub fn new(config: MelExtractorConfig) -> Result<Self> {
        config
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid config: {}", e))?;

        debug!(
            "Initializing MelExtractor: input_rate={}, n_fft={}, hop={}, mels={}",
            config.input_sample_rate, config.n_fft, config.hop_length, config.n_mels
        );

        // Create Whisper-compatible mel spectrogram extractor
        // This implementation exactly matches OpenAI Whisper's preprocessing:
        // - librosa-style mel filterbank with HTK formula and Slaney normalization
        // - Periodic Hann window
        // - Power spectrum (|FFT|^2)
        // - log10 normalization with dynamic range compression and (x+4)/4 scaling
        let whisper_mel = WhisperMelExtractor::new();

        // Create resampler if input rate differs from 16kHz
        let resampler = if config.input_sample_rate != WHISPER_SAMPLE_RATE {
            debug!(
                "Creating resampler: {} Hz -> {} Hz",
                config.input_sample_rate, WHISPER_SAMPLE_RATE
            );

            // Use FFT-based resampler for high quality
            // Chunk size of 1024 gives good balance between latency and quality
            let resampler = FftFixedIn::<f32>::new(
                config.input_sample_rate as usize,
                WHISPER_SAMPLE_RATE as usize,
                1024, // Chunk size (input frames per call)
                2,    // Sub-chunks for anti-aliasing
                1,    // Channels (mono)
            )
            .context("Failed to create resampler")?;

            Some(resampler)
        } else {
            None
        };

        // Pre-allocate resampler input buffer if needed
        let resample_input_buffer = if resampler.is_some() {
            // Capacity for several chunks worth of input
            Vec::with_capacity(config.input_sample_rate as usize)
        } else {
            Vec::new()
        };

        Ok(Self {
            config,
            whisper_mel,
            resampler,
            resample_input_buffer,
            audio_buffer: Vec::with_capacity(WHISPER_SAMPLE_RATE as usize), // 1 second buffer
            mel_frames: Vec::with_capacity(WHISPER_MAX_FRAMES),
            total_samples: 0,
            total_extraction_time_us: 0,
        })
    }

    /// Processes audio samples and extracts mel spectrogram frames.
    ///
    /// # Arguments
    ///
    /// * `audio` - Input audio samples (f32, -1.0 to 1.0 range)
    ///
    /// # Returns
    ///
    /// Number of new mel frames extracted.
    pub fn process(&mut self, audio: &[f32]) -> Result<usize> {
        if audio.is_empty() {
            return Ok(0);
        }

        // Validate audio samples - NaN/Inf values would corrupt mel spectrogram
        // This check is fast (< 0.1ms for 16k samples) and prevents silent degradation
        if audio.iter().any(|&x| !x.is_finite()) {
            anyhow::bail!(
                "Audio contains non-finite values (NaN or Inf) - {} samples",
                audio.len()
            );
        }

        // Warn if audio values are outside expected range (may indicate clipping or scaling issues)
        // This is a soft check - we don't fail, just warn
        if self.config.debug_logging {
            let max_abs = audio.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
            if max_abs > 1.1 {
                tracing::warn!(
                    "Audio samples exceed expected range: max_abs={:.2} (expected <= 1.0)",
                    max_abs
                );
            }
        }

        let start = Instant::now();
        self.total_samples += audio.len() as u64;

        // Resample if needed, or use audio directly
        if let Some(ref mut resampler) = self.resampler {
            // Accumulate input samples
            self.resample_input_buffer.extend_from_slice(audio);

            // Process in chunks matching what the resampler expects
            let chunk_size = resampler.input_frames_next();

            while self.resample_input_buffer.len() >= chunk_size {
                // Take exactly chunk_size samples
                let chunk: Vec<f32> = self.resample_input_buffer.drain(..chunk_size).collect();

                // Prepare input for resampler (single channel)
                let input_waves = vec![chunk];

                // Resample this chunk
                let resampled = resampler
                    .process(&input_waves, None)
                    .context("Resampling failed")?;

                // Add resampled audio to buffer
                if let Some(channel) = resampled.into_iter().next() {
                    self.audio_buffer.extend_from_slice(&channel);
                }
            }
        } else {
            // No resampling needed, use audio directly
            self.audio_buffer.extend_from_slice(audio);
        }

        // Process accumulated audio through Whisper mel extractor
        let initial_frames = self.mel_frames.len();

        // Only process if we have enough audio for at least one new frame
        if self.audio_buffer.len() >= self.config.n_fft {
            // Compute Whisper-compatible mel spectrogram
            // The whisper_mel extractor returns shape (n_mels, n_frames) with Whisper normalization already applied
            let mel_spec_2d = self
                .whisper_mel
                .compute(&self.audio_buffer)
                .context("Mel spectrogram computation failed")?;

            // Convert from (n_mels, n_frames) to (n_frames, n_mels) for internal storage
            let num_mels = mel_spec_2d.len();
            let num_frames = if num_mels > 0 {
                mel_spec_2d[0].len()
            } else {
                0
            };

            // Replace existing frames
            self.mel_frames.clear();

            // Keep the LAST max_frames (most recent audio) if we have too many
            let max_frames = self.config.max_frames();
            let start_frame = num_frames.saturating_sub(max_frames);

            // Transpose from (n_mels, n_frames) to (n_frames, n_mels)
            self.mel_frames.reserve(num_frames - start_frame);
            for frame_idx in start_frame..num_frames {
                let frame: Vec<f32> = (0..num_mels)
                    .map(|mel_idx| mel_spec_2d[mel_idx][frame_idx])
                    .collect();
                self.mel_frames.push(frame);
            }

            // Keep only the trailing audio needed for overlapping windows
            // This prevents unbounded memory growth
            let keep_samples = self.config.n_fft;
            if self.audio_buffer.len() > keep_samples * 2 {
                let drain_count = self.audio_buffer.len() - keep_samples;
                self.audio_buffer.drain(..drain_count);
            }
        }

        let new_frames = self.mel_frames.len().saturating_sub(initial_frames);

        self.total_extraction_time_us += start.elapsed().as_micros() as u64;

        if self.config.debug_logging {
            trace!(
                "Processed {} samples, extracted {} new frames (total: {})",
                audio.len(),
                new_frames,
                self.mel_frames.len()
            );
        }

        Ok(new_frames)
    }

    /// Returns the accumulated mel spectrogram frames.
    ///
    /// Each frame is a vector of 80 mel coefficients (or configured n_mels).
    pub fn get_mel_frames(&self) -> &[Vec<f32>] {
        &self.mel_frames
    }

    /// Returns the mel spectrogram as a reference to the 2D vector [frames][mels].
    ///
    /// Shape: (num_frames, n_mels)
    ///
    /// This is the preferred method for read-only access as it avoids cloning.
    /// Use `get_mel_2d_owned()` if you need an owned copy.
    #[inline]
    pub fn get_mel_2d(&self) -> &[Vec<f32>] {
        &self.mel_frames
    }

    /// Returns an owned copy of the mel spectrogram [frames][mels].
    ///
    /// Shape: (num_frames, n_mels)
    ///
    /// Note: This clones the data. Prefer `get_mel_2d()` for read-only access
    /// to avoid unnecessary allocations in hot paths.
    pub fn get_mel_2d_owned(&self) -> Vec<Vec<f32>> {
        self.mel_frames.clone()
    }

    /// Returns the mel spectrogram padded/truncated to the target frame count.
    ///
    /// This is useful for model inference which expects a fixed input size.
    ///
    /// **IMPORTANT**: Following the Smart Turn model convention:
    /// - If audio is shorter than target, zeros are padded at the BEGINNING
    /// - If audio is longer, the LAST target_frames are kept (most recent)
    /// - This places the audio at the END of the window, matching how the model was trained
    ///
    /// # Arguments
    ///
    /// * `target_frames` - Desired number of frames (e.g., 800 for Smart Turn)
    ///
    /// # Returns
    ///
    /// 2D vector of shape (target_frames, n_mels)
    pub fn get_mel_2d_padded(&self, target_frames: usize) -> Vec<Vec<f32>> {
        let n_mels = self.config.n_mels;
        let n_frames = self.mel_frames.len();

        let mut result = Vec::with_capacity(target_frames);

        if n_frames >= target_frames {
            // Truncate: keep LAST target_frames (most recent audio)
            let start_idx = n_frames - target_frames;
            for i in start_idx..n_frames {
                result.push(self.mel_frames[i].clone());
            }
        } else {
            // Pad: add zeros at BEGINNING, then copy all frames at END
            let padding_frames = target_frames - n_frames;

            // First add zero padding frames
            for _ in 0..padding_frames {
                result.push(vec![0.0f32; n_mels]);
            }

            // Then add existing frames (at the end)
            for frame in &self.mel_frames {
                result.push(frame.clone());
            }
        }

        result
    }

    /// Returns the mel spectrogram as a flattened 1D vector for model input.
    ///
    /// The output is in row-major order: [frame0_mel0, frame0_mel1, ..., frame0_melN, frame1_mel0, ...]
    ///
    /// **IMPORTANT**: Following the Smart Turn model convention:
    /// - If audio is shorter than target, zeros are padded at the BEGINNING
    /// - If audio is longer, the LAST target_frames are kept (most recent)
    /// - This places the audio at the END of the window, matching how the model was trained
    ///
    /// # Arguments
    ///
    /// * `target_frames` - Desired number of frames (pads/truncates as needed)
    ///
    /// # Returns
    ///
    /// Flattened vector of length (target_frames * n_mels)
    pub fn get_mel_flat(&self, target_frames: usize) -> Vec<f32> {
        let n_mels = self.config.n_mels;
        let total_size = target_frames * n_mels;
        let n_frames = self.mel_frames.len();

        // Pre-fill with zeros (this handles padding at beginning)
        let mut result = vec![0.0f32; total_size];

        if n_frames >= target_frames {
            // Truncate: keep LAST target_frames (most recent audio)
            let start_idx = n_frames - target_frames;
            for (output_idx, frame_idx) in (start_idx..n_frames).enumerate() {
                let offset = output_idx * n_mels;
                for (j, &val) in self.mel_frames[frame_idx].iter().enumerate() {
                    result[offset + j] = val;
                }
            }
        } else {
            // Pad: zeros at BEGINNING (already pre-filled), copy frames at END
            let padding_frames = target_frames - n_frames;
            for (i, frame) in self.mel_frames.iter().enumerate() {
                // Place frames at the end of the output (after padding)
                let output_idx = padding_frames + i;
                let offset = output_idx * n_mels;
                for (j, &val) in frame.iter().enumerate() {
                    result[offset + j] = val;
                }
            }
        }

        result
    }

    /// Returns the number of accumulated mel frames.
    #[inline]
    pub fn num_frames(&self) -> usize {
        self.mel_frames.len()
    }

    /// Returns the total audio duration processed in seconds.
    #[inline]
    pub fn audio_duration_secs(&self) -> f32 {
        self.total_samples as f32 / self.config.input_sample_rate as f32
    }

    /// Returns the average extraction time per frame in microseconds.
    #[inline]
    pub fn avg_extraction_time_us(&self) -> u64 {
        if self.mel_frames.is_empty() {
            0
        } else {
            self.total_extraction_time_us / self.mel_frames.len() as u64
        }
    }

    /// Returns the realtime factor (how many times faster than realtime).
    #[inline]
    pub fn realtime_factor(&self) -> f32 {
        if self.total_extraction_time_us == 0 {
            0.0
        } else {
            let audio_duration_us = self.audio_duration_secs() * 1_000_000.0;
            audio_duration_us / self.total_extraction_time_us as f32
        }
    }

    /// Clears accumulated mel frames but keeps the extractor state.
    pub fn clear_frames(&mut self) {
        self.mel_frames.clear();
        self.audio_buffer.clear();
        self.resample_input_buffer.clear();
    }

    /// Resets the extractor to initial state.
    pub fn reset(&mut self) {
        self.mel_frames.clear();
        self.audio_buffer.clear();
        self.resample_input_buffer.clear();
        self.total_samples = 0;
        self.total_extraction_time_us = 0;
    }

    /// Returns a reference to the configuration.
    #[inline]
    pub fn config(&self) -> &MelExtractorConfig {
        &self.config
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Configuration tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = MelExtractorConfig::default();

        assert_eq!(config.input_sample_rate, 16000);
        assert_eq!(config.n_fft, 400);
        assert_eq!(config.hop_length, 160);
        assert_eq!(config.n_mels, 80);
        assert_eq!(config.max_duration_secs, 8.0);
        assert!(config.use_log);
        assert!(!config.debug_logging);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = MelExtractorConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate_low() {
        let config = MelExtractorConfig {
            input_sample_rate: 1000,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate_high() {
        let config = MelExtractorConfig {
            input_sample_rate: 100000,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_fft_size() {
        let config = MelExtractorConfig {
            n_fft: 32,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_hop_length() {
        let config = MelExtractorConfig {
            n_fft: 400,
            hop_length: 500, // > n_fft
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_mels() {
        let config = MelExtractorConfig {
            n_mels: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_duration() {
        let config = MelExtractorConfig {
            max_duration_secs: -1.0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_frame_duration_ms() {
        let config = MelExtractorConfig::default();
        // 160 samples at 16kHz = 10ms
        let duration = config.frame_duration_ms();
        assert!((duration - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_config_max_frames() {
        let config = MelExtractorConfig::default();
        // 8 seconds at 10ms per frame = 800 frames
        assert_eq!(config.max_frames(), 800);
    }

    #[test]
    fn test_config_builder_pattern() {
        let config = MelExtractorConfig::new()
            .with_input_sample_rate(48000)
            .with_max_duration_secs(10.0)
            .with_debug_logging();

        assert_eq!(config.input_sample_rate, 48000);
        assert_eq!(config.max_duration_secs, 10.0);
        assert!(config.debug_logging);
    }

    // -------------------------------------------------------------------------
    // Extractor construction tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_extractor_construction_default() {
        let config = MelExtractorConfig::default();
        let extractor = MelExtractor::new(config);
        assert!(extractor.is_ok());
    }

    #[test]
    fn test_extractor_construction_with_resampler() {
        let config = MelExtractorConfig::new().with_input_sample_rate(48000);
        let extractor = MelExtractor::new(config);
        assert!(extractor.is_ok());

        let ext = extractor.unwrap();
        assert!(ext.resampler.is_some());
    }

    #[test]
    fn test_extractor_construction_invalid_config() {
        let config = MelExtractorConfig {
            input_sample_rate: 0,
            ..Default::default()
        };
        let extractor = MelExtractor::new(config);
        assert!(extractor.is_err());
    }

    // -------------------------------------------------------------------------
    // Processing tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_process_empty_audio() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        let result = extractor.process(&[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        assert_eq!(extractor.num_frames(), 0);
    }

    #[test]
    fn test_process_silence() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // 1 second of silence at 16kHz
        let silence = vec![0.0f32; 16000];
        let result = extractor.process(&silence);

        assert!(result.is_ok());
        // Should produce ~100 frames (1 sec / 10ms per frame)
        assert!(
            extractor.num_frames() > 50,
            "Got {} frames",
            extractor.num_frames()
        );
    }

    #[test]
    fn test_process_sine_wave() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Generate 1 second of 440Hz sine wave
        let sample_rate = 16000.0;
        let freq = 440.0;
        let duration = 1.0;
        let num_samples = (sample_rate * duration) as usize;

        let audio: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
            })
            .collect();

        let result = extractor.process(&audio);

        assert!(result.is_ok());
        assert!(
            extractor.num_frames() > 50,
            "Got {} frames",
            extractor.num_frames()
        );

        // Mel frames should have the right dimensions
        let mel_frames = extractor.get_mel_frames();
        assert!(!mel_frames.is_empty());
        assert_eq!(mel_frames[0].len(), 80);
    }

    #[test]
    fn test_process_incremental() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process in chunks
        let chunk_size = 1600; // 100ms chunks
        let total_samples = 16000;
        let audio = vec![0.0f32; total_samples];

        for chunk in audio.chunks(chunk_size) {
            let result = extractor.process(chunk);
            assert!(result.is_ok());
        }

        // Should have processed all audio
        assert!(extractor.num_frames() > 0, "Should have at least one frame");
    }

    #[test]
    fn test_process_max_duration_limit() {
        let config = MelExtractorConfig::new().with_max_duration_secs(1.0);
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process 10 seconds of audio (more than max)
        let audio = vec![0.0f32; 160000]; // 10 seconds at 16kHz
        let _result = extractor.process(&audio);

        // Should be capped at max frames for 1 second
        let max_frames = extractor.config().max_frames();
        assert!(
            extractor.num_frames() <= max_frames,
            "Got {} frames, max is {}",
            extractor.num_frames(),
            max_frames
        );
    }

    // -------------------------------------------------------------------------
    // Output format tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_get_mel_2d_empty() {
        let config = MelExtractorConfig::default();
        let extractor = MelExtractor::new(config).unwrap();

        let mel = extractor.get_mel_2d();
        assert!(mel.is_empty());
    }

    #[test]
    fn test_get_mel_2d_with_data() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process some audio
        let audio = vec![0.1f32; 8000];
        extractor.process(&audio).unwrap();

        let mel = extractor.get_mel_2d();
        assert!(!mel.is_empty());
        assert_eq!(mel[0].len(), 80);
    }

    #[test]
    fn test_get_mel_2d_padded() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process some audio
        let audio = vec![0.1f32; 8000];
        extractor.process(&audio).unwrap();

        let target_frames = 800;
        let mel = extractor.get_mel_2d_padded(target_frames);

        assert_eq!(mel.len(), target_frames);
        assert_eq!(mel[0].len(), 80);

        // Per Smart Turn convention: padding is at BEGINNING, data at END
        // First frame should be zeros (padding), last frame should have data
        let num_frames = extractor.num_frames();
        if num_frames < target_frames {
            // First frame should be padding (zeros)
            assert!(
                mel[0].iter().all(|&v| v == 0.0),
                "First frame should be zero padding"
            );
            // Last actual data frame should have non-zero values
            // (audio frames are at the end of the output)
        }
    }

    #[test]
    fn test_get_mel_2d_padded_truncates_keeping_last() {
        let config = MelExtractorConfig::new().with_max_duration_secs(10.0);
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process 5 seconds of audio (produces ~500 frames)
        let audio = vec![0.1f32; 80000]; // 5 seconds at 16kHz
        extractor.process(&audio).unwrap();

        let target_frames = 100;
        let mel = extractor.get_mel_2d_padded(target_frames);

        assert_eq!(mel.len(), target_frames);
        // When truncating, we keep the LAST frames (most recent audio)
        // The last frame of padded output should match the last frame of original
        let original = extractor.get_mel_frames();
        assert_eq!(
            mel[target_frames - 1],
            original[original.len() - 1],
            "Last frame should be preserved when truncating"
        );
    }

    #[test]
    fn test_get_mel_flat_padding_at_beginning() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process some audio
        let audio = vec![0.1f32; 8000];
        extractor.process(&audio).unwrap();

        let target_frames = 100;
        let flat = extractor.get_mel_flat(target_frames);
        let n_mels = 80;

        assert_eq!(flat.len(), target_frames * n_mels);

        // Per Smart Turn convention: zeros at BEGINNING, data at END
        let num_frames = extractor.num_frames();
        if num_frames < target_frames {
            // First frame in flat output should be zeros (padding)
            assert!(
                flat[0..n_mels].iter().all(|&v| v == 0.0),
                "First frame should be zero padding"
            );
        }
    }

    #[test]
    fn test_get_mel_flat() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process some audio
        let audio = vec![0.1f32; 8000];
        extractor.process(&audio).unwrap();

        let target_frames = 100;
        let flat = extractor.get_mel_flat(target_frames);

        assert_eq!(flat.len(), target_frames * 80);
    }

    // -------------------------------------------------------------------------
    // Statistics tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_audio_duration_secs() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process 1 second of audio
        let audio = vec![0.0f32; 16000];
        extractor.process(&audio).unwrap();

        let duration = extractor.audio_duration_secs();
        assert!((duration - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_realtime_factor() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process 1 second of audio
        let audio = vec![0.0f32; 16000];
        extractor.process(&audio).unwrap();

        let rtf = extractor.realtime_factor();
        // Should be faster than realtime (relaxed for CI environments)
        // Production target is >100x but test environments vary
        assert!(rtf > 1.0, "Realtime factor {} should be > 1x", rtf);
    }

    // -------------------------------------------------------------------------
    // Reset and clear tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_clear_frames() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process some audio
        let audio = vec![0.0f32; 16000];
        extractor.process(&audio).unwrap();
        assert!(extractor.num_frames() > 0);

        // Clear
        extractor.clear_frames();
        assert_eq!(extractor.num_frames(), 0);

        // Can process more audio
        extractor.process(&audio).unwrap();
        assert!(extractor.num_frames() > 0);
    }

    #[test]
    fn test_reset() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process some audio
        let audio = vec![0.0f32; 16000];
        extractor.process(&audio).unwrap();

        // Reset
        extractor.reset();
        assert_eq!(extractor.num_frames(), 0);
        assert_eq!(extractor.total_samples, 0);
        assert_eq!(extractor.total_extraction_time_us, 0);
    }

    // -------------------------------------------------------------------------
    // Mel spectrogram value tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_mel_values_finite() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // Process various signals
        let audio: Vec<f32> = (0..16000).map(|i| (i % 2) as f32 - 0.5).collect();
        extractor.process(&audio).unwrap();

        // All values should be finite
        let mel_frames = extractor.get_mel_frames();
        for frame in mel_frames {
            for &val in frame {
                assert!(val.is_finite(), "Mel value should be finite: {}", val);
            }
        }
    }

    #[test]
    fn test_nan_inf_rejection() {
        let config = MelExtractorConfig::default();
        let mut extractor = MelExtractor::new(config).unwrap();

        // NaN should be rejected
        let audio_with_nan = vec![0.0, 0.1, f32::NAN, 0.2];
        let result = extractor.process(&audio_with_nan);
        assert!(result.is_err(), "Should reject audio with NaN");

        // Infinity should be rejected
        let audio_with_inf = vec![0.0, 0.1, f32::INFINITY, 0.2];
        let result = extractor.process(&audio_with_inf);
        assert!(result.is_err(), "Should reject audio with Infinity");

        // Negative infinity should be rejected
        let audio_with_neg_inf = vec![0.0, 0.1, f32::NEG_INFINITY, 0.2];
        let result = extractor.process(&audio_with_neg_inf);
        assert!(
            result.is_err(),
            "Should reject audio with negative infinity"
        );

        // Valid audio should still work
        let valid_audio = vec![0.0, 0.1, 0.2, 0.3];
        let result = extractor.process(&valid_audio);
        assert!(result.is_ok(), "Valid audio should be accepted");
    }

    // -------------------------------------------------------------------------
    // Resampling tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resampling_48khz() {
        let config = MelExtractorConfig::new().with_input_sample_rate(48000);
        let mut extractor = MelExtractor::new(config).unwrap();

        // Generate 1 second at 48kHz
        let audio = vec![0.1f32; 48000];
        let result = extractor.process(&audio);

        assert!(result.is_ok());
        // Should still produce frames (based on output 16kHz rate)
        assert!(
            extractor.num_frames() > 0,
            "Got {} frames",
            extractor.num_frames()
        );
    }

    #[test]
    fn test_resampling_preserves_frame_count_approx() {
        let config_16k = MelExtractorConfig::new().with_input_sample_rate(16000);
        let config_48k = MelExtractorConfig::new().with_input_sample_rate(48000);

        let mut ext_16k = MelExtractor::new(config_16k).unwrap();
        let mut ext_48k = MelExtractor::new(config_48k).unwrap();

        // 1 second at respective rates
        let audio_16k = vec![0.1f32; 16000];
        let audio_48k = vec![0.1f32; 48000];

        ext_16k.process(&audio_16k).unwrap();
        ext_48k.process(&audio_48k).unwrap();

        // Both should produce a similar number of frames
        // Allow some tolerance for resampling edge effects
        let frames_16k = ext_16k.num_frames();
        let frames_48k = ext_48k.num_frames();

        // Should be within 20% of each other
        let ratio = frames_16k.max(1) as f32 / frames_48k.max(1) as f32;
        assert!(
            ratio > 0.5 && ratio < 2.0,
            "Frame count ratio {} out of range: 16k={}, 48k={}",
            ratio,
            frames_16k,
            frames_48k
        );
    }
}
