"""Segmented speech-to-text wire contract (gateway docs/segmented-stt/customer-contract-reference.md).

Covers the Python SDK side of the contract:

* ``stt_config.transcription_mode`` (auto | streaming | segmented) on the config wire;
* ``ready.stt``, the open object saying what speech-to-text the session got (``session.stt``);
* the new ``vad_event`` and ``stt_warning`` messages (typed parsers, session callbacks, Talk events);
* the additive ``code`` / ``recoverable`` / ``details`` fields on ``error``.

Session tests drive ``WebSocketSession._receive_loop`` with the exact gateway JSON frames.
"""

from __future__ import annotations

import json
from collections.abc import AsyncIterator
from typing import Any
from unittest.mock import MagicMock

import pydantic
import pytest

from bud_waav import (
    BudError,
    GatewayError,
    ReadySTT,
    STTConfig,
    SttNotice,
    SttWarning,
    TalkEvent,
    TranscriptionMode,
    VadEvent,
)
from bud_waav.config_mirror import SDK_CONFIG_REACH
from bud_waav.pipelines.stt import STTSession
from bud_waav.pipelines.talk import TalkSession
from bud_waav.ws.session import WebSocketSession

# The `ready.stt` example from the contract reference, verbatim.
READY_STT: dict[str, Any] = {
    "provider": "elevenlabs",
    "model": "scribe_v2",
    "model_source": "deployment",
    "transcription_mode": "segmented",
    "requested_mode": "auto",
    "requested_mode_source": "default",
    "interim_results": "per_segment",
    "endpointing": "gateway",
    "speech_events": "detector",
    "barge_in_ms": 500,
    "detector": "silero",
    "confidence_source": "none",
    "latency_class": "slow",
    "final_latency_typical_ms": None,
    "final_latency_slow_ms": 2010,
    "final_latency_slow_percentile": 99,
    "latency_basis": "seed",
    "final_deadline_ms": 6000,
    "lifecycle": "ga",
    "capability_source": "exact",
    "streaming_alternatives": ["scribe_v2_realtime"],
    "map_version": "2026-10-01.1",
    "notices": [
        {
            "code": "stt_language_unset",
            "message": "Set language: detection on short segments is unreliable.",
        }
    ],
}


async def _capture_config(session: WebSocketSession) -> dict[str, Any]:
    sent: list[str] = []

    async def mock_send(data: str) -> None:
        sent.append(data)

    session._ws = MagicMock()
    session._ws.send = mock_send
    await session._send_config()
    assert len(sent) == 1
    config: dict[str, Any] = json.loads(sent[0])
    return config


async def _feed(session: WebSocketSession, *frames: dict[str, Any]) -> list[dict[str, Any]]:
    """Run the receive loop over ``frames`` and return what it put on the message queue."""

    async def messages() -> AsyncIterator[str]:
        for frame in frames:
            yield json.dumps(frame)

    session._ws = messages()  # type: ignore[assignment]
    session._connected = True
    await session._receive_loop()
    queued = []
    queue = session._get_message_queue()
    while not queue.empty():
        queued.append(queue.get_nowait())
    return queued


# =============================================================================
# Config -> wire
# =============================================================================


class TestTranscriptionModeConfig:
    def test_field_defaults_to_none(self) -> None:
        assert STTConfig(provider="deepgram").transcription_mode is None

    @pytest.mark.parametrize("mode", ["auto", "streaming", "segmented"])
    def test_accepts_the_three_modes(self, mode: TranscriptionMode) -> None:
        assert STTConfig(provider="elevenlabs", transcription_mode=mode).transcription_mode == mode

    def test_rejects_an_unknown_mode(self) -> None:
        with pytest.raises(pydantic.ValidationError):
            STTConfig(provider="elevenlabs", transcription_mode="batch")  # type: ignore[arg-type]

    @pytest.mark.asyncio
    @pytest.mark.parametrize("mode", ["auto", "streaming", "segmented"])
    async def test_serialized_on_stt_config(self, mode: TranscriptionMode) -> None:
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="elevenlabs", model="scribe_v2", transcription_mode=mode),
        )
        wire = await _capture_config(session)
        assert wire["stt_config"]["transcription_mode"] == mode

    @pytest.mark.asyncio
    async def test_omitted_when_unset(self) -> None:
        session = WebSocketSession(
            url="ws://localhost:3001/ws", stt_config=STTConfig(provider="deepgram")
        )
        wire = await _capture_config(session)
        assert "transcription_mode" not in wire["stt_config"]

    def test_reachable_in_the_config_mirror(self) -> None:
        assert "transcription_mode" in SDK_CONFIG_REACH["STTWebSocketConfig"]


# =============================================================================
# Typed parsers
# =============================================================================


class TestReadySTT:
    def test_parses_the_contract_example(self) -> None:
        stt = ReadySTT.from_wire(READY_STT)
        assert stt.provider == "elevenlabs"
        assert stt.model == "scribe_v2"
        assert stt.model_source == "deployment"
        assert stt.transcription_mode == "segmented"
        assert stt.interim_results == "per_segment"
        assert stt.endpointing == "gateway"
        assert stt.speech_events == "detector"
        assert stt.barge_in_ms == 500
        assert stt.final_latency_typical_ms is None
        assert stt.final_latency_slow_ms == 2010
        assert stt.final_deadline_ms == 6000
        assert stt.streaming_alternatives == ["scribe_v2_realtime"]
        assert isinstance(stt.notices[0], SttNotice)
        assert stt.notices[0].code == "stt_language_unset"

    def test_round_trips_to_the_wire_dict(self) -> None:
        assert ReadySTT.from_wire(READY_STT).to_wire() == READY_STT

    def test_keeps_unknown_keys(self) -> None:
        stt = ReadySTT.from_wire({"provider": "openai", "future_field": {"a": 1}})
        assert stt.provider == "openai"
        assert stt.to_wire()["future_field"] == {"a": 1}

    def test_never_raises_on_an_unexpected_value_type(self) -> None:
        # A newer gateway sending a different type must not kill the receive loop.
        stt = ReadySTT.from_wire({"provider": "x", "barge_in_ms": "lots"})
        assert stt.provider == "x"
        assert stt.to_wire()["barge_in_ms"] == "lots"


class TestVadEvent:
    def test_parses_turn_closed(self) -> None:
        ev = VadEvent.from_wire(
            {
                "type": "vad_event",
                "event": "turn_closed",
                "turn_id": 7,
                "had_transcript": False,
                "reason": "no_speech",
            }
        )
        assert ev.event == "turn_closed"
        assert ev.turn_id == 7
        assert ev.had_transcript is False
        assert ev.reason == "no_speech"
        assert ev.audio_ms is None
        assert ev.sustained_ms is None
        assert ev.discarded is None

    def test_parses_optional_fields(self) -> None:
        start = VadEvent.from_wire(
            {
                "type": "vad_event",
                "event": "turn_start",
                "turn_id": 3,
                "audio_ms": 1840,
                "sustained_ms": 500,
            }
        )
        assert (start.audio_ms, start.sustained_ms) == (1840, 500)
        end = VadEvent.from_wire(
            {"type": "vad_event", "event": "speech_end", "turn_id": 3, "discarded": True}
        )
        assert end.discarded is True

    def test_never_raises_on_a_malformed_frame(self) -> None:
        ev = VadEvent.from_wire({"type": "vad_event", "speech": True})
        assert ev.event == ""


class TestSttWarning:
    def test_parses_with_detail(self) -> None:
        w = SttWarning.from_wire(
            {
                "type": "stt_warning",
                "code": "stt_segment_failed",
                "message": "lost",
                "detail": {"turn_id": 4},
            }
        )
        assert (w.code, w.message, w.detail) == ("stt_segment_failed", "lost", {"turn_id": 4})

    def test_parses_without_detail(self) -> None:
        w = SttWarning.from_wire({"type": "stt_warning", "code": "stt_degraded", "message": "m"})
        assert w.detail is None


# =============================================================================
# WebSocketSession
# =============================================================================


class TestSessionReadyStt:
    @pytest.mark.asyncio
    async def test_ready_stt_is_exposed_typed(self) -> None:
        session = WebSocketSession(url="ws://localhost:3001/ws")
        assert session.stt is None
        await _feed(
            session,
            {"type": "ready", "protocol_version": "1.0", "stream_id": "s-1", "stt": READY_STT},
        )
        assert isinstance(session.stt, ReadySTT)
        assert session.stt.transcription_mode == "segmented"
        assert session.stt.to_wire() == READY_STT

    @pytest.mark.asyncio
    async def test_ready_without_stt_leaves_none(self) -> None:
        session = WebSocketSession(url="ws://localhost:3001/ws")
        await _feed(session, {"type": "ready", "protocol_version": "1.0", "stream_id": "s-1"})
        assert session.stream_id == "s-1"
        assert session.stt is None


class TestSessionVadEvent:
    @pytest.mark.asyncio
    async def test_vad_event_reaches_callback_and_queue(self) -> None:
        session = WebSocketSession(url="ws://localhost:3001/ws")
        got: list[dict[str, Any]] = []
        errors: list[Any] = []
        session.on("vad_event", got.append)
        session.on("error", errors.append)
        frame = {
            "type": "vad_event",
            "event": "turn_closed",
            "turn_id": 7,
            "had_transcript": False,
            "reason": "no_speech",
        }

        queued = await _feed(session, frame)

        assert got == [frame]
        assert VadEvent.from_wire(got[0]).turn_id == 7
        assert queued == [{"type": "vad_event", "data": frame}]
        assert errors == []


class TestSessionSttWarning:
    @pytest.mark.asyncio
    async def test_stt_warning_is_its_own_event_never_error(self) -> None:
        session = WebSocketSession(url="ws://localhost:3001/ws")
        warnings: list[dict[str, Any]] = []
        errors: list[Any] = []
        config_warnings: list[Any] = []
        unknown: list[Any] = []
        session.on("stt_warning", warnings.append)
        session.on("error", errors.append)
        session.on("config_warning", config_warnings.append)
        session.on("server_message", unknown.append)
        frame = {
            "type": "stt_warning",
            "code": "stt_segment_failed",
            "message": "A segment was lost after its retry.",
            "detail": {"turn_id": 5, "segment_seq": 1},
        }

        queued = await _feed(session, frame)

        assert warnings == [frame]
        assert queued == [{"type": "stt_warning", "data": frame}]
        assert errors == []
        assert config_warnings == []
        assert unknown == []


class TestSessionCodedError:
    @pytest.mark.asyncio
    async def test_coded_error_carries_code_recoverable_details(self) -> None:
        session = WebSocketSession(url="ws://localhost:3001/ws")
        errors: list[Any] = []
        session.on("error", errors.append)
        details = {
            "provider": "openai",
            "model": "gpt-live-transcribe",
            "reason": "client_not_implemented",
            "streaming_alternatives": ["gpt-4o-transcribe"],
        }

        queued = await _feed(
            session,
            {
                "type": "error",
                "message": "stt_live_unsupported: no live path for openai/gpt-live-transcribe",
                "code": "stt_live_unsupported",
                "recoverable": True,
                "details": details,
            },
        )

        assert len(errors) == 1
        err = errors[0]
        assert isinstance(err, GatewayError)
        assert isinstance(err, BudError)
        assert err.code == "stt_live_unsupported"
        assert err.recoverable is True
        assert err.details == details
        assert err.message.startswith("stt_live_unsupported: ")
        assert queued == [{"type": "error", "error": err}]

    @pytest.mark.asyncio
    async def test_uncoded_error_keeps_its_old_shape(self) -> None:
        session = WebSocketSession(url="ws://localhost:3001/ws")
        errors: list[Any] = []
        session.on("error", errors.append)

        await _feed(session, {"type": "error", "message": "Provider failed"})

        err = errors[0]
        assert isinstance(err, GatewayError)
        assert isinstance(err, BudError)
        assert err.code is None
        assert err.message == "Provider failed"
        assert err.recoverable is False
        assert err.details is None
        assert str(err) == "Provider failed"


# =============================================================================
# Pipelines: TalkSession / STTSession
# =============================================================================


class TestTalkSession:
    def test_stt_warning_callback_is_typed_and_not_an_error(self) -> None:
        talk = TalkSession(url="ws://localhost:3001/ws")
        warnings: list[Any] = []
        events: list[TalkEvent] = []
        errors: list[Any] = []
        talk.on("stt_warning", warnings.append)
        talk.on("event", events.append)
        talk.on("error", errors.append)
        frame = {
            "type": "stt_warning",
            "code": "stt_audio_dropped",
            "message": "dropped",
            "detail": {"bytes": 3200},
        }

        talk._on_stt_warning(frame)

        assert isinstance(warnings[0], SttWarning)
        assert warnings[0].code == "stt_audio_dropped"
        assert events[0].type == "stt_warning"
        assert events[0].stt_warning is warnings[0]
        assert events[0].data == frame
        assert errors == []

    def test_vad_event_carries_a_typed_view(self) -> None:
        talk = TalkSession(url="ws://localhost:3001/ws")
        raw: list[Any] = []
        events: list[TalkEvent] = []
        talk.on("vad_event", raw.append)
        talk.on("event", events.append)
        frame = {"type": "vad_event", "event": "turn_start", "turn_id": 2, "sustained_ms": 500}

        talk._on_vad_event(frame)

        # The named callback keeps its raw-dict payload; the unified event adds the typed view.
        assert raw == [frame]
        assert events[0].type == "vad_event"
        assert events[0].data == frame
        assert isinstance(events[0].vad, VadEvent)
        assert events[0].vad.turn_id == 2

    @pytest.mark.asyncio
    async def test_iterator_yields_stt_warning_and_typed_vad_event(self) -> None:
        talk = TalkSession(url="ws://localhost:3001/ws")
        warning = {
            "type": "stt_warning",
            "code": "stt_rate_limited",
            "message": "slow down",
            "detail": {"scope": "key"},
        }
        vad = {"type": "vad_event", "event": "turn_closed", "turn_id": 9, "had_transcript": True}

        class _FakeWsSession:
            def __aiter__(self) -> AsyncIterator[dict[str, Any]]:
                async def gen() -> AsyncIterator[dict[str, Any]]:
                    yield {"type": "stt_warning", "data": warning}
                    yield {"type": "vad_event", "data": vad}

                return gen()

        talk._session = _FakeWsSession()  # type: ignore[assignment]
        collected = [ev async for ev in talk]

        assert [e.type for e in collected] == ["stt_warning", "vad_event"]
        assert collected[0].stt_warning is not None
        assert collected[0].stt_warning.code == "stt_rate_limited"
        assert collected[1].vad is not None
        assert collected[1].vad.had_transcript is True

    def test_exposes_ready_stt(self) -> None:
        talk = TalkSession(url="ws://localhost:3001/ws")
        assert talk.stt is None
        talk._session._stt = ReadySTT.from_wire(READY_STT)
        assert talk.stt is not None
        assert talk.stt.transcription_mode == "segmented"


class TestSTTSession:
    def test_exposes_ready_stt(self) -> None:
        stt_session = STTSession(
            url="ws://localhost:3001/ws", config=STTConfig(provider="elevenlabs")
        )
        assert stt_session.stt is None
        stt_session._session._stt = ReadySTT.from_wire(READY_STT)
        assert stt_session.stt is not None
        assert stt_session.stt.interim_results == "per_segment"
