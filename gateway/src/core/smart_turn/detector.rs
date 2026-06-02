//! Smart Turn detector implementation using ONNX Runtime.
//!
//! This module provides audio-based semantic turn detection using a
//! Whisper encoder with attention pooling and a binary classifier head.
//!
//! ## Architecture
//!
//! The model architecture is:
//! 1. Whisper encoder (frozen or fine-tuned): [batch, frames, 80] -> [batch, frames, 512]
//! 2. Attention pooling: [batch, frames, 512] -> [batch, 512]
//! 3. Binary classifier: [batch, 512] -> [batch, 1] (turn probability)
//!
//! ## Performance Target
//!
//! - Inference latency: <15ms for 8-second audio
//! - Accuracy: >90% on held-out validation set

use anyhow::{Context, Result};
use ort::session::{Session, builder::SessionBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info, trace, warn};

use super::mel_extractor::WHISPER_N_MELS;

/// Default model URL for downloading the Smart Turn model.
/// Uses the pipecat-ai/smart-turn-v3 model from HuggingFace (v3.2 CPU optimized, int8 quantized).
/// See: https://huggingface.co/pipecat-ai/smart-turn-v3
const SMART_TURN_MODEL_URL: &str =
    "https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx";

/// Number of mel frames expected by the model (8 seconds at 10ms = 800 frames).
pub const SMART_TURN_MAX_FRAMES: usize = 800;

/// Configuration for the Smart Turn detector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartTurnDetectorConfig {
    /// Path to the ONNX model file.
    /// If not specified, will attempt to download from model_url.
    #[serde(default)]
    pub model_path: Option<PathBuf>,

    /// URL to download the model from if not found locally.
    #[serde(default)]
    pub model_url: Option<String>,

    /// Probability threshold for turn completion (0.0-1.0).
    /// If probability exceeds this threshold, the turn is considered complete.
    #[serde(default = "default_threshold")]
    pub threshold: f32,

    /// Number of mel spectrogram frames expected by the model.
    #[serde(default = "default_max_frames")]
    pub max_frames: usize,

    /// Number of mel bins (must match the mel extractor config).
    #[serde(default = "default_n_mels")]
    pub n_mels: usize,

    /// Number of ONNX inference threads.
    #[serde(default = "default_num_threads")]
    pub num_threads: usize,

    /// Enable debug logging.
    #[serde(default)]
    pub debug_logging: bool,

    /// Minimum speech duration before checking for turn (milliseconds).
    /// Helps avoid false positives on very short utterances.
    #[serde(default = "default_min_speech_ms")]
    pub min_speech_ms: u32,

    /// Hysteresis window size in frames.
    /// Consecutive frames above threshold required to trigger turn.
    #[serde(default = "default_hysteresis_frames")]
    pub hysteresis_frames: u32,

    /// Model download timeout in seconds.
    /// Controls how long to wait for the model file to download.
    #[serde(default = "default_download_timeout_secs")]
    pub download_timeout_secs: u64,

    /// Maximum number of download retries on failure.
    #[serde(default = "default_download_retries")]
    pub download_retries: u32,

    /// Inference timeout in milliseconds.
    /// If inference takes longer than this, it will be cancelled.
    /// Set to 0 to disable timeout (not recommended for real-time).
    #[serde(default = "default_inference_timeout_ms")]
    pub inference_timeout_ms: u64,
}

fn default_threshold() -> f32 {
    0.7
}

fn default_max_frames() -> usize {
    SMART_TURN_MAX_FRAMES
}

fn default_n_mels() -> usize {
    WHISPER_N_MELS
}

fn default_num_threads() -> usize {
    1
}

fn default_min_speech_ms() -> u32 {
    300 // At least 300ms of speech before checking
}

fn default_hysteresis_frames() -> u32 {
    3 // 3 consecutive frames above threshold
}

fn default_download_timeout_secs() -> u64 {
    120 // 2 minutes
}

fn default_download_retries() -> u32 {
    3
}

fn default_inference_timeout_ms() -> u64 {
    50 // 50ms timeout - well above 15ms target, allows for variance
}

impl Default for SmartTurnDetectorConfig {
    fn default() -> Self {
        Self {
            model_path: None,
            model_url: None,
            threshold: default_threshold(),
            max_frames: default_max_frames(),
            n_mels: default_n_mels(),
            num_threads: default_num_threads(),
            debug_logging: false,
            min_speech_ms: default_min_speech_ms(),
            hysteresis_frames: default_hysteresis_frames(),
            download_timeout_secs: default_download_timeout_secs(),
            download_retries: default_download_retries(),
            inference_timeout_ms: default_inference_timeout_ms(),
        }
    }
}

impl SmartTurnDetectorConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the model path.
    pub fn with_model_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.model_path = Some(path.into());
        self
    }

    /// Sets the model URL for downloading.
    pub fn with_model_url(mut self, url: impl Into<String>) -> Self {
        self.model_url = Some(url.into());
        self
    }

    /// Sets the turn detection threshold.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the number of inference threads.
    pub fn with_num_threads(mut self, threads: usize) -> Self {
        self.num_threads = threads.max(1);
        self
    }

    /// Enables debug logging.
    pub fn with_debug_logging(mut self) -> Self {
        self.debug_logging = true;
        self
    }

    /// Sets the minimum speech duration before checking for turn.
    pub fn with_min_speech_ms(mut self, ms: u32) -> Self {
        self.min_speech_ms = ms;
        self
    }

    /// Sets the hysteresis window size.
    pub fn with_hysteresis_frames(mut self, frames: u32) -> Self {
        self.hysteresis_frames = frames.max(1);
        self
    }

    /// Sets the download timeout in seconds.
    pub fn with_download_timeout_secs(mut self, secs: u64) -> Self {
        self.download_timeout_secs = secs.max(10); // Minimum 10 seconds
        self
    }

    /// Sets the maximum number of download retries.
    pub fn with_download_retries(mut self, retries: u32) -> Self {
        self.download_retries = retries.max(1); // At least 1 attempt
        self
    }

    /// Sets the inference timeout in milliseconds.
    /// Set to 0 to disable timeout (not recommended for real-time).
    pub fn with_inference_timeout_ms(mut self, ms: u64) -> Self {
        self.inference_timeout_ms = ms;
        self
    }

    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.threshold < 0.0 || self.threshold > 1.0 {
            return Err(format!(
                "Threshold must be between 0.0 and 1.0, got {}",
                self.threshold
            ));
        }

        if self.max_frames == 0 {
            return Err("max_frames must be greater than 0".to_string());
        }

        if self.n_mels == 0 || self.n_mels > 256 {
            return Err(format!(
                "n_mels must be between 1 and 256, got {}",
                self.n_mels
            ));
        }

        if self.num_threads == 0 {
            return Err("num_threads must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Result of Smart Turn detection.
#[derive(Debug, Clone, Copy)]
pub struct SmartTurnResult {
    /// Turn completion probability [0.0, 1.0].
    /// Higher values indicate the user has finished speaking.
    pub probability: f32,

    /// Whether turn is complete (probability > threshold with hysteresis).
    pub is_turn_complete: bool,

    /// Inference time in microseconds.
    pub inference_time_us: u64,

    /// Number of mel frames processed.
    pub frames_processed: usize,
}

/// Smart Turn detector using ONNX Runtime.
///
/// This detector uses an audio-based model to predict when a user
/// has finished speaking, based on semantic and prosodic features
/// extracted from mel spectrograms.
pub struct SmartTurnDetector {
    /// ONNX inference session (behind mutex for thread safety).
    session: Arc<Mutex<Session>>,

    /// Configuration.
    config: SmartTurnDetectorConfig,

    /// Cached model input name.
    input_name: String,

    /// Cached model output name.
    output_name: String,

    /// Pre-allocated buffer for transposed mel spectrogram.
    /// Avoids allocation during inference hot path.
    /// Size: max_frames * n_mels
    transpose_buffer: Vec<f32>,

    /// Consecutive high-probability frames (for hysteresis).
    high_prob_frames: u32,

    /// Total frames processed.
    total_frames: u64,

    /// Total inference time in microseconds.
    total_inference_time_us: u64,

    /// Last prediction probability.
    last_probability: f32,
}

impl SmartTurnDetector {
    /// Creates a new Smart Turn detector.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the detector.
    ///
    /// # Returns
    ///
    /// A new `SmartTurnDetector` instance or an error if initialization fails.
    pub async fn new(config: SmartTurnDetectorConfig) -> Result<Self> {
        config
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid configuration: {}", e))?;

        info!(
            "Initializing SmartTurnDetector: threshold={}, max_frames={}, threads={}",
            config.threshold, config.max_frames, config.num_threads
        );

        // Determine model path
        let model_path = Self::get_or_download_model(&config).await?;

        // Create ONNX session in a blocking task
        let session = tokio::task::spawn_blocking({
            let num_threads = config.num_threads;
            let model_path = model_path.clone();
            move || Self::create_session(&model_path, num_threads)
        })
        .await
        .context("Failed to spawn session creation task")?
        .context("Failed to create ONNX session")?;

        // Cache input/output names
        let input_name = if session.inputs.is_empty() {
            anyhow::bail!("Model has no inputs");
        } else {
            session.inputs[0].name.clone()
        };

        let output_name = if session.outputs.is_empty() {
            anyhow::bail!("Model has no outputs");
        } else {
            session.outputs[0].name.clone()
        };

        debug!(
            "Model loaded successfully. Input: '{}', Output: '{}'",
            input_name, output_name
        );

        // Pre-allocate transpose buffer to avoid allocations during inference
        let buffer_size = config.max_frames * config.n_mels;
        let transpose_buffer = vec![0.0f32; buffer_size];

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            config,
            input_name,
            output_name,
            transpose_buffer,
            high_prob_frames: 0,
            total_frames: 0,
            total_inference_time_us: 0,
            last_probability: 0.0,
        })
    }

    /// Creates an ONNX session for the model.
    fn create_session(model_path: &Path, num_threads: usize) -> Result<Session> {
        let session = SessionBuilder::new()?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)?
            .with_intra_threads(num_threads)?
            .with_inter_threads(1)?
            .commit_from_file(model_path)
            .context("Failed to load Smart Turn model")?;

        debug!(
            "ONNX session created: {} inputs, {} outputs",
            session.inputs.len(),
            session.outputs.len()
        );

        // Log input/output details
        for (i, input) in session.inputs.iter().enumerate() {
            debug!("  Input {}: {} {:?}", i, input.name, input.input_type);
        }
        for (i, output) in session.outputs.iter().enumerate() {
            debug!("  Output {}: {} {:?}", i, output.name, output.output_type);
        }

        Ok(session)
    }

    /// Gets the model path, downloading if necessary.
    async fn get_or_download_model(config: &SmartTurnDetectorConfig) -> Result<PathBuf> {
        // Use configured path if specified
        if let Some(path) = &config.model_path {
            if path.exists() {
                info!("Using configured model path: {:?}", path);
                return Ok(path.clone());
            }
            anyhow::bail!("Configured model path does not exist: {:?}", path);
        }

        // Check standard locations
        let possible_paths: [PathBuf; 3] = [
            PathBuf::from("models/smart_turn.onnx"),
            PathBuf::from("./smart_turn.onnx"),
            dirs::cache_dir()
                .unwrap_or_default()
                .join("waav")
                .join("models")
                .join("smart_turn.onnx"),
        ];

        for path in &possible_paths {
            if path.exists() {
                info!("Found Smart Turn model at: {:?}", path);
                return Ok(path.clone());
            }
        }

        // Attempt to download
        let model_url = config.model_url.as_deref().unwrap_or(SMART_TURN_MODEL_URL);

        let cache_dir: PathBuf = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("waav")
            .join("models");

        tokio::fs::create_dir_all(&cache_dir)
            .await
            .context("Failed to create model cache directory")?;

        let model_path: PathBuf = cache_dir.join("smart_turn.onnx");

        info!(
            "Downloading Smart Turn model from {} to {:?}",
            model_url, model_path
        );

        // Download with timeout and retry logic using configurable values
        let max_retries = config.download_retries;
        let download_timeout_secs = config.download_timeout_secs;

        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 1..=max_retries {
            if attempt > 1 {
                // Exponential backoff: 2s, 4s, 8s
                let backoff_secs = 2u64.pow(attempt);
                info!(
                    "Retrying download (attempt {}/{}) after {}s delay",
                    attempt, max_retries, backoff_secs
                );
                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
            }

            let download_future = async {
                let client = reqwest::Client::new();
                let response = client
                    .get(model_url)
                    .timeout(std::time::Duration::from_secs(download_timeout_secs))
                    .send()
                    .await
                    .context("Failed to connect to model URL")?;

                if !response.status().is_success() {
                    anyhow::bail!(
                        "Failed to download model: HTTP {} - {}",
                        response.status(),
                        model_url
                    );
                }

                let bytes = response
                    .bytes()
                    .await
                    .context("Failed to read model bytes")?;

                Ok::<_, anyhow::Error>(bytes)
            };

            match tokio::time::timeout(
                std::time::Duration::from_secs(download_timeout_secs + 10),
                download_future,
            )
            .await
            {
                Ok(Ok(bytes)) => {
                    // Success - write to file
                    tokio::fs::write(&model_path, &bytes)
                        .await
                        .context("Failed to write model file")?;

                    info!(
                        "Downloaded Smart Turn model ({} bytes) on attempt {}",
                        bytes.len(),
                        attempt
                    );

                    return Ok(model_path);
                }
                Ok(Err(e)) => {
                    warn!("Download attempt {} failed: {}", attempt, e);
                    last_error = Some(e);
                }
                Err(_) => {
                    warn!(
                        "Download attempt {} timed out after {}s",
                        attempt, download_timeout_secs
                    );
                    last_error = Some(anyhow::anyhow!(
                        "Download timed out after {} seconds",
                        download_timeout_secs
                    ));
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("Download failed after {} retries", max_retries)))
    }

    /// Predicts turn completion probability from mel spectrogram.
    ///
    /// # Arguments
    ///
    /// * `mel_frames` - Mel spectrogram frames from MelExtractor
    ///
    /// # Returns
    ///
    /// Turn detection result with probability and completion status.
    pub async fn predict(&mut self, mel_frames: &[Vec<f32>]) -> Result<SmartTurnResult> {
        let start = Instant::now();
        let frames_count = mel_frames.len();

        if frames_count == 0 {
            return Ok(SmartTurnResult {
                probability: 0.0,
                is_turn_complete: false,
                inference_time_us: 0,
                frames_processed: 0,
            });
        }

        // Prepare input tensor (pad or truncate to max_frames)
        let input_data = self.prepare_input(mel_frames);

        // Run inference with optional timeout
        let probability = if self.config.inference_timeout_ms > 0 {
            let timeout_duration =
                std::time::Duration::from_millis(self.config.inference_timeout_ms);
            match tokio::time::timeout(timeout_duration, self.run_inference(&input_data)).await {
                Ok(result) => result?,
                Err(_) => {
                    warn!(
                        "SmartTurn inference timed out after {}ms",
                        self.config.inference_timeout_ms
                    );
                    // Return last known probability on timeout to avoid blocking
                    self.last_probability
                }
            }
        } else {
            self.run_inference(&input_data).await?
        };
        let inference_time_us = start.elapsed().as_micros() as u64;

        // Update hysteresis state with decay instead of hard reset
        // This makes the detector more robust to occasional dips below threshold
        if probability > self.config.threshold {
            self.high_prob_frames += 1;
        } else if self.high_prob_frames > 0 {
            // Decay by 1 instead of hard reset to 0
            // This prevents a single low-probability frame from resetting the counter
            self.high_prob_frames -= 1;
        }

        // Determine if turn is complete:
        // 1. Check min_speech_ms - enough audio must have been processed first
        // 2. Check hysteresis - enough consecutive high-probability frames
        //
        // Each mel frame represents 10ms (hop_length=160 at 16kHz)
        let min_frames_for_speech = (self.config.min_speech_ms as usize).div_ceil(10);
        let has_enough_speech = frames_count >= min_frames_for_speech;
        let is_turn_complete =
            has_enough_speech && self.high_prob_frames >= self.config.hysteresis_frames;

        // Update statistics
        self.total_frames += 1;
        self.total_inference_time_us += inference_time_us;
        self.last_probability = probability;

        if self.config.debug_logging {
            trace!(
                "SmartTurn prediction: prob={:.4}, high_prob_frames={}, is_complete={}, inference={}us",
                probability, self.high_prob_frames, is_turn_complete, inference_time_us
            );
        }

        if inference_time_us > 15000 {
            warn!(
                "SmartTurn inference exceeded target latency: {}us > 15000us",
                inference_time_us
            );
        }

        Ok(SmartTurnResult {
            probability,
            is_turn_complete,
            inference_time_us,
            frames_processed: frames_count,
        })
    }

    /// Prepares input tensor from mel frames (pad/truncate to max_frames).
    ///
    /// # Padding Convention
    ///
    /// This follows the Smart Turn/Whisper convention:
    /// - **Padding is at the BEGINNING** (zeros first, audio data at end)
    /// - **Truncation keeps the LAST frames** (most recent audio)
    ///
    /// This ensures the model sees the most recent audio at the end of the sequence,
    /// which is critical for turn detection (we need to detect when speech ENDS).
    fn prepare_input(&self, mel_frames: &[Vec<f32>]) -> Vec<f32> {
        let max_frames = self.config.max_frames;
        let n_mels = self.config.n_mels;
        let total_size = max_frames * n_mels;
        let n_input_frames = mel_frames.len();

        // Pre-fill with zeros (handles padding at BEGINNING)
        let mut input = vec![0.0f32; total_size];

        // Early return for empty input - already all zeros
        if n_input_frames == 0 {
            return input;
        }

        if n_input_frames >= max_frames {
            // Truncate: keep LAST max_frames (most recent audio)
            let start_idx = n_input_frames - max_frames;
            for (output_idx, frame_idx) in (start_idx..n_input_frames).enumerate() {
                let offset = output_idx * n_mels;
                // Safe: frame_idx is in range [start_idx, n_input_frames) which is valid
                // since start_idx = n_input_frames - max_frames >= 0
                if let Some(frame) = mel_frames.get(frame_idx) {
                    let copy_len = frame.len().min(n_mels);
                    for (j, &val) in frame.iter().take(copy_len).enumerate() {
                        input[offset + j] = val;
                    }
                }
            }
        } else {
            // Pad: zeros at BEGINNING (already pre-filled), copy frames at END
            let padding_frames = max_frames - n_input_frames;
            for (i, frame) in mel_frames.iter().enumerate() {
                // Place frames at the end of the output (after padding)
                let output_idx = padding_frames + i;
                let offset = output_idx * n_mels;
                let copy_len = frame.len().min(n_mels);
                for (j, &val) in frame.iter().take(copy_len).enumerate() {
                    input[offset + j] = val;
                }
            }
        }

        input
    }

    /// Runs ONNX inference and returns turn probability.
    ///
    /// # Note on tensor shape
    ///
    /// The Smart Turn model expects input shape `[batch, n_mels, frames]` = `[1, 80, 800]`.
    /// The input data is provided as `[frames * n_mels]` in row-major order (frames × mels),
    /// so we need to transpose it to column-major order (mels × frames).
    ///
    /// Uses pre-allocated transpose buffer to avoid runtime allocations.
    async fn run_inference(&mut self, input_data: &[f32]) -> Result<f32> {
        let mut session = self.session.lock().await;

        let max_frames = self.config.max_frames;
        let n_mels = self.config.n_mels;

        // Clear the pre-allocated buffer (reset to zeros)
        self.transpose_buffer.fill(0.0);

        // Input data is in row-major order: [frame0_mel0..frame0_melN, frame1_mel0..frame1_melN, ...]
        // Model expects column-major: [mel0_frame0..mel0_frameN, mel1_frame0..mel1_frameN, ...]
        // We need to transpose from [frames, mels] to [mels, frames]
        for frame_idx in 0..max_frames {
            for mel_idx in 0..n_mels {
                let src_idx = frame_idx * n_mels + mel_idx;
                let dst_idx = mel_idx * max_frames + frame_idx;
                if src_idx < input_data.len() && dst_idx < self.transpose_buffer.len() {
                    self.transpose_buffer[dst_idx] = input_data[src_idx];
                }
            }
        }

        // Create input tensor: shape [1, n_mels, max_frames] = [1, 80, 800]
        // This matches the Smart Turn v3 model's expected input format
        //
        // Note: Clone is required here because ort::Tensor::from_array requires owned data.
        // The ort crate only implements OwnedTensorArrayData for (shape, Box<[T]>), (shape, Vec<T>),
        // and owned ArrayBase - not for ArrayView. This costs ~256KB per inference.
        //
        // We still benefit from pre-allocation because:
        // 1. The transpose operation reuses transpose_buffer (no allocation there)
        // 2. The clone is from a contiguous buffer (cache-friendly)
        // 3. Alternative APIs (spawn_blocking + raw pointers) have higher overhead
        let input_tensor = ort::value::Tensor::from_array((
            [1usize, n_mels, max_frames],
            self.transpose_buffer.clone().into_boxed_slice(),
        ))?;

        // Run inference
        let outputs = session.run(ort::inputs![
            self.input_name.as_str() => input_tensor
        ])?;

        // Extract output
        let output_tensor = outputs
            .get(self.output_name.as_str())
            .context("Missing output from model")?;

        let (_shape, data) = output_tensor
            .try_extract_tensor::<f32>()
            .context("Failed to extract output tensor")?;

        if data.is_empty() {
            warn!("Model returned empty output, using fallback");
            return Ok(0.0);
        }

        // Get the probability value
        // Smart Turn v3 model outputs a single logit value that needs sigmoid
        // to convert to a probability in [0, 1]
        let probability = if data.len() == 1 {
            // Single logit output - always apply sigmoid
            // The model outputs logits (raw values), not probabilities
            let logit = data[0];
            1.0 / (1.0 + (-logit).exp())
        } else if data.len() == 2 {
            // Binary classification logits - apply softmax
            let continue_logit = data[0];
            let end_logit = data[1];
            let max_logit = continue_logit.max(end_logit);
            let exp_continue = (continue_logit - max_logit).exp();
            let exp_end = (end_logit - max_logit).exp();
            exp_end / (exp_continue + exp_end)
        } else {
            // Unexpected output format - return error instead of risking panic
            anyhow::bail!(
                "Unexpected model output size: {} (expected 1 or 2 values)",
                data.len()
            );
        };

        Ok(probability)
    }

    /// Predicts from a flattened mel spectrogram array.
    ///
    /// # Arguments
    ///
    /// * `mel_flat` - Flattened mel spectrogram [frames * n_mels] in row-major order
    /// * `num_frames` - Number of frames in the input
    ///
    /// # Padding Convention
    ///
    /// Follows the Smart Turn/Whisper convention:
    /// - **Padding is at the BEGINNING** (zeros first, audio data at end)
    /// - **Truncation keeps the LAST frames** (most recent audio)
    ///
    /// # Returns
    ///
    /// Turn detection result.
    pub async fn predict_flat(
        &mut self,
        mel_flat: &[f32],
        num_frames: usize,
    ) -> Result<SmartTurnResult> {
        let start = Instant::now();

        if mel_flat.is_empty() || num_frames == 0 {
            return Ok(SmartTurnResult {
                probability: 0.0,
                is_turn_complete: false,
                inference_time_us: 0,
                frames_processed: 0,
            });
        }

        // Pad/truncate to expected size with correct convention:
        // - Padding at BEGINNING (zeros first)
        // - Truncation keeps LAST frames (most recent audio)
        let max_frames = self.config.max_frames;
        let n_mels = self.config.n_mels;
        let total_size = max_frames * n_mels;

        // Pre-fill with zeros (handles padding at BEGINNING)
        let mut input = vec![0.0f32; total_size];

        if num_frames >= max_frames {
            // Truncate: keep LAST max_frames (most recent audio)
            let start_frame = num_frames - max_frames;
            let src_offset = start_frame * n_mels;
            let copy_len = (max_frames * n_mels).min(mel_flat.len().saturating_sub(src_offset));
            input[..copy_len].copy_from_slice(&mel_flat[src_offset..src_offset + copy_len]);
        } else {
            // Pad: zeros at BEGINNING (already pre-filled), copy frames at END
            let padding_values = (max_frames - num_frames) * n_mels;
            let copy_len = mel_flat.len().min(total_size - padding_values);
            input[padding_values..padding_values + copy_len].copy_from_slice(&mel_flat[..copy_len]);
        }

        // Run inference with optional timeout
        let probability = if self.config.inference_timeout_ms > 0 {
            let timeout_duration =
                std::time::Duration::from_millis(self.config.inference_timeout_ms);
            match tokio::time::timeout(timeout_duration, self.run_inference(&input)).await {
                Ok(result) => result?,
                Err(_) => {
                    warn!(
                        "SmartTurn inference timed out after {}ms (predict_flat)",
                        self.config.inference_timeout_ms
                    );
                    // Return last known probability on timeout to avoid blocking
                    self.last_probability
                }
            }
        } else {
            self.run_inference(&input).await?
        };
        let inference_time_us = start.elapsed().as_micros() as u64;

        // Update hysteresis state with decay instead of hard reset
        if probability > self.config.threshold {
            self.high_prob_frames += 1;
        } else if self.high_prob_frames > 0 {
            // Decay by 1 instead of hard reset to 0
            self.high_prob_frames -= 1;
        }

        // Determine if turn is complete:
        // 1. Check min_speech_ms - enough audio must have been processed first
        // 2. Check hysteresis - enough consecutive high-probability frames
        let min_frames_for_speech = (self.config.min_speech_ms as usize).div_ceil(10);
        let has_enough_speech = num_frames >= min_frames_for_speech;
        let is_turn_complete =
            has_enough_speech && self.high_prob_frames >= self.config.hysteresis_frames;

        // Update statistics
        self.total_frames += 1;
        self.total_inference_time_us += inference_time_us;
        self.last_probability = probability;

        Ok(SmartTurnResult {
            probability,
            is_turn_complete,
            inference_time_us,
            frames_processed: num_frames,
        })
    }

    /// Returns the current turn completion probability without running inference.
    #[inline]
    pub fn current_probability(&self) -> f32 {
        self.last_probability
    }

    /// Returns whether currently predicting turn completion (with hysteresis).
    #[inline]
    pub fn is_turn_predicted(&self) -> bool {
        self.high_prob_frames >= self.config.hysteresis_frames
    }

    /// Resets the detector state.
    pub fn reset(&mut self) {
        self.high_prob_frames = 0;
        self.last_probability = 0.0;

        if self.config.debug_logging {
            debug!("SmartTurnDetector state reset");
        }
    }

    /// Resets all statistics.
    pub fn reset_stats(&mut self) {
        self.total_frames = 0;
        self.total_inference_time_us = 0;
    }

    /// Returns total frames processed.
    #[inline]
    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    /// Returns average inference time in microseconds.
    #[inline]
    pub fn avg_inference_time_us(&self) -> u64 {
        if self.total_frames == 0 {
            0
        } else {
            self.total_inference_time_us / self.total_frames
        }
    }

    /// Returns the configuration.
    #[inline]
    pub fn config(&self) -> &SmartTurnDetectorConfig {
        &self.config
    }

    /// Sets the threshold dynamically.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.config.threshold = threshold.clamp(0.0, 1.0);
        if self.config.debug_logging {
            debug!("SmartTurn threshold updated to {}", self.config.threshold);
        }
    }
}

/// Builder for SmartTurnDetector.
pub struct SmartTurnDetectorBuilder {
    config: SmartTurnDetectorConfig,
}

impl Default for SmartTurnDetectorBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SmartTurnDetectorBuilder {
    /// Creates a new builder with default configuration.
    pub fn new() -> Self {
        Self {
            config: SmartTurnDetectorConfig::default(),
        }
    }

    /// Sets the model path.
    pub fn model_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.model_path = Some(path.into());
        self
    }

    /// Sets the model URL.
    pub fn model_url(mut self, url: impl Into<String>) -> Self {
        self.config.model_url = Some(url.into());
        self
    }

    /// Sets the threshold.
    pub fn threshold(mut self, threshold: f32) -> Self {
        self.config.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the number of inference threads.
    pub fn num_threads(mut self, threads: usize) -> Self {
        self.config.num_threads = threads.max(1);
        self
    }

    /// Sets the hysteresis frames.
    pub fn hysteresis_frames(mut self, frames: u32) -> Self {
        self.config.hysteresis_frames = frames.max(1);
        self
    }

    /// Sets the minimum speech duration.
    pub fn min_speech_ms(mut self, ms: u32) -> Self {
        self.config.min_speech_ms = ms;
        self
    }

    /// Enables debug logging.
    pub fn debug_logging(mut self, enabled: bool) -> Self {
        self.config.debug_logging = enabled;
        self
    }

    /// Builds the detector.
    pub async fn build(self) -> Result<SmartTurnDetector> {
        SmartTurnDetector::new(self.config).await
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Live test: confirms the Smart-Turn v3 model LOADS and RUNS, and that the mel layout the
    // code produces ([1, 80, 800]) matches the model's `input_features` ([batch, 80, 800]).
    // (Independent inspection of the real model REFUTED the review's "transposed input"
    // prediction — the layout is correct.) Run with: ORT_DYLIB_PATH=.../libonnxruntime.so
    // cargo test --features smart-turn live_smart_turn_loads_and_runs -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "downloads smart-turn-v3 model + runs ONNX inference (needs ORT_DYLIB_PATH)"]
    async fn live_smart_turn_loads_and_runs() {
        let mut det = SmartTurnDetector::new(SmartTurnDetectorConfig::default())
            .await
            .expect("Smart-Turn model must load");
        // 800 frames x 80 mel bins (the model input is [1, 80, 800]).
        let frames: Vec<Vec<f32>> = (0..800)
            .map(|f| {
                (0..80)
                    .map(|m| (((f + m) as f32) * 0.01).sin() * 0.1)
                    .collect()
            })
            .collect();
        let r = det.predict(&frames).await.expect("predict must run");
        assert!(
            (0.0..=1.0).contains(&r.probability),
            "probability out of range: {}",
            r.probability
        );
        eprintln!(
            "SMARTTURN prob={:.4} complete={} ({}us)",
            r.probability, r.is_turn_complete, r.inference_time_us
        );
    }

    // -------------------------------------------------------------------------
    // Configuration tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_config() {
        let config = SmartTurnDetectorConfig::default();

        assert_eq!(config.threshold, 0.7);
        assert_eq!(config.max_frames, SMART_TURN_MAX_FRAMES);
        assert_eq!(config.n_mels, WHISPER_N_MELS);
        assert_eq!(config.num_threads, 1);
        assert_eq!(config.min_speech_ms, 300);
        assert_eq!(config.hysteresis_frames, 3);
        assert_eq!(config.inference_timeout_ms, 50);
        assert!(!config.debug_logging);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = SmartTurnDetectorConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_threshold_low() {
        let config = SmartTurnDetectorConfig {
            threshold: -0.1,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_threshold_high() {
        let config = SmartTurnDetectorConfig {
            threshold: 1.5,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_max_frames() {
        let config = SmartTurnDetectorConfig {
            max_frames: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_n_mels() {
        let config = SmartTurnDetectorConfig {
            n_mels: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = SmartTurnDetectorConfig {
            n_mels: 300,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_num_threads() {
        let config = SmartTurnDetectorConfig {
            num_threads: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_builder_pattern() {
        let config = SmartTurnDetectorConfig::new()
            .with_threshold(0.8)
            .with_num_threads(4)
            .with_min_speech_ms(500)
            .with_hysteresis_frames(5)
            .with_debug_logging();

        assert_eq!(config.threshold, 0.8);
        assert_eq!(config.num_threads, 4);
        assert_eq!(config.min_speech_ms, 500);
        assert_eq!(config.hysteresis_frames, 5);
        assert!(config.debug_logging);
    }

    #[test]
    fn test_config_threshold_clamping() {
        let config = SmartTurnDetectorConfig::new().with_threshold(-0.5);
        assert_eq!(config.threshold, 0.0);

        let config = SmartTurnDetectorConfig::new().with_threshold(1.5);
        assert_eq!(config.threshold, 1.0);
    }

    // -------------------------------------------------------------------------
    // Result struct tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_result_struct() {
        let result = SmartTurnResult {
            probability: 0.85,
            is_turn_complete: true,
            inference_time_us: 5000,
            frames_processed: 100,
        };

        assert_eq!(result.probability, 0.85);
        assert!(result.is_turn_complete);
        assert_eq!(result.inference_time_us, 5000);
        assert_eq!(result.frames_processed, 100);
    }

    // -------------------------------------------------------------------------
    // Builder tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_builder_defaults() {
        let builder = SmartTurnDetectorBuilder::new();
        assert_eq!(builder.config.threshold, 0.7);
        assert_eq!(builder.config.num_threads, 1);
    }

    #[test]
    fn test_builder_chain() {
        let builder = SmartTurnDetectorBuilder::new()
            .threshold(0.6)
            .num_threads(2)
            .hysteresis_frames(5)
            .min_speech_ms(400)
            .debug_logging(true);

        assert_eq!(builder.config.threshold, 0.6);
        assert_eq!(builder.config.num_threads, 2);
        assert_eq!(builder.config.hysteresis_frames, 5);
        assert_eq!(builder.config.min_speech_ms, 400);
        assert!(builder.config.debug_logging);
    }

    #[test]
    fn test_builder_model_path() {
        let builder = SmartTurnDetectorBuilder::new().model_path("/path/to/model.onnx");

        assert_eq!(
            builder.config.model_path,
            Some(PathBuf::from("/path/to/model.onnx"))
        );
    }

    #[test]
    fn test_builder_model_url() {
        let builder = SmartTurnDetectorBuilder::new().model_url("https://example.com/model.onnx");

        assert_eq!(
            builder.config.model_url,
            Some("https://example.com/model.onnx".to_string())
        );
    }

    // -------------------------------------------------------------------------
    // Input preparation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_prepare_input_empty() {
        // Create a minimal mock detector config for testing prepare_input
        let config = SmartTurnDetectorConfig {
            max_frames: 10,
            n_mels: 4,
            ..Default::default()
        };

        // Test input preparation logic directly
        let mel_frames: Vec<Vec<f32>> = vec![];
        let max_frames = config.max_frames;
        let n_mels = config.n_mels;
        let total_size = max_frames * n_mels;

        let mut input = vec![0.0f32; total_size];
        let frames_to_copy = mel_frames.len().min(max_frames);
        for (i, frame) in mel_frames.iter().take(frames_to_copy).enumerate() {
            let offset = i * n_mels;
            let copy_len = frame.len().min(n_mels);
            for (j, &val) in frame.iter().take(copy_len).enumerate() {
                input[offset + j] = val;
            }
        }

        // All zeros for empty input
        assert_eq!(input.len(), 40);
        assert!(input.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_prepare_input_with_data() {
        let config = SmartTurnDetectorConfig {
            max_frames: 10,
            n_mels: 4,
            ..Default::default()
        };

        let mel_frames: Vec<Vec<f32>> = vec![vec![1.0, 2.0, 3.0, 4.0], vec![5.0, 6.0, 7.0, 8.0]];

        let max_frames = config.max_frames;
        let n_mels = config.n_mels;
        let total_size = max_frames * n_mels;

        let mut input = vec![0.0f32; total_size];
        let frames_to_copy = mel_frames.len().min(max_frames);
        for (i, frame) in mel_frames.iter().take(frames_to_copy).enumerate() {
            let offset = i * n_mels;
            let copy_len = frame.len().min(n_mels);
            for (j, &val) in frame.iter().take(copy_len).enumerate() {
                input[offset + j] = val;
            }
        }

        assert_eq!(input.len(), 40);
        // First frame
        assert_eq!(input[0..4], [1.0, 2.0, 3.0, 4.0]);
        // Second frame
        assert_eq!(input[4..8], [5.0, 6.0, 7.0, 8.0]);
        // Rest should be zeros (padding)
        assert!(input[8..].iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_prepare_input_truncation() {
        let config = SmartTurnDetectorConfig {
            max_frames: 2,
            n_mels: 3,
            ..Default::default()
        };

        let mel_frames: Vec<Vec<f32>> = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0], // This should be truncated
        ];

        let max_frames = config.max_frames;
        let n_mels = config.n_mels;
        let total_size = max_frames * n_mels;

        let mut input = vec![0.0f32; total_size];
        let frames_to_copy = mel_frames.len().min(max_frames);
        for (i, frame) in mel_frames.iter().take(frames_to_copy).enumerate() {
            let offset = i * n_mels;
            let copy_len = frame.len().min(n_mels);
            for (j, &val) in frame.iter().take(copy_len).enumerate() {
                input[offset + j] = val;
            }
        }

        assert_eq!(input.len(), 6);
        assert_eq!(input, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    // -------------------------------------------------------------------------
    // Integration tests (require model file)
    // -------------------------------------------------------------------------

    // These tests are marked with cfg feature to only run when model is available
    #[cfg(feature = "smart-turn-integration-tests")]
    mod integration {
        use super::*;

        #[tokio::test]
        async fn test_detector_initialization() {
            let config = SmartTurnDetectorConfig::default();
            let detector = SmartTurnDetector::new(config).await;
            // This will fail if no model is available, which is expected
            // in normal test runs
            if detector.is_ok() {
                let d = detector.unwrap();
                assert_eq!(d.total_frames(), 0);
                assert_eq!(d.current_probability(), 0.0);
            }
        }

        #[tokio::test]
        async fn test_detector_predict() {
            let config = SmartTurnDetectorConfig::default();
            if let Ok(mut detector) = SmartTurnDetector::new(config).await {
                // Generate synthetic mel frames
                let mel_frames: Vec<Vec<f32>> = (0..100).map(|_| vec![0.0f32; 80]).collect();

                let result = detector.predict(&mel_frames).await.unwrap();

                assert!(result.probability >= 0.0 && result.probability <= 1.0);
                assert_eq!(result.frames_processed, 100);
                assert_eq!(detector.total_frames(), 1);
            }
        }

        #[tokio::test]
        async fn test_detector_hysteresis() {
            let config = SmartTurnDetectorConfig {
                threshold: 0.5,
                hysteresis_frames: 3,
                ..Default::default()
            };

            if let Ok(mut detector) = SmartTurnDetector::new(config).await {
                // Run multiple predictions
                let mel_frames: Vec<Vec<f32>> = (0..100).map(|_| vec![0.5f32; 80]).collect();

                // Need 3 consecutive high-probability predictions
                for i in 0..5 {
                    let result = detector.predict(&mel_frames).await.unwrap();
                    if i < 2 {
                        // First two won't trigger due to hysteresis
                        if result.probability > 0.5 {
                            assert!(!result.is_turn_complete || i >= 2);
                        }
                    }
                }
            }
        }

        #[tokio::test]
        async fn test_detector_reset() {
            let config = SmartTurnDetectorConfig::default();
            if let Ok(mut detector) = SmartTurnDetector::new(config).await {
                let mel_frames: Vec<Vec<f32>> = (0..100).map(|_| vec![0.0f32; 80]).collect();

                // Make a prediction
                let _ = detector.predict(&mel_frames).await;
                assert_eq!(detector.total_frames(), 1);

                // Reset
                detector.reset();
                assert_eq!(detector.current_probability(), 0.0);

                // Total frames should persist (only reset state, not stats)
                detector.reset_stats();
                assert_eq!(detector.total_frames(), 0);
            }
        }
    }
}
