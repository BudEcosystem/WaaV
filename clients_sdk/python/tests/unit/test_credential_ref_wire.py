"""Operator credential refs, the LLM timeout and the gateway denoiser reach the config wire.

``credential_ref`` names a key the operator holds on the gateway, so a client authenticates its
LLM without carrying the key; it takes precedence over ``api_key`` there. Each field is sent only
when set, so an unset one keeps the gateway's default.
"""

from __future__ import annotations

import json
from typing import Any
from unittest.mock import MagicMock

import pytest

from bud_waav import ConversationConfig, STTConfig
from bud_waav.ws.session import WebSocketSession


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


@pytest.mark.asyncio
async def test_conversation_credential_refs_and_timeout_are_sent() -> None:
    session = WebSocketSession(
        url="ws://x/ws",
        audio=False,
        conversation_config=ConversationConfig(
            base_url="https://llm.internal/v1",
            model="m",
            credential_ref="LLM_KEY",
            reasoning_credential_ref="REASONING_KEY",
            llm_timeout_ms=120000,
        ),
    )
    conv = (await _capture_config(session))["conversation_config"]
    assert conv["credential_ref"] == "LLM_KEY"
    assert conv["reasoning_credential_ref"] == "REASONING_KEY"
    assert conv["llm_timeout_ms"] == 120000
    assert "api_key" not in conv


@pytest.mark.asyncio
async def test_unset_fields_are_omitted() -> None:
    session = WebSocketSession(
        url="ws://x/ws",
        stt_config=STTConfig(provider="deepgram"),
        conversation_config=ConversationConfig(base_url="https://llm/v1", model="m"),
    )
    wire = await _capture_config(session)
    for key in ("credential_ref", "reasoning_credential_ref", "llm_timeout_ms"):
        assert key not in wire["conversation_config"]
    assert "noise_suppression" not in wire["stt_config"]


@pytest.mark.asyncio
@pytest.mark.parametrize("on", [True, False])
async def test_noise_suppression_is_sent_either_way(on: bool) -> None:
    session = WebSocketSession(
        url="ws://x/ws", stt_config=STTConfig(provider="deepgram", noise_suppression=on)
    )
    assert (await _capture_config(session))["stt_config"]["noise_suppression"] is on
