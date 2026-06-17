pub mod audio;
pub mod cache;
pub mod conversation;
pub mod emotion;
pub mod flow;
/// Unified language system (P2): one canonical region-qualified BCP-47 token per request, mapped to
/// each provider's native notation. Mirrors the [`emotion`] mapper chassis. Registered right after
/// `emotion` per the standardization plan.
pub mod lang;
pub mod llm;
pub mod metrics;
// Canonical SSRF/URL validation shared by the DAG endpoint nodes, the
// streaming-TTS endpoint override, and the conversation LLM URL check.
pub mod net;
pub mod observability;
// Hardware execution-provider policy (MASTER_PLAN §7 H). Available exactly when
// `ort` is pulled in, i.e. whenever any of the three ONNX consumers is enabled.
#[cfg(any(feature = "turn-detect", feature = "smart-turn", feature = "silero-vad"))]
pub mod onnx;
pub mod pipeline;
pub mod providers;
pub mod readiness;
pub mod realtime;
pub mod resilience;
pub mod silero_vad;
pub mod smart_turn;
pub mod state;
pub mod stt;
pub mod text;
pub mod tts;
pub mod turn;
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

// Re-export the feature-flag-free LLM client (shared by the conversation loop
// and the DAG llm node).
pub use llm::{
    ChatMessage, LlmClient, LlmClientConfig, LlmError, LlmResponse, LlmResult, MessageRole,
    ResponseFormat, ToolDefinition,
};

// Re-export the built-in conversation orchestrator (W-O2).
pub use conversation::{
    ConversationConfig, ConversationOrchestrator, ConversationOrchestratorError,
};

// Re-export CoreState for external use
pub use state::CoreState;

// Re-export emotion types for convenience
pub use emotion::{
    DeliveryStyle, Emotion, EmotionConfig, EmotionIntensity, EmotionMapper, EmotionMethod,
    IntensityLevel, MappedEmotion, ProviderEmotionSupport, get_mapper_for_provider,
    map_emotion_for_provider, provider_supports_emotions, validate_emotion_config,
};

// Re-export the unified language system (P2): canonical language + provider mapping.
pub use lang::{
    CanonicalLanguage, LanguageMapper, MappedLanguage, NotationKind, ProviderLanguageSupport,
    get_language_mapper, language_support_matrix, map_language, resolve as resolve_language,
    to_provider_language,
};

// Re-export audio types for VAD
pub use audio::{AudioRingBuffer, VADAnalyzer, VADParams, VADState, VADTransition};

// Re-export Silero VAD types
pub use silero_vad::{SileroVAD, SileroVADConfig, SileroVADResult};

// Re-export metrics types for provider monitoring
pub use metrics::{MetricsRegistry, ProviderMetrics, ProviderMetricsSnapshot, RequestTimer};

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

// Re-export resilience primitives (circuit breaker + reconnect governor + the shared
// process-global registry) — W-D2.
pub use resilience::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerSnapshot, CircuitState, ReconnectGovernor,
    ReconnectPermit, ResilienceHandles, ResilienceRegistry,
};

// Re-export the generic reconnectable stream supervisor — W-D1.
pub use websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

// Re-export Smart Turn types for audio-based turn detection
pub use smart_turn::{
    MelExtractor, MelExtractorConfig, SMART_TURN_MAX_FRAMES, SmartTurnDetector,
    SmartTurnDetectorBuilder, SmartTurnDetectorConfig, SmartTurnResult, WHISPER_HOP_LENGTH,
    WHISPER_MAX_DURATION_SECS, WHISPER_MAX_FRAMES, WHISPER_N_FFT, WHISPER_N_MELS,
    WHISPER_SAMPLE_RATE,
};

// Re-export Turn Decision Engine types for ensemble turn detection
pub use turn_decision::{
    TurnDecision, TurnDecisionEngine, TurnDecisionEngineBuilder, TurnDecisionEngineConfig,
    TurnSignal, TurnState,
};
