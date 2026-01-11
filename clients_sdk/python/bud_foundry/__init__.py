"""
bud-foundry SDK

Python SDK for Bud Foundry AI Gateway - Speech-to-Text, Text-to-Speech, and Voice AI

Example:
    >>> from bud_foundry import BudClient
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
    STT_PROVIDER_CAPABILITIES,
    TTS_PROVIDER_CAPABILITIES,
    REALTIME_PROVIDER_CAPABILITIES,
    is_valid_stt_provider,
    is_valid_tts_provider,
    is_valid_realtime_provider,
    get_provider_capabilities,
    # Configuration types
    STTConfig,
    TTSConfig,
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
    # DAG routing types
    DAGNodeType,
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
)
from .errors import (
    BudError,
    ConnectionError,
    TimeoutError,
    ReconnectError,
    APIError,
    STTError,
    TranscriptionError,
    TTSError,
    SynthesisError,
    ConfigurationError,
)
from .pipelines import (
    BudSTT,
    STTSession,
    BudTTS,
    TTSSession,
    BudTalk,
    TalkSession,
    TalkEvent,
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
)
from .rest import RestClient
from .ws import WebSocketSession, SessionMetrics, ReconnectConfig
from .audio import AudioProcessor

__version__ = "0.1.0"
__all__ = [
    # Main client
    "BudClient",
    # Provider types
    "STTProvider",
    "TTSProvider",
    "RealtimeProvider",
    "STT_PROVIDER_CAPABILITIES",
    "TTS_PROVIDER_CAPABILITIES",
    "REALTIME_PROVIDER_CAPABILITIES",
    "is_valid_stt_provider",
    "is_valid_tts_provider",
    "is_valid_realtime_provider",
    "get_provider_capabilities",
    # Configuration types
    "STTConfig",
    "TTSConfig",
    "LiveKitConfig",
    "FeatureFlags",
    # Emotion types (Unified Emotion System)
    "Emotion",
    "DeliveryStyle",
    "EmotionIntensityLevel",
    "EmotionConfig",
    "intensity_to_number",
    # DAG routing types
    "DAGNodeType",
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
    "BudTranscribe",
    "TranscribeSession",
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
    # Utility classes
    "RestClient",
    "WebSocketSession",
    "ReconnectConfig",
    "AudioProcessor",
]
