"""
Async REST client for Bud WaaV Gateway
"""

from typing import Any, Optional
import httpx

from ..errors import APIError, ConnectionError, TimeoutError


class RestClient:
    """Async REST client for Bud WaaV Gateway."""

    def __init__(
        self,
        base_url: str,
        api_key: Optional[str] = None,
        timeout: float = 30.0,
    ):
        """
        Initialize REST client.

        Args:
            base_url: Base URL of the Bud WaaV gateway
            api_key: Optional API key for authentication
            timeout: Request timeout in seconds
        """
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout
        self._client: Optional[httpx.AsyncClient] = None

    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create the HTTP client."""
        if self._client is None or self._client.is_closed:
            headers = {}
            if self.api_key:
                headers["Authorization"] = f"Bearer {self.api_key}"
            headers["Content-Type"] = "application/json"

            self._client = httpx.AsyncClient(
                base_url=self.base_url,
                headers=headers,
                timeout=httpx.Timeout(self.timeout),
            )
        return self._client

    async def close(self) -> None:
        """Close the HTTP client."""
        if self._client and not self._client.is_closed:
            await self._client.aclose()
            self._client = None

    async def _request(
        self,
        method: str,
        endpoint: str,
        json: Optional[dict[str, Any]] = None,
        params: Optional[dict[str, Any]] = None,
    ) -> Any:
        """
        Make an HTTP request.

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
            APIError: If API returns an error response
        """
        client = await self._get_client()

        try:
            response = await client.request(
                method=method,
                url=endpoint,
                json=json,
                params=params,
            )
        except httpx.ConnectError as e:
            raise ConnectionError(
                message=f"Failed to connect to {self.base_url}{endpoint}",
                url=f"{self.base_url}{endpoint}",
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
                url=f"{self.base_url}{endpoint}",
                cause=e,
            ) from e

        if response.status_code >= 400:
            try:
                error_body = response.json()
            except Exception:
                error_body = response.text

            raise APIError.from_response(
                status_code=response.status_code,
                response_body=error_body,
                url=f"{self.base_url}{endpoint}",
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

        Args:
            room_name: Room name to join
            identity: Participant identity
            name: Participant display name
            ttl: Token TTL in seconds
            metadata: Participant metadata

        Returns:
            Token response with JWT and room info
        """
        payload: dict[str, Any] = {
            "room_name": room_name,
            "identity": identity,
        }
        if name:
            payload["name"] = name
        if ttl:
            payload["ttl"] = ttl
        if metadata:
            payload["metadata"] = metadata

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
        Create a SIP hook.

        Args:
            host: SIP host
            webhook_url: Webhook URL for incoming calls

        Returns:
            Created hook info
        """
        payload: dict[str, str] = {
            "host": host,
            "webhook_url": webhook_url,
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
            Gateway does not currently expose a DELETE /voices/{voice_id}
            endpoint. This method will return a 404 until gateway support
            is added. Use the provider's API directly in the meantime.

        Args:
            voice_id: The voice ID to delete.
            provider: The voice cloning provider.
        """
        await self.delete(f"/voices/{voice_id}", params={"provider": provider})

    async def get_cloned_voice(
        self,
        voice_id: str,
        provider: str = "elevenlabs",
    ) -> dict[str, Any]:
        """
        Get information about a cloned voice.

        .. warning::
            Gateway does not currently expose a GET /voices/{voice_id}
            endpoint. This method will return a 404 until gateway support
            is added. Use the provider's API directly in the meantime.

        Args:
            voice_id: The voice ID.
            provider: The voice cloning provider.

        Returns:
            Voice information.
        """
        result: dict[str, Any] = await self.get(
            f"/voices/{voice_id}", params={"provider": provider}
        )
        return result

    # =========================================================================
    # Recording Methods
    # =========================================================================

    async def get_recording(
        self,
        stream_id: str,
    ) -> dict[str, Any]:
        """
        Get recording metadata by stream ID.

        .. warning::
            Gateway does not currently expose a metadata-only recording
            endpoint. This method will return a 404 until gateway support
            is added. Use ``download_recording()`` to retrieve the audio.

        Args:
            stream_id: The stream/session ID.

        Returns:
            Recording information including status, duration, format.
        """
        result: dict[str, Any] = await self.get(f"/recordings/{stream_id}")
        return result

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
            Gateway does not currently expose a GET /recordings list
            endpoint. This method will return a 404 until gateway support
            is added.

        Args:
            limit: Maximum number of recordings to return.
            offset: Pagination offset.
            status: Filter by status (pending, processing, ready, failed).
            from_date: Filter by start date (ISO 8601).
            to_date: Filter by end date (ISO 8601).

        Returns:
            Dictionary with recordings list and pagination info.
        """
        params: dict[str, Any] = {
            "limit": limit,
            "offset": offset,
        }
        if status:
            params["status"] = status
        if from_date:
            params["from_date"] = from_date
        if to_date:
            params["to_date"] = to_date

        result: dict[str, Any] = await self.get("/recordings", params=params)
        return result

    async def delete_recording(
        self,
        stream_id: str,
    ) -> None:
        """
        Delete a recording.

        .. warning::
            Gateway does not currently expose a DELETE /recordings/{stream_id}
            endpoint. This method will return a 404 until gateway support
            is added.

        Args:
            stream_id: The stream/session ID.
        """
        await self.delete(f"/recordings/{stream_id}")

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
        stream_id: str,
        transfer_to: str,
    ) -> dict[str, Any]:
        """
        Transfer an active SIP call to another number.

        Args:
            stream_id: Active stream/session ID.
            transfer_to: Phone number or SIP URI to transfer to.

        Returns:
            Transfer result.
        """
        payload: dict[str, Any] = {
            "stream_id": stream_id,
            "transfer_to": transfer_to,
        }
        result: dict[str, Any] = await self.post("/sip/transfer", json=payload)
        return result

    # =========================================================================
    # Metrics Method
    # =========================================================================

    async def get_metrics(self) -> dict[str, Any]:
        """
        Get server performance metrics.

        .. warning::
            Gateway does not currently expose a GET /metrics endpoint.
            This method will return a 404 until gateway support is added.
            Use the SDK's local ``SessionMetrics`` for client-side metrics.

        Returns:
            Server metrics including STT/TTS latency, connection counts, etc.
        """
        result: dict[str, Any] = await self.get("/metrics")
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
