"""
BudClient - Main entry point for Bud WaaV SDK
"""

import asyncio
import base64
import time
import weakref
from pathlib import Path
from typing import Any, Optional, Union

from .rest.client import RestClient
from .voices import VoicesAPI
from .pipelines.stt import BudSTT
from .pipelines.tts import BudTTS
from .pipelines.talk import BudTalk, TalkSession
from .pipelines.transcribe import BudTranscribe
from .pipelines.realtime import BudRealtime, RealtimeConfig
from .pipelines.realtime_gateway import (
    GatewayRealtime,
    GatewayRealtimeConfig,
    GatewayTurnDetection,
    RealtimeTool,
)
from .types import (
    STTConfig, TTSConfig, ConversationConfig, AudioFeatures, TurnDetectionConfig,
    TranslationConfig, CANONICAL_LANGUAGES, language_capabilities,
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

        # Voice-cloning helper surface (P4): bud.voices.clone(...) + wait_until_ready.
        self._voices = VoicesAPI(self._rest_client)

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
    def voices(self) -> VoicesAPI:
        """Voice-cloning helpers (P4): ``bud.voices.clone(...)`` + ``wait_until_ready``.

        Clone a voice from samples and get back a usable TTS ``voice_id``::

            result = await bud.voices.clone(
                provider="elevenlabs", name="My Voice", samples=[wav_bytes]
            )
            voice = await result.wait_until_ready()
            # TTSConfig(provider="elevenlabs", voice_id=voice.voice_id)
        """
        return self._voices

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

    def realtime(
        self,
        provider: str = "openai",
        *,
        voice: Optional[str] = None,
        instructions: Optional[str] = None,
        model: Optional[str] = None,
        temperature: Optional[float] = None,
        max_response_tokens: Optional[int] = None,
        turn_detection: Optional[Union[GatewayTurnDetection, dict[str, Any]]] = None,
        tools: Optional[list[Union[RealtimeTool, dict[str, Any]]]] = None,
        modalities: Optional[list[str]] = None,
        transcribe_input: Optional[bool] = None,
        transcription_model: Optional[str] = None,
        input_audio_format: Optional[str] = None,
        output_audio_format: Optional[str] = None,
        reasoning_effort: Optional[str] = None,
        input_audio_noise_reduction: Optional[str] = None,
        alias: Optional[str] = None,
    ) -> GatewayRealtime:
        """The GATEWAY-NATIVE realtime entry point (recommended).

        Returns an UNCONNECTED :class:`GatewayRealtime` that speaks the gateway's
        provider-agnostic ``/realtime`` WebSocket protocol, so the SAME surface
        works for ALL 12 realtime/S2S providers — ``provider`` is just a field:
        ``openai``, ``hume``, ``azure``, ``grok``, ``inworld``, ``deepgram``,
        ``elevenlabs``, ``gemini``, ``ultravox``, ``nova_sonic``,
        ``speechmatics``, ``yandex``.

        Unlike :meth:`create_realtime` (the provider-NATIVE escape hatch that only
        knows OpenAI/Hume and can bypass the gateway), this client never speaks a
        vendor wire: the gateway connects the provider server-side from its own
        credentials. Consume events via callbacks (``session.on(...)``) AND/OR the
        unified async stream (``async for ev in session``).

        Args:
            provider: Realtime provider (default ``"openai"``). One of the 12 above.
            voice: Output TTS voice (provider-specific).
            instructions: System instructions for the assistant.
            model: Model id (provider-specific; gateway defaults apply if unset).
            temperature: Response temperature.
            max_response_tokens: Max output tokens (-1 = unlimited).
            turn_detection: A :class:`GatewayTurnDetection` or a dict
                (``{"mode":"server_vad","threshold":0.5,...}`` /
                ``{"mode":"semantic","eagerness":"high"}`` / ``{"mode":"manual"}``).
            tools: Function tools (``RealtimeTool`` or
                ``{"name","description","parameters"}`` dicts).
            modalities: Output modalities (e.g. ``["text","audio"]``).
            transcribe_input: Enable input-audio transcription.
            transcription_model: Input transcription model.
            input_audio_format / output_audio_format: Audio format overrides.
            reasoning_effort: ``minimal|low|medium|high`` for reasoning-capable
                realtime models.
            input_audio_noise_reduction: ``near_field`` | ``far_field``.
            alias: Server-side alias bundle name (P3).

        Returns:
            An unconnected :class:`GatewayRealtime`. ``await session.connect()``
            (or use it as an async context manager), then send audio/text and
            iterate events.

        Example:
            >>> session = bud.realtime(provider="openai", voice="alloy",
            ...                        instructions="You are helpful.")
            >>> async with session:
            ...     await session.send_audio(pcm)
            ...     async for ev in session:
            ...         if ev.type == "transcript": print(ev.transcript.text)
            ...         elif ev.type == "audio":    play(ev.audio.audio)
        """
        td: Optional[GatewayTurnDetection] = None
        if isinstance(turn_detection, GatewayTurnDetection):
            td = turn_detection
        elif isinstance(turn_detection, dict):
            td = GatewayTurnDetection(**turn_detection)

        tool_list: list[RealtimeTool] = []
        for t in tools or []:
            tool_list.append(t if isinstance(t, RealtimeTool) else RealtimeTool(**t))

        config = GatewayRealtimeConfig(
            provider=provider,
            voice=voice,
            instructions=instructions,
            model=model,
            temperature=temperature,
            max_response_tokens=max_response_tokens,
            turn_detection=td,
            tools=tool_list,
            modalities=modalities,
            transcribe_input=transcribe_input,
            transcription_model=transcription_model,
            input_audio_format=input_audio_format,
            output_audio_format=output_audio_format,
            reasoning_effort=reasoning_effort,
            input_audio_noise_reduction=input_audio_noise_reduction,
            alias=alias,
        )
        return GatewayRealtime(url=self._realtime_url, config=config, api_key=self.api_key)

    async def transcribe_batch(
        self,
        audio: Optional[Union[str, bytes, Path]] = None,
        *,
        url: Optional[str] = None,
        content_type: Optional[str] = None,
        config: Optional[Union[STTConfig, dict[str, Any]]] = None,
        provider: Optional[str] = None,
        language: Optional[str] = None,
        model: Optional[str] = None,
        batch: Optional[dict[str, Any]] = None,
        translation: Optional[Union[TranslationConfig, dict[str, Any]]] = None,
        callback_url: Optional[str] = None,
        callback_method: Optional[str] = None,
        poll: bool = True,
        poll_interval: float = 2.0,
        timeout: float = 300.0,
    ) -> dict[str, Any]:
        """Submit a batched/async transcription job and (by default) poll until done.

        Hits ``POST /transcribe/batch`` then polls ``GET /transcribe/batch/{job_id}``
        until the job reaches ``completed``/``error`` (or ``timeout``). The batch
        envelope REUSES the streaming :class:`~bud_waav.types.STTConfig` VERBATIM
        plus batch-only knobs (``batch=``) that the streaming path drops but ARE
        batch-capable on Deepgram prerecorded / AssemblyAI async / OpenAI
        (alternatives, detect_language, summarize, topics, intents, paragraphs,
        utterances).

        Audio source — supply ONE of:
          * ``audio`` as a URL string, a filesystem path (``str``/``Path``), or raw
            ``bytes`` (base64-encoded for you), or
          * ``url=`` explicitly.

        Async mode: pass ``callback_url`` (and optionally ``poll=False``) — the
        gateway returns ``{job_id, status:"queued"}`` immediately and POSTs the
        result to your webhook.

        Args:
            audio: URL str / file path / raw bytes. A non-existent string that
                looks like ``http(s)://`` is treated as a URL.
            url: Explicit audio URL (overrides ``audio`` URL detection).
            content_type: MIME type for inline bytes (e.g. ``audio/wav``).
            config: Full :class:`STTConfig` (or dict) reused verbatim. If omitted,
                a minimal config is built from ``provider``/``language``/``model``.
            provider: STT provider when ``config`` is not given (default deepgram).
            language: Language when ``config`` is not given (default en-US).
            model: Model when ``config`` is not given.
            batch: ``BatchFeatures`` dict (see :meth:`RestClient.submit_batch`).
            translation: Canonical translation block (typed or dict).
            callback_url / callback_method: Async webhook delivery.
            poll: Poll until terminal when no ``callback_url`` (default True).
            poll_interval: Seconds between polls.
            timeout: Max seconds to poll before raising ``TimeoutError``.

        Returns:
            The terminal ``BatchJob`` dict (``{job_id, status, result?, error?}``)
            when polled, else the submit response (``{job_id, status}``).
        """
        # Resolve audio source.
        submit_url: Optional[str] = url
        audio_b64: Optional[str] = None
        if submit_url is None and audio is not None:
            if isinstance(audio, (bytes, bytearray)):
                audio_b64 = base64.b64encode(bytes(audio)).decode("utf-8")
            elif isinstance(audio, Path):
                audio_b64 = base64.b64encode(audio.read_bytes()).decode("utf-8")
            elif isinstance(audio, str):
                if audio.startswith("http://") or audio.startswith("https://"):
                    submit_url = audio
                else:
                    audio_b64 = base64.b64encode(
                        Path(audio).read_bytes()
                    ).decode("utf-8")

        # Resolve STT config → flattened StandardSTTConfig dict.
        stt_cfg: STTConfig
        if isinstance(config, STTConfig):
            stt_cfg = config
        elif isinstance(config, dict):
            stt_cfg = STTConfig(**config)
        else:
            stt_cfg = STTConfig(
                provider=provider or "deepgram",
                language=language or "en-US",
                model=model,
            )
        stt_dict = stt_cfg.model_dump(exclude_none=True, exclude={"translation"})

        # Translation block (typed or dict) → wire.
        translation_wire: Optional[dict[str, Any]] = None
        if isinstance(translation, TranslationConfig):
            translation_wire = translation.to_wire()
        elif isinstance(translation, dict):
            translation_wire = TranslationConfig(**translation).to_wire()
        elif stt_cfg.translation is not None:
            translation_wire = stt_cfg.translation.to_wire()

        submit = await self._rest_client.submit_batch(
            url=submit_url,
            audio_base64=audio_b64,
            content_type=content_type,
            stt_config=stt_dict,
            batch=batch,
            translation=translation_wire or None,
            callback_url=callback_url,
            callback_method=callback_method,
        )

        # Webhook mode or caller opted out of polling → return submit ack.
        if callback_url is not None or not poll:
            return submit

        # Some providers run synchronously and return a terminal job immediately.
        status = str(submit.get("status", "")).lower()
        if status in ("completed", "error") or submit.get("result") is not None:
            return submit

        job_id = submit.get("job_id")
        if not job_id:
            return submit

        deadline = time.monotonic() + timeout
        while True:
            job = await self._rest_client.get_batch(job_id)
            status = str(job.get("status", "")).lower()
            if status in ("completed", "error"):
                return job
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"Batch job {job_id} did not finish within {timeout}s "
                    f"(last status: {status or 'unknown'})"
                )
            await asyncio.sleep(poll_interval)

    def agent(
        self,
        stt: Optional[Union[STTConfig, dict[str, Any]]] = None,
        tts: Optional[Union[TTSConfig, dict[str, Any]]] = None,
        llm: Optional[Union[ConversationConfig, dict[str, Any]]] = None,
        turn: Optional[dict[str, Any]] = None,
        interrupt: bool = True,
        reconnect: Optional[ReconnectConfig] = None,
        stream_id: Optional[str] = None,
        alias: Optional[str] = None,
    ) -> TalkSession:
        """Create the flagship agent loop (STT -> built-in LLM -> TTS) in ~5 lines.

        This is the ONLY SDK entry point that reaches the gateway's built-in
        conversation loop. It serializes ``conversation_config`` (the LLM loop +
        reasoning + latency-filler stack), nests turn detection into
        ``stt_config.turn_detection``, applies sane beginner defaults, and yields
        a single unified event stream (``transcript`` | ``bot_text`` | ``audio`` |
        ``warning``).

        Sane defaults applied (only when you did not set them explicitly):

        * ``allow_interruption`` ← ``interrupt`` (barge-in on by default);
        * ``strip_markdown`` ← ``True`` (spoken ``**asterisks**`` / URLs ruin TTS);
        * ``latency_filler`` ← ``"auto"`` (keep the line alive while a slow/reasoning
          turn streams in behind one short action phrase).

        The bot's reply is spoken back as ``audio`` events (the gateway streams the
        LLM reply straight to TTS on ``/ws``; there is no separate bot-text frame on
        a plain cascade session — ``bot_text`` fires only when an assistant-role
        transcript is exposed, e.g. via a DAG text node). Any gateway
        ``config_warning`` (e.g. ``reasoning_model_on_voice_path``,
        ``emotion_ignored_for_provider``, ``unknown_config_keys``) is surfaced as a
        typed ``warning`` event — never a silent no-op.

        Args:
            stt: STT configuration (typed or dict).
            tts: TTS configuration (typed or dict).
            llm: Conversation/LLM-loop configuration. A dict is coerced to
                :class:`ConversationConfig` — it MUST carry ``base_url`` and
                ``model`` (keep ``model`` FAST); reasoning/latency/barge-in knobs
                are optional. Set ``reasoning_model`` to turn the two-tier reasoning
                stack on.
            turn: Turn-detection knobs, e.g. ``{"enabled": True, "threshold": 0.6,
                "eager": True}``. ``eager`` (alias ``eager_eot``) is forwarded to
                BOTH ``stt_config.turn_detection.eager`` and
                ``conversation_config.eager_eot`` (the gateway requires both for
                eager end-of-turn).
            interrupt: Whether the bot is interruptible / barge-in (default True).
                Mapped to ``conversation_config.allow_interruption`` unless ``llm``
                already set it.
            reconnect: Reconnection configuration.
            stream_id: Optional stream ID for session tracking.

        Returns:
            An unconnected :class:`TalkSession`. ``await session.connect()`` (or
            use it as an async context manager) then iterate events.

        Example:
            >>> session = bud.agent(
            ...     stt={"provider": "deepgram", "language": "en-US"},
            ...     tts={"provider": "deepgram", "voice_id": "aura-asteria-en"},
            ...     llm={"base_url": "http://localhost:11434/v1", "model": "llama3.2:1b",
            ...          "reasoning_effort": "minimal", "latency_filler": "auto"},
            ...     turn={"eager": True},
            ... )
            >>> async with session as call:
            ...     await call.send_audio(pcm)
            ...     async for ev in call:
            ...         if ev.type == "transcript": print("user:", ev.text)
            ...         elif ev.type == "audio":    play(ev.audio.audio)
            ...         elif ev.type == "warning":  print("WARN", ev.warning.code, ev.warning.message)
        """
        conversation_config: Optional[ConversationConfig] = None
        if llm is not None:
            conversation_config = (
                llm if isinstance(llm, ConversationConfig) else ConversationConfig(**llm)
            )

        # Sane beginner defaults on the conversation loop (only fill when unset so
        # an explicit caller value always wins — None means "let the gateway/SDK
        # default apply", and these are the defaults that make a beginner agent
        # feel right out of the box).
        if conversation_config is not None:
            if conversation_config.allow_interruption is None:
                conversation_config.allow_interruption = interrupt
            if conversation_config.strip_markdown is None:
                conversation_config.strip_markdown = True
            if conversation_config.latency_filler is None:
                conversation_config.latency_filler = "auto"

        # Map turn={} to AudioFeatures.turn_detection (nested into
        # stt_config.turn_detection on the wire). Accept both ``eager`` (the wire
        # field name) and ``eager_eot`` (the conversation-loop field name) as the
        # beginner alias, and forward it to BOTH places the gateway needs.
        audio_features: Optional[AudioFeatures] = None
        if turn is not None:
            eager = bool(turn.get("eager") or turn.get("eager_eot"))
            td_kwargs = {
                k: v for k, v in turn.items() if k not in ("eager", "eager_eot")
            }
            # Default the master switch on when any turn config is supplied.
            td_kwargs.setdefault("enabled", True)
            td = TurnDetectionConfig(**td_kwargs)
            audio_features = AudioFeatures(turn_detection=td)
            if eager:
                # TurnDetectionConfig has no `eager` field (it carries detector
                # knobs only); the wire `turn_detection.eager` is set by the
                # session from conversation_config.eager_eot pairing. Forward to
                # the conversation loop, which the session serializes.
                if conversation_config is not None and conversation_config.eager_eot is None:
                    conversation_config.eager_eot = True

        return self._talk.create(
            stt=stt,
            tts=tts,
            reconnect=reconnect,
            audio_features=audio_features,
            conversation_config=conversation_config,
            stream_id=stream_id,
            alias=alias,
        )

    async def health(self) -> dict[str, Any]:
        """
        Check gateway health.

        Returns:
            Health status with version info
        """
        return await self._rest_client.health()

    async def prepare_connection(self, timeout: float = 5.0) -> bool:
        """Warm DNS/TLS to the gateway BEFORE first use (D6 prewarm).

        Call this at page-load / SDK init (it is safe to fire-and-forget) so the
        cold DNS + TCP + TLS handshake is paid up front and the socket lands in
        the REST keepalive pool. The first real ``connect()`` / REST call then
        attaches to an already-warm origin, shaving the typical 100-400ms cold
        handshake off perceived first-connect. Mirrors LiveKit
        ``Room.prepareConnection``.

        Best-effort: never raises (a dead gateway just returns ``False``); the
        WebSocket path warms the same origin host as REST, so a prewarmed REST
        pool benefits the ``/ws`` and ``/realtime`` upgrades too.

        Returns:
            ``True`` if the gateway origin was reached and warmed, else ``False``.
        """
        return await self._rest_client.warmup(timeout=timeout)

    # ``prewarm`` is an alias for parity with the SOTA / TS naming.
    prewarm = prepare_connection

    @staticmethod
    def capabilities(provider: str) -> dict[str, Any]:
        """Canonical language capabilities for ``provider`` — STATIC, offline lookup.

        Returns ``{provider, notation, auto_detect, canonical_languages}`` — how
        the provider spells languages natively + whether it auto-detects, plus
        the FULL canonical value space (:data:`CANONICAL_LANGUAGES`) the gateway
        accepts for every provider (it maps the canonical token to the native
        notation server-side). This replaces the SDK's old stale per-provider
        language whitelists; pass any canonical token and the gateway handles it.

        This is a local table lookup — synchronous, connection-free, and useful
        offline. For the authoritative, always-current server-side matrix use
        :meth:`fetch_language_capabilities`, which calls the gateway's live
        ``GET /capabilities/languages`` endpoint.
        """
        return language_capabilities(provider)

    async def fetch_language_capabilities(
        self,
        provider: Optional[str] = None,
    ) -> dict[str, Any]:
        """Fetch the LIVE unified-language support matrix from the gateway.

        Calls ``GET /capabilities/languages`` and returns
        ``{canonical_languages, providers, canonical_count}`` — the
        authoritative counterpart to the static :meth:`capabilities` table.
        The endpoint takes no query parameters; when ``provider`` is given the
        ``providers`` rows are filtered client-side.

        Args:
            provider: Optional provider id to filter the ``providers`` rows to.

        Returns:
            The live capability matrix dict.
        """
        return await self._rest_client.fetch_language_capabilities(provider=provider)

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

    async def get_metrics(self) -> str:
        """
        Get server performance metrics.

        The gateway serves ``GET /metrics`` as a Prometheus **text
        exposition** (``text/plain``); the previous ``dict`` annotation was
        stale.

        Returns:
            The Prometheus text exposition as a string.
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
