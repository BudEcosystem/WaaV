"""
Type definitions for bud-foundry SDK
"""

from enum import Enum
from typing import Any, Callable, Literal, Optional, Union
from pydantic import BaseModel, ConfigDict, Field


# =============================================================================
# Provider Types (Comprehensive List)
# =============================================================================


class STTProvider(str, Enum):
    """
    Supported Speech-to-Text providers.
    All 10 providers supported by the gateway.
    """

    DEEPGRAM = "deepgram"
    GOOGLE = "google"
    AZURE = "azure"
    CARTESIA = "cartesia"
    GATEWAY = "gateway"
    ASSEMBLYAI = "assemblyai"
    AWS_TRANSCRIBE = "aws-transcribe"
    IBM_WATSON = "ibm-watson"
    GROQ = "groq"
    OPENAI_WHISPER = "openai-whisper"


class TTSProvider(str, Enum):
    """
    Supported Text-to-Speech providers.
    All 12 providers supported by the gateway.
    """

    DEEPGRAM = "deepgram"
    ELEVENLABS = "elevenlabs"
    GOOGLE = "google"
    AZURE = "azure"
    CARTESIA = "cartesia"
    OPENAI = "openai"
    AWS_POLLY = "aws-polly"
    IBM_WATSON = "ibm-watson"
    HUME = "hume"
    LMNT = "lmnt"
    PLAYHT = "playht"
    KOKORO = "kokoro"


class RealtimeProvider(str, Enum):
    """
    Supported Realtime (Audio-to-Audio) providers.
    """

    OPENAI_REALTIME = "openai-realtime"
    HUME_EVI = "hume-evi"


# Provider capability definitions
STT_PROVIDER_CAPABILITIES: dict[STTProvider, dict[str, Any]] = {
    STTProvider.DEEPGRAM: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "nl", "ja", "ko", "zh"],
        "models": ["nova-3", "nova-2", "enhanced", "base"],
    },
    STTProvider.GOOGLE: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["default", "command_and_search", "phone_call", "video"],
    },
    STTProvider.AZURE: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["default"],
    },
    STTProvider.CARTESIA: {
        "streaming": True,
        "diarization": False,
        "languages": ["en"],
        "models": ["default"],
    },
    STTProvider.GATEWAY: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["whisper-large-v3", "whisper-medium", "whisper-small"],
    },
    STTProvider.ASSEMBLYAI: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt"],
        "models": ["default", "nano"],
    },
    STTProvider.AWS_TRANSCRIBE: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["default"],
    },
    STTProvider.IBM_WATSON: {
        "streaming": True,
        "diarization": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["default"],
    },
    STTProvider.GROQ: {
        "streaming": False,
        "diarization": False,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["whisper-large-v3-turbo"],
    },
    STTProvider.OPENAI_WHISPER: {
        "streaming": False,
        "diarization": False,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["whisper-1"],
    },
}


TTS_PROVIDER_CAPABILITIES: dict[TTSProvider, dict[str, Any]] = {
    TTSProvider.DEEPGRAM: {
        "streaming": True,
        "ssml": False,
        "emotion": False,
        "voice_cloning": False,
        "languages": ["en"],
        "models": ["aura-asteria-en", "aura-luna-en", "aura-stella-en"],
    },
    TTSProvider.ELEVENLABS: {
        "streaming": True,
        "ssml": True,
        "emotion": True,
        "voice_cloning": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "pl", "hi", "ar"],
        "models": ["eleven_turbo_v2_5", "eleven_multilingual_v2", "eleven_monolingual_v1"],
    },
    TTSProvider.GOOGLE: {
        "streaming": True,
        "ssml": True,
        "emotion": False,
        "voice_cloning": False,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["en-US-Studio-O", "en-US-Wavenet-D"],
    },
    TTSProvider.AZURE: {
        "streaming": True,
        "ssml": True,
        "emotion": True,
        "voice_cloning": True,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["en-US-JennyNeural", "en-US-GuyNeural"],
    },
    TTSProvider.CARTESIA: {
        "streaming": True,
        "ssml": False,
        "emotion": True,
        "voice_cloning": True,
        "languages": ["en"],
        "models": ["sonic-3"],
    },
    TTSProvider.OPENAI: {
        "streaming": True,
        "ssml": False,
        "emotion": False,
        "voice_cloning": False,
        "languages": ["en"],
        "models": ["tts-1", "tts-1-hd"],
    },
    TTSProvider.AWS_POLLY: {
        "streaming": True,
        "ssml": True,
        "emotion": False,
        "voice_cloning": False,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["standard", "neural", "generative"],
    },
    TTSProvider.IBM_WATSON: {
        "streaming": True,
        "ssml": True,
        "emotion": True,
        "voice_cloning": False,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja"],
        "models": ["en-US_MichaelV3Voice", "en-US_AllisonV3Voice"],
    },
    TTSProvider.HUME: {
        "streaming": True,
        "ssml": False,
        "emotion": True,
        "voice_cloning": True,
        "languages": ["en"],
        "models": ["octave"],
    },
    TTSProvider.LMNT: {
        "streaming": True,
        "ssml": False,
        "emotion": False,
        "voice_cloning": True,
        "languages": ["en"],
        "models": ["default"],
    },
    TTSProvider.PLAYHT: {
        "streaming": True,
        "ssml": False,
        "emotion": True,
        "voice_cloning": True,
        "languages": ["en"],
        "models": ["PlayHT2.0", "PlayHT2.0-turbo"],
    },
    TTSProvider.KOKORO: {
        "streaming": True,
        "ssml": False,
        "emotion": False,
        "voice_cloning": False,
        "languages": ["en", "ja", "ko", "zh"],
        "models": ["kokoro-v1"],
    },
}


REALTIME_PROVIDER_CAPABILITIES: dict[RealtimeProvider, dict[str, Any]] = {
    RealtimeProvider.OPENAI_REALTIME: {
        "function_calling": True,
        "vision": False,
        "emotion_detection": False,
        "models": ["gpt-4o-realtime-preview", "gpt-4o-mini-realtime-preview"],
        "voices": ["alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"],
    },
    RealtimeProvider.HUME_EVI: {
        "function_calling": True,
        "vision": False,
        "emotion_detection": True,
        "models": ["evi-3", "evi-4-mini"],
        "voices": [],  # Custom voice IDs only
    },
}


def is_valid_stt_provider(provider: str) -> bool:
    """Check if a string is a valid STT provider."""
    return provider in [p.value for p in STTProvider]


def is_valid_tts_provider(provider: str) -> bool:
    """Check if a string is a valid TTS provider."""
    return provider in [p.value for p in TTSProvider]


def is_valid_realtime_provider(provider: str) -> bool:
    """Check if a string is a valid realtime provider."""
    return provider in [p.value for p in RealtimeProvider]


def get_provider_capabilities(
    provider: str,
    provider_type: Literal["stt", "tts", "realtime"],
) -> dict[str, Any] | None:
    """Get capabilities for a provider."""
    if provider_type == "stt":
        try:
            return STT_PROVIDER_CAPABILITIES.get(STTProvider(provider))
        except ValueError:
            return None
    elif provider_type == "tts":
        try:
            return TTS_PROVIDER_CAPABILITIES.get(TTSProvider(provider))
        except ValueError:
            return None
    elif provider_type == "realtime":
        try:
            return REALTIME_PROVIDER_CAPABILITIES.get(RealtimeProvider(provider))
        except ValueError:
            return None
    return None


# =============================================================================
# Emotion Types (Unified Emotion System)
# =============================================================================


class Emotion(str, Enum):
    """
    Standardized emotions supported across TTS providers.
    Each emotion maps to provider-specific formats (SSML, audio tags, natural language, etc.)
    """

    NEUTRAL = "neutral"
    HAPPY = "happy"
    SAD = "sad"
    ANGRY = "angry"
    FEARFUL = "fearful"
    SURPRISED = "surprised"
    DISGUSTED = "disgusted"
    EXCITED = "excited"
    CALM = "calm"
    ANXIOUS = "anxious"
    CONFIDENT = "confident"
    CONFUSED = "confused"
    EMPATHETIC = "empathetic"
    SARCASTIC = "sarcastic"
    HOPEFUL = "hopeful"
    DISAPPOINTED = "disappointed"
    CURIOUS = "curious"
    GRATEFUL = "grateful"
    PROUD = "proud"
    EMBARRASSED = "embarrassed"
    CONTENT = "content"
    BORED = "bored"


class DeliveryStyle(str, Enum):
    """
    Delivery styles that modify how speech is expressed.
    These can be combined with emotions for nuanced expression.
    """

    NORMAL = "normal"
    WHISPERED = "whispered"
    SHOUTED = "shouted"
    RUSHED = "rushed"
    MEASURED = "measured"
    MONOTONE = "monotone"
    EXPRESSIVE = "expressive"
    PROFESSIONAL = "professional"
    CASUAL = "casual"
    STORYTELLING = "storytelling"
    SOFT = "soft"
    LOUD = "loud"
    CHEERFUL = "cheerful"
    SERIOUS = "serious"
    FORMAL = "formal"


class EmotionIntensityLevel(str, Enum):
    """
    Emotion intensity presets.
    - low: Subtle emotion (0.3 intensity)
    - medium: Moderate emotion (0.6 intensity)
    - high: Strong emotion (1.0 intensity)
    """

    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"


def intensity_to_number(intensity: Union[float, EmotionIntensityLevel]) -> float:
    """Convert intensity level to numeric value (0.0 to 1.0)."""
    if isinstance(intensity, (int, float)):
        return max(0.0, min(1.0, float(intensity)))
    mapping = {
        EmotionIntensityLevel.LOW: 0.3,
        EmotionIntensityLevel.MEDIUM: 0.6,
        EmotionIntensityLevel.HIGH: 1.0,
    }
    return mapping.get(intensity, 0.6)


class EmotionConfig(BaseModel):
    """Emotion configuration for TTS."""

    emotion: Optional[Emotion] = None
    """Primary emotion to express"""

    intensity: Optional[Union[float, EmotionIntensityLevel]] = None
    """Emotion intensity (0.0 to 1.0 or preset level)"""

    style: Optional[DeliveryStyle] = None
    """Delivery style"""

    description: Optional[str] = None
    """Free-form description (for providers like Hume)"""


class STTConfig(BaseModel):
    """STT (Speech-to-Text) configuration."""

    provider: str = "deepgram"
    """Provider name (e.g., 'deepgram', 'google', 'elevenlabs', 'microsoft-azure', 'cartesia', 'openai')"""

    language: str = "en-US"
    """Language code for transcription"""

    model: Optional[str] = None
    """Model to use (e.g., 'nova-3' for Deepgram)"""

    sample_rate: int = 16000
    """Sample rate of input audio in Hz"""

    encoding: str = "linear16"
    """Audio encoding format"""

    channels: int = 1
    """Number of audio channels"""

    interim_results: bool = True
    """Enable interim/partial results"""

    punctuate: bool = True
    """Enable punctuation"""

    profanity_filter: bool = False
    """Enable profanity filter"""

    smart_format: bool = True
    """Enable smart formatting"""

    diarize: bool = False
    """Enable speaker diarization"""

    keywords: Optional[list[str]] = None
    """Keywords to boost recognition"""

    custom_vocabulary: Optional[list[str]] = None
    """Custom vocabulary words"""


class TTSConfig(BaseModel):
    """TTS (Text-to-Speech) configuration."""

    provider: str = "deepgram"
    """Provider name (e.g., 'deepgram', 'elevenlabs', 'google', 'microsoft-azure', 'cartesia', 'openai')"""

    voice: Optional[str] = None
    """Voice name"""

    voice_id: Optional[str] = None
    """Voice ID (provider-specific)"""

    model: Optional[str] = None
    """Model to use (e.g., 'eleven_turbo_v2')"""

    sample_rate: int = 24000
    """Output sample rate in Hz"""

    audio_format: str = "linear16"
    """Output audio format"""

    speed: Optional[float] = None
    """Speech rate multiplier"""

    pitch: Optional[float] = None
    """Pitch adjustment"""

    volume: Optional[float] = None
    """Volume adjustment"""

    stability: Optional[float] = None
    """Voice stability (ElevenLabs specific, 0-1)"""

    similarity_boost: Optional[float] = None
    """Voice similarity boost (ElevenLabs specific, 0-1)"""

    style: Optional[float] = None
    """Voice style (ElevenLabs specific, 0-1)"""

    use_speaker_boost: Optional[bool] = None
    """Use speaker boost (ElevenLabs specific)"""

    # Emotion settings (Unified Emotion System)
    emotion: Optional[Emotion] = None
    """Primary emotion to express"""

    emotion_intensity: Optional[Union[float, EmotionIntensityLevel]] = None
    """Emotion intensity (0.0 to 1.0 or preset level)"""

    delivery_style: Optional[DeliveryStyle] = None
    """Delivery style"""

    emotion_description: Optional[str] = None
    """Free-form emotion description (for Hume and other natural language providers)"""

    # Hume-specific settings
    acting_instructions: Optional[str] = None
    """Acting instructions for Hume Octave (max 100 chars, e.g., 'whispered fearfully')"""

    voice_description: Optional[str] = None
    """Voice description for Hume voice design"""

    trailing_silence: Optional[float] = None
    """Trailing silence in seconds (Hume)"""

    instant_mode: Optional[bool] = None
    """Enable instant mode for lower latency (Hume)"""


class LiveKitConfig(BaseModel):
    """LiveKit configuration for room-based communication."""

    room_name: str
    """Room name to join or create"""

    identity: Optional[str] = None
    """Participant identity"""

    name: Optional[str] = None
    """Participant display name"""

    metadata: Optional[str] = None
    """Participant metadata"""


class FeatureFlags(BaseModel):
    """Feature flags for audio processing."""

    vad: bool = True
    """Voice Activity Detection"""

    noise_cancellation: bool = False
    """Noise suppression (DeepFilterNet)"""

    speaker_diarization: bool = False
    """Multi-speaker identification"""

    interim_results: bool = True
    """Partial STT results"""

    punctuation: bool = True
    """Auto-punctuation"""

    profanity_filter: bool = False
    """Filter profane words"""

    smart_format: bool = True
    """Smart text formatting"""

    word_timestamps: bool = False
    """Per-word timing"""

    echo_cancellation: bool = True
    """Browser echo cancellation"""

    filler_words: bool = False
    """Include um, uh, etc."""


class WordInfo(BaseModel):
    """Word-level transcription info."""

    word: str
    """The word"""

    start: float
    """Start time in seconds"""

    end: float
    """End time in seconds"""

    confidence: Optional[float] = None
    """Confidence score (0-1)"""

    speaker_id: Optional[int] = None
    """Speaker ID for diarization"""


class STTResult(BaseModel):
    """Speech-to-Text result."""

    text: str
    """Transcribed text"""

    is_final: bool
    """Whether this is a final result"""

    confidence: Optional[float] = None
    """Confidence score (0-1)"""

    speaker_id: Optional[int] = None
    """Speaker ID for diarization"""

    language: Optional[str] = None
    """Detected language"""

    start_time: Optional[float] = None
    """Start time in seconds"""

    end_time: Optional[float] = None
    """End time in seconds"""

    words: Optional[list[WordInfo]] = None
    """Word-level details"""


class TranscriptEvent(BaseModel):
    """Transcript event from WebSocket session."""

    type: str = "transcript"
    """Event type"""

    text: str
    """Transcribed text"""

    is_final: bool
    """Whether this is a final result"""

    confidence: Optional[float] = None
    """Confidence score (0-1)"""

    speaker_id: Optional[int] = None
    """Speaker ID for diarization"""

    language: Optional[str] = None
    """Detected language"""

    words: Optional[list[WordInfo]] = None
    """Word-level details"""

    role: Optional[Literal["user", "assistant"]] = None
    """Speaker role for realtime conversations"""


class AudioEvent(BaseModel):
    """Audio event from WebSocket session."""

    type: str = "audio"
    """Event type"""

    audio: bytes
    """Audio data (PCM)"""

    format: str = "linear16"
    """Audio format"""

    sample_rate: int = 24000
    """Sample rate in Hz"""

    duration: Optional[float] = None
    """Duration in seconds"""

    is_final: bool = False
    """Whether this is the final chunk"""

    sequence: Optional[int] = None
    """Sequence number for ordering"""


class Voice(BaseModel):
    """TTS Voice information."""

    id: str
    """Voice ID"""

    name: str
    """Voice name"""

    provider: str
    """Provider name"""

    language: Optional[str] = None
    """Supported language"""

    gender: Optional[str] = None
    """Voice gender"""

    description: Optional[str] = None
    """Voice description"""

    preview_url: Optional[str] = None
    """Preview audio URL"""


class PercentileStats(BaseModel):
    """Percentile statistics for metrics."""

    p50: float = 0.0
    """50th percentile (median)"""

    p95: float = 0.0
    """95th percentile"""

    p99: float = 0.0
    """99th percentile"""

    min: float = 0.0
    """Minimum value"""

    max: float = 0.0
    """Maximum value"""

    mean: float = 0.0
    """Mean value"""

    last: float = 0.0
    """Last recorded value"""

    count: int = 0
    """Number of samples"""


class STTMetrics(BaseModel):
    """STT performance metrics."""

    ttft: PercentileStats = Field(default_factory=PercentileStats)
    """Time to First Token"""

    processing_time: PercentileStats = Field(default_factory=PercentileStats)
    """Processing time"""

    transcription_count: int = 0
    """Total transcriptions"""

    total_audio_duration: float = 0.0
    """Total audio processed (seconds)"""

    total_characters: int = 0
    """Total characters transcribed"""


class TTSMetrics(BaseModel):
    """TTS performance metrics."""

    ttfb: PercentileStats = Field(default_factory=PercentileStats)
    """Time to First Byte"""

    synthesis_time: PercentileStats = Field(default_factory=PercentileStats)
    """Synthesis time"""

    speak_count: int = 0
    """Total speak calls"""

    total_characters: int = 0
    """Total characters synthesized"""

    throughput: PercentileStats = Field(default_factory=PercentileStats)
    """Throughput (chars/sec)"""


class MetricsSummary(BaseModel):
    """Complete metrics summary."""

    stt: STTMetrics = Field(default_factory=STTMetrics)
    """STT metrics"""

    tts: TTSMetrics = Field(default_factory=TTSMetrics)
    """TTS metrics"""

    timestamp: int = 0
    """Collection timestamp"""

    collection_duration_ms: int = 0
    """Collection duration in milliseconds"""


class LiveKitTokenRequest(BaseModel):
    """Request for LiveKit token generation."""

    room_name: str
    """Room name"""

    identity: str
    """Participant identity"""

    name: Optional[str] = None
    """Participant display name"""

    ttl: Optional[int] = None
    """Token TTL in seconds"""

    metadata: Optional[str] = None
    """Participant metadata"""


class LiveKitTokenResponse(BaseModel):
    """Response from LiveKit token generation."""

    token: str
    """JWT token"""

    room_name: str
    """Room name"""

    identity: str
    """Participant identity"""

    livekit_url: Optional[str] = None
    """LiveKit server URL"""


class RoomInfo(BaseModel):
    """LiveKit room information."""

    name: str
    """Room name"""

    sid: str
    """Room SID"""

    creation_time: int
    """Creation timestamp"""

    num_participants: int = 0
    """Number of participants"""

    active_recording: bool = False
    """Whether recording is active"""


class SIPHook(BaseModel):
    """SIP webhook hook configuration."""

    host: str
    """SIP host"""

    webhook_url: str
    """Webhook URL for incoming calls"""

    created_at: Optional[int] = None
    """Creation timestamp"""


class SIPHookCreateRequest(BaseModel):
    """Request to create SIP hook."""

    host: str
    """SIP host"""

    webhook_url: str
    """Webhook URL for incoming calls"""


class SIPHookCreateResponse(BaseModel):
    """Response from creating SIP hook."""

    host: str
    """SIP host"""

    webhook_url: str
    """Webhook URL"""

    created: bool
    """Whether newly created"""


# =============================================================================
# Realtime (Audio-to-Audio) Types
# =============================================================================


class VADConfig(BaseModel):
    """Voice Activity Detection configuration for realtime sessions."""

    enabled: bool = True
    """Enable server-side VAD"""

    threshold: float = 0.5
    """VAD threshold (0.0 to 1.0)"""

    silence_duration_ms: int = 500
    """Silence duration before speech end detection in ms"""

    prefix_padding_ms: int = 300
    """Prefix padding in ms"""


class InputTranscriptionConfig(BaseModel):
    """Input audio transcription configuration for realtime sessions."""

    enabled: bool = True
    """Enable input audio transcription"""

    model: str = "whisper-1"
    """Model to use for transcription"""


class RealtimeSessionConfig(BaseModel):
    """
    Provider-agnostic realtime session configuration.

    This configuration abstracts away provider-specific details while exposing
    common functionality. Advanced users can access provider-specific options
    through the `provider_options` field.
    """

    provider: str = "openai"
    """Provider to use (currently only 'openai' supported)"""

    model: Optional[str] = "gpt-4o-realtime-preview"
    """
    Model to use (provider-specific).

    OpenAI: "gpt-4o-realtime-preview", "gpt-4o-mini-realtime-preview"
    """

    voice: Optional[str] = "alloy"
    """
    Voice to use for audio output.

    OpenAI: "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"
    """

    instructions: Optional[str] = None
    """System instructions for the AI assistant"""

    vad: Optional[VADConfig] = Field(default_factory=VADConfig)
    """Voice Activity Detection configuration"""

    input_transcription: Optional[InputTranscriptionConfig] = Field(
        default_factory=InputTranscriptionConfig
    )
    """Input audio transcription configuration"""

    turn_detection: str = "server_vad"
    """Turn detection mode: 'server_vad' or 'none'"""

    temperature: float = 0.8
    """Temperature for response generation (0.0 to 2.0)"""

    max_response_tokens: Optional[int] = None
    """Maximum tokens for response (provider-specific limits apply)"""

    provider_options: Optional[dict[str, Any]] = None
    """
    Provider-specific options for advanced users.

    These options are passed directly to the provider and may vary
    between providers. Refer to provider documentation for details.
    """


class RealtimeTranscript(BaseModel):
    """Realtime transcript result."""

    text: str
    """The transcribed or generated text"""

    role: str
    """Role: 'user' for input transcription, 'assistant' for AI response"""

    is_final: bool
    """Whether this is a final transcript (vs interim/streaming)"""

    item_id: Optional[str] = None
    """Item ID from the provider (for correlation)"""

    response_id: Optional[str] = None
    """Response ID from the provider (for correlation)"""

    timestamp: int
    """Timestamp when transcript was received (ms since epoch)"""


class RealtimeSpeechEvent(BaseModel):
    """Speech event (speech started/stopped) for realtime sessions."""

    type: str
    """Event type: 'speech_started' or 'speech_stopped'"""

    audio_ms: int
    """Audio position in ms when event occurred"""

    item_id: Optional[str] = None
    """Item ID from the provider"""

    timestamp: int
    """Timestamp when event was received (ms since epoch)"""


class RealtimeAudioChunk(BaseModel):
    """Realtime audio data chunk."""

    data: bytes
    """Raw PCM audio data (24kHz, mono, 16-bit little-endian)"""

    sample_rate: int = 24000
    """Sample rate (always 24000 for OpenAI)"""

    channels: int = 1
    """Number of channels (always 1 for mono)"""

    is_final: bool = False
    """Whether this is the final chunk for this response"""

    response_id: Optional[str] = None
    """Response ID from the provider"""

    item_id: Optional[str] = None
    """Item ID from the provider"""

    sequence: int = 0
    """Sequence number for ordering"""

    timestamp: int = 0
    """Timestamp when chunk was received (ms since epoch)"""


# Provider-specific voice defaults
VOICE_DEFAULTS: dict[str, dict[str, Optional[str]]] = {
    "deepgram": {"model": "aura-asteria-en", "voice": "aura-asteria-en"},
    "elevenlabs": {"model": "eleven_turbo_v2", "voice": "rachel"},
    "google": {"model": "en-US-Studio-O", "voice": "en-US-Studio-O"},
    "azure": {"model": "en-US-JennyNeural", "voice": "en-US-JennyNeural"},
    "cartesia": {"model": "sonic-3", "voice": None},
    "openai": {"model": "tts-1", "voice": "alloy"},
}

# Realtime provider defaults
REALTIME_DEFAULTS: dict[str, dict[str, Any]] = {
    "openai": {
        "model": "gpt-4o-realtime-preview",
        "voice": "alloy",
        "turn_detection": "server_vad",
        "temperature": 0.8,
        "max_response_tokens": None,
    },
    "hume": {
        "evi_version": "3",
        "voice_id": None,
        "verbose_transcription": False,
    },
}


# =============================================================================
# Voice Cloning Types
# =============================================================================


class VoiceCloneProvider(str, Enum):
    """Provider for voice cloning operations."""

    HUME = "hume"
    ELEVENLABS = "elevenlabs"


class VoiceCloneRequest(BaseModel):
    """Request to clone a voice from audio samples or description."""

    provider: VoiceCloneProvider
    """Provider to use for voice cloning"""

    name: str
    """Name for the cloned voice"""

    description: Optional[str] = None
    """Description of the voice (used by Hume for voice design)"""

    audio_samples: Optional[list[str]] = None
    """Audio samples for cloning (base64-encoded). ElevenLabs: 1-2 min recommended"""

    sample_text: Optional[str] = None
    """Sample text for voice generation (Hume only)"""

    remove_background_noise: bool = False
    """Remove background noise from samples (ElevenLabs only)"""

    labels: Optional[dict[str, str]] = None
    """Labels for the voice (ElevenLabs only)"""


class VoiceCloneStatus(str, Enum):
    """Status of a cloned voice."""

    READY = "ready"
    PROCESSING = "processing"
    FAILED = "failed"


class VoiceCloneResponse(BaseModel):
    """Response from voice cloning operation."""

    voice_id: str
    """Unique identifier for the cloned voice"""

    name: str
    """Name of the cloned voice"""

    provider: VoiceCloneProvider
    """Provider that created the voice"""

    status: VoiceCloneStatus
    """Status of the voice (ready, processing, failed)"""

    created_at: str
    """ISO 8601 timestamp when the voice was created"""

    metadata: Optional[dict[str, Any]] = None
    """Additional metadata from the provider"""


# =============================================================================
# Hume EVI (Empathic Voice Interface) Types
# =============================================================================


class HumeEVIVersion(str, Enum):
    """Hume EVI version."""

    V1 = "1"
    V2 = "2"
    V3 = "3"
    V4_MINI = "4-mini"


class HumeEVIConfig(BaseModel):
    """Hume EVI configuration for audio-to-audio realtime streaming."""

    config_id: Optional[str] = None
    """EVI configuration ID from Hume dashboard"""

    resumed_chat_group_id: Optional[str] = None
    """Chat group ID for resuming a previous conversation"""

    evi_version: HumeEVIVersion = HumeEVIVersion.V3
    """EVI version to use (default: V3)"""

    voice_id: Optional[str] = None
    """Voice ID to use"""

    verbose_transcription: bool = False
    """Enable verbose transcription"""

    system_prompt: Optional[str] = None
    """System prompt override"""


class ProsodyScores(BaseModel):
    """
    Prosody (emotion) scores from Hume EVI.
    Provides 48 emotion dimensions detected in speech.
    """

    admiration: float = 0.0
    adoration: float = 0.0
    aesthetic_appreciation: float = 0.0
    amusement: float = 0.0
    anger: float = 0.0
    anxiety: float = 0.0
    awe: float = 0.0
    awkwardness: float = 0.0
    boredom: float = 0.0
    calmness: float = 0.0
    concentration: float = 0.0
    confusion: float = 0.0
    contemplation: float = 0.0
    contempt: float = 0.0
    contentment: float = 0.0
    craving: float = 0.0
    desire: float = 0.0
    determination: float = 0.0
    disappointment: float = 0.0
    disgust: float = 0.0
    distress: float = 0.0
    doubt: float = 0.0
    ecstasy: float = 0.0
    embarrassment: float = 0.0
    empathic_pain: float = 0.0
    enthusiasm: float = 0.0
    entrancement: float = 0.0
    envy: float = 0.0
    excitement: float = 0.0
    fear: float = 0.0
    guilt: float = 0.0
    horror: float = 0.0
    interest: float = 0.0
    joy: float = 0.0
    love: float = 0.0
    nostalgia: float = 0.0
    pain: float = 0.0
    pride: float = 0.0
    realization: float = 0.0
    relief: float = 0.0
    romance: float = 0.0
    sadness: float = 0.0
    satisfaction: float = 0.0
    shame: float = 0.0
    surprise_negative: float = 0.0
    surprise_positive: float = 0.0
    sympathy: float = 0.0
    tiredness: float = 0.0
    triumph: float = 0.0

    def top_emotions(self, n: int = 3) -> list[tuple[str, float]]:
        """Get the top N emotions by score."""
        scores = [
            (name, getattr(self, name))
            for name in self.model_fields
            if isinstance(getattr(self, name), float)
        ]
        scores.sort(key=lambda x: x[1], reverse=True)
        return scores[:n]

    def dominant_emotion(self) -> tuple[str, float] | None:
        """Get the dominant (highest scoring) emotion."""
        top = self.top_emotions(1)
        return top[0] if top else None


# =============================================================================
# DAG Routing Types
# =============================================================================


class DAGNodeType(str, Enum):
    """Node types supported in DAG definitions."""

    AUDIO_INPUT = "audio_input"
    AUDIO_OUTPUT = "audio_output"
    TEXT_INPUT = "text_input"
    TEXT_OUTPUT = "text_output"
    STT_PROVIDER = "stt_provider"
    TTS_PROVIDER = "tts_provider"
    LLM = "llm"
    HTTP_ENDPOINT = "http_endpoint"
    WEBHOOK = "webhook"
    TRANSFORM = "transform"
    ROUTER = "router"
    BUFFER = "buffer"
    SWITCH = "switch"


class DAGNode(BaseModel):
    """A node in the DAG pipeline."""

    id: str
    """Unique identifier for this node"""

    type: DAGNodeType
    """Type of the node"""

    config: Optional[dict[str, Any]] = None
    """Node-specific configuration"""


class DAGEdge(BaseModel):
    """An edge connecting two nodes in the DAG."""

    model_config = ConfigDict(populate_by_name=True)

    from_node: str = Field(alias="from")
    """Source node ID"""

    to_node: str = Field(alias="to")
    """Destination node ID"""

    condition: Optional[str] = None
    """Optional condition expression (Rhai script)"""


class DAGDefinition(BaseModel):
    """Complete DAG definition."""

    id: str
    """Unique identifier for this DAG"""

    name: str
    """Human-readable name"""

    version: str
    """Version string"""

    description: Optional[str] = None
    """Description of the DAG"""

    nodes: list[DAGNode]
    """Nodes in the DAG"""

    edges: list[DAGEdge]
    """Edges connecting nodes"""

    metadata: Optional[dict[str, Any]] = None
    """Optional metadata"""


class DAGConfig(BaseModel):
    """DAG configuration for WebSocket sessions."""

    template: Optional[str] = None
    """Name of a pre-registered template to use"""

    definition: Optional[DAGDefinition] = None
    """Inline DAG definition (takes precedence over template)"""

    enable_metrics: bool = False
    """Enable metrics collection for DAG execution"""

    timeout_ms: int = 30000
    """Maximum execution time in milliseconds"""


class DAGValidationResult(BaseModel):
    """Validation result for DAG definitions."""

    valid: bool
    """Whether the DAG is valid"""

    errors: list[str]
    """List of validation errors"""

    warnings: list[str]
    """List of validation warnings"""


def validate_dag_definition(dag: DAGDefinition) -> DAGValidationResult:
    """
    Validate a DAG definition.

    Checks for:
    - Required fields
    - Unique node IDs
    - Valid edge references
    - No cycles (DAG must be acyclic)
    """
    errors: list[str] = []
    warnings: list[str] = []

    # Check required fields
    if not dag.id:
        errors.append("DAG id is required")
    if not dag.name:
        errors.append("DAG name is required")
    if not dag.version:
        errors.append("DAG version is required")

    # Check for duplicate node IDs
    node_ids: set[str] = set()
    for node in dag.nodes:
        if not node.id:
            errors.append("Node id is required")
            continue
        if node.id in node_ids:
            errors.append(f"Duplicate node id: {node.id}")
        node_ids.add(node.id)

    # Check edge references
    for edge in dag.edges:
        if edge.from_node not in node_ids:
            errors.append(f"Edge references nonexistent source node: {edge.from_node}")
        if edge.to_node not in node_ids:
            errors.append(f"Edge references nonexistent target node: {edge.to_node}")

    # Check for cycles using DFS
    if not errors:
        cycle_result = _detect_cycles(dag)
        if cycle_result:
            errors.append(f"DAG contains a cycle: {' -> '.join(cycle_result)}")

    # Warnings
    if len(dag.nodes) == 0:
        warnings.append("DAG has no nodes")
    if len(dag.edges) == 0 and len(dag.nodes) > 1:
        warnings.append("DAG has multiple nodes but no edges")

    # Check for disconnected nodes
    connected_nodes: set[str] = set()
    for edge in dag.edges:
        connected_nodes.add(edge.from_node)
        connected_nodes.add(edge.to_node)
    for node in dag.nodes:
        if len(dag.nodes) > 1 and node.id not in connected_nodes:
            warnings.append(f"Node {node.id} is not connected to any other node")

    return DAGValidationResult(valid=len(errors) == 0, errors=errors, warnings=warnings)


def _detect_cycles(dag: DAGDefinition) -> list[str] | None:
    """Detect cycles in the DAG using DFS."""
    adjacency: dict[str, list[str]] = {node.id: [] for node in dag.nodes}
    for edge in dag.edges:
        adjacency[edge.from_node].append(edge.to_node)

    visited: set[str] = set()
    recursion_stack: set[str] = set()
    path: list[str] = []

    def dfs(node_id: str) -> bool:
        visited.add(node_id)
        recursion_stack.add(node_id)
        path.append(node_id)

        for neighbor in adjacency.get(node_id, []):
            if neighbor not in visited:
                if dfs(neighbor):
                    return True
            elif neighbor in recursion_stack:
                path.append(neighbor)
                return True

        path.pop()
        recursion_stack.remove(node_id)
        return False

    for node in dag.nodes:
        if node.id not in visited:
            if dfs(node.id):
                cycle_start = path.index(path[-1])
                return path[cycle_start:]

    return None


# Pre-built DAG templates
TEMPLATE_SIMPLE_STT = DAGDefinition(
    id="simple-stt",
    name="Simple STT Pipeline",
    version="1.0",
    description="Convert audio to text using speech-to-text",
    nodes=[
        DAGNode(id="input", type=DAGNodeType.AUDIO_INPUT),
        DAGNode(id="stt", type=DAGNodeType.STT_PROVIDER, config={"provider": "deepgram"}),
        DAGNode(id="output", type=DAGNodeType.TEXT_OUTPUT),
    ],
    edges=[
        DAGEdge(from_node="input", to_node="stt"),
        DAGEdge(from_node="stt", to_node="output"),
    ],
)

TEMPLATE_SIMPLE_TTS = DAGDefinition(
    id="simple-tts",
    name="Simple TTS Pipeline",
    version="1.0",
    description="Convert text to speech using text-to-speech",
    nodes=[
        DAGNode(id="input", type=DAGNodeType.TEXT_INPUT),
        DAGNode(id="tts", type=DAGNodeType.TTS_PROVIDER, config={"provider": "elevenlabs"}),
        DAGNode(id="output", type=DAGNodeType.AUDIO_OUTPUT),
    ],
    edges=[
        DAGEdge(from_node="input", to_node="tts"),
        DAGEdge(from_node="tts", to_node="output"),
    ],
)

TEMPLATE_VOICE_ASSISTANT = DAGDefinition(
    id="voice-assistant",
    name="Voice Assistant Pipeline",
    version="1.0",
    description="Full voice assistant with STT, LLM, and TTS",
    nodes=[
        DAGNode(id="audio_in", type=DAGNodeType.AUDIO_INPUT),
        DAGNode(id="stt", type=DAGNodeType.STT_PROVIDER, config={"provider": "deepgram"}),
        DAGNode(id="llm", type=DAGNodeType.LLM, config={"provider": "openai", "model": "gpt-4"}),
        DAGNode(id="tts", type=DAGNodeType.TTS_PROVIDER, config={"provider": "elevenlabs"}),
        DAGNode(id="audio_out", type=DAGNodeType.AUDIO_OUTPUT),
    ],
    edges=[
        DAGEdge(from_node="audio_in", to_node="stt"),
        DAGEdge(from_node="stt", to_node="llm"),
        DAGEdge(from_node="llm", to_node="tts"),
        DAGEdge(from_node="tts", to_node="audio_out"),
    ],
)

BUILTIN_TEMPLATES: dict[str, DAGDefinition] = {
    "simple-stt": TEMPLATE_SIMPLE_STT,
    "simple-tts": TEMPLATE_SIMPLE_TTS,
    "voice-assistant": TEMPLATE_VOICE_ASSISTANT,
}


def get_builtin_template(name: str) -> DAGDefinition | None:
    """Get a built-in template by name."""
    return BUILTIN_TEMPLATES.get(name)


# =============================================================================
# Audio Features Types
# =============================================================================


class TurnDetectionConfig(BaseModel):
    """Turn detection configuration."""

    enabled: bool = False
    """Enable turn detection"""

    threshold: float = 0.5
    """Detection threshold (0.0-1.0)"""

    silence_ms: int = 500
    """Silence duration in ms to trigger turn end"""

    prefix_padding_ms: int = 200
    """Padding before speech in ms"""

    create_response_ms: int = 300
    """Delay before creating response in ms"""


class NoiseFilterConfig(BaseModel):
    """Noise filtering configuration."""

    enabled: bool = False
    """Enable noise filtering"""

    strength: Literal["low", "medium", "high"] = "medium"
    """Noise reduction strength"""

    strength_value: Optional[float] = None
    """Numeric strength value (0.0-1.0), overrides strength if provided"""


class VADModeType(str, Enum):
    """VAD mode types."""

    NORMAL = "normal"
    AGGRESSIVE = "aggressive"
    VERY_AGGRESSIVE = "very_aggressive"


class ExtendedVADConfig(BaseModel):
    """Extended VAD configuration."""

    enabled: bool = True
    """Enable VAD"""

    threshold: float = 0.5
    """Detection threshold (0.0-1.0)"""

    mode: VADModeType = VADModeType.NORMAL
    """VAD mode for different environments"""


class AudioFeatures(BaseModel):
    """Combined audio features configuration."""

    turn_detection: Optional[TurnDetectionConfig] = None
    """Turn detection settings"""

    noise_filtering: Optional[NoiseFilterConfig] = None
    """Noise filtering settings"""

    vad: Optional[ExtendedVADConfig] = None
    """Voice activity detection settings"""


# Default configurations
DEFAULT_TURN_DETECTION = TurnDetectionConfig()
DEFAULT_NOISE_FILTER = NoiseFilterConfig()
DEFAULT_VAD = ExtendedVADConfig()


def create_audio_features(
    turn_detection: Optional[dict[str, Any]] = None,
    noise_filtering: Optional[dict[str, Any]] = None,
    vad: Optional[dict[str, Any]] = None,
) -> AudioFeatures:
    """Create audio features configuration with defaults."""
    features = AudioFeatures()

    if turn_detection:
        features.turn_detection = TurnDetectionConfig(**turn_detection)
    else:
        features.turn_detection = DEFAULT_TURN_DETECTION.model_copy()

    if noise_filtering:
        features.noise_filtering = NoiseFilterConfig(**noise_filtering)
    else:
        features.noise_filtering = DEFAULT_NOISE_FILTER.model_copy()

    if vad:
        features.vad = ExtendedVADConfig(**vad)
    else:
        features.vad = DEFAULT_VAD.model_copy()

    return features


# =============================================================================
# Recording Types
# =============================================================================


class RecordingStatus(str, Enum):
    """Recording status."""

    RECORDING = "recording"
    COMPLETED = "completed"
    FAILED = "failed"
    PROCESSING = "processing"


class RecordingFormat(str, Enum):
    """Audio format for recordings."""

    WAV = "wav"
    MP3 = "mp3"
    OGG = "ogg"
    FLAC = "flac"
    WEBM = "webm"


class RecordingInfo(BaseModel):
    """Information about a recording."""

    stream_id: str
    """Stream ID associated with the recording"""

    room_name: Optional[str] = None
    """Room name (for LiveKit recordings)"""

    duration: float
    """Duration in seconds"""

    size: int
    """Size in bytes"""

    format: RecordingFormat
    """Audio format"""

    created_at: str
    """Creation timestamp (ISO 8601)"""

    status: RecordingStatus
    """Current status"""

    sample_rate: Optional[int] = None
    """Sample rate in Hz"""

    channels: Optional[int] = None
    """Number of channels"""

    bit_depth: Optional[int] = None
    """Bit depth"""

    metadata: Optional[dict[str, Any]] = None
    """Optional metadata"""


class RecordingFilter(BaseModel):
    """Filter for listing recordings."""

    room_name: Optional[str] = None
    """Filter by room name"""

    stream_id: Optional[str] = None
    """Filter by stream ID"""

    status: Optional[RecordingStatus] = None
    """Filter by status"""

    start_date: Optional[str] = None
    """Start date (ISO 8601)"""

    end_date: Optional[str] = None
    """End date (ISO 8601)"""

    format: Optional[RecordingFormat] = None
    """Filter by format"""

    limit: Optional[int] = None
    """Maximum number of results"""

    offset: Optional[int] = None
    """Offset for pagination"""


class RecordingList(BaseModel):
    """Paginated list of recordings."""

    recordings: list[RecordingInfo]
    """Recordings in this page"""

    total: int
    """Total count of recordings matching filter"""

    has_more: bool = False
    """Whether there are more results"""
