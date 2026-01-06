pub mod cache;
pub mod emotion;
pub mod providers;
pub mod realtime;
pub mod state;
pub mod stt;
pub mod tts;
pub mod turn_detect;
pub mod voice_manager;

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
