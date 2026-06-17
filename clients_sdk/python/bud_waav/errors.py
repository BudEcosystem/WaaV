"""
Error classes for bud-waav SDK
"""

from typing import Any, Optional


class BudError(Exception):
    """Base exception for all Bud Foundry SDK errors."""

    def __init__(
        self,
        message: str,
        code: Optional[str] = None,
        cause: Optional[Exception] = None,
        context: Optional[dict[str, Any]] = None,
    ):
        super().__init__(message)
        self.message = message
        self.code = code
        self.cause = cause
        self.context = context or {}

    def __str__(self) -> str:
        if self.code:
            return f"[{self.code}] {self.message}"
        return self.message


class ConnectionError(BudError):
    """Error connecting to the server."""

    def __init__(
        self,
        message: str,
        url: Optional[str] = None,
        cause: Optional[Exception] = None,
    ):
        super().__init__(message, code="CONNECTION_ERROR", cause=cause, context={"url": url})
        self.url = url


class TimeoutError(BudError):
    """Request or connection timed out."""

    def __init__(
        self,
        message: str,
        timeout_ms: int,
        operation: Optional[str] = None,
    ):
        super().__init__(
            message,
            code="TIMEOUT",
            context={"timeout_ms": timeout_ms, "operation": operation},
        )
        self.timeout_ms = timeout_ms
        self.operation = operation


class ReconnectError(BudError):
    """Reconnection failed."""

    def __init__(
        self,
        message: str,
        attempts: int,
        last_error: Optional[Exception] = None,
    ):
        super().__init__(
            message,
            code="RECONNECT_FAILED",
            cause=last_error,
            context={"attempts": attempts},
        )
        self.attempts = attempts


class FatalConnectionError(BudError):
    """A connection is judged unrecoverable; the SDK stops its reconnect storm.

    Raised by the client-side quick-failure breaker (D4a) after a run of
    connections that each died within seconds of opening — the hallmark of a
    doomed config (bad key, rejected provider, malformed config) that the
    gateway keeps accepting at the handshake but tearing down immediately.
    Looping such a config just storms; instead we surface this typed FATAL so
    the caller fixes the config rather than retrying. This is the CLIENT analog
    of the gateway's per-provider CircuitBreaker FATAL fast-trip (which is
    upstream-only and never surfaced on the client wire), not a duplicate.
    """

    def __init__(
        self,
        message: str,
        consecutive_quick_failures: int,
        last_error: Optional[Exception] = None,
    ):
        super().__init__(
            message,
            code="FATAL_CONNECTION",
            cause=last_error,
            context={"consecutive_quick_failures": consecutive_quick_failures},
        )
        self.consecutive_quick_failures = consecutive_quick_failures


class ProtocolVersionError(BudError):
    """The gateway's wire ``protocol_version`` MAJOR differs from the SDK's.

    Emitted (as a typed ``warning`` event, not raised) when the ``ready``
    envelope carries a protocol whose major component the bundled SDK does not
    understand. A major bump means the wire contract changed in a
    backward-incompatible way, so field drift would otherwise corrupt silently.
    """

    def __init__(
        self,
        message: str,
        gateway_version: Optional[str] = None,
        sdk_version: Optional[str] = None,
    ):
        super().__init__(
            message,
            code="PROTOCOL_VERSION_MISMATCH",
            context={
                "gateway_version": gateway_version,
                "sdk_version": sdk_version,
            },
        )
        self.gateway_version = gateway_version
        self.sdk_version = sdk_version


class APIError(BudError):
    """API request failed."""

    def __init__(
        self,
        message: str,
        status_code: int,
        response_body: Optional[Any] = None,
        url: Optional[str] = None,
        method: Optional[str] = None,
    ):
        super().__init__(
            message,
            code="API_ERROR",
            context={
                "status_code": status_code,
                "url": url,
                "method": method,
                "response_body": response_body,
            },
        )
        self.status_code = status_code
        self.response_body = response_body
        self.url = url
        self.method = method

    @classmethod
    def from_response(
        cls,
        status_code: int,
        response_body: Any,
        url: Optional[str] = None,
        method: Optional[str] = None,
    ) -> "APIError":
        """Create APIError from HTTP response."""
        if isinstance(response_body, dict):
            message = response_body.get("message") or response_body.get("error") or str(response_body)
        else:
            message = str(response_body) if response_body else f"HTTP {status_code}"

        return cls(
            message=message,
            status_code=status_code,
            response_body=response_body,
            url=url,
            method=method,
        )


class RateLimitError(BudError):
    """Server returned HTTP 429 (Too Many Requests).

    The WaaV gateway applies a per-IP token bucket to REST AND WebSocket
    upgrades. This typed error carries the parsed ``Retry-After`` (seconds) so
    callers can back off deterministically instead of parsing an opaque string.
    """

    def __init__(
        self,
        message: str,
        retry_after: Optional[float] = None,
        url: Optional[str] = None,
        cause: Optional[Exception] = None,
    ):
        super().__init__(
            message,
            code="RATE_LIMITED",
            cause=cause,
            context={"retry_after": retry_after, "url": url},
        )
        self.retry_after = retry_after
        self.url = url


class STTError(BudError):
    """Speech-to-Text error."""

    def __init__(
        self,
        message: str,
        provider: Optional[str] = None,
        cause: Optional[Exception] = None,
    ):
        super().__init__(
            message,
            code="STT_ERROR",
            cause=cause,
            context={"provider": provider},
        )
        self.provider = provider


class TranscriptionError(STTError):
    """Transcription failed."""

    def __init__(
        self,
        message: str,
        audio_duration: Optional[float] = None,
        language: Optional[str] = None,
        provider: Optional[str] = None,
    ):
        super().__init__(message, provider=provider)
        self.code = "TRANSCRIPTION_ERROR"
        self.audio_duration = audio_duration
        self.language = language
        self.context.update({
            "audio_duration": audio_duration,
            "language": language,
        })


class TTSError(BudError):
    """Text-to-Speech error."""

    def __init__(
        self,
        message: str,
        provider: Optional[str] = None,
        cause: Optional[Exception] = None,
    ):
        super().__init__(
            message,
            code="TTS_ERROR",
            cause=cause,
            context={"provider": provider},
        )
        self.provider = provider


class SynthesisError(TTSError):
    """Speech synthesis failed."""

    def __init__(
        self,
        message: str,
        text_length: Optional[int] = None,
        voice: Optional[str] = None,
        provider: Optional[str] = None,
    ):
        super().__init__(message, provider=provider)
        self.code = "SYNTHESIS_ERROR"
        self.text_length = text_length
        self.voice = voice
        self.context.update({
            "text_length": text_length,
            "voice": voice,
        })


class ConfigurationError(BudError):
    """Invalid configuration."""

    def __init__(
        self,
        message: str,
        field: Optional[str] = None,
        value: Optional[Any] = None,
    ):
        super().__init__(
            message,
            code="CONFIG_ERROR",
            context={"field": field, "value": value},
        )
        self.field = field
        self.value = value
