//! Configuration types for the VoiceManager

use crate::core::{
    resilience::ResilienceHandles,
    stt::{STTConfig, standard::StandardSTTConfig},
    tts::{TTSConfig, standard::StandardTTSConfig},
};

#[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
use crate::core::smart_turn::SmartTurnProcessorConfig;

/// Configuration for speech final timing control
#[derive(Debug, Clone, Copy)]
pub struct SpeechFinalConfig {
    /// Time to wait for STT provider to send real speech_final (ms)
    /// This is the primary window - we trust STT provider during this time
    pub stt_speech_final_wait_ms: u64,
    /// Maximum time to wait for turn detection inference to complete (ms)
    pub turn_detection_inference_timeout_ms: u64,
    /// Hard upper bound timeout for any user utterance (ms)
    /// This guarantees that no utterance will wait longer than this value
    /// even if neither the STT provider nor turn detector fire
    pub speech_final_hard_timeout_ms: u64,
    /// Window to prevent duplicate speech_final events (ms)
    pub duplicate_window_ms: usize,
}

impl Default for SpeechFinalConfig {
    fn default() -> Self {
        Self {
            stt_speech_final_wait_ms: 1800, // Wait 1.8s for real speech_final from STT
            turn_detection_inference_timeout_ms: 500, // 500ms max for model inference
            speech_final_hard_timeout_ms: 4000, // 4s hard upper bound for any utterance
            duplicate_window_ms: 500,       // 500ms duplicate prevention window
        }
    }
}

/// Configuration for the VoiceManager
#[derive(Debug, Clone)]
pub struct VoiceManagerConfig {
    /// Configuration for the STT provider
    pub stt_config: STTConfig,
    /// Configuration for the TTS provider
    pub tts_config: TTSConfig,
    /// Optional standardized STT config carrying advanced features (diarization, keyterms, …).
    ///
    /// When present, [`VoiceManager::new`](crate::core::voice_manager::VoiceManager::new) builds
    /// the STT provider through the reachable W1 keystone path (`create_stt_standard` →
    /// `from_standard`) so client-supplied features are honored END-TO-END; when `None` it falls
    /// back to the flat `create_stt_provider` factory (unchanged legacy behavior). The flat
    /// `stt_config` is always kept in sync (it equals `standard_stt.base`) for cache hashing and
    /// other consumers that read the flat view.
    pub standard_stt: Option<StandardSTTConfig>,
    /// Optional standardized TTS config carrying advanced features. Mirrors `standard_stt`: when
    /// present, the TTS provider is built through `create_tts_standard` → `from_standard`.
    pub standard_tts: Option<StandardTTSConfig>,
    /// Configuration for speech final timing control
    pub speech_final_config: SpeechFinalConfig,
    /// Shared, process-global resilience handles for the STT provider (W-D2): the single
    /// reconnect governor + this provider's shared circuit breaker, from
    /// [`crate::core::state::CoreState::resilience`]. When present,
    /// [`VoiceManager::new`](crate::core::voice_manager::VoiceManager::new) injects them into the
    /// constructed STT provider so all sessions of a provider trip the same breaker and share the
    /// process-wide reconnect cap. When `None`, the provider falls back to its own per-session
    /// governor/breaker (legacy behaviour; used by unit tests that don't boot `CoreState`).
    pub resilience: Option<ResilienceHandles>,
    /// Configuration for audio-based smart turn detection (optional).
    /// When enabled, processes audio through VAD and/or ML-based turn detection
    /// to determine when the user has finished speaking.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub smart_turn_config: Option<SmartTurnProcessorConfig>,
}

impl VoiceManagerConfig {
    /// Create a new VoiceManagerConfig with default speech final configuration.
    ///
    /// Uses the flat factory path (no advanced features). For the reachable keystone path that
    /// honors advanced features, use [`Self::from_standard`].
    pub fn new(stt_config: STTConfig, tts_config: TTSConfig) -> Self {
        Self {
            stt_config,
            tts_config,
            standard_stt: None,
            standard_tts: None,
            speech_final_config: SpeechFinalConfig::default(),
            resilience: None,
            #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
            smart_turn_config: None,
        }
    }

    /// Create a VoiceManagerConfig from the standardized STT/TTS configs (W1 keystone path).
    ///
    /// The flat `stt_config`/`tts_config` are derived from the standardized bases so flat consumers
    /// (cache hashing, etc.) keep working, while `standard_stt`/`standard_tts` carry the advanced
    /// `features`/`extras` that [`VoiceManager::new`](crate::core::voice_manager::VoiceManager::new)
    /// threads into `create_stt_standard`/`create_tts_standard`.
    pub fn from_standard(
        standard_stt: StandardSTTConfig,
        standard_tts: StandardTTSConfig,
    ) -> Self {
        Self {
            stt_config: standard_stt.base.clone(),
            tts_config: standard_tts.base.clone(),
            standard_stt: Some(standard_stt),
            standard_tts: Some(standard_tts),
            speech_final_config: SpeechFinalConfig::default(),
            resilience: None,
            #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
            smart_turn_config: None,
        }
    }

    /// Attach the shared, process-global resilience handles (W-D2) for the STT provider.
    ///
    /// Call this on the config built for a live session so the constructed STT provider shares
    /// the gateway-wide reconnect governor and this provider's circuit breaker.
    pub fn with_resilience(mut self, resilience: ResilienceHandles) -> Self {
        self.resilience = Some(resilience);
        self
    }

    /// Create a new VoiceManagerConfig with custom speech final configuration
    pub fn with_speech_final_config(
        stt_config: STTConfig,
        tts_config: TTSConfig,
        speech_final_config: SpeechFinalConfig,
    ) -> Self {
        Self {
            stt_config,
            tts_config,
            standard_stt: None,
            standard_tts: None,
            speech_final_config,
            resilience: None,
            #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
            smart_turn_config: None,
        }
    }

    /// Create a new VoiceManagerConfig with smart turn detection enabled.
    ///
    /// Smart turn detection uses audio-based analysis (VAD + ML model) to
    /// determine when the user has finished speaking, complementing the
    /// text-based turn detection.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub fn with_smart_turn(
        stt_config: STTConfig,
        tts_config: TTSConfig,
        smart_turn_config: SmartTurnProcessorConfig,
    ) -> Self {
        Self {
            stt_config,
            tts_config,
            standard_stt: None,
            standard_tts: None,
            speech_final_config: SpeechFinalConfig::default(),
            resilience: None,
            smart_turn_config: Some(smart_turn_config),
        }
    }

    /// Create a new VoiceManagerConfig with all options specified.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub fn with_all(
        stt_config: STTConfig,
        tts_config: TTSConfig,
        speech_final_config: SpeechFinalConfig,
        smart_turn_config: Option<SmartTurnProcessorConfig>,
    ) -> Self {
        Self {
            stt_config,
            tts_config,
            standard_stt: None,
            standard_tts: None,
            speech_final_config,
            resilience: None,
            smart_turn_config,
        }
    }
}
