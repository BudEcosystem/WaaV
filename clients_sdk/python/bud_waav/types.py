"""
Type definitions for bud-waav SDK
"""

from enum import Enum
from typing import Any, Callable, Literal, Optional, Union
from pydantic import BaseModel, ConfigDict, Field


# =============================================================================
# Provider Types (Comprehensive List)
# =============================================================================


class STTProvider(str, Enum):
    """Speech-to-Text providers dispatchable by the gateway.

    The FULL set (32) sourced 1:1 from the gateway's STT dispatch table
    (``gateway/src/core/stt/standard.rs`` ``create_stt_standard`` +
    ``plugin/dispatch.rs`` builtin registry). Accept a bare ``str`` anywhere a
    provider is taken so a newly-added gateway provider is reachable before the
    SDK enum is regenerated (forward-compat). Drift-guarded by
    ``tests/unit/test_provider_enums.py`` against ``PROVIDER_DRIFT.md``.
    """

    ALIBABA_CLOUD = "alibaba-cloud"
    AMIVOICE = "amivoice"
    ASSEMBLYAI = "assemblyai"
    AWS_TRANSCRIBE = "aws-transcribe"
    BAIDU = "baidu"
    BHASHINI = "bhashini"
    CARTESIA = "cartesia"
    DEEPGRAM = "deepgram"
    ELEVENLABS = "elevenlabs"
    FPT_AI = "fpt-ai"
    GLADIA = "gladia"
    GNANI = "gnani"
    GOOGLE = "google"
    GROQ = "groq"
    HUAWEI_CLOUD = "huawei-cloud"
    IBM_WATSON = "ibm-watson"
    IFLYTEK = "iflytek"
    MICROSOFT_AZURE = "microsoft-azure"
    NAVER_CLOVA = "naver-clova"
    NECTEC = "nectec"
    OPENAI = "openai"
    PHONEXIA = "phonexia"
    PROSA_AI = "prosa-ai"
    REVAI = "revai"
    REVERIE = "reverie"
    SARVAM = "sarvam"
    SBERDEVICES = "sberdevices"
    SPEECHMATICS = "speechmatics"
    TENCENT = "tencent"
    TINKOFF = "tinkoff"
    VIETTEL_AI = "viettel-ai"
    YANDEX = "yandex"


class TTSProvider(str, Enum):
    """Text-to-Speech providers dispatchable by the gateway.

    The FULL set (37) sourced 1:1 from the gateway's TTS dispatch table
    (``gateway/src/core/tts/standard.rs`` ``create_tts_standard`` +
    ``plugin/dispatch.rs`` builtin registry). Accept a bare ``str`` for
    forward-compat. Drift-guarded against ``PROVIDER_DRIFT.md``.
    """

    ACAPELA = "acapela"
    ALIBABA_CLOUD = "alibaba-cloud"
    AWS_POLLY = "aws-polly"
    BAIDU = "baidu"
    BHASHINI = "bhashini"
    CARTESIA = "cartesia"
    CEREPROC = "cereproc"
    DEEPGRAM = "deepgram"
    ELEVENLABS = "elevenlabs"
    FPT_AI = "fpt-ai"
    GNANI = "gnani"
    GOOGLE = "google"
    HUAWEI_CLOUD = "huawei-cloud"
    HUME = "hume"
    IBM_WATSON = "ibm-watson"
    IFLYTEK = "iflytek"
    LMNT = "lmnt"
    MICROSOFT_AZURE = "microsoft-azure"
    MURF = "murf"
    NAVER_CLOVA = "naver-clova"
    NECTEC = "nectec"
    OPENAI = "openai"
    PLAYHT = "playht"
    PROSA_AI = "prosa-ai"
    RESEMBLE = "resemble"
    REVERIE = "reverie"
    SBERDEVICES = "sberdevices"
    SMALLEST = "smallest"
    SPEECHIFY = "speechify"
    SPEECHMATICS = "speechmatics"
    TENCENT = "tencent"
    TINKOFF = "tinkoff"
    UNREALSPEECH = "unrealspeech"
    VIETTEL_AI = "viettel-ai"
    WELLSAID = "wellsaid"
    YANDEX = "yandex"
    ZALO_AI = "zalo-ai"


class RealtimeProvider(str, Enum):
    """Realtime (speech-to-speech) providers dispatchable by the gateway.

    The FULL set (12) sourced 1:1 from the gateway realtime registry
    (``gateway/src/core/realtime/mod.rs`` ``get_supported_realtime_providers``
    / ``plugin/dispatch.rs`` ``BUILTIN_REALTIME_NAMES``). Accept a bare ``str``
    for forward-compat. Drift-guarded against ``PROVIDER_DRIFT.md``.

    NOTE: the realtime names are the bare vendor tokens the gateway's
    ``/realtime`` endpoint accepts (``openai``, ``hume``, …) — NOT the old
    ``openai-realtime``/``hume-evi`` aliases the SDK shipped before P1.
    """

    OPENAI = "openai"
    HUME = "hume"
    AZURE = "azure"
    GROK = "grok"
    INWORLD = "inworld"
    DEEPGRAM = "deepgram"
    ELEVENLABS = "elevenlabs"
    GEMINI = "gemini"
    ULTRAVOX = "ultravox"
    NOVA_SONIC = "nova_sonic"
    SPEECHMATICS = "speechmatics"
    YANDEX = "yandex"

    # Backward-compatible aliases for the pre-P1 SDK names. The realtime
    # pipeline (pipelines/realtime.py) historically used these; keep them
    # resolvable so old user code keeps importing, but they map to the
    # gateway-native tokens above (aliases are NOT re-listed by iteration).
    OPENAI_REALTIME = "openai"
    HUME_EVI = "hume"


# =============================================================================
# Canonical Language Value Space (P2 unified-language system)
# =============================================================================
#
# WaaV's gateway maps ONE canonical, region-qualified BCP-47 language token to
# EACH provider's native notation internally (so the same ``language`` string
# works on every provider — switch models without client edits). A developer
# passes one of these canonical tokens (or a bare ISO-639-1 shorthand like
# ``"en"``/``"hi"``, or the reversed/underscore/name spellings the gateway folds:
# ``"us-en"``, ``"en_US"``, ``"english"``) and the gateway resolves + renders it.
#
# This list is the value space, mirrored 1:1 from the gateway's
# ``CanonicalLanguage::all()`` (``gateway/src/core/lang/types.rs``). It is
# REFERENCE/discovery only — ``STTConfig.language``/``TtsFeatures.language`` stay
# free ``str`` on the wire (an unrecognized token is still forwarded verbatim and
# surfaced as a ``config_warning``, never a hard error). For the authoritative,
# always-current per-provider native-notation matrix call the gateway's
# ``GET /capabilities/languages``.
#
# Chinese uses ``cmn``/``yue`` (ISO-639-3) — NOT ``zh`` — because the canonical
# must disambiguate Mandarin from Cantonese (the providers do: Google STT
# ``cmn-Hans-CN`` vs ``yue-Hant-HK``). ``"auto"`` is the detection token.
CANONICAL_LANGUAGES: tuple[str, ...] = (
    "en-US", "en-GB", "en-IN", "en-AU", "en-ZA", "en-NZ",
    "es-ES", "es-MX", "es-US",
    "fr-FR", "fr-CA",
    "pt-BR", "pt-PT",
    "de-DE", "it-IT", "nl-NL", "ru-RU", "tr-TR", "pl-PL", "sv-SE", "nb-NO",
    "da-DK", "fi-FI", "uk-UA", "cs-CZ", "el-GR", "ro-RO", "hu-HU",
    "ja-JP", "ko-KR", "cmn-CN", "cmn-TW", "yue-HK",
    "hi-IN", "bn-IN", "ta-IN", "te-IN", "gu-IN", "kn-IN", "ml-IN", "mr-IN",
    "pa-IN", "or-IN",
    "ar-SA", "vi-VN", "th-TH", "id-ID", "ms-MY", "he-IL",
)
"""The canonical region-qualified BCP-47 language tokens WaaV accepts (P2).

Mirror of the gateway ``CanonicalLanguage::all()``. Pass one of these (or a bare
ISO-639-1 shorthand / common alias) as ``language``; the gateway maps it to each
provider's native notation. ``"auto"`` requests language detection. Discovery
only — the wire field stays a free string (additive/backward-compatible).
"""


# Per-provider language-notation summary, mirroring the gateway language support
# matrix (``gateway/src/core/lang/mod.rs::language_support_matrix``): how the
# provider spells languages natively + whether it auto-detects. This is the
# CANONICAL contract, NOT a stale per-provider language whitelist — passing any
# :data:`CANONICAL_LANGUAGES` token is supported for every provider (unsupported
# pairings degrade with a ``config_warning``, never a hard error). An
# uncatalogued provider defaults to ``("bcp47", False)`` (the gateway's safe
# default). ``notation`` ∈ {bcp47, iso6391, underscore, model-id, composite, none}.
_LANGUAGE_NOTATION: dict[str, tuple[str, bool]] = {
    # IDENTITY-BCP47 (native == canonical BCP-47, region preserved)
    "deepgram": ("bcp47", True),
    "microsoft-azure": ("bcp47", True),
    "azure": ("bcp47", True),
    "aws-transcribe": ("bcp47", True),
    "aws-polly": ("bcp47", False),
    "sarvam": ("bcp47", True),
    "yandex": ("bcp47", True),
    "google": ("bcp47", True),
    "gemini": ("bcp47", True),
    "nova_sonic": ("bcp47", True),
    "tinkoff": ("bcp47", False),
    # DOWNGRADE-TO-ISO639-1 (native == ISO-639-1, region dropped)
    "elevenlabs": ("iso6391", True),
    "openai": ("iso6391", True),
    "cartesia": ("iso6391", False),
    "assemblyai": ("iso6391", True),
    "speechmatics": ("iso6391", True),
    "groq": ("iso6391", True),
    "gladia": ("iso6391", True),
    "fpt-ai": ("iso6391", True),
    "reverie": ("iso6391", False),
    # UNDERSCORE
    "iflytek": ("underscore", False),
    # ENUM / NUMERIC / COMPOSITE (language IS a model id / fused token)
    "baidu": ("model-id", False),
    "tencent": ("composite", False),
    # SPECIAL (no language parameter — inferred from text/voice/description)
    "hume": ("none", True),
}


def language_capabilities(provider: str) -> dict[str, Any]:
    """Canonical language capability summary for ``provider`` (P2).

    The SDK's documented stand-in for the gateway ``GET /capabilities/languages``
    endpoint (not shipped this phase): it tells a developer (a) that the FULL
    canonical value space (:data:`CANONICAL_LANGUAGES`) is accepted for every
    provider — the gateway maps it — and (b) how the provider spells languages
    natively (``notation``) + whether it ``auto_detect``s. It intentionally does
    NOT return a stale per-provider language whitelist (the thing the SDK used to
    hardcode and let drift): passing any canonical token is supported.

    Returns ``{provider, notation, auto_detect, canonical_languages}``. An
    uncatalogued provider gets ``notation="bcp47"`` + ``auto_detect=False`` (the
    gateway's safe default) — it is never blocked.
    """
    notation, auto = _LANGUAGE_NOTATION.get(provider, ("bcp47", False))
    return {
        "provider": provider,
        "notation": notation,
        "auto_detect": auto,
        # The full canonical value space is accepted for every provider; the
        # gateway renders it to the provider's native notation server-side.
        "canonical_languages": list(CANONICAL_LANGUAGES),
    }


# Provider capability hints (best-effort REFERENCE only — NOT authoritative).
#
# This is a small curated subset for the most-used providers; it is intentionally
# NOT exhaustive across all 32/37/12 dispatchable providers. The authoritative,
# always-current capability matrix lives gateway-side (SDK_STANDARDIZATION_PLAN
# P3 `/capabilities`); ``get_provider_capabilities`` returns ``None`` for any
# valid-but-uncatalogued provider rather than blocking it. Never gate a provider
# on the presence of an entry here.
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
    STTProvider.MICROSOFT_AZURE: {
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
    STTProvider.SARVAM: {
        "streaming": True,
        "diarization": False,
        "languages": ["hi-IN", "en-IN", "ta-IN", "te-IN", "kn-IN", "ml-IN", "bn-IN"],
        "models": ["saarika:v2"],
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
    STTProvider.OPENAI: {
        "streaming": False,
        "diarization": False,
        "languages": ["en", "es", "fr", "de", "it", "pt", "ja", "ko", "zh"],
        "models": ["whisper-1", "gpt-4o-transcribe", "gpt-4o-mini-transcribe"],
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
    TTSProvider.MICROSOFT_AZURE: {
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
}


# Realtime capability hints (best-effort REFERENCE subset; see the STT note above).
# Keyed by the gateway-native realtime token (RealtimeProvider.OPENAI == "openai").
REALTIME_PROVIDER_CAPABILITIES: dict[RealtimeProvider, dict[str, Any]] = {
    RealtimeProvider.OPENAI: {
        "function_calling": True,
        "vision": False,
        "emotion_detection": False,
        "models": ["gpt-realtime", "gpt-4o-realtime-preview", "gpt-4o-mini-realtime-preview"],
        "voices": ["alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"],
    },
    RealtimeProvider.HUME: {
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

    Mirrors the gateway ``Emotion`` value space 1:1
    (``gateway/src/core/emotion/types.rs``; widened 22 → 44 in P4). The wire field
    (``tts_config.emotion``) stays a free ``str`` — an unrecognized token is still
    forwarded and resolved server-side (or warned), never a hard error — so a bare
    ``str`` is accepted anywhere this enum is, for forward-compat. Old variants are
    NEVER removed (that would break callers; the gateway resolves them).
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

    # P4 widened affective states (researched across Azure-DragonHD / Cartesia-Sonic3
    # / Hume; each has a direct provider token). AFFECTIVE STATES (what the speaker
    # FEELS), distinct from DeliveryStyle (the delivery MECHANISM).
    AMAZED = "amazed"
    AMUSED = "amused"
    AFFECTIONATE = "affectionate"
    INTRIGUED = "intrigued"
    FLIRTATIOUS = "flirtatious"
    FRUSTRATED = "frustrated"
    ANNOYED = "annoyed"
    DETERMINED = "determined"
    REASSURING = "reassuring"
    SYMPATHETIC = "sympathetic"
    NOSTALGIC = "nostalgic"
    SERENE = "serene"
    TERRIFIED = "terrified"
    ECSTATIC = "ecstatic"
    SKEPTICAL = "skeptical"
    RELIEVED = "relieved"
    PANICKED = "panicked"
    CONCERNED = "concerned"
    APOLOGETIC = "apologetic"
    RESIGNED = "resigned"
    WISTFUL = "wistful"
    CONTEMPLATIVE = "contemplative"


class DeliveryStyle(str, Enum):
    """
    Delivery styles that modify how speech is expressed.
    These can be combined with emotions for nuanced expression.

    Mirrors the gateway ``DeliveryStyle`` value space
    (``gateway/src/core/emotion/types.rs``). The P4 gateway PROMOTED the affective
    words ``soft``/``cheerful``/``serious`` to :class:`Emotion`, but still accepts
    them here as backward-compatible cross-enum aliases (they resolve to a neutral
    delivery mechanism while the feeling is captured by ``emotion``), so those old
    variants are KEPT below. The wire field stays a free ``str`` (forward-compat).
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

    # P4 scenario / narration framings (map 1:1 to Azure express-as scenario styles
    # and AWS Polly amazon:domain names; warn+default elsewhere).
    NEWSCAST = "newscast"
    NEWSCAST_CASUAL = "newscast_casual"
    NEWSCAST_FORMAL = "newscast_formal"
    CUSTOMER_SERVICE = "customer_service"
    ASSISTANT = "assistant"
    CHAT = "chat"
    ADVERTISEMENT_UPBEAT = "advertisement_upbeat"
    SPORTS_COMMENTARY = "sports_commentary"
    SPORTS_COMMENTARY_EXCITED = "sports_commentary_excited"
    DOCUMENTARY_NARRATION = "documentary_narration"
    NARRATION_PROFESSIONAL = "narration_professional"
    NARRATION_RELAXED = "narration_relaxed"
    POETRY_READING = "poetry_reading"
    LYRICAL = "lyrical"
    GENTLE = "gentle"


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


# =============================================================================
# Voice Descriptor (P4 — abstract voice selection; resolved server-side)
# =============================================================================


class VoiceGender(str, Enum):
    """Desired voice gender for :class:`VoiceDescriptor` (gateway ``Gender``)."""

    MALE = "male"
    FEMALE = "female"
    NEUTRAL = "neutral"


class VoiceAge(str, Enum):
    """Desired voice age band for :class:`VoiceDescriptor` (gateway ``Age``)."""

    YOUNG = "young"
    MIDDLE_AGED = "middle_aged"
    OLD = "old"


class VoiceDescriptor(BaseModel):
    """Abstract, provider-agnostic voice selection (P4).

    Mirrors the gateway ``VoiceDescriptor`` (``gateway/src/core/voice/types.rs``).
    Describe the voice you WANT (gender / locale / accent / age / style / name hint)
    and the gateway resolves it to a concrete provider ``voice_id`` server-side —
    so the same descriptor works across every TTS provider without hardcoding IDs.

    Used ONLY when no explicit ``voice_id`` (or ``voice``) is supplied on the TTS
    config — a raw ``voice_id`` ALWAYS wins. Serialized under
    ``tts_config.voice_descriptor`` on the wire (snake_case object); every field is
    optional and only sent when set.

    Example::

        TTSConfig(provider="deepgram",
                  voice_descriptor=VoiceDescriptor(
                      gender="female", locale="en-US", style="warm"))
    """

    model_config = ConfigDict(use_enum_values=True)

    gender: Optional[Union[VoiceGender, str]] = None
    """Desired gender: ``male`` | ``female`` | ``neutral``."""

    locale: Optional[str] = None
    """BCP-47 locale (preferred): ``en-US``, ``en-GB``, ``hi-IN``, …"""

    accent: Optional[str] = None
    """Free accent string when no ``locale`` is given: ``american``, ``british``, …"""

    age: Optional[Union[VoiceAge, str]] = None
    """Desired age band: ``young`` | ``middle_aged`` | ``old``."""

    style: Optional[str] = None
    """Free timbre/style matched against provider metadata: ``warm``, ``bright``, …"""

    name_hint: Optional[str] = None
    """Optional preferred voice name (``asteria``, ``jenny``); substring-matched."""

    def is_set(self) -> bool:
        """True if ANY field is set (i.e. resolution should be attempted)."""
        return any(
            v is not None and (not isinstance(v, str) or v.strip())
            for v in (self.gender, self.locale, self.accent, self.age, self.style, self.name_hint)
        )

    def to_wire(self) -> dict[str, Any]:
        """Serialize to the gateway ``voice_descriptor`` object (only-set fields)."""
        wire: dict[str, Any] = {}
        for key in ("gender", "locale", "accent", "age", "style", "name_hint"):
            val = getattr(self, key)
            if val is not None and (not isinstance(val, str) or val.strip()):
                wire[key] = val
        return wire


# =============================================================================
# Conversation / Agent-loop Types (built-in LLM loop + REALTIME_REASONING)
# =============================================================================


class ReasoningEffort(str, Enum):
    """Reasoning / thinking-effort dial (REALTIME_REASONING D1).

    Mapped server-side to each vendor's native thinking control and clamped to
    the model's floor. Keep a FAST, non-reasoning model on the spoken path for
    realtime latency.
    """

    OFF = "off"
    MINIMAL = "minimal"
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"


class LatencyFiller(str, Enum):
    """Latency-masking mode (REALTIME_REASONING D3).

    ``auto`` (default) speaks ONE short action phrase when first audio is slow,
    keeping the line alive while the real answer streams in behind it.
    """

    OFF = "off"
    AUTO = "auto"
    AGGRESSIVE = "aggressive"


class RoutingMode(str, Enum):
    """Two-tier reasoning routing (REALTIME_REASONING S2)."""

    AUTO = "auto"
    ALWAYS = "always"


class MuteStrategy(str, Enum):
    """User-mute strategy (conversation A-G5).

    While active, USER INPUT is suppressed (bot/lifecycle signals always flow).
    """

    ALWAYS_WHILE_BOT_SPEAKS = "always_while_bot_speaks"
    UNTIL_FIRST_BOT_COMPLETE = "until_first_bot_complete"
    FIRST_SPEECH_ONLY = "first_speech_only"


class ConversationConfig(BaseModel):
    """Built-in conversation-loop configuration for the gateway (config.rs:53-217).

    When attached to a WS session, the gateway wires up an automatic
    conversation loop: each finalized STT turn drives an OpenAI-compatible LLM
    and the reply is streamed to TTS, with per-session history and barge-in.
    Serialized into the ``conversation_config`` block of the ``config`` message.

    ``base_url`` and ``model`` are the only required fields (mirrors the gateway
    ``ConversationWebSocketConfig``); every other field is optional and only
    sent when set, so the gateway applies its own defaults.
    """

    model_config = ConfigDict(use_enum_values=True)

    base_url: str
    """OpenAI-compatible base URL for the LLM (e.g. 'https://api.openai.com/v1')."""

    model: str
    """Model identifier (e.g. 'gpt-4o-mini', 'llama3.2:1b')."""

    system_prompt: Optional[str] = None
    """Optional system prompt seeding the conversation."""

    api_key: Optional[str] = None
    """API key (literal or '${ENV_VAR}'); falls back to OPENAI_API_KEY server-side."""

    temperature: Optional[float] = None
    """Sampling temperature."""

    max_tokens: Optional[int] = None
    """Max tokens per completion."""

    streaming: Optional[bool] = None
    """Stream tokens to TTS as they arrive (gateway default true)."""

    max_history: Optional[int] = None
    """Max retained history messages (gateway default 20)."""

    allow_interruption: Optional[bool] = None
    """Whether the bot's speech is interruptible / barge-in (gateway default true)."""

    eager_eot: Optional[bool] = None
    """Eager end-of-turn: start the LLM speculatively on a turn-complete prediction."""

    provider_kind: Optional[str] = None
    """LLM vendor wire format: 'openai' (default) | 'anthropic' | 'gemini'."""

    barge_in_min_words: Optional[int] = None
    """While the bot speaks, require >= N words to interrupt it (values < 2 clamp to 2)."""

    summarize_target_tokens: Optional[int] = None
    """Token-aware context compaction threshold (0/omitted = off)."""

    mute_strategy: Optional[Union[MuteStrategy, str]] = None
    """User-mute strategy (suppress user input while active)."""

    strip_markdown: Optional[bool] = None
    """Strip markdown from LLM sentences before TTS (gateway default true)."""

    user_idle_timeout_ms: Optional[int] = None
    """Idle re-engagement after this many ms of silence (0/omitted = off)."""

    reasoning_effort: Optional[Union[ReasoningEffort, str]] = None
    """Reasoning/thinking-effort dial: off | minimal | low | medium | high."""

    latency_filler: Optional[Union[LatencyFiller, str]] = None
    """Latency-masking mode: off | auto | aggressive."""

    latency_filler_after_ms: Optional[int] = None
    """Override the masking wait threshold in ms."""

    latency_filler_phrases: Optional[list[str]] = None
    """Custom masking phrases (empty = built-in pool)."""

    reasoning_model: Optional[str] = None
    """Optional slow REASONING tier model (e.g. 'o3', 'deepseek-r1'); turns two-tier on."""

    reasoning_base_url: Optional[str] = None
    """Reasoning-tier base URL (defaults to base_url)."""

    reasoning_api_key: Optional[str] = None
    """Reasoning-tier API key (defaults to api_key)."""

    reasoning_provider_kind: Optional[str] = None
    """Reasoning-tier vendor wire format (defaults to provider_kind)."""

    reasoning_route: Optional[Union[RoutingMode, str]] = None
    """Route turns between tiers: 'auto' (default) or 'always'."""

    reasoning_budget_ms: Optional[int] = None
    """Reasoning-tier max-silence-gap budget in ms (gateway default 15000; 0 disables)."""

    degradation_message: Optional[str] = None
    """Spoken line used when EVERY LLM tier fails."""

    max_llm_calls_per_turn: Optional[int] = None
    """Per-turn ceiling on LLM re-inference rounds (tool-call loop; gateway default 8)."""

    max_reasoning_tokens: Optional[int] = None
    """Hard ceiling on the reasoning tier's output tokens."""


class TranslationConfig(BaseModel):
    """Canonical, provider-agnostic in-stream/batch translation request (P5).

    Reuses P2's canonical language tokens (:data:`CANONICAL_LANGUAGES`). Lives as
    ``STTConfig.translation`` (and on the batch envelope); the gateway emits a
    uniform ``translations:[{lang, text}]`` array merged onto the transcript
    event regardless of provider, degrading with a ``config_warning`` (never a
    400) where a provider lacks streaming translation.

    Provider classes (the gateway folds them all into one output shape):

    * Class A (arbitrary targets, side-channel): Speechmatics
      (``translation_config.target_languages``, MAX 5), Gladia
      (``realtime_processing.translation``), AssemblyAI (batch only).
    * Class B (English-only fast path): OpenAI/Groq ``/audio/translations`` —
      set ``translate_to_english=True``.
    """

    target_languages: Optional[list[str]] = None
    """Canonical target languages (region-qualified BCP-47, e.g. ``["es-ES",
    "de-DE"]``). Mapped to each provider's native codes server-side; capped
    per-provider (Speechmatics MAX 5 → warn + truncate)."""

    translate_to_english: Optional[bool] = None
    """Fast path: translate the whole stream to ENGLISH (OpenAI/Groq
    ``/audio/translations``). For Class-A providers this is sugar for
    ``target_languages=["en-US"]``."""

    partials: Optional[bool] = None
    """Emit partial (interim) translations where supported (Speechmatics
    ``enable_partials`` / Gladia live). ``None`` = provider default (finals
    only)."""

    def to_wire(self) -> dict[str, Any]:
        """Serialize to the gateway ``stt_config.translation`` shape (omit unset)."""
        wire: dict[str, Any] = {}
        if self.target_languages:
            wire["target_languages"] = list(self.target_languages)
        if self.translate_to_english is not None:
            wire["translate_to_english"] = self.translate_to_english
        if self.partials is not None:
            wire["partials"] = self.partials
        return wire


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

    translation: Optional[TranslationConfig] = None
    """Canonical in-stream/batch translation (P5). When set, the gateway emits a
    uniform ``translations:[{lang, text}]`` array merged onto the transcript;
    unsupported providers degrade with a ``config_warning`` (never a 400)."""


class TTSConfig(BaseModel):
    """TTS (Text-to-Speech) configuration."""

    provider: str = "deepgram"
    """Provider name (e.g., 'deepgram', 'elevenlabs', 'google', 'microsoft-azure', 'cartesia', 'openai')"""

    voice: Optional[str] = None
    """Voice name"""

    voice_id: Optional[str] = None
    """Voice ID (provider-specific)"""

    voice_descriptor: Optional["VoiceDescriptor"] = None
    """Abstract voice selection (gender/locale/accent/age/style/name_hint) resolved
    to a concrete provider voice_id server-side (P4). Used ONLY when no ``voice_id``
    /``voice`` is set — a raw ``voice_id`` always wins. Serialized under
    ``tts_config.voice_descriptor`` on the wire."""

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


class Translation(BaseModel):
    """One translated segment in the uniform gateway ``translations`` array (P5).

    The gateway folds Speechmatics ``AddTranslation`` / Gladia ``type:"translation"``
    / the OpenAI-Groq English fast path into this single ``{lang, text}`` shape so the
    SDK reads ONE field regardless of provider. Mirrors gateway ``Translation``
    (``gateway/src/core/stt/standard.rs``).
    """

    lang: str
    """Canonical target-language BCP-47 string (e.g. ``"es-ES"``)."""

    text: str
    """The translated text for this segment."""

    is_partial: bool = False
    """``True`` if this is a partial (interim) translation, ``False`` if final."""


class STTResult(BaseModel):
    """Speech-to-Text result."""

    text: str
    """Transcribed text"""

    is_final: bool
    """Whether this is a final result"""

    is_speech_final: bool = False
    """Whether the speaker has finished their utterance (speech endpoint detection)"""

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

    translations: list[Translation] = Field(default_factory=list)
    """Uniform in-stream translations merged onto this transcript (P5). Empty
    unless a translation-capable provider (Speechmatics/Gladia/OpenAI EN fast
    path) returned a `translations:[{lang,text}]` array on this stt_result frame."""


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

    translations: list[Translation] = Field(default_factory=list)
    """Uniform in-stream translations merged onto this transcript (P5). Empty
    unless a translation-capable provider returned a `translations:[{lang,text}]`
    array on this stt_result frame."""

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
    """Provider for voice cloning operations (gateway ``VoiceCloneProvider``).

    The FULL set sourced 1:1 from ``gateway/src/handlers/voices.rs`` (P4 widened it
    from the 2-provider SDK enum). A bare ``str`` is accepted by
    :meth:`VoicesAPI.clone` for forward-compat.
    """

    HUME = "hume"
    ELEVENLABS = "elevenlabs"
    LMNT = "lmnt"
    CARTESIA = "cartesia"
    PLAYHT = "playht"
    SPEECHIFY = "speechify"
    RESEMBLE = "resemble"


class CloneMode(str, Enum):
    """Voice-clone mode (gateway ``CloneMode``).

    - ``instant`` (default): IVC / clip clone; returns ``ready`` immediately or
      near-instantly.
    - ``professional``: async high-fidelity (ElevenLabs PVC / Resemble / PlayHT
      PVC); the returned ``voice_id`` is polled until ``ready``.
    """

    INSTANT = "instant"
    PROFESSIONAL = "professional"


class VoiceCloneRequest(BaseModel):
    """Request to clone a voice from audio samples or description.

    Mirrors the gateway ``VoiceCloneRequest`` (``POST /voices/clone``). ``provider``
    and ``name`` are required; everything else is optional and only sent when set.
    """

    model_config = ConfigDict(use_enum_values=True)

    provider: Union[VoiceCloneProvider, str]
    """Provider to use for voice cloning"""

    name: str
    """Name for the cloned voice"""

    description: Optional[str] = None
    """Description of the voice (used by Hume for voice design / ElevenLabs label)"""

    audio_samples: Optional[list[str]] = None
    """Audio samples for cloning (base64-encoded or URLs). ElevenLabs: 1-2 min recommended"""

    sample_text: Optional[str] = None
    """Sample text for voice generation (Hume only)"""

    remove_background_noise: bool = False
    """Remove background noise from samples (ElevenLabs IVC / LMNT enhance)"""

    labels: Optional[dict[str, str]] = None
    """Flat labels for the voice (ElevenLabs)"""

    mode: Union[CloneMode, str] = CloneMode.INSTANT
    """Clone MODE: ``instant`` (default) or ``professional`` (async high-fidelity)"""

    base_voice_id: Optional[str] = None
    """Design-from-existing: the base voice to clone/derive from (provider-specific)"""


class VoiceCloneStatus(str, Enum):
    """Canonical lifecycle status of a cloned voice (gateway ``CloneStatus``).

    A ``ready`` ``voice_id`` is directly usable as a TTS ``voice_id``.
    ``READY`` / ``FAILED`` are TERMINAL; the others mean the clone is still being
    produced (poll until terminal). ``PROCESSING`` is KEPT as a backward-compatible
    alias for the old SDK status (it maps onto the in-progress ``training`` state).
    """

    READY = "ready"
    VERIFYING = "verifying"
    TRAINING = "training"
    QUEUED = "queued"
    FAILED = "failed"
    # Backward-compatible alias for the pre-P4 SDK status (in-progress).
    PROCESSING = "processing"

    @property
    def is_terminal(self) -> bool:
        """True once the clone has finished (ready or failed) — stop polling."""
        return self in (VoiceCloneStatus.READY, VoiceCloneStatus.FAILED)


class VoiceCloneResponse(BaseModel):
    """Response from voice cloning operation (gateway ``VoiceCloneResponse``)."""

    voice_id: str
    """Unique identifier for the cloned voice (usable as a TTS voice_id once ready)"""

    name: str
    """Name of the cloned voice"""

    provider: Union[VoiceCloneProvider, str]
    """Provider that created the voice"""

    status: Union[VoiceCloneStatus, str]
    """Canonical status: ready | verifying | training | queued | failed"""

    requires_verification: Optional[bool] = None
    """Whether the voice still needs verification before use (ElevenLabs IVC)"""

    created_at: Optional[str] = None
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
            for name in self.__class__.model_fields
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
    """Node types supported in DAG definitions.

    Matches gateway NodeType enum with all 25 variants:
    - Input nodes: audio_input, text_input
    - Output nodes: audio_output, text_output, webhook_output
    - Provider nodes: stt_provider, tts_provider, realtime_provider
    - Processing nodes: processor, transform, passthrough
    - Endpoint nodes: http_endpoint, grpc_endpoint, websocket_endpoint,
                      ipc_endpoint, livekit_endpoint, llm_endpoint
    - Router nodes: router, split, join
    """

    # Input nodes
    AUDIO_INPUT = "audio_input"
    TEXT_INPUT = "text_input"

    # Output nodes
    AUDIO_OUTPUT = "audio_output"
    TEXT_OUTPUT = "text_output"
    WEBHOOK_OUTPUT = "webhook_output"

    # Provider nodes
    STT_PROVIDER = "stt_provider"
    TTS_PROVIDER = "tts_provider"
    REALTIME_PROVIDER = "realtime_provider"

    # Processing nodes
    PROCESSOR = "processor"
    TRANSFORM = "transform"
    PASSTHROUGH = "passthrough"

    # Endpoint nodes
    HTTP_ENDPOINT = "http_endpoint"
    GRPC_ENDPOINT = "grpc_endpoint"
    WEBSOCKET_ENDPOINT = "websocket_endpoint"
    IPC_ENDPOINT = "ipc_endpoint"
    LIVEKIT_ENDPOINT = "livekit_endpoint"
    LLM_ENDPOINT = "llm_endpoint"

    # Router nodes
    ROUTER = "router"
    SPLIT = "split"
    JOIN = "join"

    # Legacy aliases (kept for backward compatibility)
    LLM = "llm_endpoint"
    WEBHOOK = "webhook_output"
    BUFFER = "buffer"
    SWITCH = "switch"


class OutputDestination(str, Enum):
    """Output destination for DAG output nodes."""

    WEBSOCKET = "websocket"
    LIVEKIT = "livekit"
    ENDPOINT = "endpoint"
    BROADCAST = "broadcast"
    DISCARD = "discard"


class JoinStrategy(str, Enum):
    """Strategy for DAG Join nodes."""

    FIRST = "first"
    ALL = "all"
    BEST = "best"
    MERGE = "merge"


class DAGDataType(str, Enum):
    """Data types that flow between DAG nodes."""

    AUDIO = "audio"
    TEXT = "text"
    STT_RESULT = "stt_result"
    TTS_AUDIO = "tts_audio"
    JSON = "json"
    BINARY = "binary"
    MULTIPLE = "multiple"
    EMPTY = "empty"


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
    """Validation result for DAG definitions.

    Matches gateway ValidateDAGResponse format.
    """

    valid: bool
    """Whether the DAG is valid"""

    errors: list[str]
    """List of validation errors"""

    warnings: list[str]
    """List of validation warnings"""

    node_count: int = 0
    """Number of nodes in the DAG"""

    edge_count: int = 0
    """Number of edges in the DAG"""


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

    return DAGValidationResult(
        valid=len(errors) == 0,
        errors=errors,
        warnings=warnings,
        node_count=len(dag.nodes),
        edge_count=len(dag.edges),
    )


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
