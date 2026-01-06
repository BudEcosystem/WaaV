"""
Type definitions for bud-foundry SDK
"""

from enum import Enum
from typing import Any, Literal, Optional, Union
from pydantic import BaseModel, Field


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
