"""Tests for provider types and capabilities (SDK_STANDARDIZATION_PLAN P1 #4).

The SDK provider enums are the FULL gateway dispatch set: 32 STT / 37 TTS / 12
realtime, sourced 1:1 from the gateway dispatch/registry tables. These tests pin
the exact sets (so a stale or partial enum fails) and assert forward-compat: a
bare ``str`` is accepted everywhere a provider is taken, and an uncatalogued
provider is valid but simply has no capability hint (never blocked).
"""


from bud_waav import (
    STTProvider,
    TTSProvider,
    RealtimeProvider,
    STT_PROVIDER_CAPABILITIES,
    TTS_PROVIDER_CAPABILITIES,
    is_valid_stt_provider,
    is_valid_tts_provider,
    is_valid_realtime_provider,
    get_provider_capabilities,
)

# The canonical dispatch sets (gateway plugin/dispatch.rs + core/{stt,tts}/standard.rs
# create_*_standard + core/realtime/mod.rs get_supported_realtime_providers). This is
# the source-of-truth list the SDK enums must mirror exactly.
EXPECTED_STT = {
    "alibaba-cloud", "amivoice", "assemblyai", "aws-transcribe", "baidu", "bhashini",
    "cartesia", "deepgram", "elevenlabs", "fpt-ai", "gladia", "gnani", "google",
    "groq", "huawei-cloud", "ibm-watson", "iflytek", "microsoft-azure", "naver-clova",
    "nectec", "openai", "phonexia", "prosa-ai", "revai", "reverie", "sarvam",
    "sberdevices", "speechmatics", "tencent", "tinkoff", "viettel-ai", "yandex",
}
EXPECTED_TTS = {
    "acapela", "alibaba-cloud", "aws-polly", "baidu", "bhashini", "cartesia",
    "cereproc", "deepgram", "elevenlabs", "fpt-ai", "gnani", "google", "huawei-cloud",
    "hume", "ibm-watson", "iflytek", "lmnt", "microsoft-azure", "murf", "naver-clova",
    "nectec", "openai", "playht", "prosa-ai", "resemble", "reverie", "sberdevices",
    "smallest", "speechify", "speechmatics", "tencent", "tinkoff", "unrealspeech",
    "viettel-ai", "wellsaid", "yandex", "zalo-ai",
}
EXPECTED_REALTIME = {
    "openai", "hume", "azure", "grok", "inworld", "deepgram", "elevenlabs", "gemini",
    "ultravox", "nova_sonic", "speechmatics", "yandex",
}


def _values(enum_cls) -> set[str]:
    """Distinct provider VALUES exposed by iterating the enum (aliases collapse)."""
    return {m.value for m in enum_cls}


class TestSTTProviders:
    """STT provider enum = the full 32-provider gateway dispatch set."""

    def test_full_stt_set_present(self):
        assert _values(STTProvider) == EXPECTED_STT
        assert len(list(STTProvider)) == 32

    def test_representative_enum_values(self):
        assert STTProvider.DEEPGRAM.value == "deepgram"
        assert STTProvider.MICROSOFT_AZURE.value == "microsoft-azure"
        assert STTProvider.SARVAM.value == "sarvam"
        assert STTProvider.OPENAI.value == "openai"
        assert STTProvider.AWS_TRANSCRIBE.value == "aws-transcribe"
        # Providers that were UNNAMEABLE through the old 10-enum are now first-class.
        assert STTProvider.SPEECHMATICS.value == "speechmatics"
        assert STTProvider.IFLYTEK.value == "iflytek"

    def test_every_expected_provider_is_valid(self):
        for provider in EXPECTED_STT:
            assert is_valid_stt_provider(provider), f"{provider} should be valid"

    def test_invalid_stt_provider(self):
        assert is_valid_stt_provider("invalid") is False
        assert is_valid_stt_provider("") is False
        assert is_valid_stt_provider("DEEPGRAM") is False  # case sensitive

    def test_capability_hint_subset_is_consistent(self):
        # The capability dict is a curated REFERENCE subset (not exhaustive); every
        # key it does carry must be a real provider and expose a streaming hint.
        for provider, caps in STT_PROVIDER_CAPABILITIES.items():
            assert provider.value in EXPECTED_STT
            assert "streaming" in caps

    def test_uncatalogued_provider_has_no_hint_but_is_valid(self):
        # A valid provider with no curated entry returns None — never blocked.
        assert is_valid_stt_provider("viettel-ai")
        assert get_provider_capabilities("viettel-ai", "stt") is None


class TestTTSProviders:
    """TTS provider enum = the full 37-provider gateway dispatch set."""

    def test_full_tts_set_present(self):
        assert _values(TTSProvider) == EXPECTED_TTS
        assert len(list(TTSProvider)) == 37

    def test_representative_enum_values(self):
        assert TTSProvider.DEEPGRAM.value == "deepgram"
        assert TTSProvider.ELEVENLABS.value == "elevenlabs"
        assert TTSProvider.MICROSOFT_AZURE.value == "microsoft-azure"
        assert TTSProvider.HUME.value == "hume"
        assert TTSProvider.ZALO_AI.value == "zalo-ai"
        assert TTSProvider.UNREALSPEECH.value == "unrealspeech"

    def test_every_expected_provider_is_valid(self):
        for provider in EXPECTED_TTS:
            assert is_valid_tts_provider(provider), f"{provider} should be valid"

    def test_invalid_tts_provider(self):
        assert is_valid_tts_provider("invalid") is False
        assert is_valid_tts_provider("") is False

    def test_capability_hint_subset_is_consistent(self):
        for provider, caps in TTS_PROVIDER_CAPABILITIES.items():
            assert provider.value in EXPECTED_TTS
            assert "streaming" in caps


class TestRealtimeProviders:
    """Realtime provider enum = the full 12-provider gateway realtime registry."""

    def test_full_realtime_set_present(self):
        assert _values(RealtimeProvider) == EXPECTED_REALTIME
        assert len(list(RealtimeProvider)) == 12

    def test_gateway_native_tokens(self):
        # The realtime names are the bare vendor tokens /realtime accepts.
        assert RealtimeProvider.OPENAI.value == "openai"
        assert RealtimeProvider.HUME.value == "hume"
        assert RealtimeProvider.GEMINI.value == "gemini"
        assert RealtimeProvider.NOVA_SONIC.value == "nova_sonic"

    def test_backward_compat_aliases_resolve(self):
        # Old pre-P1 names remain importable but map to the gateway-native tokens.
        assert RealtimeProvider.OPENAI_REALTIME is RealtimeProvider.OPENAI
        assert RealtimeProvider.HUME_EVI is RealtimeProvider.HUME
        assert RealtimeProvider.OPENAI_REALTIME.value == "openai"

    def test_every_expected_provider_is_valid(self):
        for provider in EXPECTED_REALTIME:
            assert is_valid_realtime_provider(provider), f"{provider} should be valid"
        # The bare vendor tokens are now valid (they were rejected pre-P1).
        assert is_valid_realtime_provider("openai")
        assert is_valid_realtime_provider("hume")

    def test_invalid_realtime_provider(self):
        assert is_valid_realtime_provider("invalid") is False
        # The OLD alias strings are NOT gateway tokens and are no longer valid.
        assert is_valid_realtime_provider("openai-realtime") is False
        assert is_valid_realtime_provider("hume-evi") is False


class TestProviderCapabilities:
    """get_provider_capabilities: curated hint for known providers, None otherwise."""

    def test_get_stt_capabilities(self):
        caps = get_provider_capabilities("deepgram", "stt")
        assert caps is not None
        assert caps["streaming"] is True
        assert caps["diarization"] is True

    def test_get_tts_capabilities(self):
        caps = get_provider_capabilities("elevenlabs", "tts")
        assert caps is not None
        assert caps["streaming"] is True

    def test_get_realtime_capabilities(self):
        # Gateway-native token (NOT the old "openai-realtime" alias).
        caps = get_provider_capabilities("openai", "realtime")
        assert caps is not None
        assert caps["function_calling"] is True

    def test_invalid_provider_returns_none(self):
        assert get_provider_capabilities("invalid", "stt") is None

    def test_invalid_category_returns_none(self):
        assert get_provider_capabilities("deepgram", "invalid") is None


class TestProviderEnumDriftGuard:
    """Pin the enum sets so a partial/stale regeneration fails loudly.

    This mirrors the gateway dispatch counts. If the gateway adds a provider, the
    PROVIDER_DRIFT note in the enum docstrings and these expected-sets must be
    updated together — keeping the SDK's nameable surface honest with the server.
    """

    def test_counts_match_gateway_dispatch(self):
        assert len(EXPECTED_STT) == 32
        assert len(EXPECTED_TTS) == 37
        assert len(EXPECTED_REALTIME) == 12

    def test_enums_have_no_duplicate_values(self):
        # Iteration must yield exactly the expected count with no accidental dupes.
        assert len({m.value for m in STTProvider}) == len(list(STTProvider))
        assert len({m.value for m in TTSProvider}) == len(list(TTSProvider))
        assert len({m.value for m in RealtimeProvider}) == len(list(RealtimeProvider))
