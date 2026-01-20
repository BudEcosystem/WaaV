//! Configuration types for the VoiceManager

use crate::core::{stt::STTConfig, tts::TTSConfig};

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
    /// Configuration for speech final timing control
    pub speech_final_config: SpeechFinalConfig,
    /// Configuration for audio-based smart turn detection (optional).
    /// When enabled, processes audio through VAD and/or ML-based turn detection
    /// to determine when the user has finished speaking.
    #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
    pub smart_turn_config: Option<SmartTurnProcessorConfig>,
}

impl VoiceManagerConfig {
    /// Create a new VoiceManagerConfig with default speech final configuration
    pub fn new(stt_config: STTConfig, tts_config: TTSConfig) -> Self {
        Self {
            stt_config,
            tts_config,
            speech_final_config: SpeechFinalConfig::default(),
            #[cfg(any(feature = "silero-vad", feature = "smart-turn"))]
            smart_turn_config: None,
        }
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
            speech_final_config,
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
            speech_final_config: SpeechFinalConfig::default(),
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
            speech_final_config,
            smart_turn_config,
        }
    }
}
