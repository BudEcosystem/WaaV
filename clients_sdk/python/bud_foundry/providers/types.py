"""
Provider types for plugin discovery.

These types represent the provider metadata returned by the gateway's
plugin discovery endpoints.
"""

from dataclasses import dataclass, field
from typing import Optional


@dataclass
class LanguageInfo:
    """Language information with human-readable name."""

    code: str
    """ISO 639-1 language code (e.g., 'en')"""

    name: str
    """Human-readable name (e.g., 'English')"""


@dataclass
class ProviderMetrics:
    """Usage metrics for a provider."""

    call_count: int = 0
    """Total number of calls"""

    error_count: int = 0
    """Number of errors"""

    error_rate: float = 0.0
    """Error rate (0.0 to 1.0)"""

    uptime_seconds: int = 0
    """Uptime in seconds"""


@dataclass
class ProviderHealthDetails:
    """Health status details for a provider."""

    call_count: int = 0
    """Total number of calls"""

    error_count: int = 0
    """Number of errors"""

    error_rate: float = 0.0
    """Error rate (0.0 to 1.0)"""

    last_error: Optional[str] = None
    """Last error message (if any)"""

    uptime_seconds: int = 0
    """Uptime in seconds"""

    idle_seconds: int = 0
    """Time since last activity in seconds"""


@dataclass
class ProviderHealth:
    """Provider health status response."""

    id: str
    """Provider identifier"""

    health: str
    """Health status: 'healthy', 'degraded', or 'unhealthy'"""

    details: ProviderHealthDetails
    """Health details"""

    @property
    def is_healthy(self) -> bool:
        """Check if provider is healthy."""
        return self.health == "healthy"

    @property
    def is_degraded(self) -> bool:
        """Check if provider is degraded."""
        return self.health == "degraded"

    @property
    def is_unhealthy(self) -> bool:
        """Check if provider is unhealthy."""
        return self.health == "unhealthy"


@dataclass
class ProviderInfo:
    """
    Provider information for SDK discovery.

    Contains all metadata about a provider including its capabilities,
    supported languages, models, and features.

    Example:
        >>> provider = await bud.providers.get("deepgram")
        >>> print(provider.display_name)
        "Deepgram Nova-3"
        >>> print(provider.features)
        ['streaming', 'word-timestamps']
        >>> print(provider.languages)
        [LanguageInfo(code='en', name='English'), ...]
    """

    id: str
    """Provider identifier (e.g., 'deepgram')"""

    display_name: str
    """Human-readable name (e.g., 'Deepgram Nova-3')"""

    provider_type: str
    """Provider type: 'stt', 'tts', or 'realtime'"""

    description: str = ""
    """Brief description"""

    version: str = "1.0.0"
    """Provider version"""

    features: list[str] = field(default_factory=list)
    """Provider features (e.g., ['streaming', 'word-timestamps'])"""

    languages: list[LanguageInfo] = field(default_factory=list)
    """Supported languages with human-readable names"""

    models: list[str] = field(default_factory=list)
    """Supported models"""

    aliases: list[str] = field(default_factory=list)
    """Provider aliases (e.g., ['dg', 'deepgram-nova'])"""

    required_config: list[str] = field(default_factory=list)
    """Required configuration keys"""

    optional_config: list[str] = field(default_factory=list)
    """Optional configuration keys"""

    health: str = "healthy"
    """Health status"""

    metrics: Optional[ProviderMetrics] = None
    """Usage metrics (if available)"""

    def has_feature(self, feature: str) -> bool:
        """
        Check if provider has a specific feature.

        Args:
            feature: Feature to check (case-insensitive)

        Returns:
            True if provider has the feature
        """
        feature_lower = feature.lower()
        return any(f.lower() == feature_lower for f in self.features)

    def supports_language(self, language: str) -> bool:
        """
        Check if provider supports a specific language.

        Args:
            language: Language code or name (case-insensitive)

        Returns:
            True if provider supports the language
        """
        language_lower = language.lower()
        return any(
            lang.code.lower() == language_lower or
            lang.name.lower() == language_lower or
            language_lower in lang.name.lower()
            for lang in self.languages
        )

    def supports_model(self, model: str) -> bool:
        """
        Check if provider supports a specific model.

        Args:
            model: Model name (case-insensitive partial match)

        Returns:
            True if provider supports the model
        """
        model_lower = model.lower()
        return any(model_lower in m.lower() for m in self.models)

    @property
    def is_available(self) -> bool:
        """Check if provider is available (healthy or degraded)."""
        return self.health in ("healthy", "degraded")

    @classmethod
    def from_dict(cls, data: dict) -> "ProviderInfo":
        """Create ProviderInfo from dictionary response."""
        languages = [
            LanguageInfo(code=lang["code"], name=lang["name"])
            for lang in data.get("languages", [])
        ]

        metrics = None
        if data.get("metrics"):
            metrics = ProviderMetrics(
                call_count=data["metrics"].get("call_count", 0),
                error_count=data["metrics"].get("error_count", 0),
                error_rate=data["metrics"].get("error_rate", 0.0),
                uptime_seconds=data["metrics"].get("uptime_seconds", 0),
            )

        return cls(
            id=data["id"],
            display_name=data["display_name"],
            provider_type=data["provider_type"],
            description=data.get("description", ""),
            version=data.get("version", "1.0.0"),
            features=data.get("features", []),
            languages=languages,
            models=data.get("models", []),
            aliases=data.get("aliases", []),
            required_config=data.get("required_config", []),
            optional_config=data.get("optional_config", []),
            health=data.get("health", "healthy"),
            metrics=metrics,
        )


@dataclass
class ProcessorInfo:
    """Audio processor information."""

    id: str
    """Processor identifier"""

    name: str
    """Display name"""

    description: str = ""
    """Description"""

    supported_formats: list[str] = field(default_factory=list)
    """Supported audio formats"""

    @classmethod
    def from_dict(cls, data: dict) -> "ProcessorInfo":
        """Create ProcessorInfo from dictionary response."""
        return cls(
            id=data["id"],
            name=data["name"],
            description=data.get("description", ""),
            supported_formats=data.get("supported_formats", []),
        )


@dataclass
class PluginListResponse:
    """Response from the /plugins endpoint."""

    stt: list[ProviderInfo]
    """STT providers"""

    tts: list[ProviderInfo]
    """TTS providers"""

    realtime: list[ProviderInfo]
    """Realtime providers"""

    processors: list[ProcessorInfo]
    """Audio processors"""

    total_count: int
    """Total count of all plugins"""

    @classmethod
    def from_dict(cls, data: dict) -> "PluginListResponse":
        """Create PluginListResponse from dictionary response."""
        return cls(
            stt=[ProviderInfo.from_dict(p) for p in data.get("stt", [])],
            tts=[ProviderInfo.from_dict(p) for p in data.get("tts", [])],
            realtime=[ProviderInfo.from_dict(p) for p in data.get("realtime", [])],
            processors=[ProcessorInfo.from_dict(p) for p in data.get("processors", [])],
            total_count=data.get("total_count", 0),
        )
