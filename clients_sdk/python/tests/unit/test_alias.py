"""P3 proxy/alias model-name SDK surface.

A developer names a server-defined alias (``bud.agent(alias="support-bot")``); the
gateway resolves it to a full {stt,tts,llm,dag} bundle and echoes the concrete
providers back on ``ready`` as ``resolved_alias``. Re-pointing the alias server-side
is a zero-client-change operation (proven live in the gateway P3 commit).
"""

import inspect

from bud_waav import BudClient
from bud_waav.ws.session import WebSocketSession


def test_alias_is_serialized_top_level() -> None:
    s = WebSocketSession(url="ws://x", alias="support-bot", audio=False)
    assert s.alias == "support-bot"
    # The wire envelope carries `alias` as a single top-level field (resolved
    # server-side BEFORE provider construction).
    src = inspect.getsource(s._send_config)
    assert 'config["alias"] = self.alias' in src


def test_no_alias_field_when_unset() -> None:
    s = WebSocketSession(url="ws://x", audio=False)
    assert s.alias is None


def test_resolved_alias_echo_property() -> None:
    s = WebSocketSession(url="ws://x", audio=False)
    assert s.resolved_alias is None
    # Populated from the ready ack so a dev SEES what the alias became.
    s._resolved_alias = {"name": "support-bot", "tts": {"provider": "cartesia"}}
    assert s.resolved_alias["tts"]["provider"] == "cartesia"


def test_bud_agent_threads_alias_to_session() -> None:
    c = BudClient(base_url="http://127.0.0.1:3009")
    sess = c.agent(
        stt={"provider": "deepgram"},
        tts={"provider": "deepgram"},
        llm={"base_url": "http://x/v1", "model": "m"},
        alias="support-bot",
    )
    assert sess._session.alias == "support-bot"


def test_alias_composes_with_explicit_override() -> None:
    # alias + an explicit stt/tts in the SAME call: both reach the wire; the
    # gateway's client-wins merge lets the explicit field override the alias default.
    c = BudClient(base_url="http://127.0.0.1:3009")
    sess = c.agent(
        stt={"provider": "openai"},
        tts={"provider": "deepgram"},
        llm={"base_url": "http://x/v1", "model": "m"},
        alias="support-bot",
    )
    assert sess._session.alias == "support-bot"
    assert sess._session.stt_config is not None
    assert sess._session.stt_config.provider == "openai"
