"""
Tests for voice cloning and recording REST methods.
"""

import pytest
from unittest.mock import AsyncMock, MagicMock, patch
import base64

from bud_foundry.rest.client import RestClient


class TestVoiceCloning:
    """Tests for voice cloning methods."""

    @pytest.fixture
    def client(self):
        """Create a RestClient instance."""
        return RestClient(base_url="http://localhost:3001", api_key="test-key")

    @pytest.mark.asyncio
    async def test_clone_voice(self, client):
        """Should clone a voice with audio files."""
        # Mock the post method
        client.post = AsyncMock(
            return_value={
                "voice_id": "voice_123",
                "name": "My Voice",
                "provider": "elevenlabs",
                "is_cloned": True,
            }
        )

        audio_data = [b"\x00\x01\x02\x03", b"\x04\x05\x06\x07"]
        result = await client.clone_voice(
            name="My Voice",
            audio_files=audio_data,
            provider="elevenlabs",
            description="A cloned voice",
        )

        assert result["voice_id"] == "voice_123"
        assert result["name"] == "My Voice"

        # Verify the call
        client.post.assert_called_once()
        call_args = client.post.call_args
        assert call_args[0][0] == "/voices/clone"
        payload = call_args[1]["json"]
        assert payload["name"] == "My Voice"
        assert payload["provider"] == "elevenlabs"
        assert payload["description"] == "A cloned voice"
        # Check audio files are base64 encoded
        assert len(payload["audio_files"]) == 2
        assert payload["audio_files"][0] == base64.b64encode(audio_data[0]).decode()

    @pytest.mark.asyncio
    async def test_list_cloned_voices(self, client):
        """Should list cloned voices."""
        client.get = AsyncMock(
            return_value=[
                {"voice_id": "v1", "name": "Voice 1", "is_cloned": True},
                {"voice_id": "v2", "name": "Voice 2", "is_cloned": False},
                {"voice_id": "v3", "name": "Voice 3", "is_cloned": True},
            ]
        )

        result = await client.list_cloned_voices()

        # Should filter to only cloned voices
        assert len(result) == 2
        assert all(v["is_cloned"] for v in result)

    @pytest.mark.asyncio
    async def test_list_cloned_voices_with_provider(self, client):
        """Should list cloned voices filtered by provider."""
        client.get = AsyncMock(
            return_value=[
                {"voice_id": "v1", "name": "Voice 1", "is_cloned": True},
            ]
        )

        await client.list_cloned_voices(provider="elevenlabs")

        client.get.assert_called_once()
        call_args = client.get.call_args
        params = call_args[1]["params"]
        assert params["provider"] == "elevenlabs"
        assert params["cloned"] == "true"

    @pytest.mark.asyncio
    async def test_delete_cloned_voice(self, client):
        """Should delete a cloned voice."""
        client.delete = AsyncMock(return_value=None)

        await client.delete_cloned_voice(
            voice_id="voice_123",
            provider="elevenlabs",
        )

        client.delete.assert_called_once()
        call_args = client.delete.call_args
        assert call_args[0][0] == "/voices/voice_123"
        assert call_args[1]["params"]["provider"] == "elevenlabs"

    @pytest.mark.asyncio
    async def test_get_cloned_voice(self, client):
        """Should get a cloned voice by ID."""
        client.get = AsyncMock(
            return_value={
                "voice_id": "voice_123",
                "name": "My Voice",
                "provider": "elevenlabs",
                "is_cloned": True,
            }
        )

        result = await client.get_cloned_voice(
            voice_id="voice_123",
            provider="elevenlabs",
        )

        assert result["voice_id"] == "voice_123"
        client.get.assert_called_once()


class TestRecordings:
    """Tests for recording methods."""

    @pytest.fixture
    def client(self):
        """Create a RestClient instance."""
        return RestClient(base_url="http://localhost:3001", api_key="test-key")

    @pytest.mark.asyncio
    async def test_get_recording(self, client):
        """Should get recording info by stream ID."""
        client.get = AsyncMock(
            return_value={
                "stream_id": "stream_123",
                "status": "ready",
                "duration_ms": 5000,
                "format": "wav",
                "size_bytes": 100000,
            }
        )

        result = await client.get_recording(stream_id="stream_123")

        assert result["stream_id"] == "stream_123"
        assert result["status"] == "ready"
        client.get.assert_called_once_with("/recordings/stream_123")

    @pytest.mark.asyncio
    async def test_download_recording(self, client):
        """Should download recording as bytes."""
        audio_data = b"\x00\x01\x02\x03" * 1000
        client.get = AsyncMock(return_value=audio_data)

        result = await client.download_recording(
            stream_id="stream_123",
            format="wav",
        )

        assert result == audio_data
        client.get.assert_called_once()
        call_args = client.get.call_args
        assert call_args[0][0] == "/recordings/stream_123/download"
        assert call_args[1]["params"]["format"] == "wav"

    @pytest.mark.asyncio
    async def test_list_recordings(self, client):
        """Should list recordings with filters."""
        client.get = AsyncMock(
            return_value={
                "recordings": [
                    {"stream_id": "s1", "status": "ready"},
                    {"stream_id": "s2", "status": "ready"},
                ],
                "total": 2,
                "limit": 50,
                "offset": 0,
                "has_more": False,
            }
        )

        result = await client.list_recordings(
            limit=50,
            offset=0,
            status="ready",
        )

        assert len(result["recordings"]) == 2
        assert result["has_more"] is False

        client.get.assert_called_once()
        call_args = client.get.call_args
        assert call_args[0][0] == "/recordings"
        params = call_args[1]["params"]
        assert params["limit"] == 50
        assert params["offset"] == 0
        assert params["status"] == "ready"

    @pytest.mark.asyncio
    async def test_list_recordings_with_date_filter(self, client):
        """Should list recordings filtered by date."""
        client.get = AsyncMock(
            return_value={
                "recordings": [],
                "total": 0,
                "limit": 50,
                "offset": 0,
                "has_more": False,
            }
        )

        await client.list_recordings(
            from_date="2024-01-01T00:00:00Z",
            to_date="2024-01-31T23:59:59Z",
        )

        client.get.assert_called_once()
        call_args = client.get.call_args
        params = call_args[1]["params"]
        assert params["from_date"] == "2024-01-01T00:00:00Z"
        assert params["to_date"] == "2024-01-31T23:59:59Z"

    @pytest.mark.asyncio
    async def test_delete_recording(self, client):
        """Should delete a recording."""
        client.delete = AsyncMock(return_value=None)

        await client.delete_recording(stream_id="stream_123")

        client.delete.assert_called_once_with("/recordings/stream_123")


class TestDAGMethods:
    """Tests for DAG REST methods."""

    @pytest.fixture
    def client(self):
        """Create a RestClient instance."""
        return RestClient(base_url="http://localhost:3001", api_key="test-key")

    @pytest.mark.asyncio
    async def test_list_dag_templates(self, client):
        """Should list DAG templates."""
        client.get = AsyncMock(
            return_value=[
                {"id": "simple_stt", "name": "Simple STT"},
                {"id": "voice_assistant", "name": "Voice Assistant"},
            ]
        )

        result = await client.list_dag_templates()

        assert len(result) == 2
        assert result[0]["id"] == "simple_stt"
        client.get.assert_called_once_with("/dag/templates")

    @pytest.mark.asyncio
    async def test_validate_dag(self, client):
        """Should validate a DAG definition."""
        client.post = AsyncMock(
            return_value={
                "is_valid": True,
                "errors": [],
            }
        )

        definition = {
            "id": "test",
            "name": "Test DAG",
            "version": "1.0.0",
            "nodes": [],
            "edges": [],
        }

        result = await client.validate_dag(definition)

        assert result["is_valid"] is True
        client.post.assert_called_once()
        call_args = client.post.call_args
        assert call_args[0][0] == "/dag/validate"
        assert call_args[1]["json"] == definition

    @pytest.mark.asyncio
    async def test_validate_dag_invalid(self, client):
        """Should return errors for invalid DAG."""
        client.post = AsyncMock(
            return_value={
                "is_valid": False,
                "errors": ["DAG is empty", "No audio input node"],
            }
        )

        result = await client.validate_dag({})

        assert result["is_valid"] is False
        assert len(result["errors"]) == 2
