"""
Async REST client for Bud Foundry Gateway
"""

from typing import Any, Optional
import httpx

from ..errors import APIError, ConnectionError, TimeoutError


class RestClient:
    """Async REST client for Bud Foundry Gateway."""

    def __init__(
        self,
        base_url: str,
        api_key: Optional[str] = None,
        timeout: float = 30.0,
    ):
        """
        Initialize REST client.

        Args:
            base_url: Base URL of the Bud Foundry gateway
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

    async def delete(
        self,
        endpoint: str,
        params: Optional[dict[str, Any]] = None,
    ) -> Any:
        """Make a DELETE request."""
        return await self._request("DELETE", endpoint, params=params)

    async def health(self) -> dict[str, Any]:
        """
        Check gateway health.

        Returns:
            Health status with version info
        """
        result: dict[str, Any] = await self.get("/")
        return result

    async def list_voices(self, provider: Optional[str] = None) -> list[dict[str, Any]]:
        """
        List available TTS voices.

        Args:
            provider: Optional provider to filter voices

        Returns:
            List of available voices
        """
        params: dict[str, str] = {}
        if provider:
            params["provider"] = provider
        result: list[dict[str, Any]] = await self.get("/voices", params=params)
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
        payload: dict[str, Any] = {
            "text": text,
            "provider": provider,
            "sample_rate": sample_rate,
        }
        if voice:
            payload["voice"] = voice
        if voice_id:
            payload["voice_id"] = voice_id
        if model:
            payload["model"] = model

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

    async def delete_sip_hook(self, host: str) -> None:
        """
        Delete a SIP hook.

        Args:
            host: SIP host to delete
        """
        await self.delete(f"/sip/hooks/{host}")

    # =========================================================================
    # Voice Cloning Methods
    # =========================================================================

    async def clone_voice(
        self,
        name: str,
        audio_files: list[bytes],
        provider: str = "elevenlabs",
        description: Optional[str] = None,
        labels: Optional[dict[str, str]] = None,
    ) -> dict[str, Any]:
        """
        Clone a voice from audio samples.

        Args:
            name: Name for the cloned voice.
            audio_files: List of audio file data (bytes).
            provider: Voice cloning provider (elevenlabs, playht, resemble).
            description: Optional description for the voice.
            labels: Optional labels/tags for the voice.

        Returns:
            Created voice information with voice_id.

        Raises:
            APIError: If cloning fails.
        """
        import base64

        # Convert audio files to base64
        audio_base64 = [base64.b64encode(audio).decode("utf-8") for audio in audio_files]

        payload: dict[str, Any] = {
            "name": name,
            "provider": provider,
            "audio_files": audio_base64,
        }
        if description:
            payload["description"] = description
        if labels:
            payload["labels"] = labels

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
        Get recording information by stream ID.

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
        format: str = "wav",
    ) -> bytes:
        """
        Download a recording.

        Args:
            stream_id: The stream/session ID.
            format: Output format (wav, mp3, ogg).

        Returns:
            Audio data as bytes.
        """
        params = {"format": format}
        result: bytes = await self.get(f"/recordings/{stream_id}/download", params=params)
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

        Args:
            stream_id: The stream/session ID.
        """
        await self.delete(f"/recordings/{stream_id}")

    # =========================================================================
    # DAG Template Methods
    # =========================================================================

    async def list_dag_templates(self) -> list[dict[str, Any]]:
        """
        List available DAG templates.

        Returns:
            List of DAG template definitions.
        """
        result: list[dict[str, Any]] = await self.get("/dag/templates")
        return result

    async def validate_dag(
        self,
        definition: dict[str, Any],
    ) -> dict[str, Any]:
        """
        Validate a DAG definition.

        Args:
            definition: DAG definition to validate.

        Returns:
            Validation result with is_valid and any errors.
        """
        result: dict[str, Any] = await self.post("/dag/validate", json=definition)
        return result

    async def __aenter__(self) -> "RestClient":
        """Async context manager entry."""
        return self

    async def __aexit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        """Async context manager exit."""
        await self.close()
