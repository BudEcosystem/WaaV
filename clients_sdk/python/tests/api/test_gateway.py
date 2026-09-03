"""Test gateway connectivity.

Targets ``WAAV_GATEWAY_URL`` (default ``ws://127.0.0.1:3009``) so a developer
can point the suite at any gateway. These are LIVE tests: when no gateway is
reachable they SKIP (instead of failing), so ``pytest -k "not e2e"`` stays green
on a machine with no gateway running.
"""

import os
import pytest
import httpx
import asyncio
import websockets
import json

# Env-configurable gateway URL (plan P0 item 5). Default to the live port 3009.
WS_URL = os.environ.get("WAAV_GATEWAY_URL", "ws://127.0.0.1:3009").rstrip("/")
BASE_URL = WS_URL.replace("ws://", "http://").replace("wss://", "https://")


_gateway_up: bool | None = None


def _check_gateway() -> bool:
    """Cached check that a gateway is reachable (200 or 429 == running)."""
    global _gateway_up
    if _gateway_up is not None:
        return _gateway_up
    try:
        resp = httpx.get(f"{BASE_URL}/livez", timeout=3.0)
        _gateway_up = resp.status_code in (200, 429)
    except Exception:
        _gateway_up = False
    return _gateway_up


skip_no_gateway = pytest.mark.skipif(
    not _check_gateway(), reason=f"Gateway not reachable at {BASE_URL}"
)


@skip_no_gateway
class TestGatewayHealth:
    """Test basic gateway health and REST endpoints."""

    def test_health_check(self):
        """Test that health endpoint responds."""
        response = httpx.get(f"{BASE_URL}/", timeout=10.0)
        if response.status_code == 429:
            pytest.skip("Rate limited")
        assert response.status_code == 200
        data = response.json()
        # Gateway returns lowercase "ok" (messages.rs); accept any case.
        assert (data.get("status") or "").lower() == "ok"

    def test_voices_endpoint(self):
        """Test voices endpoint returns data."""
        response = httpx.get(f"{BASE_URL}/voices", timeout=30.0)
        if response.status_code == 429:
            pytest.skip("Rate limited")
        assert response.status_code == 200
        data = response.json()
        assert data is not None

    def test_speak_validation(self):
        """Test speak endpoint validates empty text."""
        response = httpx.post(
            f"{BASE_URL}/speak",
            json={
                "text": "",
                "tts_config": {
                    "provider": "deepgram",
                    "model": "aura-asteria-en",
                    "voice_id": "aura-asteria-en",
                    "audio_format": "linear16",
                    "sample_rate": 24000,
                },
            },
            timeout=10.0,
        )
        if response.status_code == 429:
            pytest.skip("Rate limited")
        # Should reject empty text with 400
        assert response.status_code == 400


@skip_no_gateway
class TestWebSocketEndpoints:
    """Test WebSocket endpoints connectivity."""

    @pytest.mark.asyncio
    async def test_ws_connection(self):
        """Test that /ws endpoint accepts connections."""
        try:
            async with websockets.connect(
                f"{WS_URL}/ws",
                open_timeout=5,
                close_timeout=5,
            ) as ws:
                # Send config
                await ws.send(json.dumps({
                    "type": "config",
                    "audio": True,
                    "stt_config": {
                        "provider": "deepgram",
                        "language": "en-US",
                        "sample_rate": 16000,
                        "channels": 1,
                        "punctuation": True,
                        "encoding": "linear16",
                        "model": "nova-3"
                    },
                    "tts_config": {
                        "provider": "deepgram",
                        "model": "aura-asteria-en",
                        "voice_id": "aura-asteria-en",
                        "audio_format": "linear16",
                        "sample_rate": 24000
                    }
                }))

                # Wait for response
                try:
                    response = await asyncio.wait_for(ws.recv(), timeout=5.0)
                    msg = json.loads(response)
                    # Should get 'ready' or 'error' (if API key missing)
                    assert msg.get("type") in ["ready", "error"]
                    print(f"/ws response: {msg.get('type')} - {msg.get('message', msg.get('stream_id', ''))}")
                except asyncio.TimeoutError:
                    pytest.fail("Timeout waiting for WebSocket response")
        except Exception as e:
            # If rate limited, that's OK - the endpoint exists
            if "429" in str(e):
                pytest.skip("Rate limited")
            raise

    @pytest.mark.asyncio
    async def test_ws_openai_provider(self):
        """Test that OpenAI provider is recognized."""
        try:
            async with websockets.connect(
                f"{WS_URL}/ws",
                open_timeout=5,
                close_timeout=5,
            ) as ws:
                await ws.send(json.dumps({
                    "type": "config",
                    "audio": True,
                    "stt_config": {
                        "provider": "openai",
                        "language": "en",
                        "sample_rate": 16000,
                        "channels": 1,
                        "punctuation": True,
                        "encoding": "linear16",
                        "model": "whisper-1"
                    },
                    "tts_config": {
                        "provider": "openai",
                        "model": "tts-1",
                        "voice_id": "alloy",
                        "audio_format": "linear16",
                        "sample_rate": 24000
                    }
                }))

                try:
                    response = await asyncio.wait_for(ws.recv(), timeout=5.0)
                    msg = json.loads(response)
                    assert msg.get("type") in ["ready", "error"]
                    print(f"OpenAI provider response: {msg.get('type')} - {msg.get('message', msg.get('stream_id', ''))}")

                    # If error, should NOT say "unsupported provider"
                    if msg.get("type") == "error":
                        err_msg = (msg.get("message") or "").lower()
                        assert "unsupported provider" not in err_msg
                        assert "unknown provider" not in err_msg
                except asyncio.TimeoutError:
                    pytest.fail("Timeout waiting for WebSocket response")
        except Exception as e:
            if "429" in str(e):
                pytest.skip("Rate limited")
            raise

    @pytest.mark.asyncio
    async def test_realtime_endpoint(self):
        """Test that /realtime endpoint exists."""
        try:
            async with websockets.connect(
                f"{WS_URL}/realtime",
                open_timeout=5,
                close_timeout=5,
            ) as ws:
                await ws.send(json.dumps({
                    "type": "config",
                    "provider": "openai",
                    "model": "gpt-4o-realtime-preview",
                    "voice": "alloy",
                    "instructions": "You are a helpful assistant."
                }))

                try:
                    response = await asyncio.wait_for(ws.recv(), timeout=5.0)
                    msg = json.loads(response)
                    # Any response type is OK - means endpoint exists
                    print(f"/realtime response: {msg.get('type')} - {msg.get('message', '')}")
                    assert msg.get("type") in ["session_created", "error", "closing"]
                except asyncio.TimeoutError:
                    # Timeout means endpoint exists but maybe waiting for API key
                    pass
        except websockets.exceptions.InvalidStatusCode as e:
            # 404 means route not registered
            if e.status_code == 404:
                pytest.fail("Realtime endpoint /realtime is not registered (404)")
            elif e.status_code == 429:
                pytest.skip("Rate limited")
            else:
                print(f"/realtime got status {e.status_code}")
        except Exception as e:
            if "429" in str(e):
                pytest.skip("Rate limited")
            raise
