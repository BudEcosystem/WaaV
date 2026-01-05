"""
BudTalk - Bidirectional voice pipeline
"""

from typing import Any, AsyncIterator, Callable, Optional, Union
from dataclasses import dataclass

from ..types import STTConfig, TTSConfig, STTResult, AudioEvent, FeatureFlags
from ..ws.session import WebSocketSession, SessionMetrics, ReconnectConfig


@dataclass
class TalkEvent:
    """Event from a Talk session."""

    type: str
    """Event type: transcript, audio, message, error, playback_complete"""

    transcript: Optional[STTResult] = None
    """Transcript result (if type is 'transcript')"""

    audio: Optional[AudioEvent] = None
    """Audio event (if type is 'audio')"""

    message: Optional[dict[str, Any]] = None
    """Message data (if type is 'message')"""

    error: Optional[Exception] = None
    """Error (if type is 'error')"""


class BudTalk:
    """Bidirectional voice pipeline combining STT and TTS."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
    ):
        """
        Initialize Talk pipeline.

        Args:
            url: WebSocket URL of the gateway
            api_key: Optional API key for authentication
        """
        self.url = url
        self.api_key = api_key

    def create(
        self,
        stt: Optional[Union[STTConfig, dict[str, Any]]] = None,
        tts: Optional[Union[TTSConfig, dict[str, Any]]] = None,
        livekit: Optional[dict[str, Any]] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> "TalkSession":
        """
        Create a Talk session.

        Args:
            stt: STT configuration or dict
            tts: TTS configuration or dict
            livekit: LiveKit configuration
            features: Feature flags
            reconnect: Reconnection configuration

        Returns:
            Talk session
        """
        # Convert dicts to config objects
        stt_config: Optional[STTConfig] = None
        if stt is not None:
            if isinstance(stt, dict):
                stt_config = STTConfig(
                    provider=stt.get("provider", "deepgram"),
                    language=stt.get("language", "en-US"),
                    model=stt.get("model"),
                    sample_rate=stt.get("sample_rate", 16000),
                    channels=stt.get("channels", 1),
                    encoding=stt.get("encoding", "linear16"),
                    interim_results=stt.get("interim_results", True),
                    punctuate=stt.get("punctuate", True),
                )
            else:
                stt_config = stt

        # Apply feature flags to STT config
        if stt_config and features:
            stt_config.interim_results = features.interim_results
            stt_config.punctuate = features.punctuation
            stt_config.profanity_filter = features.profanity_filter
            stt_config.smart_format = features.smart_format
            stt_config.diarize = features.speaker_diarization

        tts_config: Optional[TTSConfig] = None
        if tts is not None:
            if isinstance(tts, dict):
                tts_config = TTSConfig(
                    provider=tts.get("provider", "deepgram"),
                    voice=tts.get("voice"),
                    voice_id=tts.get("voice_id"),
                    model=tts.get("model"),
                    sample_rate=tts.get("sample_rate", 24000),
                )
            else:
                tts_config = tts

        return TalkSession(
            url=self.url,
            api_key=self.api_key,
            stt_config=stt_config,
            tts_config=tts_config,
            livekit_config=livekit,
            features=features,
            reconnect=reconnect,
        )

    async def connect(
        self,
        stt: Optional[Union[STTConfig, dict[str, Any]]] = None,
        tts: Optional[Union[TTSConfig, dict[str, Any]]] = None,
        livekit: Optional[dict[str, Any]] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> "TalkSession":
        """
        Create and connect a Talk session.

        Args:
            stt: STT configuration
            tts: TTS configuration
            livekit: LiveKit configuration
            features: Feature flags
            reconnect: Reconnection configuration

        Returns:
            Connected Talk session
        """
        session = self.create(
            stt=stt,
            tts=tts,
            livekit=livekit,
            features=features,
            reconnect=reconnect,
        )
        await session.connect()
        return session


class TalkSession:
    """Active Talk session for bidirectional voice communication."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        stt_config: Optional[STTConfig] = None,
        tts_config: Optional[TTSConfig] = None,
        livekit_config: Optional[dict[str, Any]] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ):
        """
        Initialize Talk session.

        Args:
            url: WebSocket URL
            api_key: API key
            stt_config: STT configuration
            tts_config: TTS configuration
            livekit_config: LiveKit configuration
            features: Feature flags
            reconnect: Reconnection config
        """
        self.stt_config = stt_config
        self.tts_config = tts_config
        self.livekit_config = livekit_config
        self.features = features

        self._session = WebSocketSession(
            url=url,
            api_key=api_key,
            stt_config=stt_config,
            tts_config=tts_config,
            livekit_config=livekit_config,
            reconnect=reconnect,
        )

        self._event_handlers: dict[str, list[Callable[..., Any]]] = {}

        # Wire up internal handlers
        self._session.on("transcript", self._on_transcript)
        self._session.on("audio", self._on_audio)
        self._session.on("message", self._on_message)
        self._session.on("error", self._on_error)
        self._session.on("playback_complete", self._on_playback_complete)

    def _on_transcript(self, result: STTResult) -> None:
        """Handle transcript events."""
        event = TalkEvent(type="transcript", transcript=result)
        self._emit("transcript", result)
        self._emit("event", event)

    def _on_audio(self, audio: AudioEvent) -> None:
        """Handle audio events."""
        event = TalkEvent(type="audio", audio=audio)
        self._emit("audio", audio)
        self._emit("event", event)

    def _on_message(self, message: dict[str, Any]) -> None:
        """Handle message events."""
        event = TalkEvent(type="message", message=message)
        self._emit("message", message)
        self._emit("event", event)

    def _on_error(self, error: Exception) -> None:
        """Handle error events."""
        event = TalkEvent(type="error", error=error)
        self._emit("error", error)
        self._emit("event", event)

    def _on_playback_complete(self, timestamp: Any) -> None:
        """Handle playback complete events."""
        event = TalkEvent(type="playback_complete")
        self._emit("playback_complete")
        self._emit("event", event)

    def _emit(self, event: str, *args: Any) -> None:
        """Emit an event."""
        handlers = self._event_handlers.get(event, [])
        for handler in handlers:
            try:
                handler(*args)
            except Exception:
                pass

    @property
    def connected(self) -> bool:
        """Whether the session is connected."""
        return self._session.connected

    @property
    def stream_id(self) -> Optional[str]:
        """Get the stream ID."""
        return self._session.stream_id

    def on(self, event: str, handler: Callable[..., Any]) -> None:
        """
        Register an event handler.

        Args:
            event: Event name (transcript, audio, message, error, event, etc.)
            handler: Event handler function
        """
        if event not in self._event_handlers:
            self._event_handlers[event] = []
        self._event_handlers[event].append(handler)

    def off(self, event: str, handler: Optional[Callable[..., Any]] = None) -> None:
        """Remove an event handler."""
        if event in self._event_handlers:
            if handler is None:
                self._event_handlers[event].clear()
            elif handler in self._event_handlers[event]:
                self._event_handlers[event].remove(handler)

    async def connect(self, timeout: float = 10.0) -> None:
        """Connect to the gateway."""
        await self._session.connect(timeout=timeout)

    async def disconnect(self) -> None:
        """Disconnect from the gateway."""
        await self._session.disconnect()

    async def send_audio(self, audio: Union[bytes, bytearray]) -> None:
        """
        Send audio data for transcription.

        Args:
            audio: PCM audio data (16-bit signed integer, mono)
        """
        await self._session.send_audio(audio)

    async def speak(
        self,
        text: str,
        flush: bool = False,
        allow_interruption: bool = True,
    ) -> None:
        """
        Synthesize and stream text to speech.

        Args:
            text: Text to synthesize
            flush: Whether to flush the TTS buffer immediately
            allow_interruption: Whether this TTS can be interrupted
        """
        await self._session.speak(text, flush=flush, allow_interruption=allow_interruption)

    async def clear(self) -> None:
        """Clear/stop current TTS playback."""
        await self._session.clear()

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
        await self._session.send_message(message, role=role, topic=topic)

    async def sip_transfer(self, transfer_to: str) -> None:
        """
        Transfer a SIP call.

        Args:
            transfer_to: Phone number to transfer to
        """
        await self._session.sip_transfer(transfer_to)

    def get_metrics(self) -> SessionMetrics:
        """Get session metrics."""
        return self._session.get_metrics()

    def reset_metrics(self) -> None:
        """Reset session metrics."""
        self._session.reset_metrics()

    async def __aiter__(self) -> AsyncIterator[TalkEvent]:
        """Iterate over all events."""
        async for message in self._session:
            msg_type = message.get("type")

            if msg_type == "transcript":
                yield TalkEvent(type="transcript", transcript=message["result"])
            elif msg_type == "audio":
                yield TalkEvent(type="audio", audio=message["audio"])
            elif msg_type == "message":
                yield TalkEvent(type="message", message=message["data"])
            elif msg_type == "error":
                yield TalkEvent(type="error", error=message["error"])
            elif msg_type == "playback_complete":
                yield TalkEvent(type="playback_complete")

    async def __aenter__(self) -> "TalkSession":
        """Async context manager entry."""
        await self.connect()
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.disconnect()
