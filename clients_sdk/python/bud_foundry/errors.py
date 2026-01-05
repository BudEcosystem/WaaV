"""
Error classes for bud-foundry SDK
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
