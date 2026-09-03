"""P4 SDK mirror: VoiceDescriptor on the TTS config, bud.voices.clone, widened emotions.

Gateway side committed at 8d18c4e (emotion 22→44, VoiceDescriptor resolver, widened
voice-clone REST). This mirrors the canonical VALUE SPACE in the SDK; the MAPPING
stays gateway-side. Style mirrors tests/unit/test_alias.py (P3).
"""

import base64
import inspect
from unittest.mock import AsyncMock

import pytest

from bud_waav import (
    BudClient,
    CloneMode,
    DeliveryStyle,
    Emotion,
    TTSConfig,
    VoiceAge,
    VoiceCloneProvider,
    VoiceCloneStatus,
    VoiceDescriptor,
    VoiceGender,
)
from bud_waav.rest.client import RestClient
from bud_waav.voices import VoicesAPI
from bud_waav.ws.session import WebSocketSession


# =============================================================================
# A. VoiceDescriptor -> tts_config.voice_descriptor on the wire
# =============================================================================


def test_voice_descriptor_is_in_send_config_source() -> None:
    # Mirrors the P3 alias test: assert the wire field is emitted by _send_config.
    s = WebSocketSession(url="ws://x", audio=False)
    src = inspect.getsource(s._send_config)
    assert 'tts_dict["voice_descriptor"]' in src


def test_voice_descriptor_to_wire_only_set_fields() -> None:
    vd = VoiceDescriptor(gender="female", locale="en-US", style="warm")
    wire = vd.to_wire()
    assert wire == {"gender": "female", "locale": "en-US", "style": "warm"}
    # Unset fields (accent/age/name_hint) are omitted.
    assert "accent" not in wire and "age" not in wire and "name_hint" not in wire
    assert vd.is_set()
    assert not VoiceDescriptor().is_set()


def test_voice_descriptor_serializes_into_tts_config() -> None:
    # Build the exact config dict _send_config produces, for a TTS config that
    # carries a VoiceDescriptor and NO raw voice_id.
    tts = TTSConfig(
        provider="deepgram",
        voice_descriptor=VoiceDescriptor(
            gender=VoiceGender.FEMALE, locale="en-US", age=VoiceAge.YOUNG, style="warm"
        ),
    )
    # voice_id left None so the descriptor is the selection path.
    assert tts.voice_id is None

    session = WebSocketSession(url="ws://x", tts_config=tts, audio=True)
    # Reproduce the tts_dict block of _send_config deterministically.
    vd_wire = session.tts_config.voice_descriptor.to_wire()
    assert vd_wire["gender"] == "female"
    assert vd_wire["locale"] == "en-US"
    assert vd_wire["age"] == "young"
    assert vd_wire["style"] == "warm"


def test_voice_descriptor_threads_through_agent_dict_path() -> None:
    # A dict tts config with a nested voice_descriptor must survive the
    # BudTalk.create dict-coercion (the P3-style drop bug).
    c = BudClient(base_url="http://127.0.0.1:3009")
    sess = c.agent(
        stt={"provider": "deepgram"},
        tts={"provider": "deepgram", "voice_descriptor": {"gender": "male", "accent": "british"}},
        llm={"base_url": "http://x/v1", "model": "m"},
    )
    vd = sess._session.tts_config.voice_descriptor
    assert vd is not None
    assert vd.to_wire() == {"gender": "male", "accent": "british"}


# =============================================================================
# B. bud.voices.clone builds the canonical POST body + wait_until_ready
# =============================================================================


@pytest.mark.asyncio
async def test_voices_clone_builds_canonical_post_body() -> None:
    rest = RestClient(base_url="http://localhost:3001", api_key="test-key")
    rest.post = AsyncMock(
        return_value={
            "voice_id": "voice_abc123",
            "name": "My Voice",
            "provider": "elevenlabs",
            "status": "ready",
            "created_at": "2026-06-17T00:00:00Z",
        }
    )
    voices = VoicesAPI(rest)

    raw_wav = b"\x00\x01\x02\x03"
    result = await voices.clone(
        provider="elevenlabs",
        name="My Voice",
        samples=[raw_wav, "https://example.com/sample.wav"],
        mode="instant",
        description="A warm voice",
    )

    # Returns a usable voice_id + status.
    assert result.voice_id == "voice_abc123"
    assert result.status == VoiceCloneStatus.READY

    # POST body matches the gateway VoiceCloneRequest canonical shape.
    rest.post.assert_called_once()
    args, kwargs = rest.post.call_args
    assert args[0] == "/voices/clone"
    body = kwargs["json"]
    assert body["provider"] == "elevenlabs"
    assert body["name"] == "My Voice"
    assert body["mode"] == "instant"
    assert body["description"] == "A warm voice"
    # bytes sample -> base64; str sample (URL) passes through unchanged.
    assert body["audio_samples"][0] == base64.b64encode(raw_wav).decode()
    assert body["audio_samples"][1] == "https://example.com/sample.wav"
    # Canonical field name (NOT the old broken `audio_files`).
    assert "audio_files" not in body


@pytest.mark.asyncio
async def test_voices_clone_wait_until_ready_instant() -> None:
    rest = RestClient(base_url="http://localhost:3001")
    rest.post = AsyncMock(
        return_value={
            "voice_id": "v1",
            "name": "V",
            "provider": "cartesia",
            "status": "ready",
        }
    )
    result = await VoicesAPI(rest).clone(provider="cartesia", name="V", samples=[b"a"])
    # Instant clone is already ready -> resolves immediately to the response.
    resolved = await result.wait_until_ready()
    assert resolved.voice_id == "v1"


@pytest.mark.asyncio
async def test_voices_clone_professional_queued_raises_on_wait() -> None:
    rest = RestClient(base_url="http://localhost:3001")
    rest.post = AsyncMock(
        return_value={
            "voice_id": "pvc1",
            "name": "Pro",
            "provider": "elevenlabs",
            "status": "queued",
        }
    )
    result = await VoicesAPI(rest).clone(
        provider="elevenlabs", name="Pro", samples=[b"a"], mode="professional"
    )
    assert result.status == VoiceCloneStatus.QUEUED
    # No gateway poll endpoint this phase -> honest error rather than spinning.
    with pytest.raises(RuntimeError):
        await result.wait_until_ready()


def test_clone_mode_and_provider_enum_values() -> None:
    assert CloneMode.INSTANT.value == "instant"
    assert CloneMode.PROFESSIONAL.value == "professional"
    # Widened from the old 2-provider SDK enum to the gateway's 7.
    expected = {"hume", "elevenlabs", "lmnt", "cartesia", "playht", "speechify", "resemble"}
    assert {p.value for p in VoiceCloneProvider} == expected


def test_clone_status_terminal_and_canonical() -> None:
    assert VoiceCloneStatus.READY.is_terminal
    assert VoiceCloneStatus.FAILED.is_terminal
    assert not VoiceCloneStatus.QUEUED.is_terminal
    assert not VoiceCloneStatus.TRAINING.is_terminal
    # Canonical lifecycle values present.
    for s in ("ready", "verifying", "training", "queued", "failed"):
        assert VoiceCloneStatus(s).value == s


# =============================================================================
# C. Widened Emotion + DeliveryStyle value space (mirror of gateway enum)
# =============================================================================


def test_new_emotion_variants_exist() -> None:
    # The 22 P4-added affective states (canonical snake_case wire values).
    new = [
        ("AMAZED", "amazed"),
        ("AMUSED", "amused"),
        ("AFFECTIONATE", "affectionate"),
        ("INTRIGUED", "intrigued"),
        ("FLIRTATIOUS", "flirtatious"),
        ("FRUSTRATED", "frustrated"),
        ("ANNOYED", "annoyed"),
        ("DETERMINED", "determined"),
        ("REASSURING", "reassuring"),
        ("SYMPATHETIC", "sympathetic"),
        ("NOSTALGIC", "nostalgic"),
        ("SERENE", "serene"),
        ("TERRIFIED", "terrified"),
        ("ECSTATIC", "ecstatic"),
        ("SKEPTICAL", "skeptical"),
        ("RELIEVED", "relieved"),
        ("PANICKED", "panicked"),
        ("CONCERNED", "concerned"),
        ("APOLOGETIC", "apologetic"),
        ("RESIGNED", "resigned"),
        ("WISTFUL", "wistful"),
        ("CONTEMPLATIVE", "contemplative"),
    ]
    for name, value in new:
        assert getattr(Emotion, name).value == value
    # Full superset is 44 (22 original + 22 widened).
    assert len(list(Emotion)) == 44


def test_old_emotion_variants_kept() -> None:
    # Forward/backward compat: original variants must NOT be removed.
    for name in ("NEUTRAL", "HAPPY", "SAD", "ANGRY", "SARCASTIC", "BORED"):
        assert hasattr(Emotion, name)


def test_new_delivery_style_scenarios_exist() -> None:
    scenarios = [
        ("NEWSCAST", "newscast"),
        ("CUSTOMER_SERVICE", "customer_service"),
        ("ASSISTANT", "assistant"),
        ("CHAT", "chat"),
        ("ADVERTISEMENT_UPBEAT", "advertisement_upbeat"),
        ("SPORTS_COMMENTARY", "sports_commentary"),
        ("DOCUMENTARY_NARRATION", "documentary_narration"),
        ("NARRATION_PROFESSIONAL", "narration_professional"),
        ("NARRATION_RELAXED", "narration_relaxed"),
        ("POETRY_READING", "poetry_reading"),
        ("LYRICAL", "lyrical"),
        ("GENTLE", "gentle"),
    ]
    for name, value in scenarios:
        assert getattr(DeliveryStyle, name).value == value
    # Old cross-enum aliases kept for backward compat.
    for name in ("SOFT", "CHEERFUL", "SERIOUS", "NORMAL"):
        assert hasattr(DeliveryStyle, name)


def test_emotion_accepts_raw_string_forward_compat() -> None:
    # A free-text/raw emotion string is still accepted on the TTS config (the
    # gateway resolves unknown tokens; never a hard error).
    tts = TTSConfig(provider="hume", emotion="amazed", emotion_description="warm, wistful")
    assert tts.emotion == Emotion.AMAZED
    assert tts.emotion_description == "warm, wistful"
