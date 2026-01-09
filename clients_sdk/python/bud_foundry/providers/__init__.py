"""
Provider discovery module for Bud Foundry SDK.

This module provides a user-friendly API for discovering available providers
(STT, TTS, Realtime) from the gateway.

Example:
    >>> bud = BudClient(base_url="http://localhost:3001")
    >>>
    >>> # Find all STT providers
    >>> stt_providers = await bud.providers.stt.all()
    >>>
    >>> # Find providers by language
    >>> spanish_stt = await bud.providers.stt.with_language("Spanish")
    >>>
    >>> # Find providers by feature
    >>> streaming_stt = await bud.providers.stt.with_feature("streaming")
    >>>
    >>> # Get specific provider info
    >>> deepgram = await bud.providers.get("deepgram")
    >>> print(deepgram.description)
    >>> print(deepgram.features)
"""

from typing import Optional, TYPE_CHECKING

from .types import (
    LanguageInfo,
    PluginListResponse,
    ProcessorInfo,
    ProviderHealth,
    ProviderHealthDetails,
    ProviderInfo,
    ProviderMetrics,
)

if TYPE_CHECKING:
    from ..rest.client import RestClient


class ProviderCategory:
    """
    Category-specific provider discovery.

    Provides fluent API for finding providers within a category (STT, TTS, Realtime).

    Example:
        >>> # All STT providers
        >>> providers = await bud.providers.stt.all()
        >>>
        >>> # Filter by language
        >>> spanish = await bud.providers.stt.with_language("Spanish")
        >>>
        >>> # Filter by feature
        >>> streaming = await bud.providers.stt.with_feature("streaming")
    """

    def __init__(self, category: str, rest_client: "RestClient"):
        """
        Initialize provider category.

        Args:
            category: Provider category ('stt', 'tts', or 'realtime')
            rest_client: REST client for API calls
        """
        self._category = category
        self._rest = rest_client

    async def all(self) -> list[ProviderInfo]:
        """
        Get all providers in this category.

        Returns:
            List of all providers
        """
        data = await self._rest.get(f"/plugins/{self._category}")
        return [ProviderInfo.from_dict(p) for p in data]

    async def with_language(self, language: str) -> list[ProviderInfo]:
        """
        Find providers that support a specific language.

        Args:
            language: Language code (e.g., 'en') or name (e.g., 'English', 'Spanish')

        Returns:
            List of providers supporting the language
        """
        data = await self._rest.get(
            f"/plugins/{self._category}",
            params={"language": language},
        )
        return [ProviderInfo.from_dict(p) for p in data]

    async def with_feature(self, feature: str) -> list[ProviderInfo]:
        """
        Find providers that have a specific feature.

        Args:
            feature: Feature name (e.g., 'streaming', 'word-timestamps')

        Returns:
            List of providers with the feature
        """
        data = await self._rest.get(
            f"/plugins/{self._category}",
            params={"feature": feature},
        )
        return [ProviderInfo.from_dict(p) for p in data]

    async def with_model(self, model: str) -> list[ProviderInfo]:
        """
        Find providers that support a specific model.

        Args:
            model: Model name or partial match

        Returns:
            List of providers supporting the model
        """
        data = await self._rest.get(
            f"/plugins/{self._category}",
            params={"model": model},
        )
        return [ProviderInfo.from_dict(p) for p in data]

    async def recommended(self) -> Optional[ProviderInfo]:
        """
        Get the recommended provider for this category.

        Returns the first healthy provider with streaming support (if applicable).

        Returns:
            Recommended provider or None if no healthy providers
        """
        providers = await self.all()
        # Prefer healthy providers with streaming support
        for provider in providers:
            if provider.health == "healthy" and provider.has_feature("streaming"):
                return provider
        # Fall back to any healthy provider
        for provider in providers:
            if provider.health == "healthy":
                return provider
        return providers[0] if providers else None


class ProviderRegistry:
    """
    Provider discovery registry.

    Provides a user-friendly API for discovering available providers from the gateway.
    All complexity of REST endpoints is abstracted away.

    Example:
        >>> # Access via BudClient
        >>> bud = BudClient(base_url="http://localhost:3001")
        >>>
        >>> # Get all plugins grouped by type
        >>> all_plugins = await bud.providers.discover()
        >>> print(f"Found {all_plugins.total_count} plugins")
        >>>
        >>> # Find providers by type
        >>> stt_providers = await bud.providers.stt.all()
        >>> tts_providers = await bud.providers.tts.all()
        >>>
        >>> # Find by language (human-readable)
        >>> spanish_stt = await bud.providers.stt.with_language("Spanish")
        >>>
        >>> # Find by feature
        >>> streaming_stt = await bud.providers.stt.with_feature("streaming")
        >>>
        >>> # Get specific provider info
        >>> deepgram = await bud.providers.get("deepgram")
        >>> print(deepgram.display_name)
        >>> print(deepgram.features)
        >>> print(deepgram.languages)
        >>>
        >>> # Check provider health
        >>> health = await bud.providers.health("deepgram")
        >>> print(health.is_healthy)
    """

    def __init__(self, rest_client: "RestClient"):
        """
        Initialize provider registry.

        Args:
            rest_client: REST client for API calls
        """
        self._rest = rest_client
        self._stt = ProviderCategory("stt", rest_client)
        self._tts = ProviderCategory("tts", rest_client)
        self._realtime = ProviderCategory("realtime", rest_client)

    @property
    def stt(self) -> ProviderCategory:
        """
        STT (Speech-to-Text) provider category.

        Example:
            >>> providers = await bud.providers.stt.all()
            >>> spanish = await bud.providers.stt.with_language("Spanish")
        """
        return self._stt

    @property
    def tts(self) -> ProviderCategory:
        """
        TTS (Text-to-Speech) provider category.

        Example:
            >>> providers = await bud.providers.tts.all()
            >>> with_ssml = await bud.providers.tts.with_feature("ssml")
        """
        return self._tts

    @property
    def realtime(self) -> ProviderCategory:
        """
        Realtime (Audio-to-Audio) provider category.

        Example:
            >>> providers = await bud.providers.realtime.all()
        """
        return self._realtime

    async def discover(self) -> PluginListResponse:
        """
        Discover all available plugins.

        Returns all plugins grouped by type (STT, TTS, Realtime, Processors).

        Returns:
            PluginListResponse with all available plugins
        """
        data = await self._rest.get("/plugins")
        return PluginListResponse.from_dict(data)

    async def get(self, provider_id: str) -> Optional[ProviderInfo]:
        """
        Get specific provider information by ID.

        Searches across all provider types (STT, TTS, Realtime).

        Args:
            provider_id: Provider identifier (e.g., 'deepgram', 'elevenlabs')

        Returns:
            Provider information or None if not found
        """
        try:
            data = await self._rest.get(f"/plugins/{provider_id}")
            return ProviderInfo.from_dict(data)
        except Exception:
            return None

    async def health(self, provider_id: str) -> Optional[ProviderHealth]:
        """
        Get provider health status.

        Args:
            provider_id: Provider identifier

        Returns:
            Provider health status or None if not found
        """
        try:
            data = await self._rest.get(f"/plugins/{provider_id}/health")
            details = ProviderHealthDetails(
                call_count=data["details"].get("call_count", 0),
                error_count=data["details"].get("error_count", 0),
                error_rate=data["details"].get("error_rate", 0.0),
                last_error=data["details"].get("last_error"),
                uptime_seconds=data["details"].get("uptime_seconds", 0),
                idle_seconds=data["details"].get("idle_seconds", 0),
            )
            return ProviderHealth(
                id=data["id"],
                health=data["health"],
                details=details,
            )
        except Exception:
            return None

    async def filter(
        self,
        *,
        type: Optional[str] = None,
        language: Optional[str] = None,
        feature: Optional[str] = None,
        model: Optional[str] = None,
    ) -> list[ProviderInfo]:
        """
        Filter providers across all types.

        Args:
            type: Provider type ('stt', 'tts', 'realtime')
            language: Filter by language support
            feature: Filter by feature
            model: Filter by model support

        Returns:
            List of matching providers
        """
        if type:
            # Filter within specific type
            category = getattr(self, type, None)
            if category:
                providers = await category.all()
            else:
                return []
        else:
            # Get all providers
            plugins = await self.discover()
            providers = plugins.stt + plugins.tts + plugins.realtime

        # Apply filters
        if language:
            providers = [p for p in providers if p.supports_language(language)]
        if feature:
            providers = [p for p in providers if p.has_feature(feature)]
        if model:
            providers = [p for p in providers if p.supports_model(model)]

        return providers

    async def processors(self) -> list[ProcessorInfo]:
        """
        Get all available audio processors.

        Returns:
            List of audio processors
        """
        data = await self._rest.get("/plugins/processors")
        return [ProcessorInfo.from_dict(p) for p in data]


# Re-export types for convenience
__all__ = [
    "ProviderRegistry",
    "ProviderCategory",
    "ProviderInfo",
    "ProviderHealth",
    "ProviderHealthDetails",
    "ProviderMetrics",
    "LanguageInfo",
    "ProcessorInfo",
    "PluginListResponse",
]
