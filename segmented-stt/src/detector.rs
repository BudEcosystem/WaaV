//! The speech detector: one 512-sample 16 kHz frame in, a speech probability out.
//!
//! The gateway supplies the Silero detector (it needs ONNX Runtime). This crate supplies the
//! loudness-based fallback and a scripted detector for tests.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::types::DetectorKind;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("speech detector failed: {0}")]
pub struct DetectorError(pub String);

/// One detector per session; it is moved into the engine task and never shared.
pub trait SpeechDetector: Send {
    /// One 512-sample 16 kHz mono frame (f32 in [-1, 1]) to a speech probability in [0, 1].
    fn probability(&mut self, frame: &[f32]) -> Result<f32, DetectorError>;
    /// Forget the recurrent state. The engine calls it only while no segment is open.
    fn reset(&mut self);
    fn kind(&self) -> DetectorKind;
}

/// Builds one detector for one session.
pub type DetectorFactory = Arc<
    dyn Fn() -> futures::future::BoxFuture<'static, Result<Box<dyn SpeechDetector>, DetectorError>>
        + Send
        + Sync,
>;

/// The loudness-based detector. Much worse than Silero; used only without it.
///
/// It tracks a noise floor as a slow average of frame RMS on frames it judged not speech, and
/// calls a frame speech when its RMS exceeds both an absolute floor and three times that noise
/// floor.
#[derive(Debug, Clone)]
pub struct EnergyDetector {
    noise_floor: f32,
    absolute_floor: f32,
}

impl Default for EnergyDetector {
    fn default() -> Self {
        Self {
            noise_floor: 0.002,
            absolute_floor: 0.01,
        }
    }
}

impl EnergyDetector {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SpeechDetector for EnergyDetector {
    fn probability(&mut self, frame: &[f32]) -> Result<f32, DetectorError> {
        if frame.is_empty() {
            return Ok(0.0);
        }
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        let speech = rms > self.absolute_floor && rms > 3.0 * self.noise_floor;
        if !speech {
            self.noise_floor = (0.95 * self.noise_floor + 0.05 * rms).max(0.0005);
        }
        Ok(if speech { 0.9 } else { 0.1 })
    }

    fn reset(&mut self) {}

    fn kind(&self) -> DetectorKind {
        DetectorKind::Energy
    }
}

/// A detector that returns scripted probabilities, frame by frame, then a default.
#[derive(Debug, Clone)]
pub struct ScriptedDetector {
    script: Arc<Mutex<VecDeque<Result<f32, DetectorError>>>>,
    after: f32,
}

impl ScriptedDetector {
    pub fn new(probabilities: impl IntoIterator<Item = f32>, after: f32) -> Self {
        Self {
            script: Arc::new(Mutex::new(probabilities.into_iter().map(Ok).collect())),
            after,
        }
    }

    /// A handle that can append to the script while the engine runs.
    pub fn handle(&self) -> ScriptHandle {
        ScriptHandle(Arc::clone(&self.script))
    }
}

/// Appends to a running scripted detector.
#[derive(Debug, Clone)]
pub struct ScriptHandle(Arc<Mutex<VecDeque<Result<f32, DetectorError>>>>);

impl ScriptHandle {
    pub fn push(&self, probabilities: impl IntoIterator<Item = f32>) {
        self.0.lock().extend(probabilities.into_iter().map(Ok));
    }

    pub fn push_error(&self, n: usize) {
        let mut q = self.0.lock();
        for _ in 0..n {
            q.push_back(Err(DetectorError("scripted failure".into())));
        }
    }
}

impl SpeechDetector for ScriptedDetector {
    fn probability(&mut self, _frame: &[f32]) -> Result<f32, DetectorError> {
        self.script.lock().pop_front().unwrap_or(Ok(self.after))
    }

    fn reset(&mut self) {}

    fn kind(&self) -> DetectorKind {
        DetectorKind::Scripted
    }
}

/// A factory that always builds the energy detector.
pub fn energy_factory() -> DetectorFactory {
    Arc::new(|| Box::pin(async { Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(level: f32) -> Vec<f32> {
        (0..512)
            .map(|i| if i % 2 == 0 { level } else { -level })
            .collect()
    }

    #[test]
    fn energy_detector_hears_speech_above_the_noise_floor_and_not_silence() {
        let mut d = EnergyDetector::new();
        for _ in 0..50 {
            assert!(d.probability(&frame(0.001)).unwrap() < 0.5);
        }
        assert!(d.probability(&frame(0.1)).unwrap() > 0.5);
        assert_eq!(d.kind(), DetectorKind::Energy);
    }

    #[test]
    fn energy_detector_adapts_to_a_steady_noise_floor() {
        let mut d = EnergyDetector::new();
        // A loud steady hum slowly becomes the floor once judged not speech… but a hum above the
        // absolute floor at the start is speech, so feed a level just under three times the floor.
        for _ in 0..200 {
            let _ = d.probability(&frame(0.009));
        }
        assert!(
            d.probability(&frame(0.02)).unwrap() < 0.5,
            "under 3x the learned floor"
        );
        assert!(d.probability(&frame(0.2)).unwrap() > 0.5);
    }

    #[test]
    fn scripted_detector_plays_its_script_then_the_default() {
        let mut d = ScriptedDetector::new([0.9, 0.2], 0.05);
        let h = d.handle();
        h.push([0.7]);
        let f = frame(0.0);
        assert_eq!(d.probability(&f).unwrap(), 0.9);
        assert_eq!(d.probability(&f).unwrap(), 0.2);
        assert_eq!(d.probability(&f).unwrap(), 0.7);
        assert_eq!(d.probability(&f).unwrap(), 0.05);
        h.push_error(1);
        assert!(d.probability(&f).is_err());
    }
}
