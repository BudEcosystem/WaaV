pub mod audio;
pub mod cache;
pub mod emotion;
pub mod metrics;
pub mod observability;
pub mod pipeline;
pub mod providers;
pub mod realtime;
pub mod silero_vad;
pub mod smart_turn;
pub mod state;
pub mod stt;
pub mod tts;
pub mod turn_decision;
pub mod turn_detect;
pub mod voice_manager;
pub mod websocket;

#[cfg(feature = "turn-detect")]
pub use turn_detect::{TurnDetector, TurnDetectorBuilder, TurnDetectorConfig};
#[cfg(not(feature = "turn-detect"))]
pub use turn_detect::{TurnDetector, TurnDetectorBuilder, TurnDetectorConfig};

// Re-export commonly used types for convenience
pub use stt::{
    BaseSTT, DeepgramSTT, DeepgramSTTConfig, STTConfig, STTConnectionState, STTError, STTProvider,
    STTResult, STTResultCallback, STTStats, create_stt_provider, create_stt_provider_from_enum,
    get_supported_stt_providers,
};

pub use tts::{
    AudioCallback, AudioData, BaseTTS, BoxedTTS, ConnectionState, DeepgramTTS, TTSConfig, TTSError,
    TTSFactory, TTSResult, create_tts_provider, get_tts_provider_urls,
};

pub use realtime::{
    BaseRealtime, BoxedRealtime, OpenAIRealtime, RealtimeConfig, RealtimeError, RealtimeProvider,
    RealtimeResult, create_realtime_provider, create_realtime_provider_from_enum,
    get_supported_realtime_providers,
};

pub use voice_manager::{
    STTCallback, TTSAudioCallback, TTSErrorCallback, VoiceManager, VoiceManagerConfig,
    VoiceManagerError, VoiceManagerResult,
};

// Re-export CoreState for external use
pub use state::CoreState;

// Re-export emotion types for convenience
pub use emotion::{
    DeliveryStyle, Emotion, EmotionConfig, EmotionIntensity, EmotionMapper, EmotionMethod,
    IntensityLevel, MappedEmotion, ProviderEmotionSupport, get_mapper_for_provider,
    map_emotion_for_provider, provider_supports_emotions, validate_emotion_config,
};

// Re-export audio types for VAD
pub use audio::{AudioRingBuffer, VADAnalyzer, VADParams, VADState, VADTransition};

// Re-export Silero VAD types
pub use silero_vad::{SileroVAD, SileroVADConfig, SileroVADResult};

// Re-export metrics types for provider monitoring
pub use metrics::{ProviderMetrics, ProviderMetricsSnapshot, RequestTimer};

// Re-export observability types for monitoring
pub use observability::{
    BotSpeakingState, LatencyMetrics, ObserverRegistry, UserBotLatencyObserver, VoiceObserver,
};

// Re-export pipeline types for audio processing
pub use pipeline::{FramePriority, FramePriorityQueue, PriorityFrame, QueueSnapshot};

// Re-export websocket types for reconnection
pub use websocket::{
    ReconnectionConfig, ReconnectionConfigBuilder, ReconnectionManager, ReconnectionSnapshot,
    ReconnectionState,
};

// Re-export Smart Turn types for audio-based turn detection
pub use smart_turn::{
    MelExtractor, MelExtractorConfig,
    SmartTurnDetector, SmartTurnDetectorBuilder, SmartTurnDetectorConfig, SmartTurnResult,
    SMART_TURN_MAX_FRAMES, WHISPER_HOP_LENGTH, WHISPER_MAX_DURATION_SECS, WHISPER_MAX_FRAMES,
    WHISPER_N_FFT, WHISPER_N_MELS, WHISPER_SAMPLE_RATE,
};

// Re-export Turn Decision Engine types for ensemble turn detection
pub use turn_decision::{
    TurnDecision, TurnDecisionEngine, TurnDecisionEngineBuilder, TurnDecisionEngineConfig,
    TurnSignal, TurnState,
};
