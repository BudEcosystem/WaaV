"""Drift-guarded 1:1 mirror of the gateway ``/ws`` ``config`` envelope.

WHY THIS EXISTS
---------------
The single highest-leverage SDK fix in ``SDK_STANDARDIZATION_PLAN.md`` is to make
the SDK config a complete, *drift-guarded* mirror of the gateway ``/ws`` config
tree so the failure mode "a feature exists server-side but is unreachable from the
SDK" can never silently recur. The gateway already keeps its committed OpenAPI
spec honest against the live wire structs (``gateway/tests/openapi_drift.rs``,
byte-identical gate). This module closes the loop on the SDK side: it declares,
for every gateway config schema, exactly which OpenAPI property names the SDK can
*reach* on the wire, and ``tests/unit/test_config_drift.py`` asserts that set
covers every property the committed ``gateway/docs/openapi.yaml`` exposes.

If the gateway gains or renames a config field and regenerates the spec, the drift
test fails until this registry (and the serializer in ``ws/session.py`` / the
typed config in ``types.py``) is updated — making "unreachable feature" a build
failure instead of a silent no-op.

WHAT "REACHABLE" MEANS
----------------------
A gateway property is *reachable* when the SDK has a typed way to put it on the
wire. That is one of:

* a direct typed field that serializes 1:1 (e.g. ``base_url`` in
  :class:`~bud_waav.types.ConversationConfig`);
* a typed field under a *documented alias* (e.g. the gateway's
  ``stt_config.punctuation`` is set from the SDK's :class:`STTConfig.punctuate`;
  ``tts_config.speaking_rate`` from ``TTSConfig.speed``);
* a canonical nested block the SDK populates (``stt_config.features`` /
  ``tts_config.features`` carry the standardized ``SttFeatures``/``TtsFeatures``
  vocabulary; ``extras`` is the open passthrough escape hatch).

The registry below records the gateway-property → SDK-reach mapping per schema so
the assertion is *coverage* (SDK reach ⊇ gateway properties), not *equality* —
the SDK may also expose convenience fields with no 1:1 gateway property (those
ride through ``features``/``extras``), and that is fine.

This module has NO third-party dependency (the drift test ships a tiny YAML
property extractor) so it runs in a bare SDK test venv.
"""

from __future__ import annotations

# Schema names in the committed OpenAPI spec that together make up the ``/ws``
# ``config`` envelope tree (IncomingMessage::Config branch). Source of truth:
# gateway/src/handlers/ws/config.rs (+ messages.rs for the envelope itself).
CONFIG_ENVELOPE_SCHEMA = "IncomingMessage"

# The set of OpenAPI property names, per gateway config schema, that the SDK can
# reach on the wire. Keep these in lockstep with ``ws/session.py::_send_config``
# and the typed config classes in ``types.py``. The drift test asserts each entry
# COVERS the corresponding schema's ``properties`` in gateway/docs/openapi.yaml.
#
# Every name below is a *gateway* property name (what goes on the wire), NOT the
# SDK field name. Where they differ, the comment records the SDK source.
SDK_CONFIG_REACH: dict[str, set[str]] = {
    # ---- top-level config envelope (messages.rs Config branch) ----------------
    # ``type`` is the discriminator the SDK always sets; ``audio_disabled`` is the
    # deprecated alias for ``audio: false`` (the SDK uses ``audio``).
    "ConfigEnvelope": {
        "type",
        "stream_id",
        "audio",
        "audio_disabled",
        "stt_config",
        "tts_config",
        "livekit",
        "dag_config",
        "conversation_config",
        "alias",  # P3 server-side alias name (ws/session.py::_send_config)
    },
    # ---- STTWebSocketConfig (config.rs:296) -----------------------------------
    "STTWebSocketConfig": {
        "provider",  # STTConfig.provider
        "language",  # STTConfig.language
        "sample_rate",  # STTConfig.sample_rate
        "channels",  # STTConfig.channels
        "punctuation",  # STTConfig.punctuate (SDK alias)
        "encoding",  # STTConfig.encoding
        "audio_in_codec",  # D8 STTConfig.audio_in_codec (uplink transport codec)
        "model",  # STTConfig.model
        "api_key",  # session api_key override
        "features",  # nested SttFeatures (see SttFeatures below)
        "extras",  # open passthrough (custom_vocabulary, …)
        "turn_detection",  # AudioFeatures.turn_detection (nested here)
        # P5: the canonical TranslationConfig the SDK sends as `stt_config.translation`
        # (see STTConfig.translation + ws/session.py::_send_config). Landed in lockstep
        # with the gateway openapi regen that added STTWebSocketConfig.translation, so
        # the bidirectional drift guard stays green on both directions.
        "translation",
    },
    # ---- TurnDetectionWsConfig (config.rs:351) --------------------------------
    "TurnDetectionWsConfig": {
        "enabled",  # TurnDetectionConfig.enabled
        "threshold",  # TurnDetectionConfig.threshold
        "eager",  # forwarded from turn={"eager": ...}/eager_eot pairing
    },
    # ---- SttFeatures (stt/standard.rs) — canonical advanced STT vocabulary -----
    # The SDK serializes the common subset directly from ``STTConfig`` and lets a
    # caller pass the full set via ``STTConfig.features`` / ``stt={"features":...}``.
    "SttFeatures": {
        "interim_results",
        "diarization",
        "word_timestamps",
        "smart_format",
        "profanity_filter",
        "filler_words",
        "vad_events",
        "endpointing_ms",
        "utterance_end_ms",
        "keyterms",  # STTConfig.keywords (SDK alias)
        "redaction",
        "language_detection",
        "entity_detection",
        "numerals",
        "multichannel",
        "alternatives",
        "sentiment",
        "speech_begin_event",
    },
    # ---- TTSWebSocketConfig (config.rs:478) -----------------------------------
    "TTSWebSocketConfig": {
        "provider",  # TTSConfig.provider
        "voice_id",  # TTSConfig.voice_id / TTSConfig.voice
        "speaking_rate",  # TTSConfig.speed (SDK alias)
        "audio_format",  # TTSConfig.audio_format
        "sample_rate",  # TTSConfig.sample_rate
        "audio_out_chunk_ms",  # TTSConfig.audio_out_chunk_ms
        "client_playback_rate",  # TTSConfig.client_playback_rate
        "audio_out_codec",  # D8 TTSConfig.audio_out_codec (downlink transport codec)
        "connection_timeout",  # TTSConfig.connection_timeout
        "request_timeout",  # TTSConfig.request_timeout
        "model",  # TTSConfig.model
        "pronunciations",  # TTSConfig.pronunciations
        "api_key",  # session api_key override
        "emotion",  # TTSConfig.emotion (Unified Emotion System)
        "emotion_intensity",  # TTSConfig.emotion_intensity
        "delivery_style",  # TTSConfig.delivery_style
        "emotion_description",  # TTSConfig.emotion_description
        "voice_descriptor",  # P4 abstract voice selection (TTSConfig.voice_descriptor)
        "features",  # nested TtsFeatures (see below)
        "extras",  # open passthrough (acting_instructions, instant_mode, …)
    },
    # ---- TtsFeatures (tts/standard.rs) — canonical advanced TTS vocabulary ------
    "TtsFeatures": {
        "speed",  # TTSConfig.speed (also mirrored to top-level speaking_rate)
        "pitch",  # TTSConfig.pitch
        "volume",  # TTSConfig.volume
        "stability",  # TTSConfig.stability
        "similarity_boost",  # TTSConfig.similarity_boost
        "style",  # TTSConfig.style
        "use_speaker_boost",  # TTSConfig.use_speaker_boost
        "emotion",
        "instructions",
        "ssml",
        "language",
        "word_timestamps",
        "streaming",
        "seed",
        "sample_rate",
        "optimize_streaming_latency",
        "include_timestamp_types",
        "rate_percentage",
        "pitch_percentage",
    },
    # ---- LiveKitWebSocketConfig (config.rs:407) -------------------------------
    # The SDK forwards the LiveKit block as a dict; every gateway field is a
    # legal key. Documented here so a renamed/added gateway field is caught.
    "LiveKitWebSocketConfig": {
        "room_name",
        "enable_recording",
        "waav_participant_identity",
        "waav_participant_name",
        "listen_participants",
    },
    # ---- DAGWebSocketConfig (config.rs:24) ------------------------------------
    "DAGWebSocketConfig": {
        "template",  # DAGConfig.template
        "definition",  # DAGConfig.definition (typed DAGDefinition)
        "enable_metrics",  # DAGConfig.enable_metrics
        "timeout_ms",  # DAGConfig.timeout_ms
    },
    # ---- ConversationWebSocketConfig (config.rs:53) — the flagship LLM loop -----
    # 1:1 typed mirror in ConversationConfig; every field serialized by
    # ws/session.py::_send_config (base_url+model required, rest sent when set).
    "ConversationWebSocketConfig": {
        "base_url",
        "model",
        "system_prompt",
        "api_key",
        "temperature",
        "max_tokens",
        "streaming",
        "max_history",
        "allow_interruption",
        "eager_eot",
        "provider_kind",
        "barge_in_min_words",
        "summarize_target_tokens",
        "mute_strategy",
        "strip_markdown",
        "user_idle_timeout_ms",
        "reasoning_effort",
        "latency_filler",
        "latency_filler_after_ms",
        "latency_filler_phrases",
        "reasoning_model",
        "reasoning_base_url",
        "reasoning_api_key",
        "reasoning_provider_kind",
        "reasoning_route",
        "reasoning_budget_ms",
        "degradation_message",
        "max_llm_calls_per_turn",
        "max_reasoning_tokens",
    },
}


# AdapterKind union for ``provider_kind`` / ``reasoning_provider_kind``.
# These two fields render as a plain nullable ``string`` in the OpenAPI spec
# (config.rs uses ``schema(value_type = Option<String>)``), so the typed value
# space must be hand-authored from ``gateway/src/core/llm/adapter.rs:40``.
ADAPTER_KINDS: tuple[str, ...] = ("openai", "anthropic", "gemini")
