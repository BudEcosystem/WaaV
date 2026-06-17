"""
Tier-1 SDK performance & resilience tests (PERF_RESILIENCE_PLAN D1/D4/D5).

Covers the Python WebSocketSession hardening:

* D1 LIVENESS: native WS ping_interval/ping_timeout passed on connect; the
  protocol_version major-mismatch typed warning; verify-after-reconnect; the
  app-level stale-inbound watchdog forcing a reconnect.
* D4 RECONNECT: the client-side quick-failure breaker (survival<5s x3 -> typed
  FatalConnectionError, stop the storm); fast first hop (200ms) with
  decorrelated jitter + 30s ceiling; in-flight uplink audio is NOT cleared on
  reconnect; the bounded close-handshake in disconnect().
* D5 CONTROL-QUEUE: control ops (speak/clear/send_message) buffer-and-flush on
  reconnect instead of raising ConnectionError; the uplink audio queue is
  bounded + drop-oldest.

These are pure-unit (no live gateway): the socket is faked.
"""

import asyncio
import json
import time

import pytest
import websockets

from bud_waav.errors import ConnectionError as BudConnectionError
from bud_waav.errors import (
    FatalConnectionError,
    ProtocolVersionError,
    ReconnectError,
)
from bud_waav.ws.session import ReconnectConfig, WebSocketSession

# ---------------------------------------------------------------------------
# Fakes
# ---------------------------------------------------------------------------


def _closed_exc() -> websockets.ConnectionClosed:
    """A realistic ConnectionClosed the receive loop catches."""
    return websockets.ConnectionClosed(None, None)


class FakeWS:
    """A minimal stand-in for a websockets ClientConnection.

    Records sent frames, answers ping() with an already-resolved pong future,
    and yields a scripted list of inbound frames (each a str or bytes) before
    raising ConnectionClosed (so _receive_loop exits its async-for like a real
    socket close).
    """

    def __init__(
        self,
        inbound=None,
        *,
        raise_on_close=False,
        hang_close=False,
        close_after_inbound=False,
    ):
        self.sent: list = []
        self._inbound = list(inbound or [])
        self.closed = False
        self.close_calls = 0
        self.ping_calls = 0
        self.raise_on_close = raise_on_close
        self.hang_close = hang_close
        self.send_should_raise = False
        # When False (default) the socket stays OPEN after its scripted frames
        # are drained — __anext__ blocks until close(), like a real idle socket
        # so connect() can flush against a live connection. When True it raises
        # ConnectionClosed once frames run out (to drive the reconnect path).
        self.close_after_inbound = close_after_inbound
        self._closed_evt = asyncio.Event()

    async def send(self, data):
        if self.send_should_raise:
            raise _closed_exc()
        self.sent.append(data)

    async def ping(self, data=None):
        self.ping_calls += 1
        fut = asyncio.get_event_loop().create_future()
        fut.set_result(True)  # pong already received
        return fut

    async def close(self, code=1000, reason=""):
        self.close_calls += 1
        self.closed = True
        self._closed_evt.set()
        if self.hang_close:
            await asyncio.sleep(3600)  # never returns -> tests the bound
        if self.raise_on_close:
            raise RuntimeError("close boom")

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._inbound:
            return self._inbound.pop(0)
        if self.close_after_inbound:
            raise _closed_exc()
        # Stay OPEN: block until someone closes the socket, then end iteration.
        await self._closed_evt.wait()
        raise _closed_exc()


def make_ready_session(**kwargs) -> WebSocketSession:
    """A session whose _open_ws yields a FakeWS that immediately replies ready.

    connect() then drives _send_config -> ready -> verify -> flush, exactly like
    production but with no network.
    """
    ready = json.dumps({"type": "ready", "stream_id": "s-1", "protocol_version": "1.0"})
    session = WebSocketSession(
        url="ws://localhost:3009/ws",
        audio=False,
        reconnect=ReconnectConfig(enabled=False),
        **kwargs,
    )

    async def _fake_open(timeout):
        return FakeWS(inbound=[ready])

    session._open_ws = _fake_open  # type: ignore[assignment]
    return session


# ---------------------------------------------------------------------------
# D1 LIVENESS
# ---------------------------------------------------------------------------


class TestD1PingOnConnect:
    """ping_interval / ping_timeout must be passed to websockets.connect."""

    @pytest.mark.asyncio
    async def test_ping_interval_passed_to_connect(self):
        captured = {}

        async def fake_connect(url, **kwargs):
            captured.update(kwargs)
            return FakeWS(inbound=[])  # not iterated here

        session = WebSocketSession(url="ws://localhost:3009/ws")
        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(websockets, "connect", fake_connect)
            await session._open_ws(timeout=5.0)

        assert captured.get("ping_interval") == 5.0
        assert captured.get("ping_timeout") == 3.0

    @pytest.mark.asyncio
    async def test_ping_interval_custom_values(self):
        captured = {}

        async def fake_connect(url, **kwargs):
            captured.update(kwargs)
            return FakeWS(inbound=[])

        session = WebSocketSession(
            url="ws://localhost:3009/ws", ping_interval=2.0, ping_timeout=1.0
        )
        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(websockets, "connect", fake_connect)
            await session._open_ws(timeout=5.0)

        assert captured["ping_interval"] == 2.0
        assert captured["ping_timeout"] == 1.0

    @pytest.mark.asyncio
    async def test_ping_disabled_sends_none(self):
        """ping_interval<=0 disables native pings (None), app watchdog covers it."""
        captured = {}

        async def fake_connect(url, **kwargs):
            captured.update(kwargs)
            return FakeWS(inbound=[])

        session = WebSocketSession(url="ws://localhost:3009/ws", ping_interval=0)
        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(websockets, "connect", fake_connect)
            await session._open_ws(timeout=5.0)

        assert captured["ping_interval"] is None
        assert "ping_timeout" not in captured


class TestD1ProtocolMismatch:
    """A major protocol_version mismatch emits a typed warning (not a raise)."""

    @pytest.mark.asyncio
    async def test_major_mismatch_emits_typed_warning(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        session._connected = True
        warnings = []
        session.on("warning", lambda w: warnings.append(w))

        wire = json.dumps({"type": "ready", "protocol_version": "2.0", "stream_id": "x"})

        async def one_message():
            yield wire

        session._ws = one_message()
        await session._receive_loop()

        assert len(warnings) == 1
        assert isinstance(warnings[0], ProtocolVersionError)
        assert warnings[0].gateway_version == "2.0"
        assert warnings[0].sdk_version == "1.0"
        # Session still proceeds: protocol captured, ready set.
        assert session.protocol_version == "2.0"

    @pytest.mark.asyncio
    async def test_matching_major_no_warning(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        session._connected = True
        warnings = []
        session.on("warning", lambda w: warnings.append(w))

        # 1.7 has the SAME major as the SDK's 1.0 -> no warning.
        wire = json.dumps({"type": "ready", "protocol_version": "1.7", "stream_id": "x"})

        async def one_message():
            yield wire

        session._ws = one_message()
        await session._receive_loop()

        assert warnings == []
        assert session.protocol_version == "1.7"


class TestD1VerifyAfterReconnect:
    """connect() must ping the NEW socket to prove it carries data."""

    @pytest.mark.asyncio
    async def test_connect_pings_new_socket(self):
        session = make_ready_session()
        await session.connect(timeout=2.0)
        try:
            assert session.connected is True
            # The FakeWS recorded exactly one verify ping.
            assert session._ws.ping_calls == 1
        finally:
            await session.disconnect()

    @pytest.mark.asyncio
    async def test_verify_failure_fails_connect(self):
        """A socket whose pong never returns must fail connect (half-open)."""
        ready = json.dumps({"type": "ready", "stream_id": "s", "protocol_version": "1.0"})

        class NoPongWS(FakeWS):
            async def ping(self, data=None):
                self.ping_calls += 1
                # Future that never resolves -> verify times out.
                return asyncio.get_event_loop().create_future()

        async def _fake_open(timeout):
            return NoPongWS(inbound=[ready])

        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            audio=False,
            reconnect=ReconnectConfig(enabled=False),
            ping_timeout=0.1,
        )
        session._open_ws = _fake_open  # type: ignore[assignment]

        with pytest.raises(BudConnectionError):
            await session.connect(timeout=2.0)
        assert session.connected is False


class TestD1StaleInboundWatchdog:
    """The app-level staleness timer closes a silent-but-OPEN (zombie) socket."""

    @pytest.mark.asyncio
    async def test_watchdog_closes_on_stale_inbound(self):
        # No inbound frames at all after ready -> the watchdog should fire fast.
        ws = FakeWS(inbound=[])
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            reconnect=ReconnectConfig(enabled=False),
            stale_inbound_timeout=0.3,
        )
        session._ws = ws
        session._connected = True
        # Simulate "last inbound was long ago".
        session._last_inbound_monotonic = time.monotonic() - 5.0

        zombies = []
        session.on("zombie_detected", lambda d: zombies.append(d))

        session._start_watchdog()
        # Give the watchdog poll a few cycles.
        await asyncio.sleep(0.6)
        session._stop_watchdog()

        assert ws.close_calls >= 1
        assert session._reconnecting_due_to_stale is True
        assert len(zombies) == 1

    @pytest.mark.asyncio
    async def test_watchdog_quiet_when_inbound_fresh(self):
        ws = FakeWS(inbound=[])
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            reconnect=ReconnectConfig(enabled=False),
            stale_inbound_timeout=0.4,
        )
        session._ws = ws
        session._connected = True
        session._start_watchdog()

        # Keep inbound fresh across the window.
        for _ in range(4):
            session._last_inbound_monotonic = time.monotonic()
            await asyncio.sleep(0.15)
        session._connected = False
        session._stop_watchdog()

        assert ws.close_calls == 0

    @pytest.mark.asyncio
    async def test_watchdog_disabled_when_timeout_nonpositive(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws", stale_inbound_timeout=0
        )
        session._connected = True
        session._start_watchdog()
        assert session._watchdog_task is None


# ---------------------------------------------------------------------------
# D4 RECONNECT HARDENING
# ---------------------------------------------------------------------------


class TestD4FastFirstHop:
    """The default first reconnect hop is fast (200ms), not the old 1000ms."""

    def test_default_initial_delay_is_fast(self):
        assert ReconnectConfig().initial_delay_ms == 200

    def test_breaker_defaults(self):
        cfg = ReconnectConfig()
        assert cfg.min_stable_seconds == 5.0
        assert cfg.quick_failure_threshold == 3

    @pytest.mark.asyncio
    async def test_first_hop_uses_initial_delay(self):
        """The first reconnect sleep is ~initial_delay_ms (no compounding)."""
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            reconnect=ReconnectConfig(initial_delay_ms=50, max_retries=1),
        )
        slept: list[float] = []

        async def fake_sleep(s):
            slept.append(s)

        async def ok_connect():
            session._consecutive_quick_failures = 0  # stable

        session.connect = ok_connect  # type: ignore[assignment]
        # No prior ready session this cycle.
        session._connected_at = None

        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(asyncio, "sleep", fake_sleep)
            await session._reconnect()

        assert slept, "reconnect should sleep before the first attempt"
        # First hop equals the fast initial delay (50ms -> 0.05s), not 1s.
        assert slept[0] == pytest.approx(0.05, abs=1e-6)


class TestD4QuickFailureBreaker:
    """survival<5s x3 -> typed FATAL; the storm STOPS at 3 (not max_retries=10)."""

    @pytest.mark.asyncio
    async def test_stops_after_three_quick_failures(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            reconnect=ReconnectConfig(
                initial_delay_ms=1, max_retries=10, min_stable_seconds=5.0,
                quick_failure_threshold=3,
            ),
        )

        attempts = {"n": 0}

        async def doomed_connect():
            # Each attempt briefly becomes ready then dies instantly: set
            # _connected_at to "just now" (survival ~0 < 5s) then raise.
            attempts["n"] += 1
            session._connected_at = time.monotonic()
            raise _closed_exc()

        session.connect = doomed_connect  # type: ignore[assignment]

        async def fast_sleep(_):
            return None

        errors = []
        session.on("error", lambda e: errors.append(e))

        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(asyncio, "sleep", fast_sleep)
            with pytest.raises(FatalConnectionError) as ei:
                await session._reconnect()

        # Exactly 3 attempts then FATAL — NOT the 10 max_retries would allow.
        assert attempts["n"] == 3
        assert ei.value.consecutive_quick_failures == 3
        assert session._fatal is True
        # The FATAL was surfaced as an error event EXACTLY once (the raise
        # propagates out of the except-ConnectionClosed handler, not into the
        # sibling except-Exception, so there is no double-emit).
        fatals = [e for e in errors if isinstance(e, FatalConnectionError)]
        assert len(fatals) == 1

    @pytest.mark.asyncio
    async def test_stable_session_resets_the_run(self):
        """A session that SURVIVES the stable window forgives the quick-fail run."""
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            reconnect=ReconnectConfig(
                min_stable_seconds=5.0, quick_failure_threshold=3
            ),
        )
        # Two quick failures already accrued.
        session._consecutive_quick_failures = 2
        # The just-ended session lived well past the stable window.
        session._connected_at = time.monotonic() - 30.0

        session._note_session_ended()

        assert session._consecutive_quick_failures == 0
        assert session._fatal is False

    @pytest.mark.asyncio
    async def test_one_quick_failure_does_not_trip(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        session._consecutive_quick_failures = 1
        # Should NOT raise.
        session._trip_if_doomed(None)
        assert session._fatal is False

    @pytest.mark.asyncio
    async def test_decorrelated_jitter_stays_under_ceiling(self):
        """Backoff never exceeds max_delay_ms even across many failures."""
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            reconnect=ReconnectConfig(
                initial_delay_ms=200, max_delay_ms=1000, max_retries=8,
                # Disable the breaker so we can observe many backoff hops.
                quick_failure_threshold=1000, min_stable_seconds=0.0,
            ),
        )
        slept: list[float] = []

        async def fake_sleep(s):
            slept.append(s)

        async def always_fail():
            # Never became ready -> not counted as a quick failure.
            session._connected_at = None
            raise _closed_exc()

        session.connect = always_fail  # type: ignore[assignment]

        with pytest.MonkeyPatch.context() as mp:
            mp.setattr(asyncio, "sleep", fake_sleep)
            # Breaker disabled -> the loop exhausts max_retries -> ReconnectError.
            with pytest.raises(ReconnectError):
                await session._reconnect()

        assert slept, "should have slept between attempts"
        # Ceiling honored (1000ms = 1.0s) for every hop.
        assert max(slept) <= 1.0 + 1e-9
        # First hop is the fast initial (0.2s).
        assert slept[0] == pytest.approx(0.2, abs=1e-9)


class TestD4InFlightAudioNotCleared:
    """Reconnect must NOT drop in-flight uplink audio (old session.py:496 bug)."""

    @pytest.mark.asyncio
    async def test_pending_audio_replayed_on_reconnect(self):
        session = make_ready_session()
        # Queue audio while "offline" (not connected yet).
        await session.send_audio(b"\x01\x02")
        await session.send_audio(b"\x03\x04")
        assert len(session._pending_audio) == 2

        await session.connect(timeout=2.0)
        try:
            # Both frames replayed onto the live socket; backlog drained (not lost).
            sent_binary = [m for m in session._ws.sent if isinstance(m, (bytes, bytearray))]
            assert b"\x01\x02" in sent_binary
            assert b"\x03\x04" in sent_binary
            assert len(session._pending_audio) == 0
        finally:
            await session.disconnect()


class TestD4BoundedClose:
    """disconnect() must not hang on a peer that never echoes Close."""

    @pytest.mark.asyncio
    async def test_close_is_bounded(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        session._ws = FakeWS(inbound=[], hang_close=True)
        session._connected = True

        start = time.monotonic()
        await session.disconnect(close_timeout=0.2)
        elapsed = time.monotonic() - start

        # Bounded: returns ~0.2s, NOT hanging on the 1h close.
        assert elapsed < 1.0
        assert session._ws is None

    @pytest.mark.asyncio
    async def test_close_swallows_close_error(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        session._ws = FakeWS(inbound=[], raise_on_close=True)
        session._connected = True
        # Must not propagate the close() RuntimeError.
        await session.disconnect(close_timeout=0.5)
        assert session._ws is None


# ---------------------------------------------------------------------------
# D5 CONTROL-QUEUE UNIFICATION + UPLINK SHEDDING
# ---------------------------------------------------------------------------


class TestD5ControlQueueBuffers:
    """Control ops while disconnected BUFFER (not raise) and flush on reconnect."""

    @pytest.mark.asyncio
    async def test_speak_while_disconnected_buffers(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        # Not connected, no socket.
        assert session.connected is False
        # Must NOT raise (old behavior raised ConnectionError).
        await session.speak("hello")
        await session.clear()
        await session.send_message("hi there", role="user")
        assert session._control_queue.size == 3

    @pytest.mark.asyncio
    async def test_send_message_while_disconnected_does_not_raise(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        # Explicitly assert no exception type leaks.
        await session.send_message("buffered")
        assert session._control_queue.size == 1
        payload = json.loads(session._control_queue.peek())
        assert payload["type"] == "send_message"
        assert payload["message"] == "buffered"

    @pytest.mark.asyncio
    async def test_buffered_control_ops_flush_on_connect(self):
        session = make_ready_session()
        # Buffer two control ops while offline.
        await session.speak("first")
        await session.clear()
        assert session._control_queue.size == 2

        await session.connect(timeout=2.0)
        try:
            # The fresh socket received config + both control ops, in order.
            texts = [m for m in session._ws.sent if isinstance(m, str)]
            parsed = [json.loads(t) for t in texts]
            types = [p["type"] for p in parsed]
            assert types[0] == "config"
            assert "speak" in types and "clear" in types
            # speak (queued first) precedes clear (queued second).
            assert types.index("speak") < types.index("clear")
            # Queue drained.
            assert session._control_queue.is_empty
        finally:
            await session.disconnect()

    @pytest.mark.asyncio
    async def test_connected_control_op_sends_immediately(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        ws = FakeWS(inbound=[])
        session._ws = ws
        session._connected = True

        await session.speak("now")
        # Went straight to the wire, not the buffer.
        assert session._control_queue.is_empty
        assert any(json.loads(m)["type"] == "speak" for m in ws.sent)

    @pytest.mark.asyncio
    async def test_control_op_rebuffers_if_send_raises(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        ws = FakeWS(inbound=[])
        ws.send_should_raise = True
        session._ws = ws
        session._connected = True

        await session.speak("oops")
        # Send raised -> op was buffered for the next reconnect, not lost/raised.
        assert session._control_queue.size == 1


class TestD5UplinkBounded:
    """The uplink audio backlog is bounded + drop-oldest (gateway DroppableAudio)."""

    @pytest.mark.asyncio
    async def test_pending_audio_drops_oldest_when_full(self):
        session = WebSocketSession(url="ws://localhost:3009/ws", max_pending_audio=3)
        dropped = []
        session.on("audio_dropped", lambda d: dropped.append(d))

        for i in range(5):
            await session.send_audio(bytes([i]))

        # Bounded to 3; oldest two shed.
        assert len(session._pending_audio) == 3
        assert list(session._pending_audio) == [bytes([2]), bytes([3]), bytes([4])]
        # Two drop events fired (frames 3 and 4 overflowed).
        assert len(dropped) == 2

    @pytest.mark.asyncio
    async def test_default_pending_audio_bound(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        assert session._pending_audio.maxlen == 256
