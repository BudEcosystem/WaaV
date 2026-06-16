"""
BudClient - Main entry point for Bud WaaV SDK
"""

import weakref
from typing import Any, Optional, Union

from .rest.client import RestClient
from .pipelines.stt import BudSTT
from .pipelines.tts import BudTTS
from .pipelines.talk import BudTalk, TalkSession
from .pipelines.transcribe import BudTranscribe
from .pipelines.realtime import BudRealtime, RealtimeConfig
from .types import (
    STTConfig, TTSConfig, ConversationConfig, AudioFeatures, TurnDetectionConfig,
)
from .ws.session import ReconnectConfig


class BudClient:
    """
    Main client for Bud WaaV AI Gateway.

    Provides access to all pipelines:
    - stt: Speech-to-Text
    - tts: Text-to-Speech
    - talk: Bidirectional voice
    - transcribe: Batch file transcription
    - realtime: Real-time bidirectional audio with LLM (OpenAI Realtime, Hume EVI)

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
        Initialize Bud WaaV client.

        Args:
            base_url: Base URL of the Bud WaaV gateway (e.g., "http://localhost:3001")
            api_key: Optional API key for authentication
            timeout: Default timeout for REST requests in seconds
        """
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout

        # Build WebSocket URLs
        ws_url = self.base_url.replace("http://", "ws://").replace("https://", "wss://")
        self._ws_url = f"{ws_url}/ws"
        self._realtime_url = f"{ws_url}/realtime"

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

        # Active pipeline lifecycle tracking (weakrefs to avoid preventing GC)
        self._active_pipelines: weakref.WeakSet[Any] = weakref.WeakSet()

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

    @property
    def realtime_url(self) -> str:
        """Get the realtime WebSocket URL."""
        return self._realtime_url

    def create_realtime(self, config: RealtimeConfig) -> BudRealtime:
        """
        Create a Realtime pipeline instance.

        When ``config.gateway_url`` is not set, it defaults to routing through
        this client's gateway so the provider connection is managed server-side.
        Set ``config.gateway_url`` explicitly to ``None`` and provide an
        ``api_key`` in config if you prefer direct provider connections.

        Args:
            config: Realtime configuration (provider, model, voice, etc.)

        Returns:
            BudRealtime instance ready to connect.

        Example:
            >>> config = RealtimeConfig(
            ...     provider=RealtimeProvider.OPENAI_REALTIME,
            ...     system_prompt="You are a helpful assistant."
            ... )
            >>> realtime = bud.create_realtime(config)
            >>> await realtime.connect()  # Connects via gateway automatically
        """
        # Default to gateway routing through this client's gateway
        ws_base = self.base_url.replace("http://", "ws://").replace("https://", "wss://")
        if config.gateway_url is None and not config.api_key:
            # Route through gateway - set gateway_url and forward API key
            config.gateway_url = ws_base
            if self.api_key:
                config.api_key = self.api_key
        return BudRealtime(config)

    def agent(
        self,
        stt: Optional[Union[STTConfig, dict[str, Any]]] = None,
        tts: Optional[Union[TTSConfig, dict[str, Any]]] = None,
        llm: Optional[Union[ConversationConfig, dict[str, Any]]] = None,
        turn: Optional[dict[str, Any]] = None,
        reconnect: Optional[ReconnectConfig] = None,
        stream_id: Optional[str] = None,
    ) -> TalkSession:
        """Create the flagship agent loop (STT -> built-in LLM -> TTS) in a few lines.

        This is the ONLY SDK entry point that reaches the gateway's built-in
        conversation loop. It serializes ``conversation_config`` (the LLM loop +
        reasoning stack) and nests turn detection into ``stt_config.turn_detection``.

        Args:
            stt: STT configuration (typed or dict).
            tts: TTS configuration (typed or dict).
            llm: Conversation/LLM-loop configuration. A dict is coerced to
                :class:`ConversationConfig` — it MUST carry ``base_url`` and
                ``model``; reasoning/latency/barge-in knobs are optional.
            turn: Turn-detection knobs, e.g. ``{"enabled": True, "threshold": 0.6,
                "eager_eot": True}``. ``eager_eot`` is forwarded to
                ``conversation_config.eager_eot`` (the gateway requires both).
            reconnect: Reconnection configuration.
            stream_id: Optional stream ID for session tracking.

        Returns:
            An unconnected :class:`TalkSession`. ``await session.connect()`` (or
            use it as an async context manager) then iterate events
            (``transcript`` | ``audio`` | ``message`` | ``error``).

        Example:
            >>> session = bud.agent(
            ...     stt={"provider": "deepgram", "language": "en-US"},
            ...     tts={"provider": "deepgram", "voice_id": "aura-asteria-en"},
            ...     llm={"base_url": "http://localhost:11434/v1", "model": "llama3.2:1b",
            ...          "reasoning_effort": "minimal", "latency_filler": "auto"},
            ...     turn={"eager_eot": True},
            ... )
            >>> async with session as call:
            ...     await call.send_audio(pcm)
            ...     async for ev in call:
            ...         ...
        """
        conversation_config: Optional[ConversationConfig] = None
        if llm is not None:
            conversation_config = llm if isinstance(llm, ConversationConfig) else ConversationConfig(**llm)

        # Map turn={} to AudioFeatures.turn_detection (nested into stt_config.turn_detection
        # on the wire) and forward eager_eot to the conversation loop.
        audio_features: Optional[AudioFeatures] = None
        if turn is not None:
            td_kwargs = {k: v for k, v in turn.items() if k != "eager_eot"}
            audio_features = AudioFeatures(turn_detection=TurnDetectionConfig(**td_kwargs))
            if turn.get("eager_eot") and conversation_config is not None and conversation_config.eager_eot is None:
                conversation_config.eager_eot = True

        return self._talk.create(
            stt=stt,
            tts=tts,
            reconnect=reconnect,
            audio_features=audio_features,
            conversation_config=conversation_config,
            stream_id=stream_id,
        )

    async def health(self) -> dict[str, Any]:
        """
        Check gateway health.

        Returns:
            Health status with version info
        """
        return await self._rest_client.health()

    async def list_voices(
        self,
        provider: Optional[str] = None,
    ) -> dict[str, list[dict[str, Any]]]:
        """
        List available TTS voices.

        Args:
            provider: Optional provider to filter voices

        Returns:
            Dictionary mapping provider names to lists of voice objects.
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

    async def delete_sip_hooks(self, hosts: list[str]) -> dict[str, Any]:
        """
        Delete SIP hooks by host names.

        Args:
            hosts: List of SIP host names to delete (case-insensitive).

        Returns:
            Updated list of remaining SIP hooks.
        """
        return await self._rest_client.delete_sip_hooks(hosts)

    async def remove_livekit_participant(
        self,
        room_name: str,
        identity: str,
    ) -> dict[str, Any]:
        """
        Remove a participant from a LiveKit room.

        Args:
            room_name: Room name.
            identity: Participant identity to remove.

        Returns:
            Removal response with status, room_name, and participant_identity.
        """
        return await self._rest_client.remove_livekit_participant(room_name, identity)

    async def mute_livekit_participant(
        self,
        room_name: str,
        identity: str,
        track_sid: str,
        muted: bool = True,
    ) -> dict[str, Any]:
        """
        Mute or unmute a participant's track in a LiveKit room.

        Args:
            room_name: Room name.
            identity: Participant identity.
            track_sid: Session ID of the track to mute/unmute.
            muted: Whether to mute (True) or unmute (False).

        Returns:
            Updated mute state.
        """
        return await self._rest_client.mute_livekit_participant(
            room_name, identity, track_sid=track_sid, muted=muted,
        )

    async def get_metrics(self) -> dict[str, Any]:
        """
        Get server performance metrics.

        Returns:
            Server metrics
        """
        return await self._rest_client.get_metrics()

    # =========================================================================
    # Lifecycle Management
    # =========================================================================

    def register_pipeline(self, session: Any) -> None:
        """Register an active pipeline session for lifecycle tracking.

        Sessions registered here can be disconnected with ``disconnect_all()``.
        Uses weak references so sessions are automatically removed when garbage
        collected.

        Args:
            session: A pipeline session (STTSession, TTSSession, TalkSession, etc.)
        """
        self._active_pipelines.add(session)

    def deregister_pipeline(self, session: Any) -> None:
        """Remove a pipeline session from lifecycle tracking.

        Args:
            session: A pipeline session to remove.
        """
        self._active_pipelines.discard(session)

    def get_active_pipeline_count(self) -> int:
        """Get the number of currently active pipeline sessions.

        Returns:
            Count of tracked active sessions.
        """
        return len(self._active_pipelines)

    async def disconnect_all(self) -> int:
        """Disconnect all active pipeline sessions.

        Calls ``close()`` on each tracked session. Sessions that raise
        exceptions during close are silently ignored.

        Returns:
            Number of sessions that were disconnected.
        """
        sessions = list(self._active_pipelines)
        count = 0
        for session in sessions:
            try:
                if hasattr(session, "close"):
                    await session.close()
                elif hasattr(session, "disconnect"):
                    await session.disconnect()
                count += 1
            except Exception:
                pass
        self._active_pipelines.clear()
        return count

    async def close(self) -> None:
        """Close all connections and active pipelines."""
        await self.disconnect_all()
        await self._rest_client.close()

    async def __aenter__(self) -> "BudClient":
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.close()
