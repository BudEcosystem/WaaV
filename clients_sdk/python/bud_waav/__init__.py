"""
bud-waav SDK

Python SDK for Bud WaaV AI Gateway - Speech-to-Text, Text-to-Speech, and Voice AI

Example:
    >>> from bud_waav import BudClient
    >>>
    >>> async def main():
    ...     bud = BudClient(base_url="http://localhost:3001", api_key="your-api-key")
    ...
    ...     # STT (Speech-to-Text)
    ...     async with bud.stt.connect(provider="deepgram") as session:
    ...         async for result in session.transcribe_stream(audio_generator()):
    ...             print(result.text)
    ...
    ...     # TTS (Text-to-Speech)
    ...     async with bud.tts.connect(provider="elevenlabs") as session:
    ...         await session.speak("Hello, world!")
    ...
    ...     # Talk (Bidirectional Voice)
    ...     async with bud.talk.connect(
    ...         stt={"provider": "deepgram"},
    ...         tts={"provider": "elevenlabs"}
    ...     ) as session:
    ...         async for event in session:
    ...             if event.type == "transcript":
    ...                 print(event.text)
"""

from .client import BudClient
from .types import (
    # Provider types
    STTProvider,
    TTSProvider,
    RealtimeProvider,
    CANONICAL_LANGUAGES,
    STT_PROVIDER_CAPABILITIES,
    TTS_PROVIDER_CAPABILITIES,
    REALTIME_PROVIDER_CAPABILITIES,
    is_valid_stt_provider,
    is_valid_tts_provider,
    is_valid_realtime_provider,
    get_provider_capabilities,
    language_capabilities,
    # Configuration types
    STTConfig,
    TTSConfig,
    TranslationConfig,
    LiveKitConfig,
    FeatureFlags,
    STTResult,
    TranscriptEvent,
    AudioEvent,
    Voice,
    WordInfo,
    PercentileStats,
    STTMetrics,
    TTSMetrics,
    MetricsSummary,
    LiveKitTokenRequest,
    LiveKitTokenResponse,
    RoomInfo,
    SIPHook,
    SIPHookCreateRequest,
    SIPHookCreateResponse,
    # Emotion types (Unified Emotion System)
    Emotion,
    DeliveryStyle,
    EmotionIntensityLevel,
    EmotionConfig,
    intensity_to_number,
    # Voice descriptor (abstract voice selection, resolved server-side; P4)
    VoiceDescriptor,
    VoiceGender,
    VoiceAge,
    # Conversation / agent-loop types (built-in LLM loop + reasoning)
    ConversationConfig,
    ReasoningEffort,
    LatencyFiller,
    RoutingMode,
    MuteStrategy,
    # DAG routing types
    DAGNodeType,
    OutputDestination,
    JoinStrategy,
    DAGDataType,
    DAGNode,
    DAGEdge,
    DAGDefinition,
    DAGConfig,
    DAGValidationResult,
    validate_dag_definition,
    TEMPLATE_SIMPLE_STT,
    TEMPLATE_SIMPLE_TTS,
    TEMPLATE_VOICE_ASSISTANT,
    BUILTIN_TEMPLATES,
    get_builtin_template,
    # Audio features types
    TurnDetectionConfig,
    NoiseFilterConfig,
    VADModeType,
    ExtendedVADConfig,
    AudioFeatures,
    DEFAULT_TURN_DETECTION,
    DEFAULT_NOISE_FILTER,
    DEFAULT_VAD,
    create_audio_features,
    # Recording types
    RecordingStatus,
    RecordingFormat,
    RecordingInfo,
    RecordingFilter,
    RecordingList,
    # Realtime session types
    VADConfig,
    InputTranscriptionConfig,
    RealtimeSessionConfig,
    RealtimeTranscript,
    RealtimeSpeechEvent,
    RealtimeAudioChunk,
    VOICE_DEFAULTS,
    REALTIME_DEFAULTS,
    # Voice cloning types
    VoiceCloneProvider,
    CloneMode,
    VoiceCloneRequest,
    VoiceCloneStatus,
    VoiceCloneResponse,
    # Hume EVI types
    HumeEVIVersion,
    HumeEVIConfig,
    ProsodyScores,
)
from .errors import (
    BudError,
    ConnectionError,
    TimeoutError,
    ReconnectError,
    APIError,
    RateLimitError,
    STTError,
    TranscriptionError,
    TTSError,
    SynthesisError,
    ConfigurationError,
)
from .config_mirror import ADAPTER_KINDS, SDK_CONFIG_REACH
from .pipelines import (
    BudSTT,
    STTSession,
    BudTTS,
    TTSSession,
    BudTalk,
    TalkSession,
    TalkEvent,
    ConfigWarning,
    BudTranscribe,
    TranscribeSession,
    # Realtime pipeline
    BudRealtime,
    RealtimeSession,
    RealtimeConfig,
    RealtimeState,
    ToolDefinition,
    FunctionCallEvent,
    RealtimeTranscriptEvent,
    RealtimeAudioEvent,
    EmotionEvent,
    StateChangeEvent,
    # Gateway-native realtime (provider-agnostic /realtime — all 12 providers)
    GatewayRealtime,
    GatewayRealtimeConfig,
    GatewayTurnDetection,
    RealtimeTool,
    RealtimeEvent,
    SpeechEvent,
    ErrorEvent,
    SessionCreatedEvent,
    ResponseEvent,
    REALTIME_PROVIDERS,
    get_supported_realtime_providers,
)
from .rest import RestClient
from .voices import VoicesAPI, CloneResult
from .ws import WebSocketSession, SessionMetrics, ReconnectConfig, MessageQueue, QueueConfig, QueueStats
from .audio import AudioProcessor, VADConfig as AudioVADConfig, VADFrame, VADState, VoiceActivityDetector
from .metrics.slo import SLOTracker, SLODefinition, SLOViolation, SLOComparison

__version__ = "0.1.0"
__all__ = [
    # Main client
    "BudClient",
    # Provider types
    "STTProvider",
    "TTSProvider",
    "RealtimeProvider",
    "CANONICAL_LANGUAGES",
    "STT_PROVIDER_CAPABILITIES",
    "TTS_PROVIDER_CAPABILITIES",
    "REALTIME_PROVIDER_CAPABILITIES",
    "is_valid_stt_provider",
    "is_valid_tts_provider",
    "is_valid_realtime_provider",
    "get_provider_capabilities",
    "language_capabilities",
    # Configuration types
    "STTConfig",
    "TTSConfig",
    "TranslationConfig",
    "LiveKitConfig",
    "FeatureFlags",
    # Emotion types (Unified Emotion System)
    "Emotion",
    "DeliveryStyle",
    "EmotionIntensityLevel",
    "EmotionConfig",
    "intensity_to_number",
    # Voice descriptor (abstract voice selection, resolved server-side; P4)
    "VoiceDescriptor",
    "VoiceGender",
    "VoiceAge",
    # Conversation / agent-loop types (built-in LLM loop + reasoning)
    "ConversationConfig",
    "ReasoningEffort",
    "LatencyFiller",
    "RoutingMode",
    "MuteStrategy",
    # DAG routing types
    "DAGNodeType",
    "OutputDestination",
    "JoinStrategy",
    "DAGDataType",
    "DAGNode",
    "DAGEdge",
    "DAGDefinition",
    "DAGConfig",
    "DAGValidationResult",
    "validate_dag_definition",
    "TEMPLATE_SIMPLE_STT",
    "TEMPLATE_SIMPLE_TTS",
    "TEMPLATE_VOICE_ASSISTANT",
    "BUILTIN_TEMPLATES",
    "get_builtin_template",
    # Audio features types
    "TurnDetectionConfig",
    "NoiseFilterConfig",
    "VADModeType",
    "ExtendedVADConfig",
    "AudioFeatures",
    "DEFAULT_TURN_DETECTION",
    "DEFAULT_NOISE_FILTER",
    "DEFAULT_VAD",
    "create_audio_features",
    # Recording types
    "RecordingStatus",
    "RecordingFormat",
    "RecordingInfo",
    "RecordingFilter",
    "RecordingList",
    # Realtime session types
    "VADConfig",
    "InputTranscriptionConfig",
    "RealtimeSessionConfig",
    "RealtimeTranscript",
    "RealtimeSpeechEvent",
    "RealtimeAudioChunk",
    "VOICE_DEFAULTS",
    "REALTIME_DEFAULTS",
    # Voice cloning types + helpers
    "VoiceCloneProvider",
    "CloneMode",
    "VoiceCloneRequest",
    "VoiceCloneStatus",
    "VoiceCloneResponse",
    "VoicesAPI",
    "CloneResult",
    # Hume EVI types
    "HumeEVIVersion",
    "HumeEVIConfig",
    "ProsodyScores",
    # Result types
    "STTResult",
    "TranscriptEvent",
    "AudioEvent",
    "Voice",
    "WordInfo",
    # Metrics types
    "PercentileStats",
    "STTMetrics",
    "TTSMetrics",
    "MetricsSummary",
    "SessionMetrics",
    # LiveKit types
    "LiveKitTokenRequest",
    "LiveKitTokenResponse",
    "RoomInfo",
    # SIP types
    "SIPHook",
    "SIPHookCreateRequest",
    "SIPHookCreateResponse",
    # Error types
    "BudError",
    "ConnectionError",
    "TimeoutError",
    "ReconnectError",
    "APIError",
    "RateLimitError",
    "STTError",
    "TranscriptionError",
    "TTSError",
    "SynthesisError",
    "ConfigurationError",
    # Pipeline classes
    "BudSTT",
    "STTSession",
    "BudTTS",
    "TTSSession",
    "BudTalk",
    "TalkSession",
    "TalkEvent",
    "ConfigWarning",
    "BudTranscribe",
    "TranscribeSession",
    # Drift-guarded config mirror surface
    "SDK_CONFIG_REACH",
    "ADAPTER_KINDS",
    # Realtime pipeline
    "BudRealtime",
    "RealtimeSession",
    "RealtimeConfig",
    "RealtimeState",
    "ToolDefinition",
    "FunctionCallEvent",
    "RealtimeTranscriptEvent",
    "RealtimeAudioEvent",
    "EmotionEvent",
    "StateChangeEvent",
    # Gateway-native realtime (provider-agnostic /realtime — all 12 providers)
    "GatewayRealtime",
    "GatewayRealtimeConfig",
    "GatewayTurnDetection",
    "RealtimeTool",
    "RealtimeEvent",
    "SpeechEvent",
    "ErrorEvent",
    "SessionCreatedEvent",
    "ResponseEvent",
    "REALTIME_PROVIDERS",
    "get_supported_realtime_providers",
    # Utility classes
    "RestClient",
    "WebSocketSession",
    "ReconnectConfig",
    "AudioProcessor",
    # Voice Activity Detection
    "VoiceActivityDetector",
    "AudioVADConfig",
    "VADFrame",
    "VADState",
    # Message Queue
    "MessageQueue",
    "QueueConfig",
    "QueueStats",
    # SLO Tracking
    "SLOTracker",
    "SLODefinition",
    "SLOViolation",
    "SLOComparison",
]
