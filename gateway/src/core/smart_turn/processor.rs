//! Smart Turn Processor - Unified audio-based turn detection pipeline.
//!
//! This module provides a high-level processor that combines:
//! - Silero VAD for voice activity detection
//! - MelExtractor for Whisper-compatible mel spectrogram extraction
//! - SmartTurnDetector for audio-based semantic turn detection
//! - TurnDecisionEngine for final turn decisions
//!
//! ## Usage
//!
//! ```rust,ignore
//! use waav_gateway::core::smart_turn::{SmartTurnProcessor, SmartTurnProcessorConfig};
//!
//! // Create processor
//! let config = SmartTurnProcessorConfig::default();
//! let mut processor = SmartTurnProcessor::new(config).await?;
//!
//! // Process audio chunks (f32 samples at 16kHz)
//! loop {
//!     let audio_chunk = get_audio_chunk(); // Your audio source
//!     let result = processor.process_audio(&audio_chunk).await?;
//!
//!     if result.is_turn_complete {
//!         println!("Turn complete with probability: {}", result.probability);
//!         // Handle turn completion
//!     }
//! }
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

#[cfg(feature = "silero-vad")]
use crate::core::silero_vad::{SileroVAD, SileroVADConfig};

#[cfg(feature = "smart-turn")]
use super::{
    detector::{SmartTurnDetector, SmartTurnDetectorConfig},
    mel_extractor::{MelExtractor, MelExtractorConfig, WHISPER_SAMPLE_RATE},
};

#[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
use std::time::Instant;

#[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
use tracing::trace;

use crate::core::turn_decision::{
    TurnDecision, TurnDecisionEngine, TurnDecisionEngineConfig, TurnSignal,
};

/// Default sample rate constant (Whisper uses 16kHz).
#[cfg(not(feature = "smart-turn"))]
const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Configuration for the Smart Turn Processor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartTurnProcessorConfig {
    /// Input audio sample rate in Hz.
    #[serde(default = "default_sample_rate")]
    pub input_sample_rate: u32,

    /// Whether to use Silero VAD for voice activity detection.
    #[serde(default = "default_use_vad")]
    pub use_vad: bool,

    /// Whether to use audio-based turn detection (SmartTurnDetector).
    #[serde(default = "default_use_audio_turn")]
    pub use_audio_turn: bool,

    /// Minimum frames to collect before running turn detection.
    /// Prevents inference on very short audio.
    #[serde(default = "default_min_frames")]
    pub min_frames: usize,

    /// Frames to accumulate before running inference.
    /// Higher values mean less frequent inference but potentially more latency.
    #[serde(default = "default_inference_interval_frames")]
    pub inference_interval_frames: usize,

    /// MEL extractor configuration.
    #[cfg(feature = "smart-turn")]
    #[serde(default)]
    pub mel_config: MelExtractorConfig,

    /// SmartTurn detector configuration.
    #[cfg(feature = "smart-turn")]
    #[serde(default)]
    pub detector_config: SmartTurnDetectorConfig,

    /// Turn decision engine configuration.
    #[serde(default)]
    pub decision_config: TurnDecisionEngineConfig,

    /// VAD configuration (if using VAD).
    #[serde(default)]
    pub vad_config: SileroVADConfigWrapper,

    /// Enable debug logging.
    #[serde(default)]
    pub debug_logging: bool,
}

fn default_sample_rate() -> u32 {
    WHISPER_SAMPLE_RATE
}

fn default_use_vad() -> bool {
    true
}

fn default_use_audio_turn() -> bool {
    true
}

fn default_min_frames() -> usize {
    50 // ~500ms at 10ms per frame
}

fn default_inference_interval_frames() -> usize {
    10 // Run inference every ~100ms
}

/// Wrapper for VAD config with serde support.
///
/// NOTE: a manual `Default` is required. Deriving `Default` would ignore the
/// `#[serde(default = "...")]` functions and produce field-type zeros
/// (threshold=0.0, chunk_size=0, sample_rate=0) — an INVALID VAD config (a 0.0 threshold
/// fires on every frame; 0 chunk_size/sample_rate are rejected by validation), which also
/// made `SmartTurnProcessorConfig::default()` fail its own `validate()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SileroVADConfigWrapper {
    #[serde(default = "default_vad_threshold")]
    pub threshold: f32,
    #[serde(default = "default_vad_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_vad_sample_rate")]
    pub sample_rate: u32,
    #[serde(default)]
    pub debug_logging: bool,
}

impl Default for SileroVADConfigWrapper {
    fn default() -> Self {
        Self {
            threshold: default_vad_threshold(),
            chunk_size: default_vad_chunk_size(),
            sample_rate: default_vad_sample_rate(),
            debug_logging: false,
        }
    }
}

fn default_vad_threshold() -> f32 {
    0.5
}

fn default_vad_chunk_size() -> usize {
    512
}

fn default_vad_sample_rate() -> u32 {
    16000
}

#[cfg(feature = "smart-turn")]
impl Default for SmartTurnProcessorConfig {
    fn default() -> Self {
        Self {
            input_sample_rate: default_sample_rate(),
            use_vad: default_use_vad(),
            use_audio_turn: default_use_audio_turn(),
            min_frames: default_min_frames(),
            inference_interval_frames: default_inference_interval_frames(),
            mel_config: MelExtractorConfig::default(),
            detector_config: SmartTurnDetectorConfig::default(),
            decision_config: TurnDecisionEngineConfig::default(),
            vad_config: SileroVADConfigWrapper::default(),
            debug_logging: false,
        }
    }
}

#[cfg(not(feature = "smart-turn"))]
impl Default for SmartTurnProcessorConfig {
    fn default() -> Self {
        Self {
            input_sample_rate: default_sample_rate(),
            use_vad: default_use_vad(),
            use_audio_turn: default_use_audio_turn(),
            min_frames: default_min_frames(),
            inference_interval_frames: default_inference_interval_frames(),
            decision_config: TurnDecisionEngineConfig::default(),
            vad_config: SileroVADConfigWrapper::default(),
            debug_logging: false,
        }
    }
}

impl SmartTurnProcessorConfig {
    /// Creates a new configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables/disables VAD.
    pub fn with_vad(mut self, enabled: bool) -> Self {
        self.use_vad = enabled;
        self
    }

    /// Enables/disables audio-based turn detection.
    pub fn with_audio_turn(mut self, enabled: bool) -> Self {
        self.use_audio_turn = enabled;
        self.decision_config.use_audio = enabled;
        self
    }
}

/// Methods that require the `smart-turn` feature.
#[cfg(feature = "smart-turn")]
impl SmartTurnProcessorConfig {
    /// Sets the input sample rate.
    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.input_sample_rate = rate;
        self.mel_config.input_sample_rate = rate;
        self
    }

    /// Sets the turn detection threshold.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.detector_config.threshold = threshold;
        self.decision_config.audio_threshold = threshold;
        self
    }

    /// Enables debug logging.
    pub fn with_debug_logging(mut self) -> Self {
        self.debug_logging = true;
        self.mel_config.debug_logging = true;
        self.detector_config.debug_logging = true;
        self.decision_config.debug_logging = true;
        self.vad_config.debug_logging = true;
        self
    }

    /// Validates that all component configurations are consistent.
    ///
    /// This checks that:
    /// - n_mels matches between mel_extractor and detector configs
    /// - max_frames/max_duration are compatible
    /// - Sample rates are consistent
    ///
    /// # Errors
    ///
    /// Returns an error message if validation fails.
    pub fn validate(&self) -> Result<(), String> {
        // Validate individual components
        self.mel_config.validate()?;
        self.detector_config.validate()?;

        // Cross-component validation: n_mels must match
        if self.mel_config.n_mels != self.detector_config.n_mels {
            return Err(format!(
                "n_mels mismatch: mel_config has {} but detector_config has {}. \
                 These must match for correct model input shape.",
                self.mel_config.n_mels, self.detector_config.n_mels
            ));
        }

        // Cross-component validation: max_frames should be compatible
        let mel_max_frames = self.mel_config.max_frames();
        if mel_max_frames < self.detector_config.max_frames {
            return Err(format!(
                "max_frames mismatch: mel_config produces at most {} frames \
                 (from {}s at {}ms/frame) but detector expects {}. \
                 Increase mel_config.max_duration_secs or reduce detector_config.max_frames.",
                mel_max_frames,
                self.mel_config.max_duration_secs,
                self.mel_config.frame_duration_ms(),
                self.detector_config.max_frames
            ));
        }

        // Validate VAD sample rate matches
        if self.use_vad && self.vad_config.sample_rate != self.input_sample_rate {
            return Err(format!(
                "VAD sample rate ({}) does not match input_sample_rate ({}). \
                 Set them to the same value for consistent processing.",
                self.vad_config.sample_rate, self.input_sample_rate
            ));
        }

        Ok(())
    }
}

/// Methods for when `smart-turn` feature is not enabled.
#[cfg(not(feature = "smart-turn"))]
impl SmartTurnProcessorConfig {
    /// Sets the input sample rate (without mel_config update when smart-turn is disabled).
    pub fn with_sample_rate(mut self, rate: u32) -> Self {
        self.input_sample_rate = rate;
        self
    }

    /// Sets the turn detection threshold (decision only when smart-turn is disabled).
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.decision_config.audio_threshold = threshold;
        self
    }

    /// Enables debug logging.
    pub fn with_debug_logging(mut self) -> Self {
        self.debug_logging = true;
        self.decision_config.debug_logging = true;
        self.vad_config.debug_logging = true;
        self
    }

    /// Validates configuration (minimal validation when smart-turn is disabled).
    pub fn validate(&self) -> Result<(), String> {
        // Validate VAD sample rate matches
        if self.use_vad && self.vad_config.sample_rate != self.input_sample_rate {
            return Err(format!(
                "VAD sample rate ({}) does not match input_sample_rate ({}). \
                 Set them to the same value for consistent processing.",
                self.vad_config.sample_rate, self.input_sample_rate
            ));
        }

        Ok(())
    }
}

/// Result from Smart Turn processing.
#[derive(Debug, Clone)]
pub struct SmartTurnProcessResult {
    /// Whether a turn was completed this frame.
    pub is_turn_complete: bool,

    /// Turn completion probability [0.0, 1.0].
    pub probability: f32,

    /// Whether speech is currently detected (from VAD).
    pub is_speech: bool,

    /// Current silence duration in milliseconds (if paused).
    pub silence_duration_ms: f32,

    /// Total processing latency in microseconds.
    pub latency_us: u64,

    /// Number of mel frames currently accumulated.
    pub mel_frames: usize,

    /// Detailed turn decision.
    pub decision: TurnDecision,
}

/// Smart Turn Processor - High-level audio-based turn detection.
///
/// This processor combines VAD, mel extraction, and turn detection into
/// a single unified pipeline for real-time audio processing.
pub struct SmartTurnProcessor {
    /// Configuration.
    config: SmartTurnProcessorConfig,

    /// Silero VAD for voice activity detection.
    #[cfg(feature = "silero-vad")]
    vad: Option<SileroVAD>,

    /// Mel spectrogram extractor.
    #[cfg(feature = "smart-turn")]
    mel_extractor: MelExtractor,

    /// Smart turn detector.
    #[cfg(feature = "smart-turn")]
    detector: Option<SmartTurnDetector>,

    /// Turn decision engine.
    decision_engine: TurnDecisionEngine,

    /// Audio buffer for VAD (configurable chunk_size samples).
    vad_buffer: Vec<f32>,

    /// Frames since last inference.
    frames_since_inference: usize,

    /// Total audio samples processed.
    total_samples: u64,

    /// Last VAD result.
last_vad_is_speech: bool,
    /// C-G6 dual-gate: the revived `VADAnalyzer` (confidence AND volume) —
    /// a quiet desk-tap that clears the NN threshold no longer reads as
    /// speech. Fed (silero_prob, normalized RMS) per VAD chunk.
    dual_gate: crate::core::audio::VADAnalyzer,
    

    /// Last VAD silence duration.
    last_vad_silence_ms: f32,
}

impl SmartTurnProcessor {
    /// Creates a new Smart Turn Processor.
    ///
    /// # Arguments
    ///
    /// * `config` - Processor configuration.
    ///
    /// # Returns
    ///
    /// A new processor instance or an error.
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn new(config: SmartTurnProcessorConfig) -> Result<Self> {
        // Validate configuration consistency before initialization
        config
            .validate()
            .map_err(|e| anyhow::anyhow!("Invalid processor configuration: {}", e))?;

        debug!(
            "Initializing SmartTurnProcessor: vad={}, audio_turn={}, sample_rate={}",
            config.use_vad, config.use_audio_turn, config.input_sample_rate
        );

        // Capture chunk_size before moving config
        let vad_chunk_size = config.vad_config.chunk_size;

        // Initialize VAD if enabled
        let vad = if config.use_vad {
            let vad_config = SileroVADConfig {
                threshold: config.vad_config.threshold,
                chunk_size: config.vad_config.chunk_size,
                sample_rate: config.vad_config.sample_rate,
                debug_logging: config.vad_config.debug_logging,
                ..Default::default()
            };
            Some(
                SileroVAD::new(vad_config)
                    .await
                    .context("Failed to initialize Silero VAD")?,
            )
        } else {
            None
        };

        // Initialize mel extractor
        let mel_config = MelExtractorConfig {
            input_sample_rate: config.input_sample_rate,
            ..config.mel_config.clone()
        };
        let mel_extractor =
            MelExtractor::new(mel_config).context("Failed to initialize MelExtractor")?;

        // Initialize detector if audio turn detection is enabled
        let detector = if config.use_audio_turn {
            Some(
                SmartTurnDetector::new(config.detector_config.clone())
                    .await
                    .context("Failed to initialize SmartTurnDetector")?,
            )
        } else {
            None
        };

        // Initialize decision engine
        let decision_engine = TurnDecisionEngine::new(config.decision_config.clone())
            .context("Failed to initialize TurnDecisionEngine")?;

        Ok(Self {
            config,
            vad,
            mel_extractor,
            detector,
            decision_engine,
            vad_buffer: Vec::with_capacity(vad_chunk_size),
            frames_since_inference: 0,
            total_samples: 0,
            last_vad_is_speech: false,
            dual_gate: crate::core::audio::VADAnalyzer::new(
                // RMS-calibrated dual-gate (NOT the perceptual-loudness
                // defaults): min_volume on raw normalized RMS — speech runs
                // ~0.02-0.3, digital silence/quiet taps < 0.005. Debounce 1
                // = pure confidence∧volume semantics; the silero detector's
                // own state machine keeps owning the timing.
                crate::core::audio::VADParams {
                    confidence_threshold: 0.5,
                    start_debounce_frames: 1,
                    stop_debounce_frames: 1,
                    min_volume: 0.008,
                },
            ),
            last_vad_silence_ms: 0.0,
        })
    }

    /// Creates a new Smart Turn Processor without SmartTurnDetector (VAD + Decision only).
    #[cfg(all(feature = "silero-vad", not(feature = "smart-turn")))]
    pub async fn new(config: SmartTurnProcessorConfig) -> Result<Self> {
        debug!(
            "Initializing SmartTurnProcessor (VAD only): sample_rate={}",
            config.input_sample_rate
        );

        // Capture chunk_size before moving config
        let vad_chunk_size = config.vad_config.chunk_size;

        // Initialize VAD if enabled
        let vad = if config.use_vad {
            let vad_config = SileroVADConfig {
                threshold: config.vad_config.threshold,
                chunk_size: config.vad_config.chunk_size,
                sample_rate: config.vad_config.sample_rate,
                debug_logging: config.vad_config.debug_logging,
                ..Default::default()
            };
            Some(
                SileroVAD::new(vad_config)
                    .await
                    .context("Failed to initialize Silero VAD")?,
            )
        } else {
            None
        };

        // Initialize decision engine
        let decision_engine = TurnDecisionEngine::new(config.decision_config.clone())
            .context("Failed to initialize TurnDecisionEngine")?;

        Ok(Self {
            config,
            vad,
            decision_engine,
            vad_buffer: Vec::with_capacity(vad_chunk_size),
            frames_since_inference: 0,
            total_samples: 0,
            last_vad_is_speech: false,
            dual_gate: crate::core::audio::VADAnalyzer::new(
                // RMS-calibrated dual-gate (NOT the perceptual-loudness
                // defaults): min_volume on raw normalized RMS — speech runs
                // ~0.02-0.3, digital silence/quiet taps < 0.005. Debounce 1
                // = pure confidence∧volume semantics; the silero detector's
                // own state machine keeps owning the timing.
                crate::core::audio::VADParams {
                    confidence_threshold: 0.5,
                    start_debounce_frames: 1,
                    stop_debounce_frames: 1,
                    min_volume: 0.008,
                },
            ),
            last_vad_silence_ms: 0.0,
        })
    }

    /// Creates a new Smart Turn Processor without VAD (SmartTurn only).
    #[cfg(all(feature = "smart-turn", not(feature = "silero-vad")))]
    pub async fn new(config: SmartTurnProcessorConfig) -> Result<Self> {
        debug!(
            "Initializing SmartTurnProcessor (SmartTurn only): sample_rate={}",
            config.input_sample_rate
        );

        // Capture chunk_size before moving config
        let vad_chunk_size = config.vad_config.chunk_size;

        // Initialize mel extractor
        let mel_config = MelExtractorConfig {
            input_sample_rate: config.input_sample_rate,
            ..config.mel_config.clone()
        };
        let mel_extractor =
            MelExtractor::new(mel_config).context("Failed to initialize MelExtractor")?;

        // Initialize detector if audio turn detection is enabled
        let detector = if config.use_audio_turn {
            Some(
                SmartTurnDetector::new(config.detector_config.clone())
                    .await
                    .context("Failed to initialize SmartTurnDetector")?,
            )
        } else {
            None
        };

        // Initialize decision engine
        let decision_engine = TurnDecisionEngine::new(config.decision_config.clone())
            .context("Failed to initialize TurnDecisionEngine")?;

        Ok(Self {
            config,
            mel_extractor,
            detector,
            decision_engine,
            vad_buffer: Vec::with_capacity(vad_chunk_size),
            frames_since_inference: 0,
            total_samples: 0,
            last_vad_is_speech: false,
            dual_gate: crate::core::audio::VADAnalyzer::new(
                // RMS-calibrated dual-gate (NOT the perceptual-loudness
                // defaults): min_volume on raw normalized RMS — speech runs
                // ~0.02-0.3, digital silence/quiet taps < 0.005. Debounce 1
                // = pure confidence∧volume semantics; the silero detector's
                // own state machine keeps owning the timing.
                crate::core::audio::VADParams {
                    confidence_threshold: 0.5,
                    start_debounce_frames: 1,
                    stop_debounce_frames: 1,
                    min_volume: 0.008,
                },
            ),
            last_vad_silence_ms: 0.0,
        })
    }

    /// Stub constructor when neither feature is enabled.
    #[cfg(not(any(feature = "silero-vad", feature = "smart-turn")))]
    pub async fn new(_config: SmartTurnProcessorConfig) -> Result<Self> {
        anyhow::bail!(
            "SmartTurnProcessor requires either 'silero-vad' or 'smart-turn' feature to be enabled"
        )
    }

    /// Processes audio samples and returns turn detection result.
    ///
    /// # Arguments
    ///
    /// * `audio` - f32 audio samples (normalized -1.0 to 1.0)
    ///
    /// # Returns
    ///
    /// Turn processing result.
    #[cfg(all(feature = "silero-vad", feature = "smart-turn"))]
    pub async fn process_audio(&mut self, audio: &[f32]) -> Result<SmartTurnProcessResult> {
        let start = Instant::now();
        self.total_samples += audio.len() as u64;

        // Collect signals for decision engine
        let mut signals = Vec::with_capacity(3);

        // Process through VAD if enabled
        if let Some(ref mut vad) = self.vad {
            // Buffer audio for VAD (needs exactly chunk_size samples)
            let chunk_size = self.config.vad_config.chunk_size;
            self.vad_buffer.extend_from_slice(audio);

            while self.vad_buffer.len() >= chunk_size {
                let chunk: Vec<f32> = self.vad_buffer.drain(..chunk_size).collect();
                let vad_result = vad.process(&chunk)?;

                // C-G6 dual-gate: speech requires BOTH the NN confidence AND
                // audible volume (normalized RMS — cheaper than EBU-R128;
                // the divergence is documented on VADParams::min_volume).
                // Quiet noise that clears the NN threshold is gated out.
                let rms = {
                    let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
                    (sum_sq / chunk.len() as f32).sqrt().min(1.0)
                };
                let (state, _transition) = self.dual_gate.analyze(vad_result.probability, rms);
                self.last_vad_is_speech = vad_result.is_speech
                    && matches!(
                        state,
                        crate::core::audio::VADState::Speaking
                            | crate::core::audio::VADState::Stopping
                    );
                self.last_vad_silence_ms = vad.silence_duration_ms();
            }

            signals.push(TurnSignal::Silence {
                duration_ms: self.last_vad_silence_ms,
                is_speech: self.last_vad_is_speech,
            });
        }

        // Process through mel extractor and detector
        let mel_frames_count;

        #[cfg(feature = "smart-turn")]
        {
            self.mel_extractor.process(audio)?;
            mel_frames_count = self.mel_extractor.num_frames();
            self.frames_since_inference += 1;

            // Run inference if we have enough frames and it's time
            if let Some(ref mut detector) = self.detector
                && mel_frames_count >= self.config.min_frames
                    && self.frames_since_inference >= self.config.inference_interval_frames
                {
                    let mel_frames = self.mel_extractor.get_mel_frames();
                    let result = detector.predict(mel_frames).await?;

                    self.frames_since_inference = 0;

                    signals.push(TurnSignal::Audio {
                        probability: result.probability,
                        frames_processed: mel_frames_count,
                    });

                    if self.config.debug_logging {
                        trace!(
                            "SmartTurn inference: prob={:.4}, frames={}, inference={}us",
                            result.probability, mel_frames_count, result.inference_time_us
                        );
                    }
                }
        }

        // Process through decision engine
        let decision = self.decision_engine.process(&signals);
        let latency_us = start.elapsed().as_micros() as u64;

        if self.config.debug_logging && decision.is_turn_complete {
            debug!(
                "Turn complete: prob={:.4}, reason={}",
                decision.combined_probability, decision.reason
            );
        }

        Ok(SmartTurnProcessResult {
            is_turn_complete: decision.is_turn_complete,
            probability: decision.combined_probability,
            is_speech: self.last_vad_is_speech,
            silence_duration_ms: self.last_vad_silence_ms,
            latency_us,
            mel_frames: mel_frames_count,
            decision,
        })
    }

    /// Processes audio samples (smart-turn only mode, no VAD).
    ///
    /// This variant is used when only smart-turn feature is enabled without silero-vad.
    #[cfg(all(feature = "smart-turn", not(feature = "silero-vad")))]
    pub async fn process_audio(&mut self, audio: &[f32]) -> Result<SmartTurnProcessResult> {
        use std::time::Instant;
        let start = Instant::now();
        self.total_samples += audio.len() as u64;

        let mut signals = Vec::with_capacity(2);

        // Process through mel extractor and detector
        self.mel_extractor.process(audio)?;
        let mel_frames_count = self.mel_extractor.num_frames();
        self.frames_since_inference += 1;

        // Run inference if we have enough frames and it's time
        if let Some(ref mut detector) = self.detector {
            if mel_frames_count >= self.config.min_frames
                && self.frames_since_inference >= self.config.inference_interval_frames
            {
                let mel_frames = self.mel_extractor.get_mel_frames();
                let result = detector.predict(mel_frames).await?;

                self.frames_since_inference = 0;

                signals.push(TurnSignal::Audio {
                    probability: result.probability,
                    frames_processed: mel_frames_count,
                });
            }
        }

        // Process through decision engine
        let decision = self.decision_engine.process(&signals);
        let latency_us = start.elapsed().as_micros() as u64;

        Ok(SmartTurnProcessResult {
            is_turn_complete: decision.is_turn_complete,
            probability: decision.combined_probability,
            is_speech: false, // No VAD, so we don't know
            silence_duration_ms: 0.0,
            latency_us,
            mel_frames: mel_frames_count,
            decision,
        })
    }

    /// Adds a text signal to the decision engine.
    ///
    /// Call this when you receive STT results to incorporate text-based
    /// turn detection into the ensemble.
    ///
    /// # Arguments
    ///
    /// * `probability` - Text-based turn probability from TurnDetector
    /// * `transcript_len` - Length of the current transcript
    pub fn add_text_signal(&mut self, probability: f32, transcript_len: usize) {
        let signals = vec![TurnSignal::Text {
            probability,
            transcript_len,
        }];
        let _decision = self.decision_engine.process(&signals);
    }

    /// Resets the processor state for a new conversation.
    pub fn reset(&mut self) {
        #[cfg(feature = "silero-vad")]
        if let Some(ref mut vad) = self.vad {
            vad.reset();
        }

        #[cfg(feature = "smart-turn")]
        {
            self.mel_extractor.reset();
            if let Some(ref mut detector) = self.detector {
                detector.reset();
            }
        }

        self.decision_engine.reset();
        self.vad_buffer.clear();
        self.frames_since_inference = 0;
        self.last_vad_is_speech = false;
        self.last_vad_silence_ms = 0.0;

        if self.config.debug_logging {
            debug!("SmartTurnProcessor reset");
        }
    }

    /// Returns whether currently detecting speech.
    #[inline]
    pub fn is_speech(&self) -> bool {
        self.last_vad_is_speech
    }

    /// Returns current silence duration in milliseconds.
    #[inline]
    pub fn silence_duration_ms(&self) -> f32 {
        self.last_vad_silence_ms
    }

    /// Returns the turn state.
    #[inline]
    pub fn turn_state(&self) -> crate::core::turn_decision::TurnState {
        self.decision_engine.state()
    }

    /// Returns total samples processed.
    #[inline]
    pub fn total_samples(&self) -> u64 {
        self.total_samples
    }

    /// Returns the configuration.
    #[inline]
    pub fn config(&self) -> &SmartTurnProcessorConfig {
        &self.config
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(all(test, feature = "smart-turn"))]
mod tests {
    use super::*;

    // =========================================================================
    // Configuration Tests
    // =========================================================================

    #[test]
    fn test_default_config() {
        let config = SmartTurnProcessorConfig::default();

        assert_eq!(config.input_sample_rate, 16000);
        assert!(config.use_vad);
        assert!(config.use_audio_turn);
        assert_eq!(config.min_frames, 50);
        assert_eq!(config.inference_interval_frames, 10);
        assert!(!config.debug_logging);
    }

    #[test]
    fn test_config_builder() {
        let config = SmartTurnProcessorConfig::new()
            .with_sample_rate(48000)
            .with_vad(false)
            .with_audio_turn(true)
            .with_threshold(0.8)
            .with_debug_logging();

        assert_eq!(config.input_sample_rate, 48000);
        assert_eq!(config.mel_config.input_sample_rate, 48000);
        assert!(!config.use_vad);
        assert!(config.use_audio_turn);
        assert_eq!(config.detector_config.threshold, 0.8);
        assert!(config.debug_logging);
    }

    #[test]
    fn test_config_builder_vad_only() {
        let config = SmartTurnProcessorConfig::new()
            .with_vad(true)
            .with_audio_turn(false);

        assert!(config.use_vad);
        assert!(!config.use_audio_turn);
        assert!(!config.decision_config.use_audio);
    }

    #[test]
    fn test_config_builder_audio_turn_only() {
        let config = SmartTurnProcessorConfig::new()
            .with_vad(false)
            .with_audio_turn(true);

        assert!(!config.use_vad);
        assert!(config.use_audio_turn);
        assert!(config.decision_config.use_audio);
    }

    #[test]
    fn test_config_builder_threshold_propagation() {
        let config = SmartTurnProcessorConfig::new().with_threshold(0.65);

        // Threshold should propagate to both detector and decision engine
        assert_eq!(config.detector_config.threshold, 0.65);
        assert_eq!(config.decision_config.audio_threshold, 0.65);
    }

    #[test]
    fn test_config_builder_debug_logging_propagation() {
        let config = SmartTurnProcessorConfig::new().with_debug_logging();

        // Debug logging should propagate to all sub-configs
        assert!(config.debug_logging);
        assert!(config.mel_config.debug_logging);
        assert!(config.detector_config.debug_logging);
        assert!(config.decision_config.debug_logging);
        assert!(config.vad_config.debug_logging);
    }

    #[test]
    fn test_config_builder_sample_rate_propagation() {
        let config = SmartTurnProcessorConfig::new().with_sample_rate(44100);

        // Sample rate should propagate to mel config
        assert_eq!(config.input_sample_rate, 44100);
        assert_eq!(config.mel_config.input_sample_rate, 44100);
    }

    // =========================================================================
    // Configuration Validation Tests
    // =========================================================================

    #[test]
    fn test_config_validation_success() {
        let config = SmartTurnProcessorConfig::default();
        let result = config.validate();
        assert!(result.is_ok(), "Default config should be valid");
    }

    #[test]
    fn test_config_validation_n_mels_mismatch() {
        let mut config = SmartTurnProcessorConfig::default();
        config.mel_config.n_mels = 80;
        config.detector_config.n_mels = 128; // Mismatch!

        let result = config.validate();
        assert!(result.is_err(), "Should fail on n_mels mismatch");
        let err = result.unwrap_err();
        assert!(
            err.contains("n_mels mismatch"),
            "Error should mention n_mels mismatch: {}",
            err
        );
    }

    #[test]
    fn test_config_validation_max_frames_mismatch() {
        let mut config = SmartTurnProcessorConfig::default();
        // Set mel_config to produce fewer frames than detector expects
        config.mel_config.max_duration_secs = 1.0; // ~100 frames
        config.detector_config.max_frames = 800; // Expects 800 frames

        let result = config.validate();
        assert!(result.is_err(), "Should fail on max_frames mismatch");
        let err = result.unwrap_err();
        assert!(
            err.contains("max_frames mismatch"),
            "Error should mention max_frames mismatch: {}",
            err
        );
    }

    #[test]
    fn test_config_validation_vad_sample_rate_mismatch() {
        let mut config = SmartTurnProcessorConfig::default();
        config.use_vad = true;
        config.input_sample_rate = 16000;
        config.vad_config.sample_rate = 8000; // Mismatch!

        let result = config.validate();
        assert!(result.is_err(), "Should fail on VAD sample rate mismatch");
        let err = result.unwrap_err();
        assert!(
            err.contains("VAD sample rate"),
            "Error should mention VAD sample rate: {}",
            err
        );
    }

    #[test]
    fn test_config_validation_vad_disabled_sample_rate_ignored() {
        let mut config = SmartTurnProcessorConfig::default();
        config.use_vad = false; // VAD disabled
        config.input_sample_rate = 16000;
        config.vad_config.sample_rate = 8000; // Mismatch, but VAD is disabled

        let result = config.validate();
        assert!(
            result.is_ok(),
            "Should not fail when VAD is disabled even with sample rate mismatch"
        );
    }

    // =========================================================================
    // VAD Config Wrapper Tests
    // =========================================================================

    #[test]
    fn test_vad_config_wrapper_defaults() {
        let vad_config = SileroVADConfigWrapper::default();

        assert_eq!(vad_config.threshold, 0.5);
        assert_eq!(vad_config.chunk_size, 512);
        assert_eq!(vad_config.sample_rate, 16000);
        assert!(!vad_config.debug_logging);
    }

    #[test]
    fn test_vad_config_wrapper_serialization() {
        let vad_config = SileroVADConfigWrapper {
            threshold: 0.7,
            chunk_size: 1024,
            sample_rate: 8000,
            debug_logging: true,
        };

        let json = serde_json::to_string(&vad_config).expect("serialization should work");
        let deserialized: SileroVADConfigWrapper =
            serde_json::from_str(&json).expect("deserialization should work");

        assert_eq!(deserialized.threshold, 0.7);
        assert_eq!(deserialized.chunk_size, 1024);
        assert_eq!(deserialized.sample_rate, 8000);
        assert!(deserialized.debug_logging);
    }

    #[test]
    fn test_vad_config_wrapper_deserialization_with_defaults() {
        // Empty JSON should use all defaults
        let json = "{}";
        let vad_config: SileroVADConfigWrapper =
            serde_json::from_str(json).expect("deserialization should work");

        assert_eq!(vad_config.threshold, 0.5);
        assert_eq!(vad_config.chunk_size, 512);
        assert_eq!(vad_config.sample_rate, 16000);
        assert!(!vad_config.debug_logging);
    }

    // =========================================================================
    // Result Struct Tests
    // =========================================================================

    #[test]
    fn test_result_struct() {
        let result = SmartTurnProcessResult {
            is_turn_complete: true,
            probability: 0.85,
            is_speech: false,
            silence_duration_ms: 300.0,
            latency_us: 5000,
            mel_frames: 100,
            decision: TurnDecision::default(),
        };

        assert!(result.is_turn_complete);
        assert_eq!(result.probability, 0.85);
        assert!(!result.is_speech);
        assert_eq!(result.silence_duration_ms, 300.0);
        assert_eq!(result.latency_us, 5000);
        assert_eq!(result.mel_frames, 100);
    }

    #[test]
    fn test_result_struct_turn_not_complete() {
        let result = SmartTurnProcessResult {
            is_turn_complete: false,
            probability: 0.3,
            is_speech: true,
            silence_duration_ms: 0.0,
            latency_us: 2500,
            mel_frames: 50,
            decision: TurnDecision::default(),
        };

        assert!(!result.is_turn_complete);
        assert_eq!(result.probability, 0.3);
        assert!(result.is_speech);
        assert_eq!(result.silence_duration_ms, 0.0);
    }

    #[test]
    fn test_result_struct_boundary_values() {
        // Test with boundary probability values
        let result_zero = SmartTurnProcessResult {
            is_turn_complete: false,
            probability: 0.0,
            is_speech: false,
            silence_duration_ms: 0.0,
            latency_us: 0,
            mel_frames: 0,
            decision: TurnDecision::default(),
        };
        assert_eq!(result_zero.probability, 0.0);
        assert_eq!(result_zero.mel_frames, 0);

        let result_one = SmartTurnProcessResult {
            is_turn_complete: true,
            probability: 1.0,
            is_speech: true,
            silence_duration_ms: f32::MAX,
            latency_us: u64::MAX,
            mel_frames: usize::MAX,
            decision: TurnDecision::default(),
        };
        assert_eq!(result_one.probability, 1.0);
        assert_eq!(result_one.latency_us, u64::MAX);
    }

    #[test]
    fn test_result_struct_clone() {
        let original = SmartTurnProcessResult {
            is_turn_complete: true,
            probability: 0.75,
            is_speech: true,
            silence_duration_ms: 150.0,
            latency_us: 3000,
            mel_frames: 200,
            decision: TurnDecision::default(),
        };

        let cloned = original.clone();

        assert_eq!(cloned.is_turn_complete, original.is_turn_complete);
        assert_eq!(cloned.probability, original.probability);
        assert_eq!(cloned.is_speech, original.is_speech);
        assert_eq!(cloned.silence_duration_ms, original.silence_duration_ms);
        assert_eq!(cloned.latency_us, original.latency_us);
        assert_eq!(cloned.mel_frames, original.mel_frames);
    }

    // =========================================================================
    // Processor Config Serialization Tests
    // =========================================================================

    #[test]
    fn test_processor_config_serialization() {
        let config = SmartTurnProcessorConfig::new()
            .with_sample_rate(16000)
            .with_threshold(0.7)
            .with_vad(true)
            .with_audio_turn(true);

        let json = serde_json::to_string(&config).expect("serialization should work");
        let deserialized: SmartTurnProcessorConfig =
            serde_json::from_str(&json).expect("deserialization should work");

        assert_eq!(deserialized.input_sample_rate, 16000);
        assert_eq!(deserialized.detector_config.threshold, 0.7);
        assert!(deserialized.use_vad);
        assert!(deserialized.use_audio_turn);
    }

    #[test]
    fn test_processor_config_deserialization_with_defaults() {
        // Partial JSON should use defaults for missing fields
        let json = r#"{"input_sample_rate": 48000}"#;
        let config: SmartTurnProcessorConfig =
            serde_json::from_str(json).expect("deserialization should work");

        assert_eq!(config.input_sample_rate, 48000);
        assert!(config.use_vad); // default true
        assert!(config.use_audio_turn); // default true
        assert_eq!(config.min_frames, 50); // default
    }

    // =========================================================================
    // Default Functions Tests
    // =========================================================================

    #[test]
    fn test_default_functions() {
        assert_eq!(default_sample_rate(), 16000);
        assert!(default_use_vad());
        assert!(default_use_audio_turn());
        assert_eq!(default_min_frames(), 50);
        assert_eq!(default_inference_interval_frames(), 10);
        assert_eq!(default_vad_threshold(), 0.5);
        assert_eq!(default_vad_chunk_size(), 512);
        assert_eq!(default_vad_sample_rate(), 16000);
    }
}

#[cfg(all(test, feature = "silero-vad"))]
mod dual_gate_tests {
    use crate::core::audio::{VADAnalyzer, VADParams, VADState};

    /// C-G6: the dual-gate's win — high NN confidence with NO audible
    /// volume (a quiet desk-tap that clears the model threshold) is gated
    /// out; the same confidence WITH volume passes.
    #[test]
    fn quiet_noise_is_gated_audible_speech_passes() {
        let params = VADParams {
            confidence_threshold: 0.5,
            start_debounce_frames: 1,
            stop_debounce_frames: 1,
            min_volume: 0.008,
        };
        let mut v = VADAnalyzer::new(params.clone());
        let (state, _) = v.analyze(0.9, 0.001); // confident but silent
        assert_eq!(state, VADState::Quiet, "quiet noise must NOT read as speech");

        let mut v = VADAnalyzer::new(params);
        let (state, transition) = v.analyze(0.9, 0.1); // confident and audible
        assert_eq!(state, VADState::Speaking);
        assert!(transition.is_some());
    }
}
