//! `SegmentedStt`: the segmented engine as a `BaseSTT` provider.
//!
//! | Method | Behaviour |
//! | --- | --- |
//! | `new` | refused: built only by the live factory |
//! | `connect` | builds the session's detector and starts the engine |
//! | `send_audio` | stamps and queues the chunk; never waits |
//! | `request_flush` | a commit, ordered behind the audio already sent |
//! | `disconnect` | a real stop: no flush, uploads in flight dropped |
//!
//! Callback slots are read at the moment of use, so the session may register them before or after
//! `connect`, any number of times; they survive `disconnect`.

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::oneshot;
use waav_segmented_stt::detector::{DetectorFactory, SpeechDetector};
use waav_segmented_stt::endpointer::{EndOfTurnModel, EndOfTurnTextModel};
use waav_segmented_stt::engine::{Callbacks, Clock, EngineConfig, EngineHandle, EngineParts};
use waav_segmented_stt::sequencer::SegmentUpload;
use waav_segmented_stt::types::EngineResult;

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use crate::core::stt::speech_activity::{
    FlushOutcome, NoticeCallback, SegmentAdmissionHook, SegmentOutcomeSink, SpeechActivityCallback,
    SttLiveFacts,
};

/// Everything `connect` needs to start the engine.
#[derive(Clone)]
pub struct SegmentedPlan {
    pub engine: EngineConfig,
    pub detector: DetectorFactory,
    pub upload: Arc<dyn SegmentUpload>,
    pub audio_model: Option<Arc<dyn EndOfTurnModel>>,
    pub text_model: Option<Arc<dyn EndOfTurnTextModel>>,
    pub clock: Arc<dyn Clock>,
    /// `get_provider_info`: the adapter id, which always contains `segmented`.
    pub provider_info: &'static str,
}

impl std::fmt::Debug for SegmentedPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegmentedPlan")
            .field("provider_info", &self.provider_info)
            .finish_non_exhaustive()
    }
}

/// The gateway monotonic clock, so speech-event times line up with the profiler's.
#[derive(Debug, Default)]
pub struct GatewayClock;

impl Clock for GatewayClock {
    fn now_ms(&self) -> u64 {
        crate::core::observability::now_monotonic_ns() / 1_000_000
    }
}

pub struct SegmentedStt {
    config: STTConfig,
    plan: SegmentedPlan,
    engine: Option<EngineHandle>,
    callbacks: Arc<Callbacks>,
}

impl std::fmt::Debug for SegmentedStt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegmentedStt")
            .field("provider", &self.config.provider)
            .field("model", &self.config.model)
            .field(
                "running",
                &self.engine.as_ref().is_some_and(EngineHandle::is_alive),
            )
            .finish()
    }
}

/// Map one engine result to the gateway's result type.
pub fn to_stt_result(r: EngineResult) -> STTResult {
    let mut out = STTResult::new(r.transcript, r.is_final, r.is_speech_final, r.confidence);
    if r.is_final {
        out = out.finalized();
    }
    out.vendor_confidence = r.vendor_confidence;
    out.detected_language = r.detected_language;
    out.audio_duration = r.audio_duration;
    out.vendor_request_id = r.vendor_request_id;
    out.speech_turn_id = Some(r.turn_id);
    out
}

impl SegmentedStt {
    /// The only production constructor: the live factory's.
    pub fn from_plan(config: STTConfig, plan: SegmentedPlan) -> Self {
        Self {
            config,
            plan,
            engine: None,
            callbacks: Arc::new(Callbacks::default()),
        }
    }

    fn running(&self) -> Option<&EngineHandle> {
        self.engine.as_ref().filter(|e| e.is_alive())
    }
}

#[async_trait::async_trait]
impl BaseSTT for SegmentedStt {
    fn new(_config: STTConfig) -> Result<Self, STTError> {
        Err(STTError::ConfigurationError(
            "a segmented session is built by the live factory, which knows the transport".into(),
        ))
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.running().is_some() {
            return Ok(());
        }
        let detector: Box<dyn SpeechDetector> = (self.plan.detector)().await.map_err(|e| {
            STTError::ConfigurationError(format!("stt_segmentation_unavailable: {e}"))
        })?;
        let handle = EngineHandle::spawn(
            self.plan.engine.clone(),
            EngineParts {
                detector,
                upload: Arc::clone(&self.plan.upload),
                audio_model: self.plan.audio_model.clone(),
                text_model: self.plan.text_model.clone(),
                clock: Arc::clone(&self.plan.clock),
            },
            Arc::clone(&self.callbacks),
        )
        .map_err(|e| STTError::InvalidAudioFormat(format!("stt_segmentation_unavailable: {e}")))?;
        self.engine = Some(handle);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if let Some(engine) = self.engine.take() {
            engine.stop().await;
        }
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.running().is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        match self.running() {
            Some(e) => e
                .send_audio(audio_data)
                .map_err(|e| STTError::ConnectionFailed(e.to_string())),
            None => Err(STTError::ConnectionFailed(
                "the segmented engine is not running".into(),
            )),
        }
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.callbacks.result.write() = Some(Arc::new(move |r: EngineResult| {
            let cb = Arc::clone(&callback);
            Box::pin(async move { cb(to_stt_result(r)).await })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.callbacks.fatal.write() = Some(Arc::new(move |f| {
            let cb = Arc::clone(&callback);
            Box::pin(async move {
                cb(STTError::ProviderError(format!(
                    "stt_unavailable ({}): {}",
                    f.reason, f.message
                )))
                .await
            })
        }));
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        if config.provider != self.config.provider || config.model != self.config.model {
            return Err(STTError::ConfigurationError(
                "the provider or model of a segmented session cannot change during the call".into(),
            ));
        }
        self.set_config_only(config);
        Ok(())
    }

    fn set_config_only(&mut self, config: STTConfig) {
        if config.language != self.config.language
            && let Some(e) = self.running()
        {
            let lang =
                Some(config.language.clone()).filter(|l| !l.trim().is_empty() && l != "auto");
            e.set_language(lang);
        }
        self.config = config;
    }

    fn get_provider_info(&self) -> &'static str {
        self.plan.provider_info
    }

    fn on_speech_activity(&mut self, callback: SpeechActivityCallback) -> bool {
        *self.callbacks.activity.write() = Some(callback);
        true
    }

    fn set_segment_admission(&mut self, hook: SegmentAdmissionHook) {
        *self.callbacks.admission.write() = Some(hook);
    }

    fn request_flush(&mut self) -> Option<oneshot::Receiver<FlushOutcome>> {
        self.running().map(EngineHandle::flush)
    }

    fn on_notice(&mut self, callback: NoticeCallback) {
        *self.callbacks.notice.write() = Some(callback);
    }

    fn set_outcome_sink(&mut self, sink: Arc<dyn SegmentOutcomeSink>) {
        *self.callbacks.outcome.write() = Some(sink);
    }

    fn live_facts(&self) -> Option<SttLiveFacts> {
        self.engine.as_ref().map(|e| e.facts().clone())
    }
}
