"""Voice agents on ``/ws`` (spec 025) and a conversation block without ``base_url``.

``{"type": "config", "agent": {"id": "support"}}`` names a Bud voice agent: the agent decides both
speech legs, so the session must not invent them. The gateway refuses ``stt_config.model`` and
``tts_config.model`` on an agent session (``agent_owns_legs``), which the default legs and the
``nova-3`` model default used to send.

Under the Bud control plane the conversation loop's LLM is a Bud deployment and the gateway refuses
a ``base_url``; the gateway schema requires only ``model``.
"""

from typing import Any

import pytest

from bud_waav import ConversationConfig, STTConfig, TTSConfig, VoiceAgentConfig
from bud_waav.ws.session import WebSocketSession


async def sent_config(session: WebSocketSession) -> dict[str, Any]:
    sent: list[dict[str, Any]] = []

    async def capture(data: dict[str, Any]) -> None:
        sent.append(data)

    session._send_json = capture  # type: ignore[method-assign]
    await session._send_config()
    assert len(sent) == 1
    return sent[0]


async def test_an_agent_is_sent_with_only_the_fields_set() -> None:
    cfg = await sent_config(
        WebSocketSession(
            url="ws://x",
            agent=VoiceAgentConfig(id="support", version=3, variables={"tier": "gold"}),
        )
    )
    assert cfg["agent"] == {"id": "support", "version": 3, "variables": {"tier": "gold"}}
    cfg = await sent_config(
        WebSocketSession(url="ws://x", agent=VoiceAgentConfig(id="support", text_only=True))
    )
    assert cfg["agent"] == {"id": "support", "text_only": True}


async def test_an_agent_name_alone_is_enough() -> None:
    cfg = await sent_config(WebSocketSession(url="ws://x", agent="support"))
    assert cfg["agent"] == {"id": "support"}


async def test_an_agent_session_invents_no_speech_legs() -> None:
    cfg = await sent_config(WebSocketSession(url="ws://x", agent="support"))
    assert "stt_config" not in cfg and "tts_config" not in cfg, cfg


async def test_an_agent_session_sends_the_audio_format_but_never_a_model() -> None:
    cfg = await sent_config(
        WebSocketSession(
            url="ws://x",
            agent="support",
            stt_config=STTConfig(sample_rate=8000),
            tts_config=TTSConfig(sample_rate=16000),
        )
    )
    assert cfg["stt_config"]["sample_rate"] == 8000
    assert "model" not in cfg["stt_config"], cfg["stt_config"]
    assert "model" not in cfg["tts_config"], cfg["tts_config"]


async def test_without_an_agent_the_legs_are_as_before() -> None:
    cfg = await sent_config(WebSocketSession(url="ws://x"))
    assert "agent" not in cfg
    assert cfg["stt_config"]["model"] == "nova-3"
    assert "tts_config" in cfg


def test_an_agent_needs_a_name() -> None:
    with pytest.raises(ValueError):
        VoiceAgentConfig(id="")


async def test_a_conversation_may_leave_the_base_url_to_the_gateway() -> None:
    cfg = await sent_config(
        WebSocketSession(
            url="ws://x", audio=False, conversation_config=ConversationConfig(model="chat")
        )
    )
    assert cfg["conversation_config"] == {"model": "chat"}
    cfg = await sent_config(
        WebSocketSession(
            url="ws://x",
            audio=False,
            conversation_config=ConversationConfig(base_url="https://llm/v1", model="chat"),
        )
    )
    assert cfg["conversation_config"]["base_url"] == "https://llm/v1"


def test_a_talk_session_threads_the_agent_to_its_socket() -> None:
    from bud_waav import BudClient

    talk = BudClient(base_url="http://127.0.0.1:3009").talk.create(agent="support")
    assert talk._session.agent is not None and talk._session.agent.id == "support"
