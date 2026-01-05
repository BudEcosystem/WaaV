"""
BudTranscribe - Batch file transcription pipeline
"""

import asyncio
from pathlib import Path
from typing import Any, AsyncIterator, Optional, Union

from ..types import STTConfig, STTResult, FeatureFlags
from ..ws.session import WebSocketSession, SessionMetrics, ReconnectConfig


class BudTranscribe:
    """Batch transcription pipeline for audio files."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
    ):
        """
        Initialize Transcribe pipeline.

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
    ) -> "TranscribeSession":
        """
        Create a Transcribe session.

        Args:
            config: Full STT configuration
            provider: STT provider
            language: Language code
            model: Model to use
            features: Feature flags
            reconnect: Reconnection configuration

        Returns:
            Transcribe session
        """
        if config is None:
            config = STTConfig(
                provider=provider,
                language=language,
                model=model,
                interim_results=False,  # For batch, we usually want only final results
                punctuate=features.punctuation if features else True,
                profanity_filter=features.profanity_filter if features else False,
                smart_format=features.smart_format if features else True,
                diarize=features.speaker_diarization if features else False,
            )

        return TranscribeSession(
            url=self.url,
            api_key=self.api_key,
            config=config,
            features=features,
            reconnect=reconnect,
        )

    async def file(
        self,
        file_path: Union[str, Path],
        config: Optional[STTConfig] = None,
        provider: str = "deepgram",
        language: str = "en-US",
        model: Optional[str] = None,
        features: Optional[FeatureFlags] = None,
        chunk_size: int = 4096,
    ) -> STTResult:
        """
        Transcribe an audio file.

        Args:
            file_path: Path to audio file (WAV, MP3, etc.)
            config: Full STT configuration
            provider: STT provider
            language: Language code
            model: Model to use
            features: Feature flags
            chunk_size: Size of audio chunks to send

        Returns:
            Final transcription result
        """
        session = self.create(
            config=config,
            provider=provider,
            language=language,
            model=model,
            features=features,
        )

        return await session.transcribe_file(file_path, chunk_size=chunk_size)

    async def files(
        self,
        file_paths: list[Union[str, Path]],
        config: Optional[STTConfig] = None,
        provider: str = "deepgram",
        language: str = "en-US",
        model: Optional[str] = None,
        features: Optional[FeatureFlags] = None,
        chunk_size: int = 4096,
        concurrency: int = 3,
    ) -> list[STTResult]:
        """
        Transcribe multiple audio files concurrently.

        Args:
            file_paths: List of paths to audio files
            config: Full STT configuration
            provider: STT provider
            language: Language code
            model: Model to use
            features: Feature flags
            chunk_size: Size of audio chunks to send
            concurrency: Maximum concurrent transcriptions

        Returns:
            List of transcription results
        """
        semaphore = asyncio.Semaphore(concurrency)

        async def transcribe_one(path: Union[str, Path]) -> STTResult:
            async with semaphore:
                return await self.file(
                    path,
                    config=config,
                    provider=provider,
                    language=language,
                    model=model,
                    features=features,
                    chunk_size=chunk_size,
                )

        results = await asyncio.gather(*[transcribe_one(p) for p in file_paths])
        return list(results)


class TranscribeSession:
    """Session for batch transcription."""

    def __init__(
        self,
        url: str,
        api_key: Optional[str] = None,
        config: Optional[STTConfig] = None,
        features: Optional[FeatureFlags] = None,
        reconnect: Optional[ReconnectConfig] = None,
    ):
        """
        Initialize Transcribe session.

        Args:
            url: WebSocket URL
            api_key: API key
            config: STT configuration
            features: Feature flags
            reconnect: Reconnection config
        """
        self.config = config or STTConfig(provider="deepgram")
        self.features = features

        self._url = url
        self._api_key = api_key
        self._reconnect = reconnect

    async def transcribe_file(
        self,
        file_path: Union[str, Path],
        chunk_size: int = 4096,
    ) -> STTResult:
        """
        Transcribe an audio file.

        Args:
            file_path: Path to audio file
            chunk_size: Size of audio chunks to send

        Returns:
            Final transcription result
        """
        path = Path(file_path)

        if not path.exists():
            raise FileNotFoundError(f"Audio file not found: {path}")

        # Read and process audio file
        audio_data = await self._read_audio_file(path)

        # Create session and transcribe
        session = WebSocketSession(
            url=self._url,
            api_key=self._api_key,
            stt_config=self.config,
            reconnect=self._reconnect,
        )

        full_text = ""
        final_result: Optional[STTResult] = None

        async with session:
            # Send audio in chunks
            for i in range(0, len(audio_data), chunk_size):
                chunk = audio_data[i:i + chunk_size]
                await session.send_audio(chunk)
                # Small delay to avoid overwhelming the server
                await asyncio.sleep(0.01)

            # Wait for results
            timeout = 30.0  # Max wait time
            start_time = asyncio.get_event_loop().time()

            async for message in session:
                if message.get("type") == "transcript":
                    result: STTResult = message["result"]
                    if result.is_final:
                        full_text += result.text + " "
                        final_result = result

                # Check timeout
                if asyncio.get_event_loop().time() - start_time > timeout:
                    break

                # Give some time for more results
                await asyncio.sleep(0.1)

        # Return combined result
        if final_result is None:
            final_result = STTResult(
                text=full_text.strip(),
                is_final=True,
            )
        else:
            final_result = STTResult(
                text=full_text.strip(),
                is_final=True,
                confidence=final_result.confidence,
                speaker_id=final_result.speaker_id,
                language=final_result.language,
            )

        return final_result

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
        session = WebSocketSession(
            url=self._url,
            api_key=self._api_key,
            stt_config=self.config,
            reconnect=self._reconnect,
        )

        async with session:
            # Start sending audio in background
            async def send_task() -> None:
                async for chunk in audio_stream:
                    await session.send_audio(chunk)

            task = asyncio.create_task(send_task())

            try:
                async for message in session:
                    if message.get("type") == "transcript":
                        yield message["result"]
            finally:
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass

    async def _read_audio_file(self, path: Path) -> bytes:
        """
        Read an audio file and return raw PCM data.

        Args:
            path: Path to audio file

        Returns:
            Raw PCM audio data
        """
        suffix = path.suffix.lower()

        if suffix == ".wav":
            return await self._read_wav(path)
        elif suffix == ".raw" or suffix == ".pcm":
            return await asyncio.to_thread(path.read_bytes)
        else:
            # For other formats, try to read as raw
            # In production, you'd want to use a library like pydub or ffmpeg
            return await asyncio.to_thread(path.read_bytes)

    async def _read_wav(self, path: Path) -> bytes:
        """Read a WAV file and return PCM data."""
        import wave

        def read_wav() -> bytes:
            with wave.open(str(path), "rb") as wav:
                # Verify format
                if wav.getsampwidth() != 2:
                    raise ValueError("Only 16-bit audio is supported")

                return wav.readframes(wav.getnframes())

        return await asyncio.to_thread(read_wav)

    def get_metrics(self) -> SessionMetrics:
        """Get metrics (returns empty metrics for batch)."""
        from ..ws.session import SessionMetrics
        return SessionMetrics()
