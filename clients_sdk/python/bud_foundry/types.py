"""
Type definitions for bud-foundry SDK
"""

from typing import Any, Optional
from pydantic import BaseModel, Field


class STTConfig(BaseModel):
    """STT (Speech-to-Text) configuration."""

    provider: str = "deepgram"
    """Provider name (e.g., 'deepgram', 'whisper', 'azure')"""

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
    """Provider name (e.g., 'deepgram', 'elevenlabs', 'azure')"""

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
