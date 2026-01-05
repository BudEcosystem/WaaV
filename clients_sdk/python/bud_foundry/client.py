"""
BudClient - Main entry point for Bud Foundry SDK
"""

from typing import Any, Optional

from .rest.client import RestClient
from .pipelines.stt import BudSTT
from .pipelines.tts import BudTTS
from .pipelines.talk import BudTalk
from .pipelines.transcribe import BudTranscribe


class BudClient:
    """
    Main client for Bud Foundry AI Gateway.

    Provides access to all pipelines:
    - stt: Speech-to-Text
    - tts: Text-to-Speech
    - talk: Bidirectional voice
    - transcribe: Batch file transcription

    Example:
        >>> bud = BudClient(base_url="http://localhost:3001", api_key="your-api-key")
        >>>
        >>> # STT
        >>> async with bud.stt.connect(provider="deepgram") as session:
        ...     async for result in session:
        ...         print(result.text)
        >>>
        >>> # TTS
        >>> async with bud.tts.connect(provider="elevenlabs") as session:
        ...     await session.speak("Hello, world!")
        >>>
        >>> # Talk (bidirectional)
        >>> async with bud.talk.connect(
        ...     stt={"provider": "deepgram"},
        ...     tts={"provider": "elevenlabs"}
        ... ) as session:
        ...     async for event in session:
        ...         if event.type == "transcript":
        ...             print(event.transcript.text)
    """

    def __init__(
        self,
        base_url: str,
        api_key: Optional[str] = None,
        timeout: float = 30.0,
    ):
        """
        Initialize Bud Foundry client.

        Args:
            base_url: Base URL of the Bud Foundry gateway (e.g., "http://localhost:3001")
            api_key: Optional API key for authentication
            timeout: Default timeout for REST requests in seconds
        """
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout

        # Build WebSocket URL
        ws_url = self.base_url.replace("http://", "ws://").replace("https://", "wss://")
        self._ws_url = f"{ws_url}/ws"

        # Initialize REST client
        self._rest_client = RestClient(
            base_url=self.base_url,
            api_key=self.api_key,
            timeout=self.timeout,
        )

        # Initialize pipelines
        self._stt = BudSTT(url=self._ws_url, api_key=self.api_key)
        self._tts = BudTTS(url=self._ws_url, api_key=self.api_key, rest_client=self._rest_client)
        self._talk = BudTalk(url=self._ws_url, api_key=self.api_key)
        self._transcribe = BudTranscribe(url=self._ws_url, api_key=self.api_key)

    @property
    def stt(self) -> BudSTT:
        """Get the STT (Speech-to-Text) pipeline."""
        return self._stt

    @property
    def tts(self) -> BudTTS:
        """Get the TTS (Text-to-Speech) pipeline."""
        return self._tts

    @property
    def talk(self) -> BudTalk:
        """Get the Talk (bidirectional voice) pipeline."""
        return self._talk

    @property
    def transcribe(self) -> BudTranscribe:
        """Get the Transcribe (batch file) pipeline."""
        return self._transcribe

    @property
    def rest(self) -> RestClient:
        """Get the REST client for direct API access."""
        return self._rest_client

    async def health(self) -> dict[str, Any]:
        """
        Check gateway health.

        Returns:
            Health status with version info
        """
        return await self._rest_client.health()

    async def list_voices(self, provider: Optional[str] = None) -> list[dict[str, Any]]:
        """
        List available TTS voices.

        Args:
            provider: Optional provider to filter voices

        Returns:
            List of available voices
        """
        return await self._rest_client.list_voices(provider=provider)

    async def create_livekit_token(
        self,
        room_name: str,
        identity: str,
        name: Optional[str] = None,
        ttl: Optional[int] = None,
        metadata: Optional[str] = None,
    ) -> dict[str, Any]:
        """
        Generate a LiveKit access token.

        Args:
            room_name: Room name to join
            identity: Participant identity
            name: Participant display name
            ttl: Token TTL in seconds
            metadata: Participant metadata

        Returns:
            Token response with JWT and room info
        """
        return await self._rest_client.create_livekit_token(
            room_name=room_name,
            identity=identity,
            name=name,
            ttl=ttl,
            metadata=metadata,
        )

    async def get_livekit_room(self, room_name: str) -> dict[str, Any]:
        """
        Get LiveKit room information.

        Args:
            room_name: Room name

        Returns:
            Room information
        """
        return await self._rest_client.get_livekit_room(room_name)

    async def list_livekit_rooms(self) -> list[dict[str, Any]]:
        """
        List all LiveKit rooms.

        Returns:
            List of rooms
        """
        return await self._rest_client.list_livekit_rooms()

    async def list_sip_hooks(self) -> list[dict[str, Any]]:
        """
        List all SIP hooks.

        Returns:
            List of SIP hooks
        """
        return await self._rest_client.list_sip_hooks()

    async def create_sip_hook(
        self,
        host: str,
        webhook_url: str,
    ) -> dict[str, Any]:
        """
        Create a SIP hook.

        Args:
            host: SIP host
            webhook_url: Webhook URL for incoming calls

        Returns:
            Created hook info
        """
        return await self._rest_client.create_sip_hook(
            host=host,
            webhook_url=webhook_url,
        )

    async def delete_sip_hook(self, host: str) -> None:
        """
        Delete a SIP hook.

        Args:
            host: SIP host to delete
        """
        await self._rest_client.delete_sip_hook(host)

    async def close(self) -> None:
        """Close all connections."""
        await self._rest_client.close()

    async def __aenter__(self) -> "BudClient":
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.close()
