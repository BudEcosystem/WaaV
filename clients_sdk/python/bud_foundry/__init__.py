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
)
from .rest import RestClient
from .ws import WebSocketSession, SessionMetrics, ReconnectConfig
from .audio import AudioProcessor

__version__ = "0.1.0"
__all__ = [
    # Main client
    "BudClient",
    # Configuration types
    "STTConfig",
    "TTSConfig",
    "LiveKitConfig",
    "FeatureFlags",
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
    # Utility classes
    "RestClient",
    "WebSocketSession",
    "ReconnectConfig",
    "AudioProcessor",
]
