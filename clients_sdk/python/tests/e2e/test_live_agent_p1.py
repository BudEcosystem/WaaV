"""Live P1 e2e: bud.agent() acceptance + config_warning surfacing (credential-free).

These tests run against a live gateway (default ``WAAV_GATEWAY_URL`` ws://127.0.0.1:3009;
the P1 validation used a spare snapshot on :3084). They are credential-free: they do
NOT require a vendor STT/TTS key. What they prove:

  * the gateway emits a typed ``config_warning`` (code ``unknown_config_keys``) for a
    bogus/misnested key, and the SDK surfaces it through its own receive loop as a
    typed warning — the P1 #3 "silent no-op killer";
  * the full ``bud.agent(...)`` config (conversation_config reasoning stack + nested
    turn_detection + eager pairing) is ACCEPTED by the gateway (it reaches STT/TTS
    provider construction; the ONLY thing left is a real vendor credential), proving
    the flagship loop reaches the wire (P1 #2);
  * a CORRECTLY-nested agent config lints CLEAN (zero false-positive warnings),
    proving the SDK's wire output matches the gateway schema exactly.

Run:  WAAV_GATEWAY_URL=ws://127.0.0.1:3084 pytest tests/e2e/test_live_agent_p1.py -v -s
Skip: pytest -k "not e2e"
"""

import asyncio
import json
import os

import pytest
import websockets

from bud_waav import BudClient, ConfigWarning
from bud_waav.ws.session import WebSocketSession

pytestmark = pytest.mark.e2e

_WS_BASE = os.environ.get("WAAV_GATEWAY_URL", "ws://127.0.0.1:3009").rstrip("/")
_WS_URL = f"{_WS_BASE}/ws"
_HTTP_BASE = _WS_BASE.replace("ws://", "http://").replace("wss://", "https://")
_OLLAMA = os.environ.get("WAAV_OLLAMA_URL", "http://127.0.0.1:11434")
_DUMMY_KEY = "dummy-construction-key"


async def _gateway_up() -> bool:
    import httpx
    try:
        async with httpx.AsyncClient(timeout=2.0) as c:
            r = await c.get(f"{_HTTP_BASE}/livez")
            return r.status_code == 200
    except Exception:
        return False


def _ollama_model() -> str:
    import urllib.request
    try:
        with urllib.request.urlopen(f"{_OLLAMA}/api/tags", timeout=3) as r:
            models = [m["name"] for m in json.load(r)["models"]]
        return "llama3.2:1b" if "llama3.2:1b" in models else models[0]
    except Exception:
        return "llama3.2:1b"


async def _raw_frames(payload: dict, n: int = 10, t: float = 2.5) -> list:
    ws = await websockets.connect(_WS_URL)
    await ws.send(json.dumps(payload))
    out = []
    try:
        for _ in range(n):
            m = await asyncio.wait_for(ws.recv(), timeout=t)
            if isinstance(m, bytes):
                continue
            out.append(json.loads(m))
    except (asyncio.TimeoutError, websockets.ConnectionClosed):
        pass
    try:
        await ws.close()
    except Exception:
        pass
    return out


@pytest.mark.asyncio
async def test_live_bogus_key_surfaces_typed_config_warning():
    """A bogus + misnested key → typed config_warning surfaced via the SDK loop."""
    if not await _gateway_up():
        pytest.skip(f"no gateway at {_HTTP_BASE}")

    sess = WebSocketSession(url=_WS_URL, audio=False)
    warnings: list = []
    sess.on("config_warning", lambda d: warnings.append(d))

    ws = await websockets.connect(_WS_URL)
    sess._ws = ws
    sess._connected = True
    sess._receive_task = asyncio.create_task(sess._receive_loop())
    await ws.send(json.dumps({
        "type": "config", "audio": False,
        "stt_config": {"provider": "deepgram", "language": "en-US", "sample_rate": 16000,
                       "channels": 1, "punctuation": True},
        "tts_config": {"provider": "deepgram", "model": "aura-asteria-en", "voice_id": "aura-asteria-en"},
        "bogus_top_level_key": 123,
        "reasoning_effort": "minimal",  # misnested (belongs in conversation_config)
    }))
    try:
        await asyncio.wait_for(sess._get_ready_event().wait(), timeout=4.0)
    except asyncio.TimeoutError:
        pass
    await asyncio.sleep(0.6)
    await sess.disconnect()

    # Reached ready with the pinned protocol version.
    assert sess.stream_id is not None
    assert sess.protocol_version == "1.0"
    # The advisory surfaced as a typed warning (NOT a silent no-op).
    assert len(warnings) >= 1
    cw = ConfigWarning.from_wire(warnings[0])
    assert cw.code == "unknown_config_keys"
    assert "bogus_top_level_key" in cw.detail["ignored_keys"]
    assert "reasoning_effort" in cw.detail["ignored_keys"]


@pytest.mark.asyncio
async def test_live_correct_agent_nesting_lints_clean():
    """The exact wire shape bud.agent() emits must NOT trip the unknown-keys lint."""
    if not await _gateway_up():
        pytest.skip(f"no gateway at {_HTTP_BASE}")

    frames = await _raw_frames({
        "type": "config", "audio": True,
        "stt_config": {"provider": "deepgram", "language": "en-US", "sample_rate": 16000,
                       "channels": 1, "punctuation": True, "api_key": _DUMMY_KEY,
                       "features": {"diarization": True, "keyterms": ["WaaV"]},
                       "turn_detection": {"enabled": True, "eager": True}},
        "tts_config": {"provider": "deepgram", "model": "aura-asteria-en",
                       "voice_id": "aura-asteria-en", "api_key": _DUMMY_KEY},
        "conversation_config": {
            "base_url": f"{_OLLAMA}/v1", "model": "llama3.2:1b",
            "reasoning_effort": "minimal", "latency_filler": "auto", "eager_eot": True,
            "barge_in_min_words": 3, "mute_strategy": "until_first_bot_complete",
        },
    })
    warns = [f for f in frames if f.get("type") == "config_warning"]
    assert warns == [], f"correct agent nesting must lint clean, got: {warns}"


@pytest.mark.asyncio
async def test_live_agent_config_accepted_by_gateway():
    """bud.agent() config is accepted: it reaches provider construction (dummy key).

    Credential-free boundary: with a dummy STT key the gateway proceeds past the
    full conversation_config + turn_detection validation all the way to the
    Deepgram WS connect, which fails 401 — proving the loop is wired and the ONLY
    blocker to bot audio is a real vendor credential (no schema 400).
    """
    if not await _gateway_up():
        pytest.skip(f"no gateway at {_HTTP_BASE}")

    model = _ollama_model()
    bud = BudClient(base_url=_HTTP_BASE, api_key=_DUMMY_KEY)
    session = bud.agent(
        stt={"provider": "deepgram", "language": "en-US", "model": "nova-3"},
        tts={"provider": "deepgram", "voice_id": "aura-asteria-en", "model": "aura-asteria-en"},
        llm={"base_url": f"{_OLLAMA}/v1", "model": model,
             "reasoning_effort": "minimal", "latency_filler": "auto",
             "barge_in_min_words": 3, "mute_strategy": "until_first_bot_complete"},
        turn={"threshold": 0.6, "eager": True},
    )
    errors: list = []
    session.on("error", lambda e: errors.append(str(e)))
    reached_ready = False
    try:
        await session.connect(timeout=12.0)
        reached_ready = session.connected
        await asyncio.sleep(0.5)
    except Exception as e:
        errors.append(f"{type(e).__name__}: {e}")
    finally:
        try:
            await session.disconnect()
        except Exception:
            pass

    if reached_ready:
        # If a real key were present the loop would be fully live.
        assert session.stream_id is not None
    else:
        # The config was accepted; the sole failure is the vendor credential at
        # the provider connect (401/auth/connect) — NOT a schema rejection.
        joined = " ".join(errors).lower()
        assert errors, "expected a provider-credential error in the credential-free path"
        assert any(
            tok in joined for tok in ("401", "unauthorized", "api key", "auth",
                                      "connect", "deepgram", "voice manager")
        ), f"unexpected (non-credential) failure: {errors}"
