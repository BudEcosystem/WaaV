"""
BudTTS - Text-to-Speech pipeline
"""

from typing import Any, AsyncIterator, Callable, Optional

from ..types import TTSConfig, AudioEvent
from ..ws.session import WebSocketSession, SessionMetrics, ReconnectConfig
from ..rest.client import RestClient


class BudTTS:
    """Text-to-Speech pipeline for speech synthesis."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        rest_client: Optional[RestClient] = None,
    ):
        """
        Initialize TTS pipeline.

        Args:
            url: WebSocket URL of the gateway (for streaming)
            api_key: Optional API key for authentication
            rest_client: Optional REST client for one-shot synthesis
        """
        self.url = url
        self.api_key = api_key
        self._rest_client = rest_client

    def create(
        self,
        config: Optional[TTSConfig] = None,
        provider: str = "deepgram",
        voice: Optional[str] = None,
        voice_id: Optional[str] = None,
        model: Optional[str] = None,
        sample_rate: int = 24000,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> "TTSSession":
        """
        Create a TTS session.

        Args:
            config: Full TTS configuration (overrides other params)
            provider: TTS provider
            voice: Voice name
            voice_id: Voice ID
            model: Model to use
            sample_rate: Output sample rate
            reconnect: Reconnection configuration

        Returns:
            TTS session
        """
        if config is None:
            config = TTSConfig(
                provider=provider,
                voice=voice,
                voice_id=voice_id,
                model=model,
                sample_rate=sample_rate,
            )

        return TTSSession(
            url=self.url,
            api_key=self.api_key,
            config=config,
            reconnect=reconnect,
        )

    async def connect(
        self,
        config: Optional[TTSConfig] = None,
        provider: str = "deepgram",
        voice: Optional[str] = None,
        voice_id: Optional[str] = None,
        model: Optional[str] = None,
        sample_rate: int = 24000,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> "TTSSession":
        """
        Create and connect a TTS session.

        Args:
            config: Full TTS configuration
            provider: TTS provider
            voice: Voice name
            voice_id: Voice ID
            model: Model to use
            sample_rate: Output sample rate
            reconnect: Reconnection configuration

        Returns:
            Connected TTS session
        """
        session = self.create(
            config=config,
            provider=provider,
            voice=voice,
            voice_id=voice_id,
            model=model,
            sample_rate=sample_rate,
            reconnect=reconnect,
        )
        await session.connect()
        return session

    async def synthesize(
        self,
        text: str,
        provider: str = "deepgram",
        voice: Optional[str] = None,
        voice_id: Optional[str] = None,
        model: Optional[str] = None,
        sample_rate: int = 24000,
    ) -> bytes:
        """
        Synthesize text to speech (one-shot, non-streaming).

        Args:
            text: Text to synthesize
            provider: TTS provider
            voice: Voice name
            voice_id: Voice ID
            model: Model to use
            sample_rate: Output sample rate

        Returns:
            Audio data as bytes (PCM)
        """
        if self._rest_client is None:
            # Derive REST URL from WebSocket URL
            rest_url = self.url.replace("ws://", "http://").replace("wss://", "https://")
            if rest_url.endswith("/ws"):
                rest_url = rest_url[:-3]
            self._rest_client = RestClient(base_url=rest_url, api_key=self.api_key)

        return await self._rest_client.speak(
            text=text,
            provider=provider,
            voice=voice,
            voice_id=voice_id,
            model=model,
            sample_rate=sample_rate,
        )


class TTSSession:
    """Active TTS session for streaming speech synthesis."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        config: Optional[TTSConfig] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ):
        """
        Initialize TTS session.

        Args:
            url: WebSocket URL
            api_key: API key
            config: TTS configuration
            reconnect: Reconnection config
        """
        self.config = config or TTSConfig(provider="deepgram")

        self._session = WebSocketSession(
            url=url,
            api_key=api_key,
            tts_config=self.config,
            reconnect=reconnect,
        )

        self._audio_handlers: list[Callable[[AudioEvent], Any]] = []
        self._playback_handlers: list[Callable[[], Any]] = []
        self._session.on("audio", self._on_audio)
        self._session.on("playback_complete", self._on_playback_complete)

    def _on_audio(self, event: AudioEvent) -> None:
        """Handle audio events."""
        for handler in self._audio_handlers:
            try:
                handler(event)
            except Exception:
                pass

    def _on_playback_complete(self, timestamp: Any) -> None:
        """Handle playback complete events."""
        for handler in self._playback_handlers:
            try:
                handler()
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
            event: Event name (audio, playback_complete, error, close)
            handler: Event handler function
        """
        if event == "audio":
            self._audio_handlers.append(handler)
        elif event == "playback_complete":
            self._playback_handlers.append(handler)
        else:
            self._session.on(event, handler)

    def off(self, event: str, handler: Optional[Callable[..., Any]] = None) -> None:
        """Remove an event handler."""
        if event == "audio":
            if handler is None:
                self._audio_handlers.clear()
            elif handler in self._audio_handlers:
                self._audio_handlers.remove(handler)
        elif event == "playback_complete":
            if handler is None:
                self._playback_handlers.clear()
            elif handler in self._playback_handlers:
                self._playback_handlers.remove(handler)
        else:
            self._session.off(event, handler)

    async def connect(self, timeout: float = 10.0) -> None:
        """Connect to the gateway."""
        await self._session.connect(timeout=timeout)

    async def disconnect(self) -> None:
        """Disconnect from the gateway."""
        await self._session.disconnect()

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

    def get_metrics(self) -> SessionMetrics:
        """Get session metrics."""
        return self._session.get_metrics()

    def reset_metrics(self) -> None:
        """Reset session metrics."""
        self._session.reset_metrics()

    async def __aiter__(self) -> AsyncIterator[AudioEvent]:
        """Iterate over audio events."""
        async for message in self._session:
            if message.get("type") == "audio":
                yield message["audio"]

    async def __aenter__(self) -> "TTSSession":
        """Async context manager entry."""
        await self.connect()
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.disconnect()
