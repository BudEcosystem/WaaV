"""
WebSocket session for Bud Foundry Gateway
"""

import asyncio
import json
import time
import random
from typing import Any, AsyncIterator, Callable, Optional, Union
from dataclasses import dataclass, field

import websockets
from websockets.asyncio.client import ClientConnection

from ..types import STTConfig, TTSConfig, STTResult, TranscriptEvent, AudioEvent
from ..errors import ConnectionError, ReconnectError, TimeoutError


@dataclass
class ReconnectConfig:
    """Reconnection configuration."""

    enabled: bool = True
    initial_delay_ms: int = 1000
    max_delay_ms: int = 30000
    multiplier: float = 1.5
    max_retries: int = 10
    jitter: float = 0.2


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
    """WebSocket session for real-time communication with Bud Foundry Gateway."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        stt_config: Optional[STTConfig] = None,
        tts_config: Optional[TTSConfig] = None,
        livekit_config: Optional[dict[str, Any]] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ):
        """
        Initialize WebSocket session.

        Args:
            url: WebSocket URL of the gateway
            api_key: Optional API key for authentication
            stt_config: STT configuration
            tts_config: TTS configuration
            livekit_config: LiveKit configuration
            reconnect: Reconnection configuration
        """
        self.url = url
        self.api_key = api_key
        self.stt_config = stt_config
        self.tts_config = tts_config
        self.livekit_config = livekit_config
        self.reconnect_config = reconnect or ReconnectConfig()

        self._ws: Optional[ClientConnection] = None
        self._stream_id: Optional[str] = None
        self._connected = False
        self._connecting = False
        self._closed = False
        self._ready_event = asyncio.Event()
        self._message_queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
        self._pending_audio: list[bytes] = []
        self._receive_task: Optional[asyncio.Task[None]] = None

        self._metrics = MetricsCollector()
        self._event_handlers: dict[str, list[Callable[..., Any]]] = {}

        # Timing for metrics
        self._config_sent_time: Optional[float] = None
        self._speak_start_time: Optional[float] = None

    @property
    def connected(self) -> bool:
        """Whether the session is connected."""
        return self._connected

    @property
    def stream_id(self) -> Optional[str]:
        """Get the stream ID."""
        return self._stream_id

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
                    asyncio.create_task(result)
            except Exception as e:
                self._emit("error", e)

    async def connect(self, timeout: float = 10.0) -> None:
        """
        Connect to the WebSocket server.

        Args:
            timeout: Connection timeout in seconds

        Raises:
            ConnectionError: If connection fails
            TimeoutError: If connection times out
        """
        if self._connected or self._connecting:
            return

        self._connecting = True
        self._closed = False
        start_time = time.time()

        try:
            headers = {}
            if self.api_key:
                headers["Authorization"] = f"Bearer {self.api_key}"

            try:
                self._ws = await asyncio.wait_for(
                    websockets.connect(
                        self.url,
                        additional_headers=headers,
                    ),
                    timeout=timeout,
                )
            except asyncio.TimeoutError:
                raise TimeoutError(
                    message=f"WebSocket connection timed out after {timeout}s",
                    timeout_ms=int(timeout * 1000),
                    operation="connect",
                )
            except Exception as e:
                raise ConnectionError(
                    message=f"Failed to connect to {self.url}: {e}",
                    url=self.url,
                    cause=e,
                )

            connect_time = (time.time() - start_time) * 1000
            self._metrics.record_ws_connect(connect_time)

            self._connected = True
            self._connecting = False

            # Start receive task
            self._receive_task = asyncio.create_task(self._receive_loop())

            # Send config message
            await self._send_config()

            # Wait for ready
            try:
                await asyncio.wait_for(self._ready_event.wait(), timeout=timeout)
            except asyncio.TimeoutError:
                raise TimeoutError(
                    message="Timeout waiting for ready message",
                    timeout_ms=int(timeout * 1000),
                    operation="ready",
                )

            # Send any pending audio
            for audio in self._pending_audio:
                await self.send_audio(audio)
            self._pending_audio.clear()

            self._emit("ready", self._stream_id)

        except Exception:
            self._connecting = False
            self._connected = False
            raise

    async def _send_config(self) -> None:
        """Send configuration message."""
        config: dict[str, Any] = {
            "type": "config",
            "audio": True,
        }

        if self.stt_config:
            config["stt_config"] = {
                "provider": self.stt_config.provider,
                "language": self.stt_config.language,
                "sample_rate": self.stt_config.sample_rate,
                "channels": self.stt_config.channels,
                "punctuation": self.stt_config.punctuate,
                "encoding": self.stt_config.encoding,
                "model": self.stt_config.model or "nova-3",
            }

        if self.tts_config:
            config["tts_config"] = {
                "provider": self.tts_config.provider,
                "voice_id": self.tts_config.voice_id or self.tts_config.voice,
                "sample_rate": self.tts_config.sample_rate,
                "model": self.tts_config.model or "aura-asteria-en",
            }

        if self.livekit_config:
            config["livekit"] = self.livekit_config

        self._config_sent_time = time.time()
        await self._send_json(config)

    async def _send_json(self, data: dict[str, Any]) -> None:
        """Send a JSON message."""
        if not self._ws:
            raise ConnectionError(message="Not connected", url=self.url)

        await self._ws.send(json.dumps(data))
        self._metrics.record_message_sent()

    async def _receive_loop(self) -> None:
        """Receive messages from WebSocket."""
        if not self._ws:
            return

        try:
            async for message in self._ws:
                self._metrics.record_message_received()

                if isinstance(message, bytes):
                    # Binary audio data
                    self._metrics.record_audio_received(len(message))

                    # Calculate TTFB if this is first audio after speak
                    if self._speak_start_time:
                        ttfb = (time.time() - self._speak_start_time) * 1000
                        self._metrics.record_tts_ttfb(ttfb)
                        self._speak_start_time = None

                    audio_event = AudioEvent(
                        type="audio",
                        audio=message,
                        format="linear16",
                        sample_rate=self.tts_config.sample_rate if self.tts_config else 24000,
                    )
                    self._emit("audio", audio_event)
                    await self._message_queue.put({"type": "audio", "audio": audio_event})

                else:
                    # JSON message
                    try:
                        data = json.loads(message)
                    except json.JSONDecodeError:
                        continue

                    msg_type = data.get("type")

                    if msg_type == "ready":
                        self._stream_id = data.get("stream_id")
                        self._ready_event.set()

                    elif msg_type == "stt_result":
                        # Calculate TTFT if this is first result after config
                        if self._config_sent_time:
                            ttft = (time.time() - self._config_sent_time) * 1000
                            self._metrics.record_stt_ttft(ttft)
                            self._config_sent_time = None

                        result = STTResult(
                            text=data.get("transcript", ""),
                            is_final=data.get("is_final", False),
                            confidence=data.get("confidence"),
                            speaker_id=data.get("speaker_id"),
                        )
                        self._emit("transcript", result)
                        await self._message_queue.put({"type": "transcript", "result": result})

                    elif msg_type == "tts_audio":
                        # Base64 encoded audio
                        import base64
                        audio_data = base64.b64decode(data.get("audio", ""))
                        self._metrics.record_audio_received(len(audio_data))

                        if self._speak_start_time:
                            ttfb = (time.time() - self._speak_start_time) * 1000
                            self._metrics.record_tts_ttfb(ttfb)
                            self._speak_start_time = None

                        audio_event = AudioEvent(
                            type="audio",
                            audio=audio_data,
                            format=data.get("format", "linear16"),
                            sample_rate=data.get("sample_rate", 24000),
                        )
                        self._emit("audio", audio_event)
                        await self._message_queue.put({"type": "audio", "audio": audio_event})

                    elif msg_type == "tts_playback_complete":
                        self._emit("playback_complete", data.get("timestamp"))
                        await self._message_queue.put({"type": "playback_complete", "data": data})

                    elif msg_type == "message":
                        self._emit("message", data.get("message"))
                        await self._message_queue.put({"type": "message", "data": data.get("message")})

                    elif msg_type == "participant_disconnected":
                        self._emit("participant_disconnected", data.get("participant"))
                        await self._message_queue.put({"type": "participant_disconnected", "data": data.get("participant")})

                    elif msg_type == "error":
                        from ..errors import BudError
                        error = BudError(
                            message=data.get("message", "Unknown error"),
                            code=data.get("code"),
                        )
                        self._emit("error", error)
                        await self._message_queue.put({"type": "error", "error": error})

                    elif msg_type == "pong":
                        self._emit("pong", data.get("timestamp"))

        except websockets.ConnectionClosed:
            self._connected = False
            self._emit("close")

            if self.reconnect_config.enabled and not self._closed:
                await self._reconnect()

        except Exception as e:
            self._emit("error", e)

    async def _reconnect(self) -> None:
        """Attempt to reconnect with exponential backoff."""
        delay: float = float(self.reconnect_config.initial_delay_ms)
        last_error: Optional[Exception] = None

        for attempt in range(self.reconnect_config.max_retries):
            # Add jitter
            jitter = delay * self.reconnect_config.jitter * (random.random() * 2 - 1)
            wait_time = (delay + jitter) / 1000

            await asyncio.sleep(wait_time)

            try:
                self._ready_event.clear()
                await self.connect()
                self._metrics.record_reconnect()
                self._emit("reconnect", attempt + 1)
                return

            except Exception as e:
                last_error = e
                delay = min(
                    delay * self.reconnect_config.multiplier,
                    float(self.reconnect_config.max_delay_ms),
                )

        raise ReconnectError(
            message=f"Failed to reconnect after {self.reconnect_config.max_retries} attempts",
            attempts=self.reconnect_config.max_retries,
            last_error=last_error,
        )

    async def disconnect(self) -> None:
        """Disconnect from the WebSocket server."""
        self._closed = True
        self._connected = False

        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
            self._receive_task = None

        if self._ws:
            await self._ws.close()
            self._ws = None

        self._emit("close")

    async def send_audio(self, audio: Union[bytes, bytearray]) -> None:
        """
        Send audio data.

        Args:
            audio: PCM audio data (16-bit signed integer)
        """
        if not self._connected:
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
        self._speak_start_time = time.time()
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

    async def ping(self) -> None:
        """Send a ping message."""
        await self._send_json({
            "type": "ping",
            "timestamp": int(time.time() * 1000),
        })

    def get_metrics(self) -> SessionMetrics:
        """Get current session metrics."""
        return self._metrics.get_metrics()

    def reset_metrics(self) -> None:
        """Reset all metrics."""
        self._metrics.reset()

    async def __aiter__(self) -> AsyncIterator[dict[str, Any]]:
        """Iterate over incoming messages."""
        while self._connected or not self._message_queue.empty():
            try:
                message = await asyncio.wait_for(self._message_queue.get(), timeout=0.1)
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
