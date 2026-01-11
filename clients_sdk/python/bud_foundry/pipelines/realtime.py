"""
BudRealtime - Real-time bidirectional audio with LLM integration.

Supports OpenAI Realtime API and Hume EVI.
"""

import asyncio
import base64
import json
import logging
from dataclasses import dataclass
from enum import Enum
from typing import Any, Callable, Literal, Optional

import websockets
from websockets.legacy.client import WebSocketClientProtocol

from ..types import TranscriptEvent, AudioEvent

logger = logging.getLogger(__name__)

# =============================================================================
# Types
# =============================================================================


class RealtimeProvider(str, Enum):
    """Supported realtime providers."""

    OPENAI_REALTIME = "openai-realtime"
    HUME_EVI = "hume-evi"


class RealtimeState(str, Enum):
    """Connection state."""

    DISCONNECTED = "disconnected"
    CONNECTING = "connecting"
    CONNECTED = "connected"
    RECONNECTING = "reconnecting"
    ERROR = "error"


@dataclass
class TurnDetectionConfig:
    """Turn detection settings."""

    enabled: bool = False
    threshold: Optional[float] = None
    silence_ms: Optional[int] = None
    prefix_padding_ms: Optional[int] = None
    create_response_ms: Optional[int] = None


@dataclass
class RealtimeConfig:
    """Configuration for BudRealtime."""

    provider: RealtimeProvider
    """Realtime provider to use."""

    api_key: str
    """API key for the provider."""

    model: Optional[str] = None
    """Model to use (OpenAI)."""

    evi_version: Optional[str] = None
    """EVI version (Hume)."""

    voice_id: Optional[str] = None
    """Voice ID for TTS."""

    system_prompt: Optional[str] = None
    """System prompt."""

    verbose_transcription: bool = False
    """Enable verbose transcription (Hume)."""

    resumed_chat_group_id: Optional[str] = None
    """Resume from previous chat group (Hume)."""

    temperature: Optional[float] = None
    """Temperature for LLM."""

    max_tokens: Optional[int] = None
    """Maximum response tokens."""

    turn_detection: Optional[TurnDetectionConfig] = None
    """Turn detection settings."""


@dataclass
class ToolDefinition:
    """Tool/function definition for LLM."""

    name: str
    """Function name."""

    description: str
    """Function description."""

    parameters: dict[str, Any]
    """JSON Schema for parameters."""

    type: Literal["function"] = "function"
    """Type of tool (always 'function')."""


@dataclass
class FunctionCallEvent:
    """Function call event from LLM."""

    name: str
    """Function name."""

    arguments: dict[str, Any]
    """Parsed arguments."""

    call_id: str
    """Call ID for submitting result."""


@dataclass
class EmotionEvent:
    """Emotion event (Hume EVI)."""

    emotions: dict[str, float]
    """Emotion scores."""

    dominant: str
    """Dominant emotion."""

    confidence: Optional[float] = None
    """Confidence score."""


@dataclass
class StateChangeEvent:
    """State change event."""

    previous_state: RealtimeState
    """Previous state."""

    current_state: RealtimeState
    """Current state."""


# =============================================================================
# Default Configuration
# =============================================================================

DEFAULT_OPENAI_MODEL = "gpt-4o-realtime-preview"
DEFAULT_EVI_VERSION = "3"

# =============================================================================
# BudRealtime Class
# =============================================================================


class BudRealtime:
    """
    Real-time bidirectional audio pipeline with LLM integration.

    Supports OpenAI Realtime API and Hume EVI.

    Example:
        >>> realtime = BudRealtime(RealtimeConfig(
        ...     provider=RealtimeProvider.OPENAI_REALTIME,
        ...     api_key="your-api-key",
        ...     system_prompt="You are a helpful assistant."
        ... ))
        >>>
        >>> # Register event handlers
        >>> realtime.on("transcript", lambda e: print(e.text))
        >>> realtime.on("audio", lambda e: play_audio(e.audio))
        >>>
        >>> # Connect and send audio
        >>> await realtime.connect("wss://gateway.example.com/realtime")
        >>> await realtime.send_audio(audio_bytes)
    """

    def __init__(self, config: RealtimeConfig):
        """
        Create a new BudRealtime instance.

        Args:
            config: Realtime configuration.

        Raises:
            ValueError: If provider is invalid or missing.
        """
        if not config.provider:
            raise ValueError("Provider is required")

        if config.provider not in (
            RealtimeProvider.OPENAI_REALTIME,
            RealtimeProvider.HUME_EVI,
        ):
            raise ValueError(f"Invalid provider: {config.provider}")

        # Apply defaults based on provider
        self._config = config
        if config.model is None and config.provider == RealtimeProvider.OPENAI_REALTIME:
            self._config.model = DEFAULT_OPENAI_MODEL
        if (
            config.evi_version is None
            and config.provider == RealtimeProvider.HUME_EVI
        ):
            self._config.evi_version = DEFAULT_EVI_VERSION

        self._ws: Optional[WebSocketClientProtocol] = None
        self._state: RealtimeState = RealtimeState.DISCONNECTED
        self._tools: list[ToolDefinition] = []
        self._reconnect_attempts: int = 0
        self._max_reconnect_attempts: int = 3
        self._reconnect_delay: float = 1.0
        self._url: Optional[str] = None
        self._send_timeout: float = 30.0  # Default timeout for send operations

        # Locks for thread-safe access (created lazily to avoid event loop issues)
        # asyncio.Lock() requires a running event loop in some Python versions
        self._ws_lock: Optional[asyncio.Lock] = None
        self._connect_lock: Optional[asyncio.Lock] = None

        # Event handlers
        self._handlers: dict[str, list[Callable[..., Any]]] = {
            "audio": [],
            "transcript": [],
            "function_call": [],
            "emotion": [],
            "connected": [],
            "disconnected": [],
            "state_change": [],
            "error": [],
        }

        # Background tasks
        self._receive_task: Optional[asyncio.Task[None]] = None

    @property
    def config(self) -> RealtimeConfig:
        """Get the configuration."""
        return self._config

    @property
    def provider(self) -> RealtimeProvider:
        """Get the current provider."""
        return self._config.provider

    @property
    def state(self) -> RealtimeState:
        """Get the current connection state."""
        return self._state

    @property
    def tools(self) -> list[ToolDefinition]:
        """Get registered tools."""
        return list(self._tools)

    @property
    def connected(self) -> bool:
        """Whether the session is connected."""
        return self._state == RealtimeState.CONNECTED

    def on(self, event: str, handler: Callable[..., Any]) -> None:
        """
        Register an event handler.

        Args:
            event: Event name (audio, transcript, function_call, emotion,
                   connected, disconnected, state_change, error).
            handler: Event handler function.

        Note:
            Duplicate handlers are ignored to prevent memory leaks from
            repeated registrations of the same handler.
        """
        if event in self._handlers:
            # Prevent duplicate handler registration (memory leak prevention)
            if handler not in self._handlers[event]:
                self._handlers[event].append(handler)
        else:
            logger.warning(f"Unknown event type: {event}")

    def off(self, event: str, handler: Optional[Callable[..., Any]] = None) -> None:
        """
        Remove an event handler.

        Args:
            event: Event name.
            handler: Specific handler to remove, or None to remove all.
        """
        if event in self._handlers:
            if handler is None:
                self._handlers[event].clear()
            elif handler in self._handlers[event]:
                self._handlers[event].remove(handler)

    def _emit(self, event: str, *args: Any, **kwargs: Any) -> None:
        """Emit an event to all handlers."""
        for handler in self._handlers.get(event, []):
            try:
                handler(*args, **kwargs)
            except Exception as e:
                logger.error(f"Error in event handler for {event}: {e}")

    def _set_state(self, state: RealtimeState) -> None:
        """Update connection state and emit event."""
        previous_state = self._state
        self._state = state
        self._emit(
            "state_change",
            StateChangeEvent(previous_state=previous_state, current_state=state),
        )

    def _ensure_locks(self) -> None:
        """Lazily create locks when first needed (requires running event loop)."""
        if self._ws_lock is None:
            self._ws_lock = asyncio.Lock()
        if self._connect_lock is None:
            self._connect_lock = asyncio.Lock()

    async def connect(self, url: str, timeout: float = 10.0) -> None:
        """
        Connect to the realtime gateway.

        Args:
            url: WebSocket URL to connect to.
            timeout: Connection timeout in seconds.

        Raises:
            Exception: If connection fails.

        Note:
            This method is thread-safe. Concurrent calls will be serialized
            and only the first call will establish the connection.
        """
        # Create locks lazily (requires running event loop)
        self._ensure_locks()

        # Type assertion after _ensure_locks
        assert self._connect_lock is not None

        async with self._connect_lock:
            # Check state inside lock to prevent race conditions
            if self._state in (RealtimeState.CONNECTED, RealtimeState.CONNECTING):
                return

            self._url = url
            self._set_state(RealtimeState.CONNECTING)

            try:
                # Build headers for authentication
                headers = {}
                if self._config.api_key:
                    headers["Authorization"] = f"Bearer {self._config.api_key}"

                self._ws = await asyncio.wait_for(
                    websockets.connect(url, additional_headers=headers),
                    timeout=timeout,
                )

                self._set_state(RealtimeState.CONNECTED)
                self._reconnect_attempts = 0
                self._emit("connected")

                # Send initial session config
                try:
                    await self._send_session_config()
                except Exception as config_error:
                    # Clean up if session config fails
                    await self._cleanup_on_error()
                    raise config_error

                # Start receive loop
                self._receive_task = asyncio.create_task(self._receive_loop())

            except asyncio.TimeoutError:
                await self._cleanup_on_error()
                raise TimeoutError("Connection timeout")
            except Exception as e:
                await self._cleanup_on_error()
                raise e

    async def _cleanup_on_error(self) -> None:
        """Clean up resources when an error occurs during connection."""
        # Cancel receive task if it was started
        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
            self._receive_task = None

        # Close WebSocket if open
        if self._ws:
            try:
                await self._ws.close(1011, "Connection error")
            except Exception:
                pass  # Ignore close errors during cleanup
            self._ws = None

        self._set_state(RealtimeState.ERROR)
        self._emit("disconnected")  # Emit disconnected so callers can clean up

    async def disconnect(self) -> None:
        """Disconnect from the gateway."""
        # Cancel receive task
        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
            self._receive_task = None

        # Close WebSocket
        if self._ws:
            await self._ws.close(1000, "Normal closure")
            self._ws = None

        self._set_state(RealtimeState.DISCONNECTED)
        self._emit("disconnected")

    async def send_audio(self, audio: bytes, timeout: Optional[float] = None) -> None:
        """
        Send audio data to the gateway.

        Args:
            audio: Raw audio data (PCM).
            timeout: Send timeout in seconds (defaults to _send_timeout).

        Raises:
            RuntimeError: If not connected.
            asyncio.TimeoutError: If send times out.
        """
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        send_timeout = timeout if timeout is not None else self._send_timeout

        async with self._ws_lock:
            # Check state inside lock to prevent race conditions
            if self._state != RealtimeState.CONNECTED or not self._ws:
                raise RuntimeError("Not connected")
            if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
                # OpenAI Realtime: wrap in message format
                base64_audio = base64.b64encode(audio).decode("utf-8")
                await asyncio.wait_for(
                    self._ws.send(
                        json.dumps(
                            {
                                "type": "input_audio_buffer.append",
                                "audio": base64_audio,
                            }
                        )
                    ),
                    timeout=send_timeout,
                )
            else:
                # Hume EVI: send raw binary
                await asyncio.wait_for(self._ws.send(audio), timeout=send_timeout)

    async def send_text(self, text: str, timeout: Optional[float] = None) -> None:
        """
        Send text message to the LLM.

        Args:
            text: Text message to send.
            timeout: Send timeout in seconds (defaults to _send_timeout).

        Raises:
            RuntimeError: If not connected.
            asyncio.TimeoutError: If send times out.
        """
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        send_timeout = timeout if timeout is not None else self._send_timeout

        async with self._ws_lock:
            # Check state inside lock to prevent race conditions
            if self._state != RealtimeState.CONNECTED or not self._ws:
                raise RuntimeError("Not connected")
            if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
                await asyncio.wait_for(
                    self._ws.send(
                        json.dumps(
                            {
                                "type": "conversation.item.create",
                                "item": {
                                    "type": "message",
                                    "role": "user",
                                    "content": [
                                        {
                                            "type": "input_text",
                                            "text": text,
                                        }
                                    ],
                                },
                            }
                        )
                    ),
                    timeout=send_timeout,
                )
                # Trigger response
                await asyncio.wait_for(
                    self._ws.send(json.dumps({"type": "response.create"})),
                    timeout=send_timeout,
                )
            else:
                # Hume EVI text message
                await asyncio.wait_for(
                    self._ws.send(
                        json.dumps(
                            {
                                "type": "user_message",
                                "text": text,
                            }
                        )
                    ),
                    timeout=send_timeout,
                )

    async def add_tool(self, tool: ToolDefinition) -> None:
        """
        Add a tool/function for the LLM to use.

        Args:
            tool: Tool definition.
        """
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        async with self._ws_lock:
            self._tools.append(tool)

            # If connected, update session with new tool (already in lock)
            if self._state == RealtimeState.CONNECTED and self._ws:
                await self._send_session_config_unlocked()

    async def remove_tool(self, name: str) -> None:
        """
        Remove a tool by name.

        Args:
            name: Tool name to remove.
        """
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        async with self._ws_lock:
            self._tools = [t for t in self._tools if t.name != name]

            # If connected, update session to sync tool removal with server (already in lock)
            if self._state == RealtimeState.CONNECTED and self._ws:
                await self._send_session_config_unlocked()

    async def submit_function_result(self, call_id: str, result: Any) -> None:
        """
        Submit function call result to the LLM.

        Args:
            call_id: Call ID from the function call event.
            result: Result to return to the LLM.

        Raises:
            RuntimeError: If not connected.
        """
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        async with self._ws_lock:
            # Check state inside lock to prevent race conditions
            if self._state != RealtimeState.CONNECTED or not self._ws:
                raise RuntimeError("Not connected")

            if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
                await self._ws.send(
                    json.dumps(
                        {
                            "type": "conversation.item.create",
                            "item": {
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": json.dumps(result),
                            },
                        }
                    )
                )
                # Trigger response
                await self._ws.send(json.dumps({"type": "response.create"}))
            else:
                # Hume EVI tool result
                await self._ws.send(
                    json.dumps(
                        {
                            "type": "tool_response",
                            "tool_call_id": call_id,
                            "content": json.dumps(result),
                        }
                    )
                )

    async def interrupt(self) -> None:
        """Interrupt/cancel the current response."""
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        async with self._ws_lock:
            # Check state inside lock to prevent race conditions
            if self._state != RealtimeState.CONNECTED or not self._ws:
                return

            if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
                await self._ws.send(json.dumps({"type": "response.cancel"}))
            else:
                # Hume EVI interrupt
                await self._ws.send(json.dumps({"type": "user_interruption"}))

    async def commit_audio_buffer(self) -> None:
        """Commit the audio buffer (OpenAI Realtime)."""
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        async with self._ws_lock:
            # Check state inside lock to prevent race conditions
            if self._state != RealtimeState.CONNECTED or not self._ws:
                return

            if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
                await self._ws.send(json.dumps({"type": "input_audio_buffer.commit"}))

    # =========================================================================
    # Private Methods
    # =========================================================================

    async def _send_session_config(self) -> None:
        """Send session configuration to the gateway."""
        # Ensure locks exist
        self._ensure_locks()
        assert self._ws_lock is not None

        async with self._ws_lock:
            await self._send_session_config_unlocked()

    async def _send_session_config_unlocked(self) -> None:
        """Send session configuration to the gateway (must be called with lock held)."""
        # Check state - caller must hold _ws_lock
        if not self._ws or self._state != RealtimeState.CONNECTED:
            return

        if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
            session_config: dict[str, Any] = {
                "type": "session.update",
                "session": {
                    "modalities": ["text", "audio"],
                    "instructions": self._config.system_prompt,
                    "voice": self._config.voice_id or "alloy",
                    "input_audio_format": "pcm16",
                    "output_audio_format": "pcm16",
                    "tools": [
                        {
                            "type": "function",
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                        for t in self._tools
                    ],
                    "tool_choice": "auto" if self._tools else "none",
                },
            }

            if self._config.turn_detection and self._config.turn_detection.enabled:
                session_config["session"]["turn_detection"] = {
                    "type": "server_vad",
                    "threshold": self._config.turn_detection.threshold,
                    "silence_duration_ms": self._config.turn_detection.silence_ms,
                    "prefix_padding_ms": self._config.turn_detection.prefix_padding_ms,
                }

            if self._config.temperature is not None:
                session_config["session"]["temperature"] = self._config.temperature

            if self._config.max_tokens is not None:
                session_config["session"][
                    "max_response_output_tokens"
                ] = self._config.max_tokens

            await self._ws.send(json.dumps(session_config))
        else:
            # Hume EVI session setup
            session_config = {
                "type": "session_settings",
                "system_prompt": self._config.system_prompt,
                "evi_version": self._config.evi_version,
                "verbose_transcription": self._config.verbose_transcription,
            }

            if self._config.resumed_chat_group_id:
                session_config["resumed_chat_group_id"] = (
                    self._config.resumed_chat_group_id
                )

            if self._config.voice_id:
                session_config["voice_id"] = self._config.voice_id

            if self._tools:
                session_config["tools"] = [
                    {
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                    for t in self._tools
                ]

            await self._ws.send(json.dumps(session_config))

    async def _receive_loop(self) -> None:
        """Background task to receive and process messages."""
        if not self._ws:
            return

        try:
            async for message in self._ws:
                await self._handle_message(message)
        except websockets.exceptions.ConnectionClosedError as e:
            logger.warning(f"Connection closed: {e}")
            await self._handle_close(e.code)
        except asyncio.CancelledError:
            raise
        except Exception as e:
            logger.error(f"Error in receive loop: {e}")
            self._emit("error", e)

    async def _handle_message(self, data: str | bytes) -> None:
        """Handle incoming message."""
        # Handle binary audio data
        if isinstance(data, bytes):
            self._emit("audio", AudioEvent(audio=data))
            return

        # Handle JSON messages
        try:
            message = json.loads(data)
            self._route_message(message)
        except json.JSONDecodeError as e:
            logger.error(f"Failed to parse message: {e}")

    def _route_message(self, message: dict[str, Any]) -> None:
        """Route message to appropriate handler."""
        msg_type = message.get("type", "")

        if self._config.provider == RealtimeProvider.OPENAI_REALTIME:
            self._handle_openai_message(msg_type, message)
        else:
            self._handle_hume_message(msg_type, message)

    def _handle_openai_message(self, msg_type: str, message: dict[str, Any]) -> None:
        """Handle OpenAI Realtime messages."""
        if msg_type == "response.audio.delta":
            base64_audio = message.get("delta", "")
            audio = base64.b64decode(base64_audio)
            self._emit("audio", AudioEvent(audio=audio))

        elif msg_type == "response.audio_transcript.delta":
            self._emit(
                "transcript",
                TranscriptEvent(
                    text=message.get("delta", ""),
                    is_final=False,
                    role="assistant",
                ),
            )

        elif msg_type == "response.audio_transcript.done":
            self._emit(
                "transcript",
                TranscriptEvent(
                    text=message.get("transcript", ""),
                    is_final=True,
                    role="assistant",
                ),
            )

        elif msg_type == "conversation.item.input_audio_transcription.completed":
            self._emit(
                "transcript",
                TranscriptEvent(
                    text=message.get("transcript", ""),
                    is_final=True,
                    role="user",
                ),
            )

        elif msg_type == "response.function_call_arguments.done":
            self._emit(
                "function_call",
                FunctionCallEvent(
                    name=message.get("name", ""),
                    arguments=json.loads(message.get("arguments", "{}")),
                    call_id=message.get("call_id", ""),
                ),
            )

        elif msg_type == "error":
            error_info = message.get("error", {})
            self._emit("error", Exception(error_info.get("message", "Unknown error")))

    def _handle_hume_message(self, msg_type: str, message: dict[str, Any]) -> None:
        """Handle Hume EVI messages."""
        if msg_type == "audio":
            base64_audio = message.get("data", "")
            audio = base64.b64decode(base64_audio)
            self._emit("audio", AudioEvent(audio=audio))

        elif msg_type in ("user_message", "assistant_message"):
            role: Literal["user", "assistant"] = (
                "user" if msg_type == "user_message" else "assistant"
            )
            content = message.get("message", {})

            self._emit(
                "transcript",
                TranscriptEvent(
                    text=content.get("content", ""),
                    is_final=True,
                    role=role,
                ),
            )

            # Handle emotions from Hume
            models = message.get("models", {})
            prosody = models.get("prosody")
            if prosody:
                scores: dict[str, float] = prosody.get("scores", {})

                # Find dominant emotion
                dominant = "neutral"
                max_score = 0.0
                for emotion, score in scores.items():
                    if score > max_score:
                        max_score = score
                        dominant = emotion

                self._emit(
                    "emotion",
                    EmotionEvent(
                        emotions=scores,
                        dominant=dominant,
                        confidence=max_score,
                    ),
                )

        elif msg_type == "tool_call":
            self._emit(
                "function_call",
                FunctionCallEvent(
                    name=message.get("name", ""),
                    arguments=json.loads(message.get("parameters", "{}")),
                    call_id=message.get("tool_call_id", ""),
                ),
            )

        elif msg_type == "error":
            self._emit("error", Exception(message.get("message", "Unknown error")))

    async def _handle_close(self, code: int) -> None:
        """Handle connection close."""
        if code != 1000:
            # Abnormal closure, attempt reconnect
            if self._reconnect_attempts < self._max_reconnect_attempts and self._url:
                self._set_state(RealtimeState.RECONNECTING)
                self._reconnect_attempts += 1
                await asyncio.sleep(self._reconnect_delay * self._reconnect_attempts)
                try:
                    await self.connect(self._url)
                    return
                except Exception as e:
                    logger.error(f"Reconnect failed: {e}")

            self._set_state(RealtimeState.ERROR)
        else:
            self._set_state(RealtimeState.DISCONNECTED)

        self._emit("disconnected")

    # =========================================================================
    # Context Manager Support
    # =========================================================================

    async def __aenter__(self) -> "BudRealtime":
        """Async context manager entry (does not auto-connect)."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.disconnect()


# =============================================================================
# Realtime Session (Wrapper for compatibility)
# =============================================================================


class RealtimeSession(BudRealtime):
    """
    Alias for BudRealtime for API consistency with other pipelines.
    """

    pass


__all__ = [
    "RealtimeProvider",
    "RealtimeState",
    "TurnDetectionConfig",
    "RealtimeConfig",
    "ToolDefinition",
    "FunctionCallEvent",
    "TranscriptEvent",
    "AudioEvent",
    "EmotionEvent",
    "StateChangeEvent",
    "BudRealtime",
    "RealtimeSession",
]
