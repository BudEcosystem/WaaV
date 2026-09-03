"""
Tier-2 SDK perf & resilience tests (PERF_RESILIENCE_PLAN D6/D7), Python SDK.

Covers the connect-time + pooling hardening:

* D6 CONNECT-TIME: the connect timeout default is RAISED 10s -> 35s (server-side
  PROVIDER_READY can take ~30s for an audio=true session) and that default
  propagates through every pipeline wrapper; BudClient.prepare_connection()/
  prewarm() warms DNS/TLS; STT keepalive-silence emits a gated ~100ms zero-PCM
  frame on an idle cadence and never fights real audio.
* D7 POOLING (REST): the httpx.AsyncClient is built with tuned httpx.Limits and
  REUSED across calls (one warm pool, not per-call); a transient 429/503 backs
  off (honoring Retry-After) instead of being fatal, and after the retries it
  surfaces a typed RateLimitError; the WS connect path classifies 503 as a
  retryable RateLimitError too.

Pure-unit: no live gateway. The WS socket is faked (reusing the Tier-1 FakeWS
shape) and the REST transport is an httpx.MockTransport so the REAL _request
retry loop + REAL httpx.Limits are exercised.
"""

import asyncio
import inspect
import json
import time

import httpx
import pytest
import websockets
from websockets.datastructures import Headers
from websockets.exceptions import InvalidStatus
from websockets.http11 import Response

from bud_waav.client import BudClient
from bud_waav.errors import APIError, RateLimitError
from bud_waav.errors import ConnectionError as BudConnectionError
from bud_waav.pipelines.realtime_gateway import GatewayRealtime
from bud_waav.pipelines.stt import STTSession
from bud_waav.pipelines.talk import TalkSession
from bud_waav.pipelines.tts import TTSSession
from bud_waav.rest.client import (
    DEFAULT_KEEPALIVE_EXPIRY,
    DEFAULT_MAX_KEEPALIVE_CONNECTIONS,
    RestClient,
)
from bud_waav.types import STTConfig
from bud_waav.ws.session import (
    DEFAULT_CONNECT_TIMEOUT,
    DEFAULT_STT_KEEPALIVE_INTERVAL,
    ReconnectConfig,
    WebSocketSession,
)

# ---------------------------------------------------------------------------
# Fakes (mirrors tests/unit/test_tier1_resilience.py FakeWS)
# ---------------------------------------------------------------------------


def _closed_exc() -> websockets.ConnectionClosed:
    return websockets.ConnectionClosed(None, None)


class FakeWS:
    """Minimal stand-in for a websockets ClientConnection (see Tier-1 tests)."""

    def __init__(self, inbound=None):
        self.sent: list = []
        self._inbound = list(inbound or [])
        self.closed = False
        self.close_calls = 0
        self.ping_calls = 0
        self.send_should_raise = False
        self._closed_evt = asyncio.Event()

    async def send(self, data):
        if self.send_should_raise:
            raise _closed_exc()
        self.sent.append(data)

    async def ping(self, data=None):
        self.ping_calls += 1
        fut = asyncio.get_event_loop().create_future()
        fut.set_result(True)
        return fut

    async def close(self, code=1000, reason=""):
        self.close_calls += 1
        self.closed = True
        self._closed_evt.set()

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._inbound:
            return self._inbound.pop(0)
        await self._closed_evt.wait()
        raise _closed_exc()


def make_ready_session(**kwargs) -> WebSocketSession:
    """A session whose _open_ws yields a FakeWS that immediately replies ready."""
    ready = json.dumps({"type": "ready", "stream_id": "s-1", "protocol_version": "1.0"})
    session = WebSocketSession(
        url="ws://localhost:3009/ws",
        reconnect=ReconnectConfig(enabled=False),
        **kwargs,
    )

    async def _fake_open(timeout):
        # Record the timeout connect() chose so a test can assert the default.
        session._last_open_timeout = timeout  # type: ignore[attr-defined]
        return FakeWS(inbound=[ready])

    session._open_ws = _fake_open  # type: ignore[assignment]
    return session


def make_rest_client(handler, **kwargs) -> RestClient:
    """A RestClient whose pooled AsyncClient is driven by an httpx.MockTransport.

    The REAL _request retry loop runs; only the network is faked. The configured
    httpx.Limits are still attached to the client so they can be asserted.
    """
    client = RestClient(base_url="http://gw.local", **kwargs)

    async def _get_client():
        if client._client is None or client._client.is_closed:
            client._client = httpx.AsyncClient(
                base_url=client.base_url,
                transport=httpx.MockTransport(handler),
                limits=client._limits,
            )
        return client._client

    client._get_client = _get_client  # type: ignore[assignment]
    return client


# ---------------------------------------------------------------------------
# D6 CONNECT TIMEOUT (10s -> 35s)
# ---------------------------------------------------------------------------


class TestD6ConnectTimeout:
    def test_default_constant_is_35s(self):
        # The documented fix: PROVIDER_READY up to 30s, so default is 35s.
        assert DEFAULT_CONNECT_TIMEOUT == 35.0

    @pytest.mark.asyncio
    async def test_connect_uses_35s_default(self):
        session = make_ready_session(audio=False)
        await session.connect()  # no explicit timeout -> the new default
        try:
            assert session._last_open_timeout == 35.0  # type: ignore[attr-defined]
        finally:
            await session.disconnect()

    @pytest.mark.asyncio
    async def test_connect_explicit_timeout_still_honored(self):
        session = make_ready_session(audio=False)
        await session.connect(timeout=4.0)
        try:
            assert session._last_open_timeout == 4.0  # type: ignore[attr-defined]
        finally:
            await session.disconnect()

    def test_pipeline_wrappers_default_to_35s(self):
        """Every pipeline connect() wrapper inherits the 35s default (D6)."""
        for cls in (STTSession, TTSSession, TalkSession, GatewayRealtime):
            sig = inspect.signature(cls.connect)
            assert sig.parameters["timeout"].default == 35.0, cls.__name__


# ---------------------------------------------------------------------------
# D6 PREWARM / prepare_connection
# ---------------------------------------------------------------------------


class TestD6Prewarm:
    @pytest.mark.asyncio
    async def test_warmup_sends_head_and_returns_true(self):
        seen = {"methods": []}

        def handler(request):
            seen["methods"].append(request.method)
            return httpx.Response(200, json={"status": "ok"})

        rest = make_rest_client(handler)
        ok = await rest.warmup(timeout=2.0)
        await rest.close()

        assert ok is True
        # A HEAD warms the pool without a body.
        assert seen["methods"] == ["HEAD"]

    @pytest.mark.asyncio
    async def test_warmup_falls_back_to_get_when_head_unsupported(self):
        seen = {"methods": []}

        def handler(request):
            seen["methods"].append(request.method)
            if request.method == "HEAD":
                # Simulate a server/proxy that rejects HEAD at the transport layer.
                raise httpx.ConnectError("HEAD blocked", request=request)
            return httpx.Response(200, json={"status": "ok"})

        rest = make_rest_client(handler)
        ok = await rest.warmup(timeout=2.0)
        await rest.close()

        assert ok is True
        assert seen["methods"] == ["HEAD", "GET"]

    @pytest.mark.asyncio
    async def test_warmup_never_raises_on_dead_gateway(self):
        def handler(request):
            raise httpx.ConnectError("no route", request=request)

        rest = make_rest_client(handler)
        # Both HEAD and the GET fallback fail -> swallowed, returns False.
        ok = await rest.warmup(timeout=2.0)
        await rest.close()
        assert ok is False

    @pytest.mark.asyncio
    async def test_budclient_prepare_connection_and_alias(self):
        bud = BudClient(base_url="http://gw.local")
        calls = {"n": 0}

        async def fake_warmup(timeout=5.0):
            calls["n"] += 1
            return True

        bud._rest_client.warmup = fake_warmup  # type: ignore[assignment]

        assert await bud.prepare_connection() is True
        # prewarm is an alias of prepare_connection.
        assert await bud.prewarm() is True
        assert calls["n"] == 2
        await bud.close()


# ---------------------------------------------------------------------------
# D6 STT KEEPALIVE-SILENCE
# ---------------------------------------------------------------------------


class TestD6Keepalive:
    def test_default_interval_constant(self):
        assert DEFAULT_STT_KEEPALIVE_INTERVAL == 5.0

    def test_keepalive_frame_size_from_sample_rate(self):
        # 16kHz default -> 100ms * 16000 * 2 bytes = 3200 zero bytes.
        s = WebSocketSession(url="ws://x/ws", stt_config=STTConfig(provider="deepgram"))
        frame = s._keepalive_frame()
        assert frame == b"\x00" * 3200

        # 24kHz -> 100ms * 24000 * 2 = 4800.
        s2 = WebSocketSession(
            url="ws://x/ws",
            stt_config=STTConfig(provider="deepgram", sample_rate=24000),
        )
        assert s2._keepalive_frame() == b"\x00" * 4800

    @pytest.mark.asyncio
    async def test_keepalive_sends_silence_when_idle(self):
        ws = FakeWS(inbound=[])
        session = WebSocketSession(
            url="ws://x/ws", audio=True, stt_keepalive_interval=0.1
        )
        session._ws = ws
        session._connected = True
        # Last real audio was long ago -> the loop should emit a silence frame.
        session._last_real_audio_monotonic = time.monotonic() - 10.0

        fired = []
        session.on("stt_keepalive", lambda d: fired.append(d))

        session._start_keepalive()
        await asyncio.sleep(0.35)
        session._stop_keepalive()
        session._connected = False

        silence = [m for m in ws.sent if isinstance(m, (bytes, bytearray))]
        assert silence, "expected at least one keepalive silence frame"
        assert all(set(bytes(m)) == {0} for m in silence)  # pure silence
        assert len(fired) >= 1

    @pytest.mark.asyncio
    async def test_keepalive_gated_by_real_audio(self):
        """Recent REAL audio must SUPPRESS the keepalive (never fight capture)."""
        ws = FakeWS(inbound=[])
        session = WebSocketSession(
            url="ws://x/ws", audio=True, stt_keepalive_interval=0.1
        )
        session._ws = ws
        session._connected = True
        session._start_keepalive()

        # Keep "sending real audio" every 50ms across the window: the gate sees
        # last-real-audio < interval each tick, so it emits ZERO silence frames.
        for _ in range(6):
            session._last_real_audio_monotonic = time.monotonic()
            await asyncio.sleep(0.05)
        session._stop_keepalive()
        session._connected = False

        silence = [m for m in ws.sent if isinstance(m, (bytes, bytearray))]
        assert silence == []

    @pytest.mark.asyncio
    async def test_send_audio_resets_keepalive_timer(self):
        session = WebSocketSession(url="ws://x/ws", audio=True)
        # No socket yet -> buffers, but must still stamp the real-audio clock.
        before = session._last_real_audio_monotonic
        await session.send_audio(b"\x01\x02")
        assert before is None
        assert session._last_real_audio_monotonic is not None

    def test_keepalive_not_started_for_audio_false(self):
        session = WebSocketSession(url="ws://x/ws", audio=False)
        session._connected = True
        session._start_keepalive()
        # Config-only session has no STT stream to keep warm.
        assert session._keepalive_task is None

    def test_keepalive_disabled_when_interval_nonpositive(self):
        session = WebSocketSession(url="ws://x/ws", audio=True, stt_keepalive_interval=0)
        session._connected = True
        session._start_keepalive()
        assert session._keepalive_task is None


# ---------------------------------------------------------------------------
# D7 REST POOLING (httpx.Limits + client reuse)
# ---------------------------------------------------------------------------


class TestD7Pooling:
    def test_limits_are_tuned_not_default(self):
        rest = RestClient(base_url="http://gw.local")
        lim = rest._limits
        assert lim.max_keepalive_connections == DEFAULT_MAX_KEEPALIVE_CONNECTIONS
        assert lim.keepalive_expiry == DEFAULT_KEEPALIVE_EXPIRY
        # Not httpx's bare default (which leaves keepalive_expiry None-ish/5s).
        assert lim.max_keepalive_connections is not None

    def test_limits_overridable(self):
        rest = RestClient(
            base_url="http://gw.local",
            max_keepalive_connections=3,
            max_connections=7,
            keepalive_expiry=12.0,
        )
        assert rest._limits.max_keepalive_connections == 3
        assert rest._limits.max_connections == 7
        assert rest._limits.keepalive_expiry == 12.0

    @pytest.mark.asyncio
    async def test_client_is_reused_across_calls(self):
        """Sequential calls must SHARE one pooled client (not per-call)."""
        builds = {"n": 0}

        def handler(request):
            return httpx.Response(200, json={"ok": True})

        rest = RestClient(base_url="http://gw.local")

        real_transport = httpx.MockTransport(handler)

        async def _get_client():
            if rest._client is None or rest._client.is_closed:
                builds["n"] += 1
                rest._client = httpx.AsyncClient(
                    base_url=rest.base_url,
                    transport=real_transport,
                    limits=rest._limits,
                )
            return rest._client

        rest._get_client = _get_client  # type: ignore[assignment]

        for _ in range(5):
            await rest.get("/voices")
        await rest.close()

        # The client was constructed exactly ONCE for five sequential calls.
        assert builds["n"] == 1

    @pytest.mark.asyncio
    async def test_real_client_memoized_same_instance(self):
        """The production _get_client returns the SAME object each call."""
        rest = RestClient(base_url="http://gw.local")
        c1 = await rest._get_client()
        c2 = await rest._get_client()
        assert c1 is c2
        # And it carries the tuned limits.
        await rest.close()


# ---------------------------------------------------------------------------
# D7 REST 503 / 429 BACKOFF (honored, not fatal)
# ---------------------------------------------------------------------------


class TestD7RestBackoff:
    @pytest.mark.asyncio
    async def test_503_backs_off_then_succeeds(self):
        calls = {"n": 0}

        def handler(request):
            calls["n"] += 1
            if calls["n"] <= 2:
                # Retry-After:0 keeps the test instant; 503 must NOT be fatal.
                return httpx.Response(503, headers={"Retry-After": "0"}, text="at capacity")
            return httpx.Response(200, json={"recovered": True})

        rest = make_rest_client(handler, max_retries=3)
        result = await rest.get("/voices")
        await rest.close()

        assert result == {"recovered": True}
        assert calls["n"] == 3  # two 503s, then OK

    @pytest.mark.asyncio
    async def test_429_honors_retry_after_then_succeeds(self):
        calls = {"n": 0}
        slept: list[float] = []

        def handler(request):
            calls["n"] += 1
            if calls["n"] == 1:
                return httpx.Response(429, headers={"Retry-After": "2"}, text="slow down")
            return httpx.Response(200, json={"ok": True})

        async def fake_sleep(s):
            slept.append(s)

        rest = make_rest_client(handler, max_retries=3)
        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(asyncio, "sleep", fake_sleep)
            result = await rest.get("/voices")
        await rest.close()

        assert result == {"ok": True}
        # The Retry-After (2s) drove the backoff (within +-20% jitter).
        assert slept and 1.6 <= slept[0] <= 2.4

    @pytest.mark.asyncio
    async def test_persistent_503_raises_typed_rate_limit_error(self):
        def handler(request):
            return httpx.Response(503, headers={"Retry-After": "0"}, text="at capacity")

        rest = make_rest_client(handler, max_retries=2)
        with pytest.raises(RateLimitError) as ei:
            await rest.get("/voices")
        await rest.close()

        # Typed (not opaque APIError); 503 is surfaced as a RateLimitError.
        assert ei.value.code == "RATE_LIMITED"
        assert "503" in ei.value.message

    @pytest.mark.asyncio
    async def test_persistent_429_raises_rate_limit_with_retry_after(self):
        def handler(request):
            return httpx.Response(429, headers={"Retry-After": "0"}, text="nope")

        rest = make_rest_client(handler, max_retries=1)
        with pytest.raises(RateLimitError) as ei:
            await rest.get("/voices")
        await rest.close()
        assert ei.value.retry_after == 0.0

    @pytest.mark.asyncio
    async def test_non_retryable_4xx_still_raises_api_error_immediately(self):
        calls = {"n": 0}

        def handler(request):
            calls["n"] += 1
            return httpx.Response(400, json={"message": "bad text"})

        rest = make_rest_client(handler, max_retries=3)
        with pytest.raises(APIError) as ei:
            await rest.post("/speak", json={"text": ""})
        await rest.close()

        # 400 is a real error: NO retry, immediate APIError.
        assert ei.value.status_code == 400
        assert calls["n"] == 1

    @pytest.mark.asyncio
    async def test_success_path_unaffected(self):
        def handler(request):
            assert request.url.path == "/voices"
            return httpx.Response(200, json={"deepgram": []})

        rest = make_rest_client(handler)
        result = await rest.list_voices()
        await rest.close()
        assert result == {"deepgram": []}


# ---------------------------------------------------------------------------
# D7 WS CONNECT 503/429 classification (parity with the REST path)
# ---------------------------------------------------------------------------


class TestD7WsConnectClassification:
    def test_ws_503_classified_as_retryable_rate_limit(self):
        """A 503 WS upgrade must become a (retryable) RateLimitError, not fatal."""
        resp = Response(503, "Server at capacity", Headers())
        exc = InvalidStatus(resp)
        typed = WebSocketSession._classify_connect_error(exc, "ws://x/ws")
        assert isinstance(typed, RateLimitError)
        assert "503" in typed.message

    def test_ws_429_still_captures_retry_after(self):
        """Verify the WS upgrade 429 capture exists under websockets (task ask)."""
        h = Headers()
        h["Retry-After"] = "9"
        exc = InvalidStatus(Response(429, "Too Many Requests", h))
        typed = WebSocketSession._classify_connect_error(exc, "ws://x/ws")
        assert isinstance(typed, RateLimitError)
        assert typed.retry_after == 9.0

    def test_ws_other_status_is_plain_connection_error(self):
        exc = InvalidStatus(Response(401, "Unauthorized", Headers()))
        typed = WebSocketSession._classify_connect_error(exc, "ws://x/ws")
        assert isinstance(typed, BudConnectionError)
        assert not isinstance(typed, RateLimitError)

    @pytest.mark.asyncio
    async def test_ws_connect_retries_503_then_succeeds(self):
        """connect() backs off a 503 upgrade (Retry-After) then connects."""
        ready = json.dumps({"type": "ready", "stream_id": "s", "protocol_version": "1.0"})
        attempts = {"n": 0}
        slept: list[float] = []

        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            audio=False,
            reconnect=ReconnectConfig(enabled=False),
        )

        async def _flaky_open(timeout):
            attempts["n"] += 1
            if attempts["n"] == 1:
                raise WebSocketSession._classify_connect_error(
                    InvalidStatus(Response(503, "cap", Headers())), session.url
                )
            return FakeWS(inbound=[ready])

        async def fake_sleep(s):
            slept.append(s)

        session._open_ws = _flaky_open  # type: ignore[assignment]

        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(asyncio, "sleep", fake_sleep)
            await session.connect(timeout=2.0, max_rate_limit_retries=3)
        try:
            assert session.connected is True
            assert attempts["n"] == 2  # one 503, then OK
            assert slept  # backed off before the retry
        finally:
            await session.disconnect()
