//! Silero VAD detector implementation using ONNX Runtime.
//!
//! This module provides the core inference logic for the Silero VAD model.

use anyhow::{Context, Result};
use ndarray::{Array1, Array2, Array3};
use ort::session::{Session, builder::SessionBuilder};
use std::path::Path;
use std::time::Instant;
use tracing::{debug, info, trace};

use super::config::SileroVADConfig;

// Import SIMD-optimized operations
use crate::utils::simd_ops;

/// Default URL for downloading the Silero VAD model.
const SILERO_VAD_MODEL_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";

/// Silero VAD v5 model dimensions.
const SILERO_V5_HIDDEN_SIZE: usize = 64;

/// Result of VAD processing for a single chunk.
#[derive(Debug, Clone, Copy)]
pub struct SileroVADResult {
    /// Speech probability [0.0, 1.0]
    pub probability: f32,

    /// Whether speech is detected (probability > threshold)
    pub is_speech: bool,

    /// Inference time in microseconds
    pub inference_time_us: u64,
}

/// Silero VAD detector using ONNX Runtime.
///
/// This detector wraps the Silero VAD v5 model and provides:
/// - Single-chunk inference with state management
/// - Speech start/end detection with configurable debouncing
/// - Automatic periodic state reset to prevent drift
/// - Thread-safe state for concurrent access
pub struct SileroVAD {
    /// ONNX inference session
    session: Session,

    /// Configuration
    config: SileroVADConfig,

    /// LSTM hidden state (h)
    /// Shape: [2, 1, 64] for Silero v5
    state_h: Array3<f32>,

    /// LSTM cell state (c)
    /// Shape: [2, 1, 64] for Silero v5
    state_c: Array3<f32>,

    /// Sample rate tensor (constant)
    /// Shape: [1]
    sample_rate_tensor: Array1<i64>,

    /// Pre-allocated input buffer
    /// Shape: [1, chunk_size]
    input_buffer: Array2<f32>,

    /// Consecutive speech frames count
    speech_frames: u32,

    /// Consecutive silence frames count
    silence_frames: u32,

    /// Whether currently in speech state
    in_speech: bool,

    /// Total frames processed
    total_frames: u64,

    /// Time of last state reset (monotonic)
    last_state_reset: Instant,
}

impl SileroVAD {
    /// Creates a new Silero VAD detector.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the detector
    ///
    /// # Returns
    ///
    /// A new `SileroVAD` instance or an error if initialization fails.
    pub async fn new(config: SileroVADConfig) -> Result<Self> {
        config
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid VAD configuration: {}", e))?;

        info!("Initializing Silero VAD with config: {:?}", config);

        // Determine model path
        let model_path = if let Some(path) = &config.model_path {
            path.clone()
        } else {
            Self::get_or_download_model().await?
        };

        // Create ONNX session (spawn_blocking to avoid blocking async runtime)
        let session = tokio::task::spawn_blocking({
            let num_threads = config.num_threads;
            let model_path = model_path.clone();
            move || Self::create_session(&model_path, num_threads)
        })
        .await
        .context("Failed to spawn session creation task")?
        .context("Failed to create ONNX session")?;

        // Initialize state tensors
        let state_h = Array3::zeros((2, 1, SILERO_V5_HIDDEN_SIZE));
        let state_c = Array3::zeros((2, 1, SILERO_V5_HIDDEN_SIZE));
        let sample_rate_tensor = Array1::from_vec(vec![config.sample_rate as i64]);
        let input_buffer = Array2::zeros((1, config.chunk_size));

        info!(
            "Silero VAD initialized successfully. Model: {:?}",
            model_path
        );

        Ok(Self {
            session,
            config,
            state_h,
            state_c,
            sample_rate_tensor,
            input_buffer,
            speech_frames: 0,
            silence_frames: 0,
            in_speech: false,
            total_frames: 0,
            last_state_reset: Instant::now(),
        })
    }

    /// Creates an ONNX session for the model.
    fn create_session(model_path: &Path, num_threads: usize) -> Result<Session> {
        let session = SessionBuilder::new()?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)?
            .with_intra_threads(num_threads)?
            .with_inter_threads(1)?
            .commit_from_file(model_path)
            .context("Failed to load Silero VAD model")?;

        debug!(
            "ONNX session created with {} inputs, {} outputs",
            session.inputs.len(),
            session.outputs.len()
        );

        Ok(session)
    }

    /// Gets the model path, downloading if necessary.
    async fn get_or_download_model() -> Result<std::path::PathBuf> {
        // Check standard locations
        let possible_paths = [
            std::path::PathBuf::from("models/silero_vad.onnx"),
            std::path::PathBuf::from("./silero_vad.onnx"),
            dirs::cache_dir()
                .unwrap_or_default()
                .join("waav")
                .join("models")
                .join("silero_vad.onnx"),
        ];

        for path in &possible_paths {
            if path.exists() {
                info!("Found Silero VAD model at: {:?}", path);
                return Ok(path.clone());
            }
        }

        // Download the model
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("waav")
            .join("models");

        tokio::fs::create_dir_all(&cache_dir)
            .await
            .context("Failed to create model cache directory")?;

        let model_path = cache_dir.join("silero_vad.onnx");

        info!("Downloading Silero VAD model to {:?}", model_path);

        let response = reqwest::get(SILERO_VAD_MODEL_URL)
            .await
            .context("Failed to download Silero VAD model")?;

        let bytes = response
            .bytes()
            .await
            .context("Failed to read model bytes")?;

        tokio::fs::write(&model_path, &bytes)
            .await
            .context("Failed to write model file")?;

        info!("Downloaded Silero VAD model ({} bytes)", bytes.len());

        Ok(model_path)
    }

    /// Processes a single audio chunk and returns the speech probability.
    ///
    /// # Arguments
    ///
    /// * `audio` - Audio samples (must be exactly `chunk_size` samples)
    ///
    /// # Returns
    ///
    /// VAD result containing speech probability and detection status.
    ///
    /// # Notes
    ///
    /// This method automatically resets internal LSTM states periodically
    /// based on the `state_reset_interval_secs` configuration to prevent
    /// state drift in long conversations.
    ///
    /// # Panics
    ///
    /// Panics if `audio.len() != config.chunk_size`.
    pub fn process(&mut self, audio: &[f32]) -> Result<SileroVADResult> {
        assert_eq!(
            audio.len(),
            self.config.chunk_size,
            "Audio chunk must be exactly {} samples, got {}",
            self.config.chunk_size,
            audio.len()
        );

        let start = Instant::now();

        // Check for automatic state reset (prevents drift in long conversations)
        self.maybe_reset_state();

        // Copy audio to input buffer using SIMD-optimized copy
        // This uses vectorized memcpy for efficient transfer
        let input_slice = self
            .input_buffer
            .as_slice_mut()
            .expect("Input buffer should be contiguous");
        simd_ops::copy_to_tensor_simd(audio, input_slice);

        // Run inference
        let probability = self.run_inference()?;
        let inference_time_us = start.elapsed().as_micros() as u64;

        // Determine if speech is detected
        let is_speech = probability > self.config.threshold;

        // Update state tracking
        self.update_state(is_speech);
        self.total_frames += 1;

        if self.config.debug_logging {
            trace!(
                "VAD frame {}: prob={:.4}, is_speech={}, in_speech={}, inference={}us",
                self.total_frames, probability, is_speech, self.in_speech, inference_time_us
            );
        }

        Ok(SileroVADResult {
            probability,
            is_speech,
            inference_time_us,
        })
    }

    /// Checks if enough time has passed since the last state reset and
    /// resets the internal LSTM states if needed.
    ///
    /// This prevents state accumulation/drift that can occur in long
    /// conversations, matching the behavior of the pipecat/smart-turn
    /// reference implementation.
    fn maybe_reset_state(&mut self) {
        // Skip if automatic reset is disabled (interval == 0)
        if self.config.state_reset_interval_secs <= 0.0 {
            return;
        }

        let elapsed = self.last_state_reset.elapsed().as_secs_f32();
        if elapsed >= self.config.state_reset_interval_secs {
            // Reset only the LSTM states, not the speech/silence tracking
            self.state_h.fill(0.0);
            self.state_c.fill(0.0);
            self.last_state_reset = Instant::now();

            if self.config.debug_logging {
                debug!(
                    "VAD state auto-reset after {:.2}s (interval: {}s)",
                    elapsed, self.config.state_reset_interval_secs
                );
            }
        }
    }

    /// Runs ONNX inference and returns the speech probability.
    fn run_inference(&mut self) -> Result<f32> {
        // Prepare input tensors using ort 2.0 API
        // Silero VAD v5 expects: input, sr, h, c
        // Must create Tensor objects from raw data (shape, data) tuples

        // Input audio: shape [1, chunk_size]
        let input_data: Vec<f32> = self.input_buffer.iter().copied().collect();
        let input_tensor = ort::value::Tensor::from_array((
            [1usize, self.config.chunk_size],
            input_data.into_boxed_slice(),
        ))?;

        // Sample rate: shape [1]
        let sr_data: Vec<i64> = self.sample_rate_tensor.iter().copied().collect();
        let sr_tensor = ort::value::Tensor::from_array(([1usize], sr_data.into_boxed_slice()))?;

        // Hidden state h: shape [2, 1, 64]
        let h_data: Vec<f32> = self.state_h.iter().copied().collect();
        let h_tensor = ort::value::Tensor::from_array((
            [2usize, 1usize, SILERO_V5_HIDDEN_SIZE],
            h_data.into_boxed_slice(),
        ))?;

        // Cell state c: shape [2, 1, 64]
        let c_data: Vec<f32> = self.state_c.iter().copied().collect();
        let c_tensor = ort::value::Tensor::from_array((
            [2usize, 1usize, SILERO_V5_HIDDEN_SIZE],
            c_data.into_boxed_slice(),
        ))?;

        // Run inference
        let outputs = self.session.run(ort::inputs![
            "input" => input_tensor,
            "sr" => sr_tensor,
            "h" => h_tensor,
            "c" => c_tensor,
        ])?;

        // Extract outputs using ort 2.0 API
        // try_extract_tensor returns (&Shape, &[T]) tuple
        // output: probability, hn: new hidden state, cn: new cell state
        let probability = {
            let output_tensor = outputs
                .get("output")
                .context("Missing 'output' in model outputs")?;
            let (_shape, data) = output_tensor
                .try_extract_tensor::<f32>()
                .context("Failed to extract output tensor")?;
            // Probability is at index 0 (shape is [1, 1])
            data[0]
        };

        // Update hidden states
        if let Some(hn) = outputs.get("hn") {
            let (_shape, hn_data) = hn
                .try_extract_tensor::<f32>()
                .context("Failed to extract hn tensor")?;
            // Shape is [2, 1, 64], stored in row-major order
            for i in 0..2 {
                for j in 0..SILERO_V5_HIDDEN_SIZE {
                    // Linear index: i * (1 * 64) + 0 * 64 + j = i * 64 + j
                    self.state_h[[i, 0, j]] = hn_data[i * SILERO_V5_HIDDEN_SIZE + j];
                }
            }
        }

        if let Some(cn) = outputs.get("cn") {
            let (_shape, cn_data) = cn
                .try_extract_tensor::<f32>()
                .context("Failed to extract cn tensor")?;
            // Shape is [2, 1, 64], stored in row-major order
            for i in 0..2 {
                for j in 0..SILERO_V5_HIDDEN_SIZE {
                    // Linear index: i * (1 * 64) + 0 * 64 + j = i * 64 + j
                    self.state_c[[i, 0, j]] = cn_data[i * SILERO_V5_HIDDEN_SIZE + j];
                }
            }
        }

        Ok(probability)
    }

    /// Updates internal state tracking for speech start/end detection.
    fn update_state(&mut self, is_speech: bool) {
        if is_speech {
            self.speech_frames += 1;
            self.silence_frames = 0;

            // Check for speech start
            if !self.in_speech {
                let speech_duration_ms =
                    self.speech_frames as f32 * self.config.chunk_duration_ms();
                if speech_duration_ms >= self.config.min_speech_duration_ms as f32 {
                    self.in_speech = true;
                    if self.config.debug_logging {
                        debug!(
                            "Speech started after {}ms ({} frames)",
                            speech_duration_ms, self.speech_frames
                        );
                    }
                }
            }
        } else {
            self.silence_frames += 1;
            self.speech_frames = 0;

            // Check for speech end
            if self.in_speech {
                let silence_duration_ms =
                    self.silence_frames as f32 * self.config.chunk_duration_ms();
                if silence_duration_ms >= self.config.min_silence_duration_ms as f32 {
                    self.in_speech = false;
                    if self.config.debug_logging {
                        debug!(
                            "Speech ended after {}ms of silence ({} frames)",
                            silence_duration_ms, self.silence_frames
                        );
                    }
                }
            }
        }
    }

    /// Returns whether currently in speech state (with debouncing).
    #[inline]
    pub fn is_in_speech(&self) -> bool {
        self.in_speech
    }

    /// Returns the duration of current silence in milliseconds.
    #[inline]
    pub fn silence_duration_ms(&self) -> f32 {
        self.silence_frames as f32 * self.config.chunk_duration_ms()
    }

    /// Returns the duration of current speech in milliseconds.
    #[inline]
    pub fn speech_duration_ms(&self) -> f32 {
        self.speech_frames as f32 * self.config.chunk_duration_ms()
    }

    /// Resets the VAD state (hidden states and counters).
    ///
    /// Call this when starting a new conversation or after a long pause.
    pub fn reset(&mut self) {
        self.state_h.fill(0.0);
        self.state_c.fill(0.0);
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.in_speech = false;
        self.total_frames = 0;
        self.last_state_reset = Instant::now();

        if self.config.debug_logging {
            debug!("VAD state reset");
        }
    }

    /// Returns the time since the last state reset in seconds.
    #[inline]
    pub fn time_since_state_reset_secs(&self) -> f32 {
        self.last_state_reset.elapsed().as_secs_f32()
    }

    /// Returns the total number of frames processed.
    #[inline]
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Returns a reference to the configuration.
    #[inline]
    pub fn config(&self) -> &SileroVADConfig {
        &self.config
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Most tests require the ONNX model file.
    // These are integration tests that should be run with the model available.

    #[test]
    fn test_vad_result_struct() {
        let result = SileroVADResult {
            probability: 0.75,
            is_speech: true,
            inference_time_us: 500,
        };

        assert_eq!(result.probability, 0.75);
        assert!(result.is_speech);
        assert_eq!(result.inference_time_us, 500);
    }

    #[test]
    fn test_config_chunk_duration() {
        let config = SileroVADConfig::default();
        // 512 samples at 16kHz = 32ms
        let duration = config.chunk_duration_ms();
        assert!((duration - 32.0).abs() < 0.01);
    }

    // Integration tests (require model file)
    #[cfg(feature = "silero-vad-integration-tests")]
    mod integration {
        use super::*;

        #[tokio::test]
        async fn test_vad_initialization() {
            let config = SileroVADConfig::default();
            let vad = SileroVAD::new(config).await;
            assert!(vad.is_ok());
        }

        #[tokio::test]
        async fn test_vad_process_silence() {
            let config = SileroVADConfig::default();
            let mut vad = SileroVAD::new(config).await.unwrap();

            // Process silence (zeros)
            let silence = vec![0.0f32; 512];
            let result = vad.process(&silence).unwrap();

            // Silence should have low probability
            assert!(result.probability < 0.3);
            assert!(!result.is_speech);
        }

        #[tokio::test]
        async fn test_vad_process_speech() {
            let config = SileroVADConfig::default();
            let mut vad = SileroVAD::new(config).await.unwrap();

            // Generate synthetic speech-like signal (sine wave)
            let speech: Vec<f32> = (0..512).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();

            let result = vad.process(&speech).unwrap();

            // This is a synthetic test - actual results depend on model
            // Just verify we get a valid probability
            assert!(result.probability >= 0.0 && result.probability <= 1.0);
        }

        #[tokio::test]
        async fn test_vad_state_persistence() {
            let config = SileroVADConfig::default();
            let mut vad = SileroVAD::new(config).await.unwrap();

            // Process multiple chunks
            let chunk = vec![0.0f32; 512];
            for _ in 0..10 {
                vad.process(&chunk).unwrap();
            }

            assert_eq!(vad.total_frames(), 10);
        }

        #[tokio::test]
        async fn test_vad_reset() {
            let config = SileroVADConfig::default();
            let mut vad = SileroVAD::new(config).await.unwrap();

            // Process some chunks
            let chunk = vec![0.0f32; 512];
            for _ in 0..5 {
                vad.process(&chunk).unwrap();
            }

            assert_eq!(vad.total_frames(), 5);

            // Reset
            vad.reset();

            assert_eq!(vad.total_frames(), 0);
            assert!(!vad.is_in_speech());
        }

        #[tokio::test]
        async fn test_vad_inference_performance() {
            let config = SileroVADConfig::default();
            let mut vad = SileroVAD::new(config).await.unwrap();

            let chunk = vec![0.0f32; 512];
            let mut total_time_us = 0u64;
            let iterations = 100;

            for _ in 0..iterations {
                let result = vad.process(&chunk).unwrap();
                total_time_us += result.inference_time_us;
            }

            let avg_time_us = total_time_us / iterations as u64;
            println!("Average inference time: {}us", avg_time_us);

            // Should be well under 1ms (1000us) per chunk
            assert!(avg_time_us < 2000, "Inference too slow: {}us", avg_time_us);
        }
    }
}
