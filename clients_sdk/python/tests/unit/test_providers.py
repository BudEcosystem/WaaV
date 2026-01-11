"""
Tests for provider types and capabilities.
"""

import pytest

from bud_foundry import (
    STTProvider,
    TTSProvider,
    RealtimeProvider,
    STT_PROVIDER_CAPABILITIES,
    TTS_PROVIDER_CAPABILITIES,
    REALTIME_PROVIDER_CAPABILITIES,
    is_valid_stt_provider,
    is_valid_tts_provider,
    is_valid_realtime_provider,
    get_provider_capabilities,
)


class TestSTTProviders:
    """Tests for STT provider types."""

    def test_all_stt_providers_defined(self):
        """All 10 STT providers should be defined."""
        expected_providers = [
            "deepgram",
            "google",
            "azure",
            "cartesia",
            "gateway",
            "assemblyai",
            "aws-transcribe",
            "ibm-watson",
            "groq",
            "openai-whisper",
        ]
        for provider in expected_providers:
            assert is_valid_stt_provider(provider), f"Provider {provider} should be valid"

    def test_stt_provider_enum_values(self):
        """STT provider enum should have correct values."""
        assert STTProvider.DEEPGRAM.value == "deepgram"
        assert STTProvider.GOOGLE.value == "google"
        assert STTProvider.AZURE.value == "azure"
        assert STTProvider.CARTESIA.value == "cartesia"
        assert STTProvider.GATEWAY.value == "gateway"
        assert STTProvider.ASSEMBLYAI.value == "assemblyai"
        assert STTProvider.AWS_TRANSCRIBE.value == "aws-transcribe"
        assert STTProvider.IBM_WATSON.value == "ibm-watson"
        assert STTProvider.GROQ.value == "groq"
        assert STTProvider.OPENAI_WHISPER.value == "openai-whisper"

    def test_invalid_stt_provider(self):
        """Invalid provider should return False."""
        assert is_valid_stt_provider("invalid") is False
        assert is_valid_stt_provider("") is False
        assert is_valid_stt_provider("DEEPGRAM") is False  # Case sensitive

    def test_stt_provider_capabilities(self):
        """Each STT provider should have capabilities defined."""
        for provider in STTProvider:
            assert provider in STT_PROVIDER_CAPABILITIES
            caps = STT_PROVIDER_CAPABILITIES[provider]
            assert "streaming" in caps
            # These are the actual capability keys
            assert "diarization" in caps or "models" in caps


class TestTTSProviders:
    """Tests for TTS provider types."""

    def test_all_tts_providers_defined(self):
        """All 12 TTS providers should be defined."""
        expected_providers = [
            "deepgram",
            "elevenlabs",
            "google",
            "azure",
            "cartesia",
            "openai",
            "aws-polly",
            "ibm-watson",
            "hume",
            "lmnt",
            "playht",
            "kokoro",
        ]
        for provider in expected_providers:
            assert is_valid_tts_provider(provider), f"Provider {provider} should be valid"

    def test_tts_provider_enum_values(self):
        """TTS provider enum should have correct values."""
        assert TTSProvider.DEEPGRAM.value == "deepgram"
        assert TTSProvider.ELEVENLABS.value == "elevenlabs"
        assert TTSProvider.GOOGLE.value == "google"
        assert TTSProvider.AZURE.value == "azure"
        assert TTSProvider.CARTESIA.value == "cartesia"
        assert TTSProvider.OPENAI.value == "openai"
        assert TTSProvider.AWS_POLLY.value == "aws-polly"
        assert TTSProvider.IBM_WATSON.value == "ibm-watson"
        assert TTSProvider.HUME.value == "hume"
        assert TTSProvider.LMNT.value == "lmnt"
        assert TTSProvider.PLAYHT.value == "playht"
        assert TTSProvider.KOKORO.value == "kokoro"

    def test_invalid_tts_provider(self):
        """Invalid provider should return False."""
        assert is_valid_tts_provider("invalid") is False
        assert is_valid_tts_provider("") is False

    def test_tts_provider_capabilities(self):
        """Each TTS provider should have capabilities defined."""
        for provider in TTSProvider:
            assert provider in TTS_PROVIDER_CAPABILITIES
            caps = TTS_PROVIDER_CAPABILITIES[provider]
            assert "streaming" in caps
            # These are the actual capability keys
            assert "ssml" in caps or "models" in caps


class TestRealtimeProviders:
    """Tests for Realtime provider types."""

    def test_realtime_providers_defined(self):
        """Both realtime providers should be defined."""
        assert is_valid_realtime_provider("openai-realtime")
        assert is_valid_realtime_provider("hume-evi")

    def test_realtime_provider_enum_values(self):
        """Realtime provider enum should have correct values."""
        assert RealtimeProvider.OPENAI_REALTIME.value == "openai-realtime"
        assert RealtimeProvider.HUME_EVI.value == "hume-evi"

    def test_invalid_realtime_provider(self):
        """Invalid provider should return False."""
        assert is_valid_realtime_provider("invalid") is False
        assert is_valid_realtime_provider("openai") is False
        assert is_valid_realtime_provider("hume") is False

    def test_realtime_provider_capabilities(self):
        """Each realtime provider should have capabilities defined."""
        for provider in RealtimeProvider:
            assert provider in REALTIME_PROVIDER_CAPABILITIES
            caps = REALTIME_PROVIDER_CAPABILITIES[provider]
            # These are the actual capability keys
            assert "function_calling" in caps
            assert "models" in caps


class TestProviderCapabilities:
    """Tests for get_provider_capabilities function."""

    def test_get_stt_capabilities(self):
        """Should get STT provider capabilities."""
        caps = get_provider_capabilities("deepgram", "stt")
        assert caps is not None
        assert caps["streaming"] is True
        assert caps["diarization"] is True

    def test_get_tts_capabilities(self):
        """Should get TTS provider capabilities."""
        caps = get_provider_capabilities("elevenlabs", "tts")
        assert caps is not None
        assert caps["streaming"] is True

    def test_get_realtime_capabilities(self):
        """Should get realtime provider capabilities."""
        caps = get_provider_capabilities("openai-realtime", "realtime")
        assert caps is not None
        assert caps["function_calling"] is True

    def test_invalid_provider_returns_none(self):
        """Invalid provider should return None."""
        caps = get_provider_capabilities("invalid", "stt")
        assert caps is None

    def test_invalid_category_returns_none(self):
        """Invalid category should return None."""
        caps = get_provider_capabilities("deepgram", "invalid")
        assert caps is None
