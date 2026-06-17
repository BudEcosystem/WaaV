"""
Bud Foundry pipelines
"""

from .stt import BudSTT, STTSession
from .tts import BudTTS, TTSSession
from .talk import BudTalk, TalkSession, TalkEvent, ConfigWarning
from .transcribe import BudTranscribe, TranscribeSession
from .realtime import (
    BudRealtime,
    RealtimeSession,
    RealtimeConfig,
    RealtimeState,
    ToolDefinition,
    FunctionCallEvent,
    TranscriptEvent as RealtimeTranscriptEvent,
    AudioEvent as RealtimeAudioEvent,
    EmotionEvent,
    StateChangeEvent,
)
from .realtime_gateway import (
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

__all__ = [
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
    # Realtime pipeline (provider-native escape hatch)
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
]
