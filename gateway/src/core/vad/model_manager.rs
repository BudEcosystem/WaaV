//! ONNX model manager for Silero VAD

use anyhow::{Context, Result};
use ort::session::Session;
use ort::session::builder::SessionBuilder;
use ort::value::Value;
use std::path::Path;
use std::sync::Arc;
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use super::{assets, config::VADConfig};

/// Context size for Silero VAD (64 samples for 16kHz, 32 for 8kHz)
const CONTEXT_SIZE_16K: usize = 64;
const CONTEXT_SIZE_8K: usize = 32;

/// Internal state for Silero VAD LSTM
/// The model uses a combined state tensor
#[derive(Clone)]
pub struct SileroState {
    /// Combined LSTM state tensor [2, batch_size, 128]
    pub state: ndarray::Array3<f32>,
    /// Sample rate indicator (8000 or 16000)
    pub sr: i64,
    /// Context buffer for temporal padding
    pub context: Vec<f32>,
}

impl SileroState {
    /// Create new state for the given sample rate
    pub fn new(sample_rate: u32) -> Self {
        // Silero VAD uses state tensor with shape [2, batch_size=1, 128]
        let state = ndarray::Array3::<f32>::zeros((2, 1, 128));

        // Context size depends on sample rate
        let context_size = if sample_rate == 8000 {
            CONTEXT_SIZE_8K
        } else {
            CONTEXT_SIZE_16K
        };
        let context = vec![0.0f32; context_size];

        Self {
            state,
            sr: sample_rate as i64,
            context,
        }
    }

    /// Reset the LSTM state
    pub fn reset(&mut self) {
        self.state.fill(0.0);
        self.context.fill(0.0);
    }

    /// Get context size for this sample rate
    pub fn context_size(&self) -> usize {
        if self.sr == 8000 {
            CONTEXT_SIZE_8K
        } else {
            CONTEXT_SIZE_16K
        }
    }
}

/// ONNX model manager for Silero VAD
pub struct ModelManager {
    session: Arc<Mutex<Session>>,
    config: VADConfig,
    /// Current LSTM state
    state: SileroState,
    /// Cached input names
    input_names: Vec<String>,
    /// Cached output names
    output_names: Vec<String>,
}

impl ModelManager {
    /// Create a new ModelManager with the given configuration
    pub async fn new(config: VADConfig) -> Result<Self> {
        let model_path = assets::model_path(&config)?;

        info!("Loading Silero VAD model from: {:?}", model_path);

        // Load model on blocking thread
        let session = tokio::task::spawn_blocking({
            let model_path = model_path.clone();
            let config = config.clone();
            move || Self::create_session(&model_path, &config)
        })
        .await
        .context("Failed to spawn blocking task for VAD model loading")??;

        // Cache input/output names
        let input_names: Vec<String> = session
            .inputs
            .iter()
            .map(|input| input.name.clone())
            .collect();

        let output_names: Vec<String> = session
            .outputs
            .iter()
            .map(|output| output.name.clone())
            .collect();

        info!("VAD model input names: {:?}", input_names);
        info!("VAD model output names: {:?}", output_names);

        let state = SileroState::new(config.sample_rate);

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            config,
            state,
            input_names,
            output_names,
        })
    }

    fn create_session(model_path: &Path, config: &VADConfig) -> Result<Session> {
        let mut builder = SessionBuilder::new()?
            .with_optimization_level(config.graph_optimization_level.to_ort_level())?;

        if let Some(num_threads) = config.num_threads {
            builder = builder
                .with_intra_threads(num_threads)?
                .with_inter_threads(1)?;
        }

        let session = builder.commit_from_file(model_path)?;

        Self::validate_model(&session)?;

        Ok(session)
    }

    fn validate_model(session: &Session) -> Result<()> {
        let inputs = &session.inputs;
        let outputs = &session.outputs;

        debug!("VAD model inputs: {}", inputs.len());
        for (i, input) in inputs.iter().enumerate() {
            debug!("  Input {}: {} ({:?})", i, input.name, input.input_type);
        }

        debug!("VAD model outputs: {}", outputs.len());
        for (i, output) in outputs.iter().enumerate() {
            debug!("  Output {}: {} ({:?})", i, output.name, output.output_type);
        }

        // Silero VAD should have:
        // Inputs: input (audio), sr (sample rate), h (hidden state), c (cell state)
        // Outputs: output (probability), hn (new hidden), cn (new cell)
        if inputs.len() < 4 {
            warn!(
                "VAD model has {} inputs, expected at least 4. Model format may differ.",
                inputs.len()
            );
        }

        if outputs.len() < 3 {
            warn!(
                "VAD model has {} outputs, expected at least 3. Model format may differ.",
                outputs.len()
            );
        }

        Ok(())
    }

    /// Run inference on an audio frame
    ///
    /// # Arguments
    /// * `audio` - Audio samples (f32, normalized to [-1, 1])
    ///
    /// # Returns
    /// Speech probability (0.0 - 1.0)
    pub fn predict(&mut self, audio: &[f32]) -> Result<f32> {
        let frame_size = audio.len();
        let context_size = self.state.context_size();

        debug!(
            "VAD inference: frame_size={}, context_size={}, sample_rate={}",
            frame_size, context_size, self.state.sr
        );

        // Build audio with context padding: [context | audio]
        // Total size = context_size + frame_size (e.g., 64 + 512 = 576 for 16kHz)
        let mut audio_with_context = Vec::with_capacity(context_size + frame_size);
        audio_with_context.extend_from_slice(&self.state.context);
        audio_with_context.extend_from_slice(audio);

        let total_size = audio_with_context.len();

        // Update context buffer with last N samples from audio
        let context_start = if audio.len() >= context_size {
            audio.len() - context_size
        } else {
            0
        };
        self.state.context.clear();
        self.state.context.extend_from_slice(&audio[context_start..]);
        // Pad if audio was shorter than context size
        while self.state.context.len() < context_size {
            self.state.context.insert(0, 0.0);
        }

        // Prepare inputs - Silero VAD uses: input, state, sr
        // Create values for each input as needed
        let state_dim = self.state.state.dim();

        let inputs: Vec<(&str, Value)> = self.input_names.iter()
            .filter_map(|name| {
                let value: Option<Value> = match name.as_str() {
                    "input" => {
                        // Use audio with context padding
                        Value::from_array(([1usize, total_size], audio_with_context.clone())).ok().map(|v| v.into())
                    }
                    "state" => {
                        let state_data: Vec<f32> = self.state.state.iter().copied().collect();
                        Value::from_array(([state_dim.0, state_dim.1, state_dim.2], state_data)).ok().map(|v| v.into())
                    }
                    "sr" => {
                        // sr should be a 1D array with single element
                        let sr_shape: [usize; 1] = [1];
                        Value::from_array((sr_shape, vec![self.state.sr])).ok().map(|v| v.into())
                    }
                    other => {
                        warn!("Unknown VAD input name: {}", other);
                        None
                    }
                };
                value.map(|v| (name.as_str(), v))
            })
            .collect();

        // Run inference
        let mut session = self.session.lock();
        let outputs = session.run(inputs)?;

        // Extract probability - first output
        let prob_name = if !self.output_names.is_empty() {
            &self.output_names[0]
        } else {
            "output"
        };

        let prob_tensor = outputs
            .get(prob_name)
            .context("No probability output from VAD model")?
            .try_extract_tensor::<f32>()
            .context("Failed to extract probability tensor")?;

        let (_shape, prob_data) = prob_tensor;
        let probability = prob_data.first().copied().unwrap_or(0.0);

        // Update state from outputs - second output
        let state_out_name = if self.output_names.len() > 1 {
            &self.output_names[1]
        } else {
            "stateN"
        };

        if let Some(state_value) = outputs.get(state_out_name) {
            if let Ok(state_tensor) = state_value.try_extract_tensor::<f32>() {
                let (shape, data) = state_tensor;
                if shape.len() == 3 {
                    let d0 = shape[0] as usize;
                    let d1 = shape[1] as usize;
                    let d2 = shape[2] as usize;
                    if let Ok(new_state) = ndarray::Array3::from_shape_vec((d0, d1, d2), data.to_vec()) {
                        self.state.state = new_state;
                    }
                }
            }
        }

        debug!("VAD probability: {:.4}", probability);

        Ok(probability)
    }

    /// Reset the LSTM state
    pub fn reset(&mut self) {
        self.state.reset();
        debug!("VAD model state reset");
    }

    /// Get the current configuration
    pub fn config(&self) -> &VADConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silero_state_new_16k() {
        let state = SileroState::new(16000);
        assert_eq!(state.sr, 16000);
        assert_eq!(state.state.dim(), (2, 1, 128));
        assert_eq!(state.context.len(), CONTEXT_SIZE_16K);
        assert_eq!(state.context_size(), 64);
    }

    #[test]
    fn test_silero_state_new_8k() {
        let state = SileroState::new(8000);
        assert_eq!(state.sr, 8000);
        assert_eq!(state.state.dim(), (2, 1, 128));
        assert_eq!(state.context.len(), CONTEXT_SIZE_8K);
        assert_eq!(state.context_size(), 32);
    }

    #[test]
    fn test_silero_state_reset() {
        let mut state = SileroState::new(16000);
        // Modify state
        state.state[[0, 0, 0]] = 1.0;
        state.state[[1, 0, 0]] = 1.0;
        state.context[0] = 0.5;

        // Reset
        state.reset();

        // Should be zeros
        assert_eq!(state.state[[0, 0, 0]], 0.0);
        assert_eq!(state.state[[1, 0, 0]], 0.0);
        assert_eq!(state.context[0], 0.0);
    }
}
