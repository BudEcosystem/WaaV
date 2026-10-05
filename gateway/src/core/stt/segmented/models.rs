//! The process-wide models of segmented sessions: the Silero detector, the on-demand SmartTurn
//! pool and the text end-of-turn model.
//!
//! Each model is resolved once per process at start-up ([`warm_up`]), so no session downloads a
//! model inside `connect`, and a missing model is known before a call arrives. The resolver reads
//! [`build_support`] at admission to refuse a session the build cannot segment.

use std::sync::Arc;
use std::sync::OnceLock;

use parking_lot::RwLock;
use waav_segmented_stt::detector::{DetectorError, DetectorFactory, EnergyDetector, SpeechDetector};
use waav_segmented_stt::endpointer::{EndOfTurnError, EndOfTurnModel, EndOfTurnTextModel};
use waav_segmented_stt::types::{DetectorFallback, DetectorKind};

/// Where one model stands in this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    /// The build has no feature for it.
    NotBuilt,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct Models {
    silero: RwLock<ModelState>,
    smart_turn: RwLock<ModelState>,
    #[cfg(feature = "smart-turn")]
    smart_turn_pool: RwLock<Option<Arc<SmartTurnOnDemand>>>,
}

fn models() -> &'static Models {
    static M: OnceLock<Models> = OnceLock::new();
    M.get_or_init(|| Models {
        silero: RwLock::new(if cfg!(feature = "silero-vad") {
            ModelState::Loading
        } else {
            ModelState::NotBuilt
        }),
        smart_turn: RwLock::new(if cfg!(feature = "smart-turn") {
            ModelState::Loading
        } else {
            ModelState::NotBuilt
        }),
        #[cfg(feature = "smart-turn")]
        smart_turn_pool: RwLock::new(None),
    })
}

pub fn silero_state() -> ModelState {
    models().silero.read().clone()
}

pub fn smart_turn_state() -> ModelState {
    models().smart_turn.read().clone()
}

/// Load both models once, in the background. A failure is retried once a minute.
pub fn warm_up() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    #[cfg(feature = "silero-vad")]
    tokio::spawn(async {
        loop {
            match crate::core::silero_vad::SileroVAD::new(silero_config()).await {
                Ok(_) => {
                    *models().silero.write() = ModelState::Ready;
                    tracing::info!("segmented speech-to-text: Silero detector ready");
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "segmented speech-to-text: Silero detector unavailable; retrying in 60 s");
                    *models().silero.write() = ModelState::Failed(e.to_string());
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }
        }
    });
    #[cfg(feature = "smart-turn")]
    tokio::spawn(async {
        loop {
            match SmartTurnOnDemand::load(2).await {
                Ok(pool) => {
                    *models().smart_turn_pool.write() = Some(Arc::new(pool));
                    *models().smart_turn.write() = ModelState::Ready;
                    tracing::info!("segmented speech-to-text: SmartTurn end-of-turn model ready");
                    break;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "segmented speech-to-text: SmartTurn unavailable; retrying in 60 s");
                    *models().smart_turn.write() = ModelState::Failed(e.to_string());
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            }
        }
    });
}

/// What this process can segment with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorSupport {
    pub kind: DetectorKind,
    pub fallback: Option<DetectorFallback>,
    /// Set when the session must be refused (`stt_segmentation_unavailable`, `detector_refused`).
    pub refused: Option<String>,
}

/// The detector a new session gets. A build with Silero whose model failed refuses the session
/// unless the operator allowed the energy detector; a build without Silero uses the energy
/// detector and says so.
pub fn build_support(allow_energy: bool) -> DetectorSupport {
    match silero_state() {
        ModelState::Ready | ModelState::Loading => DetectorSupport {
            kind: DetectorKind::Silero,
            fallback: None,
            refused: None,
        },
        ModelState::NotBuilt => DetectorSupport {
            kind: DetectorKind::Energy,
            fallback: Some(DetectorFallback::FeatureNotBuilt),
            refused: None,
        },
        ModelState::Failed(why) if allow_energy => {
            let _ = why;
            DetectorSupport {
                kind: DetectorKind::Energy,
                fallback: Some(DetectorFallback::ModelUnavailable),
                refused: None,
            }
        }
        ModelState::Failed(why) => DetectorSupport {
            kind: DetectorKind::Silero,
            fallback: None,
            refused: Some(why),
        },
    }
}

#[cfg(feature = "silero-vad")]
fn silero_config() -> crate::core::silero_vad::SileroVADConfig {
    let mut c = crate::core::silero_vad::SileroVADConfig::default();
    // The engine resets the recurrent state itself, on the sample clock and never mid-segment;
    // the wall-clock reset would fire every 5 s, mid-utterance included.
    c.state_reset_interval_secs = 0.0;
    c
}

#[cfg(feature = "silero-vad")]
struct SileroSpeechDetector(crate::core::silero_vad::SileroVAD);

#[cfg(feature = "silero-vad")]
impl SpeechDetector for SileroSpeechDetector {
    fn probability(&mut self, frame: &[f32]) -> Result<f32, DetectorError> {
        self.0
            .process(frame)
            .map(|r| r.probability)
            .map_err(|e| DetectorError(e.to_string()))
    }

    fn reset(&mut self) {
        self.0.reset();
    }

    fn kind(&self) -> DetectorKind {
        DetectorKind::Silero
    }
}

/// Builds one detector per session, of the kind `support` says.
pub fn detector_factory(support: &DetectorSupport) -> DetectorFactory {
    match support.kind {
        #[cfg(feature = "silero-vad")]
        DetectorKind::Silero => Arc::new(|| {
            Box::pin(async {
                let vad = crate::core::silero_vad::SileroVAD::new(silero_config())
                    .await
                    .map_err(|e| DetectorError(e.to_string()))?;
                Ok(Box::new(SileroSpeechDetector(vad)) as Box<dyn SpeechDetector>)
            })
        }),
        _ => Arc::new(|| Box::pin(async { Ok(Box::new(EnergyDetector::new()) as Box<dyn SpeechDetector>) })),
    }
}

/// The audio end-of-turn model when it is ready.
pub fn audio_model() -> Option<Arc<dyn EndOfTurnModel>> {
    #[cfg(feature = "smart-turn")]
    {
        models()
            .smart_turn_pool
            .read()
            .clone()
            .map(|p| p as Arc<dyn EndOfTurnModel>)
    }
    #[cfg(not(feature = "smart-turn"))]
    {
        None
    }
}

/// SmartTurn asked once per pause, on the whole clip: one mel computation, one prediction (the
/// usage the accuracy test validates). The continuous pipeline is not used: its mel window is
/// rebuilt from a buffer trimmed to 400 samples on every call and never reaches the model's window.
#[cfg(feature = "smart-turn")]
pub struct SmartTurnOnDemand {
    pool: Vec<tokio::sync::Mutex<crate::core::smart_turn::SmartTurnDetector>>,
    next: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "smart-turn")]
impl std::fmt::Debug for SmartTurnOnDemand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmartTurnOnDemand").field("pool", &self.pool.len()).finish()
    }
}

#[cfg(feature = "smart-turn")]
impl SmartTurnOnDemand {
    pub async fn load(size: usize) -> anyhow::Result<Self> {
        let mut pool = Vec::with_capacity(size);
        for _ in 0..size.max(1) {
            let d = crate::core::smart_turn::SmartTurnDetector::new(
                crate::core::smart_turn::SmartTurnDetectorConfig::default(),
            )
            .await?;
            pool.push(tokio::sync::Mutex::new(d));
        }
        Ok(Self {
            pool,
            next: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

#[cfg(feature = "smart-turn")]
#[async_trait::async_trait]
impl EndOfTurnModel for SmartTurnOnDemand {
    async fn completion_probability(&self, audio_16k: Vec<f32>) -> Result<f32, EndOfTurnError> {
        use crate::core::smart_turn::{MelExtractor, MelExtractorConfig, SMART_TURN_MAX_FRAMES};
        let frames = tokio::task::spawn_blocking(move || {
            // A fresh extractor per call: no other caller's audio in its buffer.
            let mut mel = MelExtractor::new(MelExtractorConfig::default())?;
            mel.process(&audio_16k)?;
            anyhow::Ok(mel.get_mel_2d_padded(SMART_TURN_MAX_FRAMES))
        })
        .await
        .map_err(|e| EndOfTurnError(e.to_string()))?
        .map_err(|e| EndOfTurnError(e.to_string()))?;
        let i = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.pool.len();
        let mut detector = self.pool[i].lock().await;
        detector
            .predict(&frames)
            .await
            .map(|r| r.probability)
            .map_err(|e| EndOfTurnError(e.to_string()))
    }
}

/// The gateway's text end-of-turn model as the engine's second rung.
pub struct TextTurnModel(pub Arc<tokio::sync::RwLock<crate::core::turn_detect::TurnDetector>>);

impl std::fmt::Debug for TextTurnModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextTurnModel")
    }
}

#[async_trait::async_trait]
impl EndOfTurnTextModel for TextTurnModel {
    async fn is_complete(&self, turn_text: &str) -> Result<bool, EndOfTurnError> {
        self.0
            .read()
            .await
            .is_turn_complete(turn_text)
            .await
            .map_err(|e| EndOfTurnError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_silero_uses_the_energy_detector_and_says_why() {
        if cfg!(feature = "silero-vad") {
            return;
        }
        let s = build_support(false);
        assert_eq!(s.kind, DetectorKind::Energy);
        assert_eq!(s.fallback, Some(DetectorFallback::FeatureNotBuilt));
        assert!(s.refused.is_none());
    }

    #[test]
    fn a_failed_silero_model_refuses_unless_the_operator_allows_energy() {
        let m = models();
        let before = m.silero.read().clone();
        *m.silero.write() = ModelState::Failed("no file".into());
        assert!(build_support(false).refused.is_some());
        let allowed = build_support(true);
        assert_eq!(allowed.kind, DetectorKind::Energy);
        assert_eq!(allowed.fallback, Some(DetectorFallback::ModelUnavailable));
        *m.silero.write() = before;
    }
}
