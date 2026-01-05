"""
BudSTT - Speech-to-Text pipeline
"""

from typing import Any, AsyncIterator, Callable, Optional, Union

from ..types import STTConfig, STTResult, FeatureFlags
from ..ws.session import WebSocketSession, SessionMetrics, ReconnectConfig


class BudSTT:
    """Speech-to-Text pipeline for real-time transcription."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
    ):
        """
        Initialize STT pipeline.

        Args:
            url: WebSocket URL of the gateway
            api_key: Optional API key for authentication
        """
        self.url = url
        self.api_key = api_key

    def create(
        self,
        config: Optional[STTConfig] = None,
        provider: str = "deepgram",
        language: str = "en-US",
        model: Optional[str] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> "STTSession":
        """
        Create an STT session.

        Args:
            config: Full STT configuration (overrides other params)
            provider: STT provider
            language: Language code
            model: Model to use
            features: Feature flags
            reconnect: Reconnection configuration

        Returns:
            STT session
        """
        if config is None:
            config = STTConfig(
                provider=provider,
                language=language,
                model=model,
                interim_results=features.interim_results if features else True,
                punctuate=features.punctuation if features else True,
                profanity_filter=features.profanity_filter if features else False,
                smart_format=features.smart_format if features else True,
                diarize=features.speaker_diarization if features else False,
            )

        return STTSession(
            url=self.url,
            api_key=self.api_key,
            config=config,
            features=features,
            reconnect=reconnect,
        )

    async def connect(
        self,
        config: Optional[STTConfig] = None,
        provider: str = "deepgram",
        language: str = "en-US",
        model: Optional[str] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ) -> "STTSession":
        """
        Create and connect an STT session.

        Args:
            config: Full STT configuration
            provider: STT provider
            language: Language code
            model: Model to use
            features: Feature flags
            reconnect: Reconnection configuration

        Returns:
            Connected STT session
        """
        session = self.create(
            config=config,
            provider=provider,
            language=language,
            model=model,
            features=features,
            reconnect=reconnect,
        )
        await session.connect()
        return session


class STTSession:
    """Active STT session for streaming transcription."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        config: Optional[STTConfig] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ):
        """
        Initialize STT session.

        Args:
            url: WebSocket URL
            api_key: API key
            config: STT configuration
            features: Feature flags
            reconnect: Reconnection config
        """
        self.config = config or STTConfig(provider="deepgram")
        self.features = features

        self._session = WebSocketSession(
            url=url,
            api_key=api_key,
            stt_config=self.config,
            reconnect=reconnect,
        )

        self._transcript_handlers: list[Callable[[STTResult], Any]] = []
        self._session.on("transcript", self._on_transcript)

    def _on_transcript(self, result: STTResult) -> None:
        """Handle transcript events."""
        for handler in self._transcript_handlers:
            try:
                handler(result)
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
            event: Event name (transcript, error, close)
            handler: Event handler function
        """
        if event == "transcript":
            self._transcript_handlers.append(handler)
        else:
            self._session.on(event, handler)

    def off(self, event: str, handler: Optional[Callable[..., Any]] = None) -> None:
        """Remove an event handler."""
        if event == "transcript":
            if handler is None:
                self._transcript_handlers.clear()
            elif handler in self._transcript_handlers:
                self._transcript_handlers.remove(handler)
        else:
            self._session.off(event, handler)

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

    async def transcribe_stream(
        self,
        audio_stream: AsyncIterator[bytes],
    ) -> AsyncIterator[STTResult]:
        """
        Transcribe an audio stream.

        Args:
            audio_stream: Async iterator yielding audio chunks

        Yields:
            STT results as they come in
        """
        # Start consuming audio in background
        async def send_audio_task() -> None:
            async for chunk in audio_stream:
                await self.send_audio(chunk)

        import asyncio
        task = asyncio.create_task(send_audio_task())

        try:
            async for message in self._session:
                if message.get("type") == "transcript":
                    yield message["result"]
        finally:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

    def get_metrics(self) -> SessionMetrics:
        """Get session metrics."""
        return self._session.get_metrics()

    def reset_metrics(self) -> None:
        """Reset session metrics."""
        self._session.reset_metrics()

    async def __aiter__(self) -> AsyncIterator[STTResult]:
        """Iterate over transcription results."""
        async for message in self._session:
            if message.get("type") == "transcript":
                yield message["result"]

    async def __aenter__(self) -> "STTSession":
        """Async context manager entry."""
        await self.connect()
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.disconnect()
