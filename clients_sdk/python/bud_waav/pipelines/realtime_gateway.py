"""
GatewayRealtime - gateway-native, provider-agnostic realtime S2S client.

Unlike :class:`bud_waav.pipelines.realtime.BudRealtime` (which can speak either a
provider-NATIVE protocol or the gateway protocol, and only knows OpenAI/Hume),
this client speaks **only** the WaaV gateway's provider-agnostic ``/realtime``
WebSocket protocol. Because the gateway abstracts every provider behind one wire,
a single SDK surface works for ALL realtime providers — the provider is just a
field on the config message.

The exact wire is mirrored from the gateway handler
(``gateway/src/handlers/realtime/messages.rs`` + ``handler.rs``):

OUT (client -> gateway), all JSON text frames (audio is raw binary):
  * ``{"type":"config", ...RealtimeSessionConfig}``  — first message, selects
    provider/model/voice/instructions/turn_detection/tools/... and connects the
    provider server-side.
  * raw binary PCM frames                              — input audio (NOT base64).
  * ``{"type":"text","text":...}``
  * ``{"type":"create_response", "response"?:{...}}``  — optional per-response override.
  * ``{"type":"cancel_response"}``                     — barge-in / interrupt.
  * ``{"type":"commit_audio"}`` / ``{"type":"clear_audio"}``
  * ``{"type":"function_result","call_id":...,"result":...}``
  * ``{"type":"update_session", ...RealtimeSessionConfig}``

IN (gateway -> client):
  * ``{"type":"session_created","session_id":...,"provider":...,"model":...}``
  * ``{"type":"session_updated"}``
  * ``{"type":"transcript","text":...,"role":"user|assistant","is_final":bool}``
  * ``{"type":"speech_event","event":"started|stopped","audio_ms":int}``
  * ``{"type":"function_call","call_id":...,"name":...,"arguments":...}``
  * ``{"type":"response_started","response_id":...}``
  * ``{"type":"response_done","response_id":...}``
  * ``{"type":"error","code"?:...,"message":...}``
  * ``{"type":"closing","reason":...}``
  * raw binary frames                                  — output audio (PCM).

Migrated to ``websockets.asyncio.client`` (the legacy ``websockets.legacy`` API
is deprecated).
"""

from __future__ import annotations

import asyncio
import json
import logging
from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Callable, Optional, Union

import websockets
from websockets.asyncio.client import ClientConnection, connect as ws_connect

from ..types import TranscriptEvent, AudioEvent

logger = logging.getLogger(__name__)

# =============================================================================
# Supported realtime providers (provider-agnostic — the gateway routes each one
# behind the SAME /realtime wire). Mirror of the gateway
# `get_supported_realtime_providers()` (gateway/src/core/realtime/mod.rs:187).
# =============================================================================

REALTIME_PROVIDERS: tuple[str, ...] = (
    "openai",
    "hume",
    "azure",
    "grok",
    "inworld",
    "deepgram",
    "elevenlabs",
    "gemini",
    "ultravox",
    "nova_sonic",
    "speechmatics",
    "yandex",
)
"""The 12 realtime/S2S providers reachable through the gateway ``/realtime``
endpoint. Pass any of these as ``provider`` to :meth:`BudClient.realtime`."""


def get_supported_realtime_providers() -> tuple[str, ...]:
    """Return the realtime providers the gateway ``/realtime`` endpoint supports."""
    return REALTIME_PROVIDERS


# =============================================================================
# Events (the unified stream)
# =============================================================================


@dataclass
class FunctionCallEvent:
    """Function/tool call requested by the model."""

    name: str
    arguments: dict[str, Any]
    call_id: str


@dataclass
class SpeechEvent:
    """Server VAD speech-activity event."""

    event: str  # "started" | "stopped"
    audio_ms: int = 0


@dataclass
class ErrorEvent:
    """Error advisory from the gateway (NOT necessarily fatal)."""

    message: str
    code: Optional[str] = None


@dataclass
class SessionCreatedEvent:
    """Emitted once the gateway has connected the provider."""

    session_id: str
    provider: str
    model: str


@dataclass
class ResponseEvent:
    """Response lifecycle marker (started/done)."""

    response_id: str


@dataclass
class RealtimeEvent:
    """A single item on the unified ``async for`` event stream.

    ``type`` is one of: ``session_created`` | ``transcript`` | ``audio`` |
    ``speech_event`` | ``function_call`` | ``response_started`` |
    ``response_done`` | ``error`` | ``closing``. The matching payload attribute is
    populated; the rest are ``None``.
    """

    type: str
    transcript: Optional[TranscriptEvent] = None
    audio: Optional[AudioEvent] = None
    function_call: Optional[FunctionCallEvent] = None
    speech: Optional[SpeechEvent] = None
    error: Optional[ErrorEvent] = None
    session: Optional[SessionCreatedEvent] = None
    response: Optional[ResponseEvent] = None
    reason: Optional[str] = None


# =============================================================================
# Config
# =============================================================================


@dataclass
class GatewayTurnDetection:
    """Turn-detection config for the gateway ``/realtime`` config message.

    Serializes to the gateway ``TurnDetectionConfig`` tagged enum
    (``{"mode": "server_vad"|"semantic"|"manual", ...}``).
    """

    mode: str = "server_vad"  # "server_vad" | "semantic" | "manual"
    threshold: Optional[float] = None
    silence_duration_ms: Optional[int] = None
    prefix_padding_ms: Optional[int] = None
    eagerness: Optional[str] = None  # semantic only

    def to_wire(self) -> dict[str, Any]:
        wire: dict[str, Any] = {"mode": self.mode}
        if self.mode == "server_vad":
            if self.threshold is not None:
                wire["threshold"] = self.threshold
            if self.silence_duration_ms is not None:
                wire["silence_duration_ms"] = self.silence_duration_ms
            if self.prefix_padding_ms is not None:
                wire["prefix_padding_ms"] = self.prefix_padding_ms
        elif self.mode == "semantic" and self.eagerness is not None:
            wire["eagerness"] = self.eagerness
        return wire


@dataclass
class RealtimeTool:
    """Function/tool definition exposed to the model."""

    name: str
    description: Optional[str] = None
    parameters: Optional[dict[str, Any]] = None

    def to_wire(self) -> dict[str, Any]:
        fn: dict[str, Any] = {"name": self.name}
        if self.description is not None:
            fn["description"] = self.description
        if self.parameters is not None:
            fn["parameters"] = self.parameters
        return {"type": "function", "function": fn}


@dataclass
class GatewayRealtimeConfig:
    """Provider-agnostic gateway ``/realtime`` session config.

    Mirrors the gateway ``RealtimeSessionConfig``
    (gateway/src/handlers/realtime/messages.rs). Every field is optional except
    ``provider``; unset fields are omitted from the wire so the gateway/provider
    defaults apply.
    """

    provider: str
    model: Optional[str] = None
    voice: Optional[str] = None
    instructions: Optional[str] = None
    temperature: Optional[float] = None
    max_response_tokens: Optional[int] = None
    turn_detection: Optional[GatewayTurnDetection] = None
    tools: list[RealtimeTool] = field(default_factory=list)
    modalities: Optional[list[str]] = None
    transcribe_input: Optional[bool] = None
    transcription_model: Optional[str] = None
    input_audio_format: Optional[str] = None
    output_audio_format: Optional[str] = None
    reasoning_effort: Optional[str] = None
    input_audio_noise_reduction: Optional[str] = None
    alias: Optional[str] = None

    def to_config_message(self) -> dict[str, Any]:
        """Build the ``{"type":"config", ...}`` wire message (provider-agnostic).

        This is the EXACT shape the gateway ``RealtimeIncomingMessage::Config``
        deserializes — no provider-native fields leak in.
        """
        msg: dict[str, Any] = {"type": "config", "provider": self.provider}
        if self.model is not None:
            msg["model"] = self.model
        if self.voice is not None:
            msg["voice"] = self.voice
        if self.instructions is not None:
            msg["instructions"] = self.instructions
        if self.temperature is not None:
            msg["temperature"] = self.temperature
        if self.max_response_tokens is not None:
            msg["max_response_tokens"] = self.max_response_tokens
        if self.turn_detection is not None:
            msg["turn_detection"] = self.turn_detection.to_wire()
        if self.tools:
            msg["tools"] = [t.to_wire() for t in self.tools]
        if self.modalities is not None:
            msg["modalities"] = self.modalities
        if self.transcribe_input is not None:
            msg["transcribe_input"] = self.transcribe_input
        if self.transcription_model is not None:
            msg["transcription_model"] = self.transcription_model
        if self.input_audio_format is not None:
            msg["input_audio_format"] = self.input_audio_format
        if self.output_audio_format is not None:
            msg["output_audio_format"] = self.output_audio_format
        if self.reasoning_effort is not None:
            msg["reasoning_effort"] = self.reasoning_effort
        if self.input_audio_noise_reduction is not None:
            msg["input_audio_noise_reduction"] = self.input_audio_noise_reduction
        if self.alias is not None:
            msg["alias"] = self.alias
        return msg


# =============================================================================
# GatewayRealtime session
# =============================================================================


class GatewayRealtime:
    """A gateway-native, provider-agnostic realtime session.

    Speaks ONLY the gateway ``/realtime`` wire, so the same object works for all
    12 realtime providers. Construct via :meth:`BudClient.realtime` (recommended)
    or directly.

    Two consumption styles (use either or both):

    * Callbacks: ``session.on("transcript", handler)`` (events: ``audio``,
      ``transcript``, ``function_call``, ``speech_event``, ``session_created``,
      ``response_started``, ``response_done``, ``error``, ``connected``,
      ``disconnected``, ``closing``).
    * Unified async stream: ``async for event in session: ...`` yielding
      :class:`RealtimeEvent`.

    Example::

        session = bud.realtime(provider="openai", voice="alloy",
                               instructions="You are helpful.")
        async with session:
            await session.send_audio(pcm_bytes)
            async for ev in session:
                if ev.type == "transcript":  print(ev.transcript.text)
                elif ev.type == "audio":     play(ev.audio.audio)
                elif ev.type == "error":     print("ERR", ev.error.message)
    """

    # Output sample rate hint for AudioEvent (gateway default S2S output is pcm16
    # @ 24kHz for OpenAI-family providers).
    _OUTPUT_SAMPLE_RATE = 24000

    def __init__(
        self,
        url: str,
        config: GatewayRealtimeConfig,
        api_key: Optional[str] = None,
    ):
        """
        Args:
            url: Gateway ``/realtime`` WebSocket URL (e.g. ``ws://host:port/realtime``).
            config: Provider-agnostic session config.
            api_key: Optional gateway auth bearer token (sent as ``Authorization``).
        """
        if not config.provider:
            raise ValueError("provider is required")
        if config.provider.lower() not in REALTIME_PROVIDERS:
            raise ValueError(
                f"Unsupported realtime provider: {config.provider!r}. "
                f"Supported: {', '.join(REALTIME_PROVIDERS)}"
            )

        self._url = url
        self._config = config
        self._api_key = api_key
        self._ws: Optional[ClientConnection] = None
        self._connected = False
        self._session_id: Optional[str] = None
        self._send_timeout = 30.0

        # Callback handlers.
        self._handlers: dict[str, list[Callable[..., Any]]] = {
            "audio": [],
            "transcript": [],
            "function_call": [],
            "speech_event": [],
            "session_created": [],
            "response_started": [],
            "response_done": [],
            "error": [],
            "connected": [],
            "disconnected": [],
            "closing": [],
        }

        # Unified async event stream plumbing.
        self._queue: asyncio.Queue[Optional[RealtimeEvent]] = asyncio.Queue()
        self._receive_task: Optional[asyncio.Task[None]] = None

    # ------------------------------------------------------------------ props
    @property
    def config(self) -> GatewayRealtimeConfig:
        return self._config

    @property
    def provider(self) -> str:
        return self._config.provider

    @property
    def connected(self) -> bool:
        return self._connected

    @property
    def session_id(self) -> Optional[str]:
        return self._session_id

    # ------------------------------------------------------------- callbacks
    def on(self, event: str, handler: Callable[..., Any]) -> None:
        """Register a callback for ``event``. Duplicate handlers are ignored."""
        if event in self._handlers:
            if handler not in self._handlers[event]:
                self._handlers[event].append(handler)
        else:
            logger.warning("Unknown realtime event type: %s", event)

    def off(self, event: str, handler: Optional[Callable[..., Any]] = None) -> None:
        """Remove a callback (or all callbacks for ``event`` when ``handler`` is None)."""
        if event in self._handlers:
            if handler is None:
                self._handlers[event].clear()
            elif handler in self._handlers[event]:
                self._handlers[event].remove(handler)

    def _emit(self, event: str, *args: Any) -> None:
        for handler in self._handlers.get(event, []):
            try:
                handler(*args)
            except Exception as exc:  # noqa: BLE001 - never let a handler kill the loop
                logger.error("Error in realtime handler for %s: %s", event, exc)

    # -------------------------------------------------------------- lifecycle
    async def connect(self, timeout: float = 10.0) -> None:
        """Open the gateway WebSocket and send the ``config`` message."""
        if self._connected:
            return

        headers = {}
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"

        self._ws = await asyncio.wait_for(
            ws_connect(self._url, additional_headers=headers or None),
            timeout=timeout,
        )
        self._connected = True
        self._emit("connected")

        # First message: provider-agnostic config (connects the provider server-side).
        await self._ws.send(json.dumps(self._config.to_config_message()))

        # Start the receive pump that fans messages out to callbacks + the queue.
        self._receive_task = asyncio.create_task(self._receive_loop())

    async def disconnect(self) -> None:
        """Close the session and stop the receive pump."""
        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
            self._receive_task = None

        if self._ws:
            try:
                await self._ws.close()
            except Exception:  # noqa: BLE001
                pass
            self._ws = None

        if self._connected:
            self._connected = False
            self._emit("disconnected")
        # Unblock any async-for consumer.
        await self._queue.put(None)

    # ----------------------------------------------------------------- senders
    async def send_audio(self, audio: bytes, timeout: Optional[float] = None) -> None:
        """Stream input audio as a raw binary PCM frame (NOT base64 — the gateway
        forwards binary frames to the provider verbatim)."""
        await self._send_binary(audio, timeout)

    async def send_text(self, text: str, timeout: Optional[float] = None) -> None:
        """Send a text turn to the conversation."""
        await self._send_json({"type": "text", "text": text}, timeout)

    async def create_response(
        self,
        *,
        instructions: Optional[str] = None,
        voice: Optional[str] = None,
        modalities: Optional[list[str]] = None,
        max_output_tokens: Optional[int] = None,
        out_of_band: Optional[bool] = None,
        metadata: Optional[dict[str, Any]] = None,
        timeout: Optional[float] = None,
    ) -> None:
        """Ask the model to generate a response.

        With no arguments this sends a bare ``{"type":"create_response"}`` (the
        session defaults). Any argument adds a per-response override
        (``ClientResponseConfig``).
        """
        override: dict[str, Any] = {}
        if instructions is not None:
            override["instructions"] = instructions
        if voice is not None:
            override["voice"] = voice
        if modalities is not None:
            override["modalities"] = modalities
        if max_output_tokens is not None:
            override["max_output_tokens"] = max_output_tokens
        if out_of_band is not None:
            override["out_of_band"] = out_of_band
        if metadata is not None:
            override["metadata"] = metadata

        msg: dict[str, Any] = {"type": "create_response"}
        if override:
            msg["response"] = override
        await self._send_json(msg, timeout)

    async def interrupt(self, timeout: Optional[float] = None) -> None:
        """Barge-in: cancel the in-flight response (gateway runs the full
        barge-in sequence)."""
        await self._send_json({"type": "cancel_response"}, timeout)

    # Alias matching the BudRealtime/legacy name.
    async def cancel_response(self, timeout: Optional[float] = None) -> None:
        await self.interrupt(timeout)

    async def commit_audio(self, timeout: Optional[float] = None) -> None:
        """Commit the input audio buffer (manual turn detection)."""
        await self._send_json({"type": "commit_audio"}, timeout)

    async def clear_audio(self, timeout: Optional[float] = None) -> None:
        """Clear the input audio buffer."""
        await self._send_json({"type": "clear_audio"}, timeout)

    async def submit_function_result(
        self, call_id: str, result: Any, timeout: Optional[float] = None
    ) -> None:
        """Return a tool/function result. ``result`` is JSON-encoded as the wire
        ``result`` string."""
        payload = result if isinstance(result, str) else json.dumps(result)
        await self._send_json(
            {"type": "function_result", "call_id": call_id, "result": payload},
            timeout,
        )

    async def update_session(
        self, config: Optional[GatewayRealtimeConfig] = None, timeout: Optional[float] = None
    ) -> None:
        """Update the live session config mid-stream (``update_session``)."""
        cfg = config or self._config
        msg = cfg.to_config_message()
        msg["type"] = "update_session"
        await self._send_json(msg, timeout)

    # ------------------------------------------------------------------ stream
    def __aiter__(self) -> AsyncIterator[RealtimeEvent]:
        return self._event_stream()

    async def _event_stream(self) -> AsyncIterator[RealtimeEvent]:
        while True:
            item = await self._queue.get()
            if item is None:  # sentinel: stream ended
                return
            yield item

    async def events(self) -> AsyncIterator[RealtimeEvent]:
        """Explicit accessor for the unified event stream (same as ``async for``)."""
        async for ev in self._event_stream():
            yield ev

    # ------------------------------------------------------------------ context
    async def __aenter__(self) -> "GatewayRealtime":
        await self.connect()
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        await self.disconnect()

    # ------------------------------------------------------------------ private
    async def _send_json(self, message: dict[str, Any], timeout: Optional[float]) -> None:
        if not self._connected or not self._ws:
            raise RuntimeError("Not connected")
        await asyncio.wait_for(
            self._ws.send(json.dumps(message)),
            timeout=timeout if timeout is not None else self._send_timeout,
        )

    async def _send_binary(self, data: bytes, timeout: Optional[float]) -> None:
        if not self._connected or not self._ws:
            raise RuntimeError("Not connected")
        await asyncio.wait_for(
            self._ws.send(data),
            timeout=timeout if timeout is not None else self._send_timeout,
        )

    async def _receive_loop(self) -> None:
        assert self._ws is not None
        try:
            async for raw in self._ws:
                await self._handle_raw(raw)
        except websockets.exceptions.ConnectionClosed as exc:
            logger.info("Realtime connection closed: %s", exc)
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # noqa: BLE001
            logger.error("Realtime receive loop error: %s", exc)
            self._emit("error", ErrorEvent(message=str(exc)))
            await self._queue.put(RealtimeEvent(type="error", error=ErrorEvent(message=str(exc))))
        finally:
            self._connected = False
            self._emit("disconnected")
            await self._queue.put(None)

    async def _handle_raw(self, raw: Union[str, bytes]) -> None:
        # Binary frame == output audio (PCM).
        if isinstance(raw, (bytes, bytearray)):
            audio_event = AudioEvent(audio=bytes(raw), sample_rate=self._OUTPUT_SAMPLE_RATE)
            self._emit("audio", audio_event)
            await self._queue.put(RealtimeEvent(type="audio", audio=audio_event))
            return

        try:
            message = json.loads(raw)
        except json.JSONDecodeError as exc:
            logger.error("Failed to parse realtime message: %s", exc)
            return

        await self._dispatch(message)

    async def _dispatch(self, message: dict[str, Any]) -> None:
        msg_type = message.get("type", "")

        if msg_type == "session_created":
            self._session_id = message.get("session_id")
            ev = SessionCreatedEvent(
                session_id=message.get("session_id", ""),
                provider=message.get("provider", self._config.provider),
                model=message.get("model", ""),
            )
            self._emit("session_created", ev)
            await self._queue.put(RealtimeEvent(type="session_created", session=ev))

        elif msg_type == "session_updated":
            # No payload; surface as a session_created-less marker only via callback.
            self._emit("session_created", None)

        elif msg_type == "transcript":
            tev = TranscriptEvent(
                text=message.get("text", ""),
                is_final=message.get("is_final", True),
                role=message.get("role", "assistant"),
            )
            self._emit("transcript", tev)
            await self._queue.put(RealtimeEvent(type="transcript", transcript=tev))

        elif msg_type == "speech_event":
            sev = SpeechEvent(
                event=message.get("event", ""),
                audio_ms=int(message.get("audio_ms", 0) or 0),
            )
            self._emit("speech_event", sev)
            await self._queue.put(RealtimeEvent(type="speech_event", speech=sev))

        elif msg_type == "function_call":
            try:
                args = json.loads(message.get("arguments", "{}") or "{}")
            except json.JSONDecodeError:
                args = {}
            fev = FunctionCallEvent(
                name=message.get("name", ""),
                arguments=args,
                call_id=message.get("call_id", ""),
            )
            self._emit("function_call", fev)
            await self._queue.put(RealtimeEvent(type="function_call", function_call=fev))

        elif msg_type == "response_started":
            rev = ResponseEvent(response_id=message.get("response_id", ""))
            self._emit("response_started", rev)
            await self._queue.put(RealtimeEvent(type="response_started", response=rev))

        elif msg_type == "response_done":
            rev = ResponseEvent(response_id=message.get("response_id", ""))
            self._emit("response_done", rev)
            await self._queue.put(RealtimeEvent(type="response_done", response=rev))

        elif msg_type == "error":
            eev = ErrorEvent(message=message.get("message", "Unknown error"), code=message.get("code"))
            self._emit("error", eev)
            await self._queue.put(RealtimeEvent(type="error", error=eev))

        elif msg_type == "closing":
            reason = message.get("reason", "")
            self._emit("closing", reason)
            await self._queue.put(RealtimeEvent(type="closing", reason=reason))

        else:
            logger.debug("Unhandled realtime message type: %s", msg_type)


__all__ = [
    "GatewayRealtime",
    "GatewayRealtimeConfig",
    "GatewayTurnDetection",
    "RealtimeTool",
    "RealtimeEvent",
    "FunctionCallEvent",
    "SpeechEvent",
    "ErrorEvent",
    "SessionCreatedEvent",
    "ResponseEvent",
    "REALTIME_PROVIDERS",
    "get_supported_realtime_providers",
]
