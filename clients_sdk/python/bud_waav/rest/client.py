"""
Async REST client for Bud WaaV Gateway
"""

import asyncio
import random
from typing import Any, Optional
import httpx

from ..errors import APIError, ConnectionError, RateLimitError, TimeoutError

# REST connection-pool tuning (D7). httpx defaults are UNtuned (no explicit
# limits / no keepalive expiry), so a memoized client still drops idle keepalive
# connections unpredictably and a batch of sequential calls can re-do TLS. These
# defaults keep a healthy warm pool so the SDK's typical sequential workload
# (submit_batch -> poll, or list_voices/clone bursts) REUSES one TCP/TLS
# connection instead of paying a handshake per call.
#
#   * max_keepalive_connections: idle sockets kept warm for reuse (per host).
#   * max_connections: hard ceiling on concurrent sockets (backpressure, not a
#     self-DDoS) — cooperates with the gateway's per-IP connection cap.
#   * keepalive_expiry: how long an idle socket is kept before close; 90s
#     comfortably spans a poll loop's inter-request gap.
DEFAULT_MAX_KEEPALIVE_CONNECTIONS = 20
DEFAULT_MAX_CONNECTIONS = 100
DEFAULT_KEEPALIVE_EXPIRY = 90.0

# Transient HTTP statuses the gateway returns under load. Both are RETRIABLE,
# NOT fatal, and are handled identically to the WS connect path:
#   * 429 — per-IP token bucket / tower_governor rate limit (carries Retry-After).
#   * 503 — "Server at capacity" (global connection-limit saturation).
_RETRYABLE_STATUSES = (429, 503)


class RestClient:
    """Async REST client for Bud WaaV Gateway."""

    def __init__(
        self,
        base_url: str,
        api_key: Optional[str] = None,
        timeout: float = 30.0,
        *,
        max_keepalive_connections: int = DEFAULT_MAX_KEEPALIVE_CONNECTIONS,
        max_connections: int = DEFAULT_MAX_CONNECTIONS,
        keepalive_expiry: float = DEFAULT_KEEPALIVE_EXPIRY,
        max_retries: int = 3,
    ):
        """
        Initialize REST client.

        Args:
            base_url: Base URL of the Bud WaaV gateway
            api_key: Optional API key for authentication
            timeout: Request timeout in seconds
            max_keepalive_connections: Idle keepalive sockets kept warm for reuse
                (D7 pooling). See module ``DEFAULT_*`` for the rationale.
            max_connections: Hard ceiling on concurrent sockets.
            keepalive_expiry: Seconds an idle socket is kept before close.
            max_retries: Backoff retries on a transient 429/503 before giving up.
        """
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout
        self._limits = httpx.Limits(
            max_keepalive_connections=max_keepalive_connections,
            max_connections=max_connections,
            keepalive_expiry=keepalive_expiry,
        )
        self._max_retries = max(0, max_retries)
        self._client: Optional[httpx.AsyncClient] = None

    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create the pooled HTTP client (D7).

        The client is MEMOIZED and reused across every call so the tuned
        keepalive pool actually persists — sequential batch/poll/voices/clone
        calls share one warm TCP/TLS connection instead of re-handshaking. A new
        client is built only on first use or after :meth:`close`.
        """
        if self._client is None or self._client.is_closed:
            headers = {}
            if self.api_key:
                headers["Authorization"] = f"Bearer {self.api_key}"
            headers["Content-Type"] = "application/json"

            self._client = httpx.AsyncClient(
                base_url=self.base_url,
                headers=headers,
                timeout=httpx.Timeout(self.timeout),
                limits=self._limits,
            )
        return self._client

    async def close(self) -> None:
        """Close the HTTP client."""
        if self._client and not self._client.is_closed:
            await self._client.aclose()
            self._client = None

    async def warmup(self, timeout: float = 5.0) -> bool:
        """Pre-resolve DNS + establish a TLS keepalive socket to the gateway (D6).

        Sends one cheap request to the gateway origin so the TCP+TLS handshake is
        paid NOW (e.g. at page-load / SDK init) and the resulting socket lands in
        the keepalive pool — the first real call then attaches to an
        already-warm connection instead of paying cold DNS+TLS (typically
        100-400ms). Mirrors LiveKit ``Room.prepareConnection``.

        A HEAD to ``/`` is preferred (no body); if the gateway rejects HEAD we
        fall back to a GET so the socket still gets warmed. Best-effort: any
        failure (gateway down, network) is swallowed and returns ``False`` — a
        prewarm must never raise into the caller's init path.

        Returns:
            ``True`` if a request round-tripped (socket warmed), else ``False``.
        """
        try:
            client = await self._get_client()
            try:
                await client.head("/", timeout=timeout)
            except httpx.HTTPStatusError:
                # Reached the server (warmed) but it returned an error status.
                return True
            except httpx.HTTPError:
                # HEAD may be unsupported/blocked — try GET to still warm the pool.
                await client.get("/", timeout=timeout)
            return True
        except Exception:
            return False

    @staticmethod
    def _parse_retry_after(headers: httpx.Headers) -> Optional[float]:
        """Parse a numeric ``Retry-After`` (seconds) header, if present."""
        ra = headers.get("retry-after")
        if ra is None:
            return None
        try:
            return float(ra)
        except (TypeError, ValueError):
            return None

    async def _request(
        self,
        method: str,
        endpoint: str,
        json: Optional[dict[str, Any]] = None,
        params: Optional[dict[str, Any]] = None,
    ) -> Any:
        """
        Make an HTTP request, backing off on transient 429/503 (D7).

        A 429 (rate limit) or 503 ("Server at capacity") is TRANSIENT, not fatal:
        the call is retried with jittered backoff that honors ``Retry-After``
        when the gateway supplies it (identical to the WS connect path). After
        ``max_retries`` it is surfaced as a typed :class:`RateLimitError` (NOT an
        opaque APIError) so the caller can back off deterministically. All other
        ``>= 400`` responses raise :class:`APIError` immediately.

        Args:
            method: HTTP method (GET, POST, DELETE, etc.)
            endpoint: API endpoint
            json: JSON body for POST/PUT requests
            params: Query parameters

        Returns:
            Response data

        Raises:
            ConnectionError: If connection fails
            TimeoutError: If request times out
            RateLimitError: If still throttled (429/503) after all retries
            APIError: If API returns any other error response
        """
        client = await self._get_client()
        url = f"{self.base_url}{endpoint}"

        attempt = 0
        while True:
            try:
                response = await client.request(
                    method=method,
                    url=endpoint,
                    json=json,
                    params=params,
                )
            except httpx.ConnectError as e:
                raise ConnectionError(
                    message=f"Failed to connect to {url}",
                    url=url,
                    cause=e,
                ) from e
            except httpx.TimeoutException as e:
                raise TimeoutError(
                    message=f"Request timed out after {self.timeout}s",
                    timeout_ms=int(self.timeout * 1000),
                    operation=f"{method} {endpoint}",
                ) from e
            except httpx.HTTPError as e:
                raise ConnectionError(
                    message=f"HTTP error: {e}",
                    url=url,
                    cause=e,
                ) from e

            # Transient saturation (429/503): back off and retry, do NOT treat as
            # fatal. Honor Retry-After; else exponential base + jitter.
            if response.status_code in _RETRYABLE_STATUSES:
                retry_after = self._parse_retry_after(response.headers)
                if attempt >= self._max_retries:
                    label = (
                        "Rate limited (429)"
                        if response.status_code == 429
                        else "Server at capacity (503)"
                    )
                    raise RateLimitError(
                        message=f"{label} after {attempt + 1} attempt(s): {method} {endpoint}",
                        retry_after=retry_after,
                        url=url,
                    )
                base = retry_after if retry_after is not None else min(2 ** attempt, 15)
                jitter = base * 0.2 * (random.random() * 2 - 1)
                await asyncio.sleep(max(0.0, base + jitter))
                attempt += 1
                continue

            if response.status_code >= 400:
                try:
                    error_body = response.json()
                except Exception:
                    error_body = response.text

                raise APIError.from_response(
                    status_code=response.status_code,
                    response_body=error_body,
                    url=url,
                    method=method,
                )

            if response.status_code == 204:
                return None

            content_type = response.headers.get("content-type", "")
            if "application/json" in content_type:
                return response.json()
            elif "audio/" in content_type or "application/octet-stream" in content_type:
                return response.content
            else:
                return response.text

    async def get(
        self,
        endpoint: str,
        params: Optional[dict[str, Any]] = None,
    ) -> Any:
        """Make a GET request."""
        return await self._request("GET", endpoint, params=params)

    async def post(
        self,
        endpoint: str,
        json: Optional[dict[str, Any]] = None,
        params: Optional[dict[str, Any]] = None,
    ) -> Any:
        """Make a POST request."""
        return await self._request("POST", endpoint, json=json, params=params)

    async def put(
        self,
        endpoint: str,
        json: Optional[dict[str, Any]] = None,
        params: Optional[dict[str, Any]] = None,
    ) -> Any:
        """Make a PUT request."""
        return await self._request("PUT", endpoint, json=json, params=params)

    async def delete(
        self,
        endpoint: str,
        params: Optional[dict[str, Any]] = None,
        json: Optional[dict[str, Any]] = None,
    ) -> Any:
        """Make a DELETE request."""
        return await self._request("DELETE", endpoint, params=params, json=json)

    async def health(self) -> dict[str, Any]:
        """
        Check gateway health.

        Returns:
            Health status with version info
        """
        result: dict[str, Any] = await self.get("/")
        return result

    async def list_voices(
        self,
        provider: Optional[str] = None,
    ) -> dict[str, list[dict[str, Any]]]:
        """
        List available TTS voices.

        The gateway returns voices grouped by provider name, e.g.
        ``{"deepgram": [...], "elevenlabs": [...]}``.

        Args:
            provider: Optional provider to filter voices.

        Returns:
            Dictionary mapping provider names to lists of voice objects.
        """
        params: dict[str, str] = {}
        if provider:
            params["provider"] = provider
        result: dict[str, list[dict[str, Any]]] = await self.get("/voices", params=params)
        return result

    async def speak(
        self,
        text: str,
        provider: str = "deepgram",
        voice: Optional[str] = None,
        voice_id: Optional[str] = None,
        model: Optional[str] = None,
        sample_rate: int = 24000,
    ) -> bytes:
        """
        Synthesize text to speech (one-shot).

        The gateway expects ``{"text": "...", "tts_config": {...}}``.

        Args:
            text: Text to synthesize
            provider: TTS provider
            voice: Voice name
            voice_id: Voice ID (provider-specific)
            model: Model to use
            sample_rate: Output sample rate

        Returns:
            Audio data as bytes
        """
        tts_config: dict[str, Any] = {
            "provider": provider,
            "model": model or "aura-asteria-en",
            "sample_rate": sample_rate,
        }
        if voice_id:
            tts_config["voice_id"] = voice_id
        elif voice:
            tts_config["voice_id"] = voice

        payload: dict[str, Any] = {
            "text": text,
            "tts_config": tts_config,
        }

        result: bytes = await self.post("/speak", json=payload)
        return result

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

        The gateway ``TokenRequest`` (handlers/livekit/token.rs) requires exactly
        ``{room_name, participant_name, participant_identity}`` — the previous
        ``identity``/``name`` field names were silently ignored and the call 422'd.
        ``ttl``/``metadata`` are not part of the gateway contract and are not sent.

        Args:
            room_name: Room name to join
            identity: Participant identity (sent as ``participant_identity``)
            name: Participant display name (sent as ``participant_name``;
                defaults to the identity when omitted — the field is required
                by the gateway)
            ttl: Deprecated/ignored — the gateway sets the token TTL server-side
            metadata: Deprecated/ignored — not part of the gateway contract

        Returns:
            Token response with JWT and room info
        """
        payload: dict[str, Any] = {
            "room_name": room_name,
            "participant_identity": identity,
            "participant_name": name or identity,
        }

        result: dict[str, Any] = await self.post("/livekit/token", json=payload)
        return result

    async def get_livekit_room(self, room_name: str) -> dict[str, Any]:
        """
        Get LiveKit room information.

        Args:
            room_name: Room name

        Returns:
            Room information
        """
        result: dict[str, Any] = await self.get(f"/livekit/rooms/{room_name}")
        return result

    async def list_livekit_rooms(self) -> list[dict[str, Any]]:
        """
        List all LiveKit rooms.

        Returns:
            List of rooms
        """
        result: list[dict[str, Any]] = await self.get("/livekit/rooms")
        return result

    async def list_sip_hooks(self) -> list[dict[str, Any]]:
        """
        List all SIP hooks.

        Returns:
            List of SIP hooks
        """
        result: list[dict[str, Any]] = await self.get("/sip/hooks")
        return result

    async def create_sip_hook(
        self,
        host: str,
        webhook_url: str,
    ) -> dict[str, Any]:
        """
        Create (or replace) a SIP hook.

        The gateway ``SipHooksRequest`` (handlers/sip/hooks.rs) is an ENVELOPE:
        ``{"hooks": [{"host": ..., "url": ...}]}``. Because ``hooks`` is
        ``#[serde(default)]``, the previous flat ``{host, webhook_url}`` body
        deserialized to an EMPTY hook list — a silent no-op (or destructive
        replace-with-nothing). Hooks with a matching host are replaced.

        Args:
            host: SIP host pattern (case-insensitive)
            webhook_url: HTTPS URL to forward webhook events to (wire field: ``url``)

        Returns:
            The updated hook list.
        """
        payload: dict[str, Any] = {
            "hooks": [{"host": host, "url": webhook_url}],
        }
        result: dict[str, Any] = await self.post("/sip/hooks", json=payload)
        return result

    async def delete_sip_hooks(self, hosts: list[str]) -> dict[str, Any]:
        """
        Delete SIP hooks by host names.

        The gateway endpoint is DELETE /sip/hooks with a JSON body containing
        host names to remove. Hosts defined in the server configuration cannot
        be deleted (returns 405).

        Args:
            hosts: List of SIP host names to delete (case-insensitive).

        Returns:
            Updated list of remaining SIP hooks.

        Raises:
            APIError: If hosts list is empty (400) or hosts are protected (405).
        """
        result: dict[str, Any] = await self.delete(
            "/sip/hooks", json={"hosts": hosts}
        )
        return result

    # =========================================================================
    # Batched / Async STT (P5)
    # =========================================================================

    async def submit_batch(
        self,
        *,
        url: Optional[str] = None,
        audio_base64: Optional[str] = None,
        content_type: Optional[str] = None,
        stt_config: Optional[dict[str, Any]] = None,
        batch: Optional[dict[str, Any]] = None,
        translation: Optional[dict[str, Any]] = None,
        callback_url: Optional[str] = None,
        callback_method: Optional[str] = None,
    ) -> dict[str, Any]:
        """Submit a batched/async transcription job (``POST /transcribe/batch``).

        The canonical batch envelope WRAPS the streaming ``StandardSTTConfig``
        VERBATIM (so the full typed ``SttFeatures`` — diarization/redaction/etc. —
        are reused) plus the batch-only knobs that the streaming path intentionally
        drops but ARE batch-capable (``alternatives`` / ``detect_language`` /
        ``summarize`` / ``topics`` / ``intents`` / ``paragraphs`` / ``utterances``).

        Supply audio as EITHER ``url`` OR ``audio_base64`` (+ optional
        ``content_type``). When ``callback_url`` is set the gateway returns
        ``{job_id, status:"queued"}`` immediately and POSTs the result to the
        webhook; otherwise poll :meth:`get_batch` / use :meth:`transcribe_batch`.

        Args:
            url: Audio URL (Deepgram ``{"url"}`` / AssemblyAI ``audio_url``).
            audio_base64: Inline base64 audio bytes (mutually exclusive with ``url``).
            content_type: MIME type for inline bytes (e.g. ``audio/wav``).
            stt_config: ``StandardSTTConfig`` fields (provider/language/model +
                ``features{}`` + ``extras{}``), flattened onto the envelope.
            batch: ``BatchFeatures`` dict (``paragraphs``/``utterances``/
                ``summarize``/``topics``/``intents``/``detect_language``/
                ``alternatives``).
            translation: Canonical :class:`~bud_waav.types.TranslationConfig` dict
                (batch enables AssemblyAI/Speechmatics/Gladia targets).
            callback_url: Webhook for async delivery.
            callback_method: ``POST`` (default) or ``PUT``.

        Returns:
            ``{job_id, status, ...}`` (and ``result`` when run synchronously).

        Raises:
            ValueError: If neither/both of ``url`` and ``audio_base64`` are given.
        """
        if (url is None) == (audio_base64 is None):
            raise ValueError("Provide exactly one of `url` or `audio_base64`")

        payload: dict[str, Any] = {}
        if url is not None:
            payload["url"] = url
        else:
            payload["audio_base64"] = audio_base64
            if content_type is not None:
                payload["content_type"] = content_type

        # StandardSTTConfig is flattened onto the envelope (gateway #[serde(flatten)]).
        if stt_config:
            payload.update(stt_config)
        if batch:
            payload["batch"] = batch
        if translation:
            payload["translation"] = translation
        if callback_url is not None:
            payload["callback_url"] = callback_url
        if callback_method is not None:
            payload["callback_method"] = callback_method

        result: dict[str, Any] = await self.post("/transcribe/batch", json=payload)
        return result

    async def get_batch(self, job_id: str) -> dict[str, Any]:
        """Poll a batch job (``GET /transcribe/batch/{job_id}``).

        Returns the canonical ``BatchJob``: ``{job_id, status, result?, error?}``
        with ``status`` ∈ ``queued|processing|completed|error``.
        """
        result: dict[str, Any] = await self.get(f"/transcribe/batch/{job_id}")
        return result

    # =========================================================================
    # Voice Cloning Methods
    # =========================================================================

    async def clone_voice(
        self,
        name: str,
        provider: str = "elevenlabs",
        audio_samples: Optional[list[str]] = None,
        audio_files: Optional[list[bytes]] = None,
        description: Optional[str] = None,
        labels: Optional[dict[str, str]] = None,
        mode: str = "instant",
        base_voice_id: Optional[str] = None,
        remove_background_noise: bool = False,
        sample_text: Optional[str] = None,
    ) -> dict[str, Any]:
        """
        Clone a voice via the gateway's canonical ``POST /voices/clone`` body.

        Mirrors the gateway ``VoiceCloneRequest`` (P4). Supply samples either as
        already-encoded ``audio_samples`` (base64 strings or URLs) OR as raw
        ``audio_files`` (bytes, base64-encoded here). Returns the gateway
        ``VoiceCloneResponse`` dict: ``{voice_id, name, provider, status, ...}`` —
        a ``ready`` voice_id is directly usable as a TTS ``voice_id``.

        Args:
            name: Name for the cloned voice (required).
            provider: Voice-clone provider (hume, elevenlabs, lmnt, cartesia,
                playht, speechify, resemble).
            audio_samples: Samples as base64 strings or URLs (canonical wire field).
            audio_files: Raw audio bytes (convenience; base64-encoded into
                ``audio_samples``). Merged with ``audio_samples`` if both given.
            description: Optional description (Hume design / ElevenLabs label).
            labels: Optional flat labels (ElevenLabs).
            mode: ``instant`` (default) or ``professional`` (async).
            base_voice_id: Base voice to design/derive from (provider-specific).
            remove_background_noise: Strip background noise (ElevenLabs IVC / LMNT).
            sample_text: Sample text for voice generation (Hume only).

        Returns:
            Created voice information with ``voice_id`` and ``status``.

        Raises:
            APIError: If cloning fails (e.g. 400 invalid request, 401 missing key).
        """
        import base64

        samples: list[str] = list(audio_samples or [])
        if audio_files:
            samples.extend(
                base64.b64encode(audio).decode("utf-8") for audio in audio_files
            )

        payload: dict[str, Any] = {
            "provider": provider,
            "name": name,
            "audio_samples": samples,
            "mode": mode,
            "remove_background_noise": remove_background_noise,
        }
        if description:
            payload["description"] = description
        if labels:
            payload["labels"] = labels
        if base_voice_id:
            payload["base_voice_id"] = base_voice_id
        if sample_text:
            payload["sample_text"] = sample_text

        result: dict[str, Any] = await self.post("/voices/clone", json=payload)
        return result

    async def list_cloned_voices(
        self,
        provider: Optional[str] = None,
    ) -> list[dict[str, Any]]:
        """
        List cloned voices.

        Args:
            provider: Optional provider to filter by.

        Returns:
            List of cloned voice information.
        """
        params: dict[str, str] = {}
        if provider:
            params["provider"] = provider
            params["cloned"] = "true"

        result: list[dict[str, Any]] = await self.get("/voices", params=params)
        # Filter to only cloned voices
        return [v for v in result if v.get("is_cloned", False)]

    async def delete_cloned_voice(
        self,
        voice_id: str,
        provider: str = "elevenlabs",
    ) -> None:
        """
        Delete a cloned voice.

        .. warning::
            The gateway serves no ``DELETE /voices/{voice_id}`` route — this
            raises a typed 501 ``APIError`` immediately (fail-fast, instead of
            a confusing network 404). Use the provider's API directly.

        Args:
            voice_id: The voice ID to delete.
            provider: The voice cloning provider.
        """
        raise APIError(
            f"delete_cloned_voice({voice_id!r}) is not supported: the gateway serves no "
            "DELETE /voices/{id} route. Use the provider's API directly.",
            status_code=501,
        )

    async def get_cloned_voice(
        self,
        voice_id: str,
        provider: str = "elevenlabs",
    ) -> dict[str, Any]:
        """
        Get information about a cloned voice.

        .. warning::
            The gateway serves no ``GET /voices/{voice_id}`` route — this raises
            a typed 501 ``APIError`` immediately (fail-fast, instead of a
            confusing network 404). Use ``list_cloned_voices()`` and filter, or
            the provider's API directly.

        Args:
            voice_id: The voice ID.
            provider: The voice cloning provider.

        Returns:
            Voice information.
        """
        raise APIError(
            f"get_cloned_voice({voice_id!r}) is not supported: the gateway serves no "
            "GET /voices/{id} route. Use list_cloned_voices() and filter client-side.",
            status_code=501,
        )

    # =========================================================================
    # Recording Methods
    # =========================================================================

    async def get_recording(
        self,
        stream_id: str,
    ) -> bytes:
        """
        Get recording metadata by stream ID.

        .. note::
            The gateway serves recordings at ``GET /recording/{stream_id}``
            (singular) as audio bytes; there is no metadata-only endpoint.
            This method is a deprecated alias of ``download_recording()`` —
            the previous ``/recordings/{id}`` (plural) path always 404'd.

        Args:
            stream_id: The stream/session ID.

        Returns:
            Audio data as bytes (OGG format) — same as ``download_recording()``.
        """
        return await self.download_recording(stream_id)

    async def download_recording(
        self,
        stream_id: str,
    ) -> bytes:
        """
        Download a recording by stream ID.

        The gateway serves recordings as audio/ogg from
        GET /recording/{stream_id}. The response includes
        Content-Disposition with a suggested filename.

        Args:
            stream_id: The stream/session ID.

        Returns:
            Audio data as bytes (OGG format).
        """
        result: bytes = await self.get(f"/recording/{stream_id}")
        return result

    async def list_recordings(
        self,
        limit: int = 50,
        offset: int = 0,
        status: Optional[str] = None,
        from_date: Optional[str] = None,
        to_date: Optional[str] = None,
    ) -> dict[str, Any]:
        """
        List recordings with optional filters.

        .. warning::
            The gateway serves no ``GET /recordings`` list route — this raises
            a typed 501 ``APIError`` immediately (fail-fast, instead of a
            confusing network 404). Recordings are addressed by stream id via
            ``download_recording()``.

        Args:
            limit: Maximum number of recordings to return.
            offset: Pagination offset.
            status: Filter by status (pending, processing, ready, failed).
            from_date: Filter by start date (ISO 8601).
            to_date: Filter by end date (ISO 8601).

        Returns:
            Dictionary with recordings list and pagination info.
        """
        raise APIError(
            "list_recordings() is not supported: the gateway serves no GET /recordings "
            "route. Recordings are addressed by stream id via download_recording().",
            status_code=501,
        )

    async def delete_recording(
        self,
        stream_id: str,
    ) -> None:
        """
        Delete a recording.

        .. warning::
            The gateway serves no ``DELETE /recordings/{stream_id}`` route —
            this raises a typed 501 ``APIError`` immediately (fail-fast,
            instead of a confusing network 404).

        Args:
            stream_id: The stream/session ID.
        """
        raise APIError(
            f"delete_recording({stream_id!r}) is not supported: the gateway serves no "
            "DELETE /recordings/{id} route.",
            status_code=501,
        )

    # =========================================================================
    # DAG Template Methods
    # =========================================================================

    async def list_dag_templates(self) -> dict[str, Any]:
        """
        List available DAG templates.

        Returns:
            Dict with 'templates' list and 'count' integer matching gateway
            ListTemplatesResponse format::

                {"templates": [{"name": "...", "version": "...", "description": "..."}], "count": N}
        """
        result: dict[str, Any] = await self.get("/dag/templates")
        return result

    async def validate_dag(
        self,
        definition: dict[str, Any],
    ) -> dict[str, Any]:
        """
        Validate a DAG definition server-side.

        Args:
            definition: DAG definition to validate.

        Returns:
            Gateway ValidateDAGResponse format::

                {"valid": bool, "errors": [...], "warnings": [...],
                 "node_count": int, "edge_count": int}
        """
        result: dict[str, Any] = await self.post("/dag/validate", json={"dag": definition})
        return result

    async def get_dag_template(self, name: str) -> dict[str, Any]:
        """
        Get a specific DAG template by name.

        Args:
            name: Template name.

        Returns:
            Dict with 'name' and 'template' (DAGDefinition) matching gateway response.

        Raises:
            APIError: If template not found (404).
        """
        result: dict[str, Any] = await self.get(f"/dag/templates/{name}")
        return result

    # =========================================================================
    # LiveKit Participant Methods
    # =========================================================================

    async def remove_livekit_participant(
        self,
        room_name: str,
        identity: str,
    ) -> dict[str, Any]:
        """
        Remove a participant from a LiveKit room.

        The gateway endpoint is DELETE /livekit/participant with a JSON body
        containing room_name and participant_identity. The room name is
        normalized with the auth.id prefix for tenant isolation.

        Args:
            room_name: Room name.
            identity: Participant identity to remove.

        Returns:
            Removal response with status, room_name, and participant_identity.

        Raises:
            APIError: If participant not found (404) or LiveKit not configured (500).
        """
        payload: dict[str, Any] = {
            "room_name": room_name,
            "participant_identity": identity,
        }
        result: dict[str, Any] = await self.delete(
            "/livekit/participant", json=payload
        )
        return result

    async def mute_livekit_participant(
        self,
        room_name: str,
        identity: str,
        track_sid: str,
        muted: bool = True,
    ) -> dict[str, Any]:
        """
        Mute or unmute a participant's track in a LiveKit room.

        The gateway endpoint is POST /livekit/participant/mute with a JSON body.
        The room name is normalized with the auth.id prefix for tenant isolation.

        Args:
            room_name: Room name.
            identity: Participant identity.
            track_sid: Session ID of the track to mute/unmute.
            muted: Whether to mute (True) or unmute (False).

        Returns:
            Updated mute state with room_name, participant_identity,
            track_sid, and muted fields.

        Raises:
            APIError: If participant/track not found (404) or LiveKit not configured (500).
        """
        payload: dict[str, Any] = {
            "room_name": room_name,
            "participant_identity": identity,
            "track_sid": track_sid,
            "muted": muted,
        }
        result: dict[str, Any] = await self.post(
            "/livekit/participant/mute", json=payload
        )
        return result

    # =========================================================================
    # SIP Transfer Method
    # =========================================================================

    async def sip_transfer(
        self,
        room_name: str,
        participant_identity: str,
        transfer_to: str,
    ) -> dict[str, Any]:
        """
        Transfer an active SIP call (SIP REFER) to another number.

        The gateway endpoint is ``POST /sip/transfer`` with a JSON body of
        ``{room_name, participant_identity, transfer_to}`` — the gateway
        ``SIPTransferRequest`` (handlers/sip/transfer.rs) requires ALL THREE
        fields (the previous ``stream_id`` payload was never a gateway field
        and 422'd on every call). The room name is normalized with the auth
        prefix server-side for tenant isolation.

        Args:
            room_name: LiveKit room name where the SIP participant is connected.
            participant_identity: Identity of the SIP participant to transfer
                (obtainable by listing the room's participants).
            transfer_to: Destination — international (``+1234567890``),
                national (``07123456789``), or internal extension (``1234``).

        Returns:
            Transfer result: ``{status, room_name, participant_identity,
            transfer_to}`` where ``status`` is ``"completed"`` or
            ``"initiated"`` (confirmation timed out but likely succeeded).
        """
        payload: dict[str, Any] = {
            "room_name": room_name,
            "participant_identity": participant_identity,
            "transfer_to": transfer_to,
        }
        result: dict[str, Any] = await self.post("/sip/transfer", json=payload)
        return result

    # =========================================================================
    # Metrics Method
    # =========================================================================

    async def get_metrics(self) -> str:
        """
        Get server performance metrics.

        The gateway serves ``GET /metrics`` as a public **Prometheus text
        exposition** (``text/plain``) — turn counters, frame latencies,
        provider health, etc. The previous docstring claimed the endpoint
        did not exist and the annotation claimed ``dict``; both were stale.

        Returns:
            The Prometheus text exposition as a string (parse with a
            Prometheus client library, or scrape directly).
        """
        result: str = await self.get("/metrics")
        return result

    # =========================================================================
    # Capability Discovery
    # =========================================================================

    async def fetch_language_capabilities(
        self,
        provider: Optional[str] = None,
    ) -> dict[str, Any]:
        """
        Fetch the LIVE unified-language support matrix from the gateway.

        Calls ``GET /capabilities/languages`` (gateway
        ``LanguageCapabilitiesResponse``, handlers/capabilities.rs) — the
        authoritative, always-current counterpart to the SDK's static
        :func:`bud_waav.types.language_capabilities` table::

            {
                "canonical_languages": [
                    {"bcp47": "en-US", "lang_subtag": "en",
                     "iso639_1": "en", "region": "US"},
                    ...
                ],
                "providers": [
                    {"provider": "deepgram", "notation": "bcp47",
                     "supports_auto": true,
                     "example_cmn_cn": "zh-CN", "example_en_us": "en-US"},
                    ...
                ],
                "canonical_count": N,
            }

        The endpoint takes NO query parameters — when ``provider`` is given
        the ``providers`` rows are filtered CLIENT-side (an unknown provider
        yields an empty ``providers`` list; the canonical language list is
        always returned in full).

        Args:
            provider: Optional provider id to filter the ``providers`` rows to.

        Returns:
            The capability matrix dict shown above.
        """
        result: dict[str, Any] = await self.get("/capabilities/languages")
        if provider is not None and isinstance(result.get("providers"), list):
            result = {
                **result,
                "providers": [
                    row
                    for row in result["providers"]
                    if isinstance(row, dict) and row.get("provider") == provider
                ],
            }
        return result

    # =========================================================================
    # Speak with TTS Config Method
    # =========================================================================

    async def speak_with_config(
        self,
        text: str,
        tts_config: dict[str, Any],
    ) -> bytes:
        """
        Synthesize text to speech with full TTS configuration.

        Args:
            text: Text to synthesize.
            tts_config: Full TTS configuration dict including provider, voice,
                       emotion, etc.

        Returns:
            Audio data as bytes.
        """
        payload: dict[str, Any] = {"text": text, "tts_config": tts_config}
        result: bytes = await self.post("/speak", json=payload)
        return result

    async def __aenter__(self) -> "RestClient":
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.close()
