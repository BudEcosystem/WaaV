"""
Tests for BudClient lifecycle management.
"""

import pytest
from unittest.mock import AsyncMock, MagicMock

from bud_waav.client import BudClient


class TestLifecycleManagement:
    """Tests for pipeline lifecycle tracking."""

    @pytest.fixture
    def client(self):
        """Create a BudClient instance."""
        return BudClient(base_url="http://localhost:3001")

    def test_initial_pipeline_count(self, client):
        """Should start with zero active pipelines."""
        assert client.get_active_pipeline_count() == 0

    def test_register_pipeline(self, client):
        """Should track registered pipelines."""
        session = MagicMock()
        client.register_pipeline(session)
        assert client.get_active_pipeline_count() == 1

    def test_register_multiple_pipelines(self, client):
        """Should track multiple registered pipelines."""
        s1 = MagicMock()
        s2 = MagicMock()
        s3 = MagicMock()

        client.register_pipeline(s1)
        client.register_pipeline(s2)
        client.register_pipeline(s3)

        assert client.get_active_pipeline_count() == 3

    def test_deregister_pipeline(self, client):
        """Should remove deregistered pipelines."""
        session = MagicMock()
        client.register_pipeline(session)
        assert client.get_active_pipeline_count() == 1

        client.deregister_pipeline(session)
        assert client.get_active_pipeline_count() == 0

    def test_deregister_nonexistent(self, client):
        """Should not raise when deregistering untracked pipeline."""
        session = MagicMock()
        client.deregister_pipeline(session)  # Should not raise
        assert client.get_active_pipeline_count() == 0

    def test_duplicate_register(self, client):
        """Registering same session twice should count as one."""
        session = MagicMock()
        client.register_pipeline(session)
        client.register_pipeline(session)
        assert client.get_active_pipeline_count() == 1

    @pytest.mark.asyncio
    async def test_disconnect_all(self, client):
        """disconnect_all should call close on all tracked sessions."""
        s1 = MagicMock()
        s1.close = AsyncMock()
        s2 = MagicMock()
        s2.close = AsyncMock()

        client.register_pipeline(s1)
        client.register_pipeline(s2)

        count = await client.disconnect_all()
        assert count == 2
        s1.close.assert_called_once()
        s2.close.assert_called_once()
        assert client.get_active_pipeline_count() == 0

    @pytest.mark.asyncio
    async def test_disconnect_all_with_disconnect_method(self, client):
        """disconnect_all should use disconnect() if close() not available."""
        session = MagicMock(spec=[])  # No close method
        session.disconnect = AsyncMock()

        client.register_pipeline(session)

        count = await client.disconnect_all()
        assert count == 1
        session.disconnect.assert_called_once()

    @pytest.mark.asyncio
    async def test_disconnect_all_handles_errors(self, client):
        """disconnect_all should not raise if a session.close() fails."""
        s1 = MagicMock()
        s1.close = AsyncMock(side_effect=Exception("connection error"))
        s2 = MagicMock()
        s2.close = AsyncMock()

        client.register_pipeline(s1)
        client.register_pipeline(s2)

        # Should not raise
        count = await client.disconnect_all()
        # s2 should still be closed despite s1 error
        s2.close.assert_called_once()

    @pytest.mark.asyncio
    async def test_disconnect_all_empty(self, client):
        """disconnect_all on empty set should return 0."""
        count = await client.disconnect_all()
        assert count == 0

    @pytest.mark.asyncio
    async def test_close_disconnects_all(self, client):
        """BudClient.close() should disconnect all active pipelines."""
        session = MagicMock()
        session.close = AsyncMock()
        client.register_pipeline(session)

        # Mock the REST client close
        client._rest_client.close = AsyncMock()

        await client.close()

        session.close.assert_called_once()
        client._rest_client.close.assert_called_once()

    def test_weak_reference_cleanup(self, client):
        """Sessions that get garbage collected should be auto-removed."""
        session = MagicMock()
        client.register_pipeline(session)
        assert client.get_active_pipeline_count() == 1

        # Delete the session reference
        del session

        # WeakSet should automatically remove it (may require GC)
        import gc
        gc.collect()
        assert client.get_active_pipeline_count() == 0
