"""
WebSocket session for Bud WaaV Gateway
"""

import asyncio
import base64
import json
import time
import random
from collections import deque
from typing import Any, AsyncIterator, Callable, Optional, Union
from dataclasses import dataclass, field

import websockets
from websockets.asyncio.client import ClientConnection

from ..types import (
    STTConfig, TTSConfig, STTResult, TranscriptEvent, AudioEvent,
    AudioFeatures, DAGConfig, ConversationConfig, intensity_to_number,
    Translation,
)
from ..errors import (
    ConnectionError,
    ReconnectError,
    TimeoutError,
    RateLimitError,
    FatalConnectionError,
    ProtocolVersionError,
)
from .queue import MessageQueue, QueueConfig

# Protocol version the gateway emits on `ready` (messages.rs PROTOCOL_VERSION, plan
# W-K1). We assert the MAJOR matches so a breaking wire change surfaces as an
# explicit mismatch instead of silent field drift.
PROTOCOL_VERSION = "1.0"


@dataclass
class ReconnectConfig:
    """Reconnection configuration.

    The defaults encode the Tier-1 D4 hardening:

    * ``initial_delay_ms`` is a FAST first hop (200ms, down from 1000ms) so a
      transient client<->gateway blip heals in well under a second instead of
      the measured ~1.65s. The compounding backoff is replaced by decorrelated
      jitter (see :meth:`WebSocketSession._reconnect`), bounded by
      ``max_delay_ms``.
    * ``quick_failure_threshold`` / ``min_stable_seconds`` arm the client-side
      quick-failure breaker (ported from Pipecat
      ``websocket_service.py`` ``_MIN_STABLE_CONNECTION_DURATION`` /
      ``_MAX_CONSECUTIVE_QUICK_FAILURES``): if a connection dies within
      ``min_stable_seconds`` of becoming ready ``quick_failure_threshold`` times
      in a row, the loop STOPS with a typed :class:`FatalConnectionError`
      instead of storming a doomed config.
    """

    enabled: bool = True
    initial_delay_ms: int = 200
    max_delay_ms: int = 30000
    multiplier: float = 1.5
    max_retries: int = 10
    jitter: float = 0.2
    # D4a quick-failure breaker (Pipecat-ported). A "quick failure" is a session
    # that becomes ready then dies < min_stable_seconds later.
    min_stable_seconds: float = 5.0
    quick_failure_threshold: int = 3


@dataclass
class PercentileStats:
    """Percentile statistics for metrics."""

    p50: float = 0.0
    p95: float = 0.0
    p99: float = 0.0
    min: float = 0.0
    max: float = 0.0
    mean: float = 0.0
    last: float = 0.0
    count: int = 0


@dataclass
class SessionMetrics:
    """Session performance metrics."""

    stt_ttft: PercentileStats = field(default_factory=PercentileStats)
    tts_ttfb: PercentileStats = field(default_factory=PercentileStats)
    e2e_latency: PercentileStats = field(default_factory=PercentileStats)
    ws_connect_ms: float = 0.0
    reconnect_count: int = 0
    messages_sent: int = 0
    messages_received: int = 0
    audio_bytes_sent: int = 0
    audio_bytes_received: int = 0


class MetricsCollector:
    """Collects and calculates performance metrics."""

    def __init__(self, max_samples: int = 1000):
        self._max_samples = max_samples
        self._stt_ttft: list[float] = []
        self._tts_ttfb: list[float] = []
        self._e2e_latency: list[float] = []
        self._ws_connect_ms: float = 0.0
        self._reconnect_count: int = 0
        self._messages_sent: int = 0
        self._messages_received: int = 0
        self._audio_bytes_sent: int = 0
        self._audio_bytes_received: int = 0

    def record_stt_ttft(self, ms: float) -> None:
        """Record STT time-to-first-token."""
        self._add_sample(self._stt_ttft, ms)

    def record_tts_ttfb(self, ms: float) -> None:
        """Record TTS time-to-first-byte."""
        self._add_sample(self._tts_ttfb, ms)

    def record_e2e_latency(self, ms: float) -> None:
        """Record end-to-end latency."""
        self._add_sample(self._e2e_latency, ms)

    def record_ws_connect(self, ms: float) -> None:
        """Record WebSocket connection time."""
        self._ws_connect_ms = ms

    def record_reconnect(self) -> None:
        """Record a reconnection."""
        self._reconnect_count += 1

    def record_message_sent(self) -> None:
        """Record a message sent."""
        self._messages_sent += 1

    def record_message_received(self) -> None:
        """Record a message received."""
        self._messages_received += 1

    def record_audio_sent(self, bytes_count: int) -> None:
        """Record audio bytes sent."""
        self._audio_bytes_sent += bytes_count

    def record_audio_received(self, bytes_count: int) -> None:
        """Record audio bytes received."""
        self._audio_bytes_received += bytes_count

    def _add_sample(self, samples: list[float], value: float) -> None:
        """Add a sample with reservoir sampling."""
        if len(samples) < self._max_samples:
            samples.append(value)
        else:
            idx = random.randint(0, len(samples) - 1)
            samples[idx] = value

    def _calculate_percentiles(self, samples: list[float]) -> PercentileStats:
        """Calculate percentile statistics."""
        if not samples:
            return PercentileStats()

        sorted_samples = sorted(samples)
        n = len(sorted_samples)

        def percentile(p: float) -> float:
            idx = int(p * n)
            return sorted_samples[min(idx, n - 1)]

        return PercentileStats(
            p50=percentile(0.50),
            p95=percentile(0.95),
            p99=percentile(0.99),
            min=sorted_samples[0],
            max=sorted_samples[-1],
            mean=sum(sorted_samples) / n,
            last=sorted_samples[-1] if sorted_samples else 0.0,
            count=n,
        )

    def get_metrics(self) -> SessionMetrics:
        """Get current metrics snapshot."""
        return SessionMetrics(
            stt_ttft=self._calculate_percentiles(self._stt_ttft),
            tts_ttfb=self._calculate_percentiles(self._tts_ttfb),
            e2e_latency=self._calculate_percentiles(self._e2e_latency),
            ws_connect_ms=self._ws_connect_ms,
            reconnect_count=self._reconnect_count,
            messages_sent=self._messages_sent,
            messages_received=self._messages_received,
            audio_bytes_sent=self._audio_bytes_sent,
            audio_bytes_received=self._audio_bytes_received,
        )

    def reset(self) -> None:
        """Reset all metrics."""
        self._stt_ttft.clear()
        self._tts_ttfb.clear()
        self._e2e_latency.clear()
        self._ws_connect_ms = 0.0
        self._reconnect_count = 0
        self._messages_sent = 0
        self._messages_received = 0
        self._audio_bytes_sent = 0
        self._audio_bytes_received = 0


class WebSocketSession:
    """WebSocket session for real-time communication with Bud WaaV Gateway."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        stt_config: Optional[STTConfig] = None,
        tts_config: Optional[TTSConfig] = None,
        livekit_config: Optional[dict[str, Any]] = None,
        audio_features: Optional[AudioFeatures] = None,
        dag_config: Optional[DAGConfig] = None,
        conversation_config: Optional[ConversationConfig] = None,
        stream_id: Optional[str] = None,
        reconnect: Optional[ReconnectConfig] = None,
        audio: bool = True,
        alias: Optional[str] = None,
        ping_interval: float = 5.0,
        ping_timeout: float = 3.0,
        stale_inbound_timeout: float = 12.0,
        control_queue: Optional[MessageQueue] = None,
        max_pending_audio: int = 256,
    ):
        """
        Initialize WebSocket session.

        Args:
            url: WebSocket URL of the gateway
            api_key: Optional API key for authentication
            stt_config: STT configuration
            tts_config: TTS configuration
            livekit_config: LiveKit configuration
            audio_features: Audio features. Only ``turn_detection`` is serialized
                (nested into ``stt_config.turn_detection``, its real wire home);
                ``noise_filtering``/``vad`` have NO /ws wire field yet and are
                intentionally NOT sent (see SDK_STANDARDIZATION_PLAN P4).
            dag_config: DAG routing configuration
            conversation_config: Built-in LLM conversation-loop configuration
                (reasoning, latency filler, barge-in, etc.). Serialized into the
                ``conversation_config`` block of the config message.
            stream_id: Optional stream ID for session tracking
            reconnect: Reconnection configuration
            ping_interval: Seconds between native WS PING frames (D1 liveness).
                Overrides the websockets-library 20s default with a voice-cadence
                5s so a frozen-but-OPEN gateway (zombie socket) is detected fast.
                Set <=0 to disable native pings (falls back to the app staleness
                timer only).
            ping_timeout: Seconds to wait for the matching PONG before the
                websockets library tears the socket down (D1). 3s pairs with the
                5s interval; on miss the EXISTING reconnect path runs.
            stale_inbound_timeout: Belt-and-suspenders app-level staleness window
                (D1). During an active session the gateway emits frequent
                inbound frames; if NONE arrive for this many seconds we declare a
                zombie and reconnect, covering paths the native ping/pong cannot
                (e.g. a wedged read pump). Well under the gateway's ~300s
                idle-close. Set <=0 to disable.
            control_queue: Optional pre-built control :class:`MessageQueue` (D5).
                When omitted a bounded drop-oldest queue is created so control
                ops (speak/clear/send_message) buffer-and-flush across a
                reconnect instead of raising, matching the TS SDK.
            max_pending_audio: Bound for the uplink audio backlog buffered while
                disconnected (D5). Oldest frames are shed first (mirrors the
                gateway's DroppableAudio drop-oldest egress discipline) so a slow
                or absent gateway never grows uplink latency without limit.
        """
        self.url = url
        self.api_key = api_key
        self.stt_config = stt_config
        self.tts_config = tts_config
        self.livekit_config = livekit_config
        self.audio_features = audio_features
        self.dag_config = dag_config
        self.conversation_config = conversation_config
        # When False, the gateway skips STT/TTS provider construction and reaches
        # `ready` immediately (config-only session). Default True (a voice session).
        self.audio = audio
        # Server-defined proxy/alias name (P3). The gateway resolves it to a full
        # {stt,tts,llm,dag} bundle BEFORE provider construction; an explicit config
        # field above always wins. The resolved concrete providers come back on the
        # `ready` ack as `resolved_alias` (see `resolved_alias` property).
        self.alias = alias
        self.requested_stream_id = stream_id
        self.reconnect_config = reconnect or ReconnectConfig()

        self._ws: Optional[ClientConnection] = None
        self._stream_id: Optional[str] = None
        self._protocol_version: Optional[str] = None
        self._resolved_alias: Optional[dict[str, Any]] = None
        self._connected = False
        self._connecting = False
        self._closed = False
        self._authenticated = False
        # Lazy-initialized to avoid requiring a running event loop in __init__
        self._ready_event: Optional[asyncio.Event] = None
        self._auth_event: Optional[asyncio.Event] = None
        self._message_queue: Optional[asyncio.Queue[dict[str, Any]]] = None
        # Uplink audio backlog buffered while disconnected. Bounded + drop-oldest
        # (D5): a long stall sheds the STALEST frames first so reconnect replays
        # only recent audio and uplink latency stays bounded.
        self._pending_audio: deque[bytes] = deque(maxlen=max(1, max_pending_audio))
        self._receive_task: Optional[asyncio.Task[None]] = None

        # D1 liveness knobs.
        self._ping_interval = ping_interval
        self._ping_timeout = ping_timeout
        self._stale_inbound_timeout = stale_inbound_timeout
        # Monotonic timestamp of the last inbound frame (any type). Drives the
        # app-level staleness watchdog. None until the first frame / a fresh
        # socket; set on connect so a silent gateway is caught from t0.
        self._last_inbound_monotonic: Optional[float] = None
        self._watchdog_task: Optional[asyncio.Task[None]] = None
        # Set when the watchdog (or a ping miss) forces a reconnect, so the
        # receive loop's own ConnectionClosed handler doesn't double-reconnect.
        self._reconnecting_due_to_stale = False

        # D5 control-op queue (buffer-and-flush like the TS SDK). Drop-oldest,
        # bounded; control ops are small JSON so the default cap is generous.
        self._control_queue: MessageQueue = control_queue or MessageQueue(
            QueueConfig(max_size=256, max_age_ms=60_000.0, drop_oldest=True)
        )

        # D4a quick-failure breaker state. _connected_at marks when the CURRENT
        # session became ready; on close we compare against min_stable_seconds.
        self._connected_at: Optional[float] = None
        self._consecutive_quick_failures = 0
        # Latched once the breaker trips so a doomed config can't be retried by a
        # later code path; a successful stable session resets it.
        self._fatal = False

        self._metrics = MetricsCollector()
        self._event_handlers: dict[str, list[Callable[..., Any]]] = {}

        # Timing for metrics
        self._config_sent_time: Optional[float] = None
        self._speak_start_time: Optional[float] = None

        # Track async handler tasks to prevent orphaned exceptions
        self._handler_tasks: set[asyncio.Task[Any]] = set()

        # Lock for connect() to prevent race conditions
        self._connect_lock: Optional[asyncio.Lock] = None

    @property
    def connected(self) -> bool:
        """Whether the session is connected."""
        return self._connected

    @property
    def stream_id(self) -> Optional[str]:
        """Get the stream ID."""
        return self._stream_id

    @property
    def protocol_version(self) -> Optional[str]:
        """Gateway wire-protocol version from the `ready` envelope (plan W-K1)."""
        return self._protocol_version

    @property
    def resolved_alias(self) -> Optional[dict[str, Any]]:
        """The concrete providers the gateway resolved ``alias`` to (P3).

        Populated from the ``ready`` ack when an ``alias`` was sent — e.g.
        ``{"name": "support-bot", "kind": "agent", "stt": {"provider": "deepgram",
        ...}, "tts": {"provider": "cartesia", ...}, "llm": {...}}`` (no secrets).
        ``None`` when no alias was named (or it was unknown). Lets a developer SEE
        what the alias became, and re-point it server-side with zero client change.
        """
        return self._resolved_alias

    def on(self, event: str, handler: Callable[..., Any]) -> None:
        """
        Register an event handler.

        Args:
            event: Event name (ready, transcript, audio, error, close, metrics)
            handler: Event handler function
        """
        if event not in self._event_handlers:
            self._event_handlers[event] = []
        self._event_handlers[event].append(handler)

    def off(self, event: str, handler: Optional[Callable[..., Any]] = None) -> None:
        """
        Remove an event handler.

        Args:
            event: Event name
            handler: Handler to remove (None removes all)
        """
        if event in self._event_handlers:
            if handler is None:
                self._event_handlers[event].clear()
            elif handler in self._event_handlers[event]:
                self._event_handlers[event].remove(handler)

    def _emit(self, event: str, *args: Any, **kwargs: Any) -> None:
        """Emit an event to all registered handlers."""
        handlers = self._event_handlers.get(event, [])
        for handler in handlers:
            try:
                result = handler(*args, **kwargs)
                if asyncio.iscoroutine(result):
                    # Track the task to prevent orphaned exceptions
                    task = asyncio.create_task(result)
                    self._handler_tasks.add(task)
                    task.add_done_callback(self._on_handler_task_done)
            except Exception as e:
                self._emit("error", e)

    def _on_handler_task_done(self, task: asyncio.Task[Any]) -> None:
        """Callback when an async handler task completes."""
        self._handler_tasks.discard(task)
        # Log any exception from the handler
        if not task.cancelled() and task.exception():
            exc = task.exception()
            # Emit error for handler exceptions (avoid infinite recursion)
            if exc:
                import sys
                print(f"Async event handler error: {exc}", file=sys.stderr)

    def _get_connect_lock(self) -> asyncio.Lock:
        """Get or create the connect lock (lazy initialization for event loop safety)."""
        if self._connect_lock is None:
            self._connect_lock = asyncio.Lock()
        return self._connect_lock

    def _get_ready_event(self) -> asyncio.Event:
        """Get or create the ready event (lazy initialization for event loop safety)."""
        if self._ready_event is None:
            self._ready_event = asyncio.Event()
        return self._ready_event

    def _get_auth_event(self) -> asyncio.Event:
        """Get or create the auth event (lazy initialization for event loop safety)."""
        if self._auth_event is None:
            self._auth_event = asyncio.Event()
        return self._auth_event

    def _get_message_queue(self) -> asyncio.Queue[dict[str, Any]]:
        """Get or create the message queue (lazy initialization for event loop safety)."""
        if self._message_queue is None:
            self._message_queue = asyncio.Queue()
        return self._message_queue

    @staticmethod
    def _classify_connect_error(e: Exception, url: str) -> Exception:
        """Map a raw websockets connect failure to a typed SDK error.

        The WaaV gateway applies a per-IP token bucket to the WS upgrade, so a
        429 is common and must be a typed :class:`RateLimitError` carrying
        ``Retry-After`` (not an opaque ConnectionError) so callers can back off.
        """
        status = None
        headers: dict[str, Any] = {}
        resp = getattr(e, "response", None)
        if resp is not None:
            status = getattr(resp, "status_code", None)
            headers = getattr(resp, "headers", {}) or {}
        # websockets<14 used status_code directly on the exception.
        if status is None:
            status = getattr(e, "status_code", None)
        if status == 429:
            retry_after: Optional[float] = None
            try:
                ra = headers.get("retry-after") if hasattr(headers, "get") else None
                if ra is not None:
                    retry_after = float(ra)
            except (TypeError, ValueError):
                retry_after = None
            return RateLimitError(
                message=f"Rate limited (429) connecting to {url}",
                retry_after=retry_after,
                url=url,
                cause=e,
            )
        return ConnectionError(
            message=f"Failed to connect to {url}: {e}",
            url=url,
            cause=e,
        )

    async def _open_ws(self, timeout: float) -> ClientConnection:
        """Open the raw WebSocket, classifying timeouts and 429s into typed errors.

        D1 liveness: the gateway NEVER initiates pings to clients (it only
        auto-pongs, handler.rs:683) and exposes no JSON ping op, so a frozen-
        but-OPEN gateway is otherwise invisible. We therefore drive native WS
        PING from the client at a fast voice cadence — the websockets library's
        20s/20s default did NOT trip in the SIGSTOP zombie test. ``ping_interval
        <= 0`` disables native pings (app staleness watchdog still covers it).
        """
        headers = {}
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"
        connect_kwargs: dict[str, Any] = {"additional_headers": headers}
        if self._ping_interval and self._ping_interval > 0:
            connect_kwargs["ping_interval"] = self._ping_interval
            connect_kwargs["ping_timeout"] = self._ping_timeout
        else:
            connect_kwargs["ping_interval"] = None
        try:
            return await asyncio.wait_for(
                websockets.connect(self.url, **connect_kwargs),
                timeout=timeout,
            )
        except asyncio.TimeoutError:
            raise TimeoutError(
                message=f"WebSocket connection timed out after {timeout}s",
                timeout_ms=int(timeout * 1000),
                operation="connect",
            )
        except (TimeoutError, RateLimitError, ConnectionError):
            raise
        except Exception as e:
            raise self._classify_connect_error(e, self.url)

    async def connect(
        self,
        timeout: float = 10.0,
        max_rate_limit_retries: int = 3,
    ) -> None:
        """
        Connect to the WebSocket server.

        On HTTP 429 (the gateway's per-IP token bucket) the connect is retried
        with jittered backoff honoring the ``Retry-After`` header, then re-raised
        as a typed :class:`RateLimitError` if still throttled.

        Args:
            timeout: Connection timeout in seconds
            max_rate_limit_retries: Number of 429 backoff retries before giving up

        Raises:
            RateLimitError: If still rate-limited (429) after all retries
            ConnectionError: If connection fails
            TimeoutError: If connection times out
        """
        async with self._get_connect_lock():
            # Double-check inside lock to prevent race condition
            if self._connected or self._connecting:
                return

            self._connecting = True
            self._closed = False
            start_time = time.monotonic()

            try:
                # 429-aware connect with jittered backoff on the WS upgrade.
                attempt = 0
                while True:
                    try:
                        self._ws = await self._open_ws(timeout)
                        break
                    except RateLimitError as rle:
                        if attempt >= max_rate_limit_retries:
                            raise
                        # Honor Retry-After when present, else exponential base;
                        # add +-20% jitter to avoid a synchronized retry storm.
                        base = rle.retry_after if rle.retry_after else min(2 ** attempt, 15)
                        jitter = base * 0.2 * (random.random() * 2 - 1)
                        await asyncio.sleep(max(0.0, base + jitter))
                        attempt += 1

                connect_time = (time.monotonic() - start_time) * 1000
                self._metrics.record_ws_connect(connect_time)

                self._connected = True
                self._connecting = False

                # Start receive task
                self._receive_task = asyncio.create_task(self._receive_loop())

                # Send config message
                await self._send_config()

                # Wait for ready
                try:
                    await asyncio.wait_for(self._get_ready_event().wait(), timeout=timeout)
                except asyncio.TimeoutError:
                    raise TimeoutError(
                        message="Timeout waiting for ready message",
                        timeout_ms=int(timeout * 1000),
                        operation="ready",
                    )

                # D1 verify-after-reconnect (Pipecat _verify_connection): a socket
                # being OPEN is not proof it carries data. Round-trip a native WS
                # ping on the NEW socket and wait for its pong before declaring
                # healthy, so a half-open replacement is caught at connect time.
                await self._verify_connection()

                # D4a: this session reached ready. Record WHEN so the breaker in
                # _reconnect() can later judge a quick failure (died < stable) vs
                # a recovered session. We deliberately do NOT reset the quick-
                # failure run here — a doomed config becomes ready then dies in a
                # loop, so resetting on every ready would wipe the count and let
                # it storm forever. The breaker resets the run only once a session
                # SURVIVES min_stable_seconds.
                self._connected_at = time.monotonic()
                # D1: seed the staleness clock so a gateway that goes silent right
                # after ready is still caught, and (re)start the app watchdog.
                self._last_inbound_monotonic = time.monotonic()
                self._reconnecting_due_to_stale = False
                self._start_watchdog()

                # D5/D4d: flush buffered CONTROL ops first (the gateway has the
                # fresh config + is ready), then replay buffered uplink AUDIO. The
                # audio backlog is a bounded drop-oldest deque, so a reconnect that
                # fires mid-utterance replays the most-recent frames instead of
                # dropping them (the old code cleared them outright).
                await self._flush_control_queue()
                pending = list(self._pending_audio)
                self._pending_audio.clear()
                for audio in pending:
                    await self.send_audio(audio)

                self._emit("ready", self._stream_id)

            except Exception:
                self._connecting = False
                self._connected = False
                raise

    async def _verify_connection(self) -> None:
        """Prove the freshly-opened socket actually carries data (D1).

        Ported from Pipecat ``WebsocketService._verify_connection``: OPEN is not
        liveness. ``websockets`` exposes ``ws.ping()`` which returns a future that
        resolves when the matching pong returns; we await it under
        ``ping_timeout``. A failure/timeout raises so :meth:`connect` treats the
        new socket as dead (the caller's reconnect path then tries again) instead
        of declaring a half-open zombie healthy. No native ping support → skip
        (browser-style runtimes can't ping anyway; the staleness timer covers it).
        """
        ws = self._ws
        if ws is None or not hasattr(ws, "ping"):
            return
        timeout = self._ping_timeout if self._ping_timeout and self._ping_timeout > 0 else 3.0
        try:
            pong_waiter = await ws.ping()
            await asyncio.wait_for(pong_waiter, timeout=timeout)
        except asyncio.TimeoutError as e:
            raise ConnectionError(
                message=(
                    "Verify-after-reconnect failed: no pong within "
                    f"{timeout}s on the new socket"
                ),
                url=self.url,
                cause=e,
            )
        except Exception as e:
            # Socket already closed / ping unsupported at runtime → treat as dead.
            raise ConnectionError(
                message=f"Verify-after-reconnect failed: {e}",
                url=self.url,
                cause=e,
            )

    def _start_watchdog(self) -> None:
        """Start (or restart) the app-level stale-inbound watchdog (D1).

        Belt-and-suspenders to the native ping/pong: catches a wedged read pump
        or any path where the socket stays OPEN but no frames arrive. Disabled
        when ``stale_inbound_timeout <= 0``.
        """
        if self._stale_inbound_timeout <= 0:
            return
        self._stop_watchdog()
        self._watchdog_task = asyncio.create_task(self._watchdog_loop())

    def _stop_watchdog(self) -> None:
        """Cancel the staleness watchdog if running."""
        if self._watchdog_task is not None:
            self._watchdog_task.cancel()
            self._watchdog_task = None

    async def _watchdog_loop(self) -> None:
        """Reconnect if no inbound frame arrives within the staleness window (D1).

        Polls at a fraction of the window so detection latency is bounded. On a
        stale verdict it sets ``_reconnecting_due_to_stale`` (so the receive
        loop's own ConnectionClosed handler does NOT also reconnect) and closes
        the socket; closing wakes ``_receive_loop`` which runs the EXISTING
        reconnect path — the single authoritative reconnect mechanism.
        """
        timeout = self._stale_inbound_timeout
        # Poll often enough to detect within ~1/4 window, capped to a sane floor.
        poll = max(0.25, min(timeout / 4.0, 3.0))
        try:
            while self._connected and not self._closed:
                await asyncio.sleep(poll)
                if not self._connected or self._closed:
                    return
                last = self._last_inbound_monotonic
                if last is None:
                    continue
                if (time.monotonic() - last) >= timeout and not self._reconnecting_due_to_stale:
                    # Zombie: OPEN but silent. Force the existing reconnect path.
                    self._reconnecting_due_to_stale = True
                    self._emit("zombie_detected", {"stale_seconds": timeout})
                    ws = self._ws
                    if ws is not None:
                        try:
                            await ws.close(code=1006, reason="stale-inbound watchdog")
                        except Exception:
                            pass
                    return
        except asyncio.CancelledError:
            return

    async def _flush_control_queue(self) -> None:
        """Replay buffered control ops onto the live socket, in order (D5).

        Drains the drop-oldest control queue and re-sends each op SEQUENTIALLY
        (awaiting each send) so order is preserved — a queued ``speak`` must
        precede a queued ``clear``. Called from :meth:`connect` AFTER the fresh
        config + ready so the gateway accepts the ops (it rejects pre-config ops
        and any 2nd config on a live session). Mirrors the TS SDK's
        ``flushQueue``. If the socket dies mid-flush the remaining ops are
        re-buffered for the next reconnect.
        """
        if self._control_queue.is_empty:
            return
        for raw in self._control_queue.drain():
            payload = raw.decode() if isinstance(raw, bytes) else raw
            ws = self._ws
            if ws is None or not self._connected:
                # Lost the socket again before flush completed — re-buffer this
                # and stop; the next successful connect() replays the rest.
                self._control_queue.enqueue(payload)
                return
            try:
                await ws.send(payload)
                self._metrics.record_message_sent()
            except Exception:
                self._control_queue.enqueue(payload)
                return

    async def _send_config(self) -> None:
        """Send configuration message with all gateway-supported fields.

        The gateway requires both stt_config and tts_config when audio=true.
        If only one is provided, a minimal default for the other is included.
        """
        config: dict[str, Any] = {
            "type": "config",
            "audio": self.audio,
        }

        # Proxy/alias name (P3): a single top-level field the gateway resolves to a
        # full {stt,tts,llm,dag} bundle server-side. Composes with everything else —
        # any explicit stt/tts/conversation config below overrides the alias default.
        if self.alias:
            config["alias"] = self.alias

        # Include stream_id if requested
        if self.requested_stream_id:
            config["stream_id"] = self.requested_stream_id

        if self.stt_config:
            stt_dict: dict[str, Any] = {
                "provider": self.stt_config.provider,
                "language": self.stt_config.language,
                "sample_rate": self.stt_config.sample_rate,
                "channels": self.stt_config.channels,
                "punctuation": self.stt_config.punctuate,
                "encoding": self.stt_config.encoding,
                "model": self.stt_config.model or "nova-3",
            }
            # Include API key if provided (gateway allows per-request override)
            if self.api_key:
                stt_dict["api_key"] = self.api_key

            # Advanced STT features → stt_config.features{} (canonical SttFeatures,
            # gateway/src/core/stt/standard.rs). Flat-on-top was silently dropped;
            # nesting here makes the ~20 already-typed fields actually reach the wire.
            stt_features: dict[str, Any] = {}
            if self.stt_config.interim_results is not None:
                stt_features["interim_results"] = self.stt_config.interim_results
            if self.stt_config.diarize:
                stt_features["diarization"] = self.stt_config.diarize
            if self.stt_config.smart_format is not None:
                stt_features["smart_format"] = self.stt_config.smart_format
            if self.stt_config.profanity_filter:
                stt_features["profanity_filter"] = self.stt_config.profanity_filter
            if self.stt_config.keywords:
                # SttFeatures models boost terms as `keyterms`.
                stt_features["keyterms"] = list(self.stt_config.keywords)
            if stt_features:
                stt_dict["features"] = stt_features

            # custom_vocabulary has NO canonical SttFeatures field — route it
            # through the open `extras` passthrough so it still reaches the provider.
            if self.stt_config.custom_vocabulary:
                stt_dict["extras"] = {
                    "custom_vocabulary": list(self.stt_config.custom_vocabulary)
                }

            # Canonical in-stream translation (P5) → stt_config.translation. The
            # gateway folds Speechmatics/Gladia side-channels + the OpenAI/Groq
            # English fast-path into a uniform translations:[{lang,text}] array;
            # unsupported providers degrade with a config_warning (never a 400).
            translation = getattr(self.stt_config, "translation", None)
            if translation is not None:
                tw = translation.to_wire()
                if tw:
                    stt_dict["translation"] = tw

            # ML turn detection → stt_config.turn_detection (config.rs:345). This is
            # the REAL wire home for turn_detection (it was a dead no-op before).
            td = self.audio_features.turn_detection if self.audio_features else None
            if td is not None and td.enabled:
                td_wire: dict[str, Any] = {
                    "enabled": True,
                    "threshold": td.threshold,
                }
                # Eager end-of-turn needs BOTH stt_config.turn_detection.eager AND
                # conversation_config.eager_eot (config.rs:357-361). The SDK pairs
                # them: when the conversation loop opts into eager_eot, set the
                # detector's eager flag so the speculative-start path is armed.
                if (
                    self.conversation_config is not None
                    and getattr(self.conversation_config, "eager_eot", None)
                ):
                    td_wire["eager"] = True
                stt_dict["turn_detection"] = td_wire

            config["stt_config"] = stt_dict
        elif self.audio:
            # Gateway requires stt_config when audio=true - provide minimal default.
            # Skipped for audio=false (config-only) sessions.
            config["stt_config"] = {
                "provider": "deepgram",
                "language": "en-US",
                "sample_rate": 16000,
                "channels": 1,
                "punctuation": True,
                "encoding": "linear16",
                "model": "nova-3",
            }

        if self.tts_config:
            tts_dict: dict[str, Any] = {
                "provider": self.tts_config.provider,
                "voice_id": self.tts_config.voice_id or self.tts_config.voice,
                "model": self.tts_config.model or "aura-asteria-en",
            }
            # Abstract voice selection (P4): the gateway resolves a VoiceDescriptor
            # to a concrete provider voice_id server-side. Sent under
            # `tts_config.voice_descriptor` (snake_case object); a raw voice_id
            # always wins, so the gateway only uses this when voice_id is absent.
            vd = self.tts_config.voice_descriptor
            if vd is not None:
                vd_wire = vd.to_wire() if hasattr(vd, "to_wire") else dict(vd)
                if vd_wire:
                    tts_dict["voice_descriptor"] = vd_wire
            # Include optional TTS fields recognized by gateway
            if self.tts_config.sample_rate:
                tts_dict["sample_rate"] = self.tts_config.sample_rate
            if self.tts_config.audio_format:
                tts_dict["audio_format"] = self.tts_config.audio_format
            if self.tts_config.speed is not None:
                tts_dict["speaking_rate"] = self.tts_config.speed
            # Include API key if provided (gateway allows per-request override)
            if self.api_key:
                tts_dict["api_key"] = self.api_key
            # Emotion fields (Unified Emotion System - gateway supports these)
            if self.tts_config.emotion is not None:
                tts_dict["emotion"] = self.tts_config.emotion.value if hasattr(self.tts_config.emotion, 'value') else str(self.tts_config.emotion)
            if self.tts_config.emotion_intensity is not None:
                tts_dict["emotion_intensity"] = intensity_to_number(self.tts_config.emotion_intensity)
            if self.tts_config.delivery_style is not None:
                tts_dict["delivery_style"] = self.tts_config.delivery_style.value if hasattr(self.tts_config.delivery_style, 'value') else str(self.tts_config.delivery_style)
            if self.tts_config.emotion_description is not None:
                tts_dict["emotion_description"] = self.tts_config.emotion_description

            # Advanced TTS features → tts_config.features{} (canonical TtsFeatures,
            # gateway/src/core/tts/standard.rs). These were typed but never sent.
            tts_features: dict[str, Any] = {}
            if self.tts_config.speed is not None:
                # Also carried as top-level speaking_rate; mirror into features so
                # providers that read TtsFeatures.speed honor it too.
                tts_features["speed"] = self.tts_config.speed
            if self.tts_config.pitch is not None:
                tts_features["pitch"] = self.tts_config.pitch
            if self.tts_config.volume is not None:
                tts_features["volume"] = self.tts_config.volume
            if self.tts_config.stability is not None:
                tts_features["stability"] = self.tts_config.stability
            if self.tts_config.similarity_boost is not None:
                tts_features["similarity_boost"] = self.tts_config.similarity_boost
            if self.tts_config.style is not None:
                tts_features["style"] = self.tts_config.style
            if self.tts_config.use_speaker_boost is not None:
                tts_features["use_speaker_boost"] = self.tts_config.use_speaker_boost
            if tts_features:
                tts_dict["features"] = tts_features

            # Hume-specific knobs have NO canonical TtsFeatures field — route them
            # through the open `extras` passthrough (acting_instructions, instant_mode,
            # trailing_silence, voice_description).
            tts_extras: dict[str, Any] = {}
            if self.tts_config.acting_instructions is not None:
                tts_extras["acting_instructions"] = self.tts_config.acting_instructions
            if self.tts_config.instant_mode is not None:
                tts_extras["instant_mode"] = self.tts_config.instant_mode
            if self.tts_config.trailing_silence is not None:
                tts_extras["trailing_silence"] = self.tts_config.trailing_silence
            if self.tts_config.voice_description is not None:
                tts_extras["voice_description"] = self.tts_config.voice_description
            if tts_extras:
                tts_dict["extras"] = tts_extras

            config["tts_config"] = tts_dict
        elif self.audio:
            # Gateway requires tts_config when audio=true - provide minimal default.
            # Skipped for audio=false (config-only) sessions.
            config["tts_config"] = {
                "provider": "deepgram",
                "model": "aura-asteria-en",
                "voice_id": "aura-asteria-en",
                "sample_rate": 24000,
            }

        if self.livekit_config:
            config["livekit"] = self.livekit_config

        # DAG routing configuration
        if self.dag_config:
            dag_dict: dict[str, Any] = {}
            if self.dag_config.template:
                dag_dict["template"] = self.dag_config.template
            if self.dag_config.definition:
                dag_dict["definition"] = self.dag_config.definition.model_dump(by_alias=True)
            if self.dag_config.enable_metrics:
                dag_dict["enable_metrics"] = self.dag_config.enable_metrics
            if self.dag_config.timeout_ms != 30000:
                dag_dict["timeout_ms"] = self.dag_config.timeout_ms
            config["dag_config"] = dag_dict

        # Built-in conversation loop → conversation_config (config.rs:53-217).
        # The entire LLM loop + reasoning stack was 0% reachable before this.
        if self.conversation_config is not None:
            conv = self.conversation_config
            conv_dict: dict[str, Any] = {
                # base_url + model are required by the gateway.
                "base_url": conv.base_url,
                "model": conv.model,
            }
            # Emit every OTHER field only when explicitly set (None = let the
            # gateway apply its own default). Pydantic enum values are already
            # coerced to str via use_enum_values=True on the model.
            optional_fields = (
                "system_prompt", "api_key", "temperature", "max_tokens",
                "streaming", "max_history", "allow_interruption", "eager_eot",
                "provider_kind", "barge_in_min_words", "summarize_target_tokens",
                "mute_strategy", "strip_markdown", "user_idle_timeout_ms",
                "reasoning_effort", "latency_filler", "latency_filler_after_ms",
                "latency_filler_phrases", "reasoning_model", "reasoning_base_url",
                "reasoning_api_key", "reasoning_provider_kind", "reasoning_route",
                "reasoning_budget_ms", "degradation_message",
                "max_llm_calls_per_turn", "max_reasoning_tokens",
            )
            for fname in optional_fields:
                value = getattr(conv, fname, None)
                if value is not None:
                    conv_dict[fname] = value
            config["conversation_config"] = conv_dict

        self._config_sent_time = time.monotonic()
        await self._send_json(config)

    async def _send_json(self, data: dict[str, Any]) -> None:
        """Send a JSON control message, buffering it when offline (D5).

        Previously this RAISED ConnectionError on a dead socket, so a ``speak`` /
        ``clear`` / ``send_message`` issued during a reconnect blip was lost and
        the caller saw an exception — unlike the TS SDK, which enqueues and
        flushes on reopen. Now an offline control op is appended to the bounded
        drop-oldest control queue and replayed by :meth:`_flush_control_queue`
        once the session is ready again, so Python and TS behave identically.

        The ``config`` message is the one exception: it is sent only from inside
        :meth:`connect` while the socket is open and MUST go to the wire
        immediately (it is what makes the session ready), so it is never queued.
        """
        payload = json.dumps(data)

        # The `config` message is sent ONLY from inside connect() on a freshly
        # opened socket, and MUST reach the wire immediately (it is what makes
        # the session ready). It is gated on the socket OBJECT, not the
        # `connected` flag, and never buffered: a missing/failed socket here is a
        # hard failure connect() counts as a failed attempt.
        if data.get("type") == "config":
            if self._ws is None:
                raise ConnectionError(message="Not connected", url=self.url)
            await self._ws.send(payload)
            self._metrics.record_message_sent()
            return

        # Control ops (speak/clear/send_message/…). Mirror the TS SDK: when the
        # session is live, send now; otherwise buffer-and-flush (drop-oldest)
        # instead of raising ConnectionError, so an op issued during a reconnect
        # blip survives and replays on reopen.
        if self._connected and self._ws is not None:
            try:
                await self._ws.send(payload)
                self._metrics.record_message_sent()
                return
            except Exception:
                # Socket died mid-send — fall through and buffer for the replay.
                pass

        self._control_queue.enqueue(payload)

    async def _receive_loop(self) -> None:
        """Receive messages from WebSocket."""
        if not self._ws:
            return

        try:
            async for message in self._ws:
                self._metrics.record_message_received()
                # D1: every inbound frame proves the socket is alive — feed the
                # app-level staleness watchdog so it only fires on a truly silent
                # (zombie) connection.
                self._last_inbound_monotonic = time.monotonic()

                if isinstance(message, bytes):
                    # Binary audio data
                    self._metrics.record_audio_received(len(message))

                    # Calculate TTFB if this is first audio after speak
                    if self._speak_start_time:
                        ttfb = (time.monotonic() - self._speak_start_time) * 1000
                        self._metrics.record_tts_ttfb(ttfb)
                        self._speak_start_time = None

                    audio_event = AudioEvent(
                        type="audio",
                        audio=message,
                        format="linear16",
                        sample_rate=self.tts_config.sample_rate if self.tts_config else 24000,
                    )
                    self._emit("audio", audio_event)
                    await self._get_message_queue().put({"type": "audio", "audio": audio_event})

                else:
                    # JSON message
                    try:
                        data = json.loads(message)
                    except json.JSONDecodeError:
                        continue

                    msg_type = data.get("type")

                    if msg_type == "ready":
                        self._stream_id = data.get("stream_id")
                        # P3: capture the resolved-alias echo — the concrete
                        # providers the gateway resolved `alias` to (no secrets), so
                        # a developer can SEE what e.g. "support-bot" became.
                        self._resolved_alias = data.get("resolved_alias")
                        # Capture + drift-check the wire protocol version (plan W-K1).
                        # The gateway emits this specifically so SDKs detect a
                        # breaking contract change instead of silent field drift.
                        self._protocol_version = data.get("protocol_version")
                        if (
                            self._protocol_version is not None
                            and self._protocol_version.split(".")[0]
                            != PROTOCOL_VERSION.split(".")[0]
                        ):
                            msg = (
                                "WaaV protocol version mismatch: gateway sent "
                                f"{self._protocol_version!r}, SDK expects "
                                f"{PROTOCOL_VERSION!r}. Update bud-waav."
                            )
                            import sys
                            print(msg, file=sys.stderr)
                            # Also surface a typed warning so callers can react
                            # programmatically (parity with the TS SDK's
                            # PROTOCOL_VERSION_MISMATCH event). It's a WARNING, not
                            # a raise: the session still proceeds (best-effort).
                            warning = ProtocolVersionError(
                                message=msg,
                                gateway_version=self._protocol_version,
                                sdk_version=PROTOCOL_VERSION,
                            )
                            self._emit("warning", warning)
                            await self._get_message_queue().put(
                                {"type": "warning", "warning": warning}
                            )
                        self._get_ready_event().set()

                    elif msg_type == "stt_result":
                        # Calculate TTFT if this is first result after config
                        if self._config_sent_time:
                            ttft = (time.monotonic() - self._config_sent_time) * 1000
                            self._metrics.record_stt_ttft(ttft)
                            self._config_sent_time = None

                        # P5: uniform translations[] array (Speechmatics/Gladia/
                        # OpenAI-EN fast path), folded onto the transcript. Absent
                        # on non-translation sessions (gateway skips it when empty).
                        translations = [
                            Translation(
                                lang=t.get("lang", ""),
                                text=t.get("text", ""),
                                is_partial=t.get("is_partial", False),
                            )
                            for t in data.get("translations", [])
                        ]
                        result = STTResult(
                            text=data.get("transcript", ""),
                            is_final=data.get("is_final", False),
                            is_speech_final=data.get("is_speech_final", False),
                            confidence=data.get("confidence"),
                            speaker_id=data.get("speaker_id"),
                            translations=translations,
                        )
                        # Carry the optional speaker role through the queue (the
                        # gateway's cascade stt_result has no role, but a DAG
                        # text_output / assistant echo may set role="assistant").
                        # The Talk pipeline routes role="assistant" to a `bot_text`
                        # event; the STTResult shape itself is unchanged.
                        role = data.get("role")
                        self._emit("transcript", result)
                        await self._get_message_queue().put(
                            {"type": "transcript", "result": result, "role": role}
                        )

                    elif msg_type == "tts_audio":
                        # Base64 encoded audio
                        audio_data = base64.b64decode(data.get("audio", ""))
                        self._metrics.record_audio_received(len(audio_data))

                        if self._speak_start_time:
                            ttfb = (time.monotonic() - self._speak_start_time) * 1000
                            self._metrics.record_tts_ttfb(ttfb)
                            self._speak_start_time = None

                        audio_event = AudioEvent(
                            type="audio",
                            audio=audio_data,
                            format=data.get("format", "linear16"),
                            sample_rate=data.get("sample_rate", 24000),
                        )
                        self._emit("audio", audio_event)
                        await self._get_message_queue().put({"type": "audio", "audio": audio_event})

                    elif msg_type == "audio_end":
                        # End of TTS audio stream marker
                        self._emit("audio_end", data)
                        await self._get_message_queue().put({"type": "audio_end", "data": data})

                    elif msg_type == "tts_playback_complete":
                        self._emit("playback_complete", data.get("timestamp"))
                        await self._get_message_queue().put({"type": "playback_complete", "data": data})

                    elif msg_type == "turn_completed":
                        # Turn detection signals speaker finished their turn
                        self._emit("turn_completed", data)
                        await self._get_message_queue().put({"type": "turn_completed", "data": data})

                    elif msg_type == "vad_event":
                        # Voice Activity Detection event (speech_start/speech_end)
                        self._emit("vad_event", data)
                        await self._get_message_queue().put({"type": "vad_event", "data": data})

                    elif msg_type == "message":
                        self._emit("message", data.get("message"))
                        await self._get_message_queue().put({"type": "message", "data": data.get("message")})

                    elif msg_type == "participant_disconnected":
                        self._emit("participant_disconnected", data.get("participant"))
                        await self._get_message_queue().put({"type": "participant_disconnected", "data": data.get("participant")})

                    elif msg_type == "error":
                        from ..errors import BudError
                        error = BudError(
                            message=data.get("message", "Unknown error"),
                            code=data.get("code"),
                        )
                        self._emit("error", error)
                        await self._get_message_queue().put({"type": "error", "error": error})

                    elif msg_type == "pong":
                        self._emit("pong", data.get("timestamp"))

                    elif msg_type == "auth_required":
                        # Server requires authentication - send token
                        if self.api_key:
                            await self._send_json({
                                "type": "auth",
                                "token": self.api_key,
                            })
                        else:
                            from ..errors import BudError
                            error = BudError(
                                message="Server requires authentication but no API key provided",
                                code="AUTH_REQUIRED",
                            )
                            self._emit("error", error)
                            await self._get_message_queue().put({"type": "error", "error": error})

                    elif msg_type == "authenticated":
                        # Auth successful
                        self._authenticated = True
                        self._get_auth_event().set()
                        self._emit("authenticated", data.get("id"))

                    elif msg_type == "sip_transfer_error":
                        from ..errors import BudError
                        error = BudError(
                            message=data.get("message", "SIP transfer failed"),
                            code="SIP_TRANSFER_ERROR",
                        )
                        self._emit("error", error)
                        await self._get_message_queue().put({"type": "error", "error": error})

                    elif msg_type == "plugin_response":
                        # Plugin-specific response
                        self._emit("plugin_response", data)
                        await self._get_message_queue().put({"type": "plugin_response", "data": data})

                    elif msg_type == "config_warning":
                        # Non-fatal gateway advisory (e.g. reasoning_model_on_voice_path,
                        # emotion_ignored_for_provider). Surface it as a typed warning so a
                        # silent server-side degrade becomes visible to the developer.
                        self._emit("config_warning", data)
                        await self._get_message_queue().put({"type": "config_warning", "data": data})

                    elif msg_type is not None:
                        # Forward-compat: never silently drop an unrecognized server
                        # message — surface it so new gateway events are observable.
                        self._emit("server_message", data)
                        await self._get_message_queue().put({"type": "server_message", "data": data})

        except websockets.ConnectionClosed:
            self._connected = False
            self._emit("close")

            if self.reconnect_config.enabled and not self._closed:
                await self._reconnect()

        except Exception as e:
            self._emit("error", e)

    def _note_session_ended(self) -> None:
        """Feed the quick-failure breaker with the just-ended session's lifespan (D4a).

        Called at the top of :meth:`_reconnect` (the prior session has just
        dropped). A session that became ready then died within
        ``min_stable_seconds`` is a "quick failure" — the signature of a doomed
        config the gateway keeps accepting at the handshake but immediately tears
        down. We count consecutive quick failures; a session that SURVIVED the
        stable window resets the run (and clears the fatal latch).
        """
        if self._connected_at is None:
            # Never reached ready this cycle (pure handshake failure). Leave the
            # run as-is; the per-attempt loop below counts those separately.
            return
        survival = time.monotonic() - self._connected_at
        self._connected_at = None
        if survival < self.reconnect_config.min_stable_seconds:
            self._consecutive_quick_failures += 1
        else:
            # A genuinely stable session recovered the line — forgive the run.
            self._consecutive_quick_failures = 0
            self._fatal = False

    def _trip_if_doomed(self, last_error: Optional[Exception]) -> None:
        """Raise a typed FATAL once the quick-failure threshold is hit (D4a).

        Stops the SDK's OWN reconnect storm against an unrecoverable config
        instead of looping ``max_retries`` times. This is the client analog of
        the gateway's per-provider CircuitBreaker FATAL fast-trip (upstream-only,
        never on the client wire) — it bounds retries of a doomed CONFIG, it does
        not mirror the gateway's breaker.
        """
        if self._consecutive_quick_failures >= self.reconnect_config.quick_failure_threshold:
            self._fatal = True
            err = FatalConnectionError(
                message=(
                    "Reconnect stopped: "
                    f"{self._consecutive_quick_failures} connections in a row each "
                    f"survived < {self.reconnect_config.min_stable_seconds}s "
                    "(likely an unrecoverable config — check API key / provider / "
                    "config)."
                ),
                consecutive_quick_failures=self._consecutive_quick_failures,
                last_error=last_error,
            )
            self._emit("error", err)
            raise err

    async def _reconnect(self) -> None:
        """Reconnect with a fast first hop + decorrelated jitter, gated by the
        quick-failure breaker (D4a/D4b).

        Backoff is decorrelated jitter — ``sleep = min(cap, U(base, prev*3))``
        (SOTA / AWS) — NOT raw ``base * multiplier**n``, which compounded into the
        measured storm. The first hop starts at ``initial_delay_ms`` (200ms) for
        sub-second recovery; the ceiling stays at ``max_delay_ms`` (30s). Before
        every wait the breaker may trip FATAL so the fast first hop can never
        storm a doomed config harder.
        """
        # Account the session that just dropped, then trip BEFORE any retry so a
        # doomed config that died fast doesn't even get the fast first hop.
        self._note_session_ended()
        self._stop_watchdog()
        if self._closed:
            return
        self._trip_if_doomed(None)

        base = float(self.reconnect_config.initial_delay_ms)
        cap = float(self.reconnect_config.max_delay_ms)
        # Decorrelated-jitter running delay (ms). Seeded at the fast first hop.
        delay = base
        last_error: Optional[Exception] = None

        for attempt in range(self.reconnect_config.max_retries):
            # Check if disconnect() was called - stop reconnecting
            if self._closed:
                return

            await asyncio.sleep(delay / 1000.0)

            # Check again after sleep in case disconnect() was called during wait
            if self._closed:
                return

            try:
                self._get_ready_event().clear()
                await self.connect()
                self._metrics.record_reconnect()
                self._emit("reconnect", attempt + 1)
                return

            except FatalConnectionError:
                # Breaker tripped inside connect()'s own reconnect chain — propagate.
                raise
            except Exception as e:
                last_error = e
                # A reconnect attempt that briefly became ready then died is a
                # quick failure too; fold it into the run and maybe trip FATAL.
                self._note_session_ended()
                self._trip_if_doomed(last_error)
                # Decorrelated jitter: next = min(cap, uniform(base, delay*3)).
                lo = base
                hi = max(delay * 3.0, base)
                delay = min(cap, random.uniform(lo, hi))

        raise ReconnectError(
            message=f"Failed to reconnect after {self.reconnect_config.max_retries} attempts",
            attempts=self.reconnect_config.max_retries,
            last_error=last_error,
        )

    async def disconnect(self, close_timeout: float = 1.0) -> None:
        """Disconnect from the WebSocket server.

        D4: the close handshake is BOUNDED (was unbounded). ``ws.close()`` waits
        for the server's Close echo, which a wedged/zombie gateway will never
        send — leaving disconnect() hung. We fire the close then await it under
        ``close_timeout`` (default 1s, Pipecat fastapi.py:187 pattern) and move
        on regardless, so shutdown is prompt even against a dead peer.

        Args:
            close_timeout: Max seconds to await the close handshake before
                abandoning it and dropping the socket.
        """
        self._closed = True
        self._connected = False

        # Stop the liveness watchdog first so it can't race a reconnect during teardown.
        self._stop_watchdog()

        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
            self._receive_task = None

        # Cancel any pending handler tasks
        if self._handler_tasks:
            for task in list(self._handler_tasks):
                task.cancel()
            # Wait for all tasks to complete (with suppressed CancelledError)
            await asyncio.gather(*self._handler_tasks, return_exceptions=True)
            self._handler_tasks.clear()

        if self._ws:
            ws = self._ws
            self._ws = None
            try:
                await asyncio.wait_for(ws.close(), timeout=close_timeout)
            except (asyncio.TimeoutError, Exception):
                # Bounded: a peer that never echoes Close must not hang shutdown.
                # The socket is abandoned; the OS reclaims it.
                pass

        self._emit("close")

    async def send_audio(self, audio: Union[bytes, bytearray]) -> None:
        """
        Send audio data.

        While disconnected the frame is buffered in a BOUNDED, drop-oldest deque
        (D5): if the backlog is already full the stalest frame is shed before the
        new one is appended (an ``audio_dropped`` event fires). This mirrors the
        gateway's own DroppableAudio drop-oldest egress discipline so a slow or
        absent gateway can never grow uplink latency without limit; on reconnect
        the most-recent frames replay (rather than being cleared outright).

        Args:
            audio: PCM audio data (16-bit signed integer)
        """
        if not self._connected:
            # deque(maxlen=…) drops the oldest automatically on overflow; surface
            # it so callers can SEE backpressure shedding.
            if len(self._pending_audio) >= (self._pending_audio.maxlen or 0):
                self._emit("audio_dropped", {"reason": "uplink_backlog_full"})
            self._pending_audio.append(bytes(audio))
            return

        if not self._ws:
            raise ConnectionError(message="Not connected", url=self.url)

        await self._ws.send(bytes(audio))
        self._metrics.record_audio_sent(len(audio))

    async def speak(
        self,
        text: str,
        flush: bool = False,
        allow_interruption: bool = True,
    ) -> None:
        """
        Send text for speech synthesis.

        Args:
            text: Text to synthesize
            flush: Whether to flush the TTS buffer immediately
            allow_interruption: Whether this TTS can be interrupted
        """
        self._speak_start_time = time.monotonic()
        await self._send_json({
            "type": "speak",
            "text": text,
            "flush": flush,
            "allow_interruption": allow_interruption,
        })

    async def clear(self) -> None:
        """Clear/stop current TTS playback."""
        await self._send_json({"type": "clear"})

    async def send_message(
        self,
        message: str,
        role: str = "user",
        topic: Optional[str] = None,
    ) -> None:
        """
        Send a data message to other participants.

        Args:
            message: Message content
            role: Message role (user, assistant, system)
            topic: Optional topic/channel
        """
        msg: dict[str, Any] = {
            "type": "send_message",
            "message": message,
            "role": role,
        }
        if topic:
            msg["topic"] = topic
        await self._send_json(msg)

    async def sip_transfer(self, transfer_to: str) -> None:
        """
        Transfer a SIP call.

        Args:
            transfer_to: Phone number to transfer to
        """
        await self._send_json({
            "type": "sip_transfer",
            "transfer_to": transfer_to,
        })

    async def send_audio_end(self) -> None:
        """Signal end of audio input stream to the server."""
        await self._send_json({"type": "audio_end"})

    async def send_custom(self, message_type: str, payload: dict[str, Any]) -> None:
        """
        Send a custom plugin message.

        Args:
            message_type: Plugin message type identifier
            payload: Message payload
        """
        await self._send_json({
            "type": "custom",
            "message_type": message_type,
            "payload": payload,
        })

    def get_metrics(self) -> SessionMetrics:
        """Get current session metrics."""
        return self._metrics.get_metrics()

    def reset_metrics(self) -> None:
        """Reset all metrics."""
        self._metrics.reset()

    async def __aiter__(self) -> AsyncIterator[dict[str, Any]]:
        """Iterate over incoming messages."""
        queue = self._get_message_queue()
        while self._connected or not queue.empty():
            try:
                message = await asyncio.wait_for(queue.get(), timeout=0.1)
                yield message
            except asyncio.TimeoutError:
                if not self._connected:
                    break
                continue

    async def __aenter__(self) -> "WebSocketSession":
        """Async context manager entry."""
        await self.connect()
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.disconnect()
