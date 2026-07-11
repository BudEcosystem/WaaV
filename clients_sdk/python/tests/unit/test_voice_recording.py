"""
Tests for voice cloning and recording REST methods.
"""

import pytest
from unittest.mock import AsyncMock, MagicMock, patch
import base64

from bud_waav.errors import APIError
from bud_waav.rest.client import RestClient


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
        # P4: the canonical gateway wire field is `audio_samples` (base64-encoded).
        # `audio_files` bytes are encoded into it (the old `audio_files` wire key
        # was wrong and silently dropped server-side).
        assert len(payload["audio_samples"]) == 2
        assert payload["audio_samples"][0] == base64.b64encode(audio_data[0]).decode()
        assert payload["mode"] == "instant"

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
        """Fail-fast: the gateway serves no DELETE /voices/{id} route — a typed
        501 APIError is raised immediately instead of a confusing network 404."""
        client.delete = AsyncMock(return_value=None)

        with pytest.raises(APIError) as excinfo:
            await client.delete_cloned_voice(
                voice_id="voice_123",
                provider="elevenlabs",
            )

        assert excinfo.value.status_code == 501
        assert "not supported" in str(excinfo.value)
        client.delete.assert_not_called()

    @pytest.mark.asyncio
    async def test_get_cloned_voice(self, client):
        """Fail-fast: the gateway serves no GET /voices/{id} route — a typed
        501 APIError is raised immediately."""
        client.get = AsyncMock(return_value={})

        with pytest.raises(APIError) as excinfo:
            await client.get_cloned_voice(
                voice_id="voice_123",
                provider="elevenlabs",
            )

        assert excinfo.value.status_code == 501
        client.get.assert_not_called()


class TestRecordings:
    """Tests for recording methods."""

    @pytest.fixture
    def client(self):
        """Create a RestClient instance."""
        return RestClient(base_url="http://localhost:3001", api_key="test-key")

    @pytest.mark.asyncio
    async def test_get_recording(self, client):
        """get_recording is a deprecated alias of download_recording — it must hit
        the SERVED route GET /recording/{id} (singular; the old plural path 404'd)."""
        audio_data = b"\x00\x01" * 500
        client.get = AsyncMock(return_value=audio_data)

        result = await client.get_recording(stream_id="stream_123")

        assert result == audio_data
        client.get.assert_called_once_with("/recording/stream_123")

    @pytest.mark.asyncio
    async def test_download_recording(self, client):
        """Should download recording from GET /recording/{stream_id} (singular)."""
        audio_data = b"\x00\x01\x02\x03" * 1000
        client.get = AsyncMock(return_value=audio_data)

        result = await client.download_recording(stream_id="stream_123")

        assert result == audio_data
        client.get.assert_called_once_with("/recording/stream_123")

    @pytest.mark.asyncio
    async def test_list_recordings(self, client):
        """Fail-fast: the gateway serves no GET /recordings route — typed 501."""
        client.get = AsyncMock(return_value={})

        with pytest.raises(APIError) as excinfo:
            await client.list_recordings(limit=10)

        assert excinfo.value.status_code == 501
        client.get.assert_not_called()

    @pytest.mark.asyncio
    async def test_delete_recording(self, client):
        """Fail-fast: the gateway serves no DELETE /recordings/{id} route — typed 501."""
        client.delete = AsyncMock(return_value=None)

        with pytest.raises(APIError) as excinfo:
            await client.delete_recording(stream_id="stream_123")

        assert excinfo.value.status_code == 501
        client.delete.assert_not_called()


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
        assert call_args[1]["json"] == {"dag": definition}

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
