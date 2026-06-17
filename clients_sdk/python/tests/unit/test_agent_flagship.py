"""bud.agent() flagship + config_warning surfacing (SDK_STANDARDIZATION_PLAN P1 #2/#3).

Covers, without a live gateway (the live round-trip is in tests/e2e):
* bud.agent(...) serializes conversation_config + nests turn_detection, applies
  sane defaults (allow_interruption, strip_markdown, latency_filler), and pairs
  eager EoT across stt_config.turn_detection.eager + conversation_config.eager_eot;
* config_warning frames surface as a typed ``warning`` event in the unified stream
  (both the on()-callback path and the __aiter__ path) and are NOT swallowed.
"""

import json
from unittest.mock import MagicMock

import pytest

from bud_waav import BudClient, ConfigWarning, ConversationConfig
from bud_waav.pipelines.talk import TalkSession, TalkEvent
from bud_waav.ws.session import WebSocketSession


async def _capture_config(session: WebSocketSession) -> dict:
    sent = []

    async def mock_send(data):
        sent.append(data)

    session._ws = MagicMock()
    session._ws.send = mock_send
    await session._send_config()
    assert len(sent) == 1
    return json.loads(sent[0])


class TestAgentSerialization:
    """bud.agent() must reach the gateway conversation loop with sane defaults."""

    def _client(self) -> BudClient:
        return BudClient(base_url="http://localhost:3084", api_key=None)

    @pytest.mark.asyncio
    async def test_agent_serializes_conversation_and_turn(self):
        bud = self._client()
        session = bud.agent(
            stt={"provider": "deepgram", "language": "en-US"},
            tts={"provider": "deepgram", "voice_id": "aura-asteria-en"},
            llm={
                "base_url": "http://127.0.0.1:11434/v1",
                "model": "llama3.2:1b",
                "reasoning_effort": "minimal",
                "latency_filler": "auto",
            },
            turn={"threshold": 0.6, "eager": True},
        )
        config = await _capture_config(session._session)

        # conversation_config reaches the wire.
        cc = config["conversation_config"]
        assert cc["base_url"] == "http://127.0.0.1:11434/v1"
        assert cc["model"] == "llama3.2:1b"
        assert cc["reasoning_effort"] == "minimal"
        assert cc["latency_filler"] == "auto"
        # Sane defaults applied by agent().
        assert cc["allow_interruption"] is True
        assert cc["strip_markdown"] is True
        # eager pairing: BOTH the conversation flag and the detector flag are set.
        assert cc["eager_eot"] is True
        td = config["stt_config"]["turn_detection"]
        assert td["enabled"] is True
        assert td["threshold"] == 0.6
        assert td["eager"] is True
        # turn_detection lives nested, never at the top level.
        assert "turn_detection" not in config

    @pytest.mark.asyncio
    async def test_agent_interrupt_false_disables_barge_in(self):
        bud = self._client()
        session = bud.agent(
            stt={"provider": "deepgram"},
            llm={"base_url": "http://x/v1", "model": "m"},
            interrupt=False,
        )
        cc = (await _capture_config(session._session))["conversation_config"]
        assert cc["allow_interruption"] is False

    @pytest.mark.asyncio
    async def test_agent_explicit_llm_value_wins_over_default(self):
        # An explicit latency_filler/strip_markdown on llm must NOT be overwritten.
        bud = self._client()
        session = bud.agent(
            stt={"provider": "deepgram"},
            llm=ConversationConfig(
                base_url="http://x/v1", model="m",
                latency_filler="off", strip_markdown=False, allow_interruption=False,
            ),
        )
        cc = (await _capture_config(session._session))["conversation_config"]
        assert cc["latency_filler"] == "off"
        assert cc["strip_markdown"] is False
        assert cc["allow_interruption"] is False

    @pytest.mark.asyncio
    async def test_agent_eager_alias_eager_eot(self):
        # turn={"eager_eot": True} is accepted as the alias of turn={"eager": True}.
        bud = self._client()
        session = bud.agent(
            stt={"provider": "deepgram"},
            llm={"base_url": "http://x/v1", "model": "m"},
            turn={"eager_eot": True},
        )
        config = await _capture_config(session._session)
        assert config["conversation_config"]["eager_eot"] is True
        assert config["stt_config"]["turn_detection"]["eager"] is True

    def test_agent_returns_unconnected_talk_session(self):
        bud = self._client()
        session = bud.agent(stt={"provider": "deepgram"})
        assert isinstance(session, TalkSession)
        assert session.connected is False


class TestConfigWarningSurfacing:
    """A gateway config_warning must surface as a typed warning, never swallowed."""

    def test_config_warning_dataclass_from_wire(self):
        cw = ConfigWarning.from_wire({
            "type": "config_warning",
            "code": "unknown_config_keys",
            "message": "Ignored unknown keys: bogus_key (did you mean to nest it?)",
            "detail": {"ignored_keys": ["bogus_key"]},
        })
        assert cw.code == "unknown_config_keys"
        assert "bogus_key" in cw.message
        assert cw.detail == {"ignored_keys": ["bogus_key"]}

    def test_config_warning_optional_detail(self):
        cw = ConfigWarning.from_wire({"code": "reasoning_model_on_voice_path", "message": "m"})
        assert cw.detail is None

    @pytest.mark.asyncio
    async def test_session_emits_config_warning_event(self):
        # The raw session surfaces config_warning on its own event + queue.
        session = WebSocketSession(url="ws://localhost:3084/ws")
        got = []
        session.on("config_warning", lambda d: got.append(d))

        wire = json.dumps({
            "type": "config_warning",
            "code": "emotion_ignored_for_provider",
            "message": "emotion=excited ignored by deepgram",
            "detail": {"provider": "deepgram", "emotion": "excited"},
        })

        async def one_message():
            yield wire

        session._ws = one_message()
        session._connected = True
        await session._receive_loop()

        assert len(got) == 1
        assert got[0]["code"] == "emotion_ignored_for_provider"

    def test_talk_session_routes_warning_callback(self):
        session = TalkSession(url="ws://localhost:3084/ws")
        warnings = []
        events = []
        session.on("warning", lambda w: warnings.append(w))
        session.on("event", lambda e: events.append(e))

        session._on_config_warning({
            "type": "config_warning",
            "code": "reasoning_model_on_voice_path",
            "message": "you placed a reasoning model on the spoken path",
            "detail": {"measured_ttft_ms": 5905},
        })

        assert len(warnings) == 1
        assert isinstance(warnings[0], ConfigWarning)
        assert warnings[0].code == "reasoning_model_on_voice_path"
        assert warnings[0].detail["measured_ttft_ms"] == 5905
        # Also flows through the unified `event` channel as type=="warning".
        assert len(events) == 1
        assert events[0].type == "warning"
        assert events[0].warning.code == "reasoning_model_on_voice_path"

    @pytest.mark.asyncio
    async def test_talk_iter_yields_warning_and_bot_text(self):
        # Feed config_warning + a user transcript + an assistant-role transcript
        # through the queue and assert the unified iterator yields
        # warning/transcript/bot_text (the warning is NOT swallowed; an
        # assistant-role transcript is routed to bot_text).
        from bud_waav import STTResult

        session = TalkSession(url="ws://localhost:3084/ws")

        class _FakeWsSession:
            """Stand-in whose async iteration replays a fixed queue payload list.

            ``async for x in obj`` resolves ``__aiter__`` on the TYPE, so the fake
            must define it on the class (an instance attribute won't bind).
            """

            def __aiter__(self):
                async def gen():
                    yield {
                        "type": "config_warning",
                        "data": {
                            "code": "unknown_config_keys",
                            "message": "x",
                            "detail": {"ignored_keys": ["z"]},
                        },
                    }
                    yield {
                        "type": "transcript",
                        "result": STTResult(text="user said hi", is_final=True),
                        "role": "user",
                    }
                    yield {
                        "type": "transcript",
                        "result": STTResult(text="bot reply", is_final=True),
                        "role": "assistant",
                    }

                return gen()

        session._session = _FakeWsSession()  # type: ignore[assignment]

        collected: list[TalkEvent] = []
        async for ev in session:
            collected.append(ev)

        types = [e.type for e in collected]
        assert types == ["warning", "transcript", "bot_text"]
        assert collected[0].warning.code == "unknown_config_keys"
        assert collected[1].text == "user said hi"
        assert collected[2].type == "bot_text"
        assert collected[2].text == "bot reply"
