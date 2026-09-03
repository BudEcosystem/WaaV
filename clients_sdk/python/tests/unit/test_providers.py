"""Tests for provider types and capabilities (SDK_STANDARDIZATION_PLAN P1 #4).

The SDK provider enums are the FULL gateway dispatch set: 32 STT / 37 TTS / 12
realtime, sourced 1:1 from the gateway dispatch/registry tables. These tests pin
the exact sets (so a stale or partial enum fails) and assert forward-compat: a
bare ``str`` is accepted everywhere a provider is taken, and an uncatalogued
provider is valid but simply has no capability hint (never blocked).
"""


import re
from pathlib import Path

from bud_waav import (
    STTProvider,
    TTSProvider,
    RealtimeProvider,
    CANONICAL_LANGUAGES,
    STT_PROVIDER_CAPABILITIES,
    TTS_PROVIDER_CAPABILITIES,
    is_valid_stt_provider,
    is_valid_tts_provider,
    is_valid_realtime_provider,
    get_provider_capabilities,
)


def _gateway_canonical_languages() -> list[str]:
    """Parse the gateway's ``CanonicalLanguage::all()`` BCP-47 set from ``types.rs``.

    Walks up from this test file to the repo root, reads the enum's ``as_bcp47`` arms (the
    authoritative ``variant => "ll-RR"`` mapping), and returns every non-``auto`` BCP-47 string.
    Returns ``[]`` if the gateway source isn't checked out alongside the SDK (skip, don't fail).
    """
    here = Path(__file__).resolve()
    types_rs = None
    for parent in here.parents:
        cand = parent / "gateway" / "src" / "core" / "lang" / "types.rs"
        if cand.exists():
            types_rs = cand
            break
    if types_rs is None:
        return []
    text = types_rs.read_text()
    # The as_bcp47 match arms: `EnUs => "en-US",` — collect the quoted BCP-47 strings, skip "auto".
    body = re.search(r"pub const fn as_bcp47\(&self\).*?\{(.*?)\n    \}", text, re.S)
    assert body, "could not locate as_bcp47 in gateway types.rs"
    codes = re.findall(r'=>\s*"([a-z]{2,3}-[A-Z]{2})"', body.group(1))
    return codes


class TestCanonicalLanguages:
    """P2 unified-language value space: the SDK mirror must match the gateway 1:1."""

    def test_no_duplicates_and_region_qualified(self):
        assert len(CANONICAL_LANGUAGES) == len(set(CANONICAL_LANGUAGES))
        for code in CANONICAL_LANGUAGES:
            # region-qualified ll-RR (lowercase lang subtag + UPPERCASE region)
            assert re.fullmatch(r"[a-z]{2,3}-[A-Z]{2}", code), code

    def test_key_tokens_present(self):
        # The headline tokens the research calls out, incl. the cmn/yue Chinese fork and Odia.
        for code in ["en-US", "en-GB", "en-IN", "cmn-CN", "cmn-TW", "yue-HK", "or-IN", "nb-NO"]:
            assert code in CANONICAL_LANGUAGES, code
        # Chinese canonical uses cmn/yue, NOT zh-*.
        assert not any(c.startswith("zh-") for c in CANONICAL_LANGUAGES)

    def test_matches_gateway_all(self):
        gateway = _gateway_canonical_languages()
        if not gateway:
            import pytest

            pytest.skip("gateway source not available alongside SDK")
        assert sorted(CANONICAL_LANGUAGES) == sorted(gateway), (
            "SDK CANONICAL_LANGUAGES drifted from gateway CanonicalLanguage::all(); "
            f"SDK-only={set(CANONICAL_LANGUAGES) - set(gateway)}, "
            f"gateway-only={set(gateway) - set(CANONICAL_LANGUAGES)}"
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


class TestLanguageCapabilities:
    """P2 capabilities accessor: the SDK exposes the canonical language value
    space + per-provider native-notation summary, replacing stale whitelists."""

    def test_capabilities_exposes_full_canonical_set(self):
        from bud_waav import language_capabilities

        caps = language_capabilities("deepgram")
        # Identity-BCP47 provider that auto-detects.
        assert caps["provider"] == "deepgram"
        assert caps["notation"] == "bcp47"
        assert caps["auto_detect"] is True
        # The FULL canonical value space is offered for every provider (the gateway maps it).
        assert sorted(caps["canonical_languages"]) == sorted(CANONICAL_LANGUAGES)

    def test_capabilities_notation_kinds(self):
        from bud_waav import language_capabilities

        # The headline notation forks the research calls out.
        assert language_capabilities("elevenlabs")["notation"] == "iso6391"  # downgrade
        assert language_capabilities("iflytek")["notation"] == "underscore"
        assert language_capabilities("baidu")["notation"] == "model-id"
        assert language_capabilities("tencent")["notation"] == "composite"
        assert language_capabilities("hume")["notation"] == "none"

    def test_capabilities_uncatalogued_provider_safe_default(self):
        from bud_waav import language_capabilities

        # A valid-but-uncatalogued provider is never blocked — safe BCP-47 default.
        caps = language_capabilities("some-brand-new-provider")
        assert caps["notation"] == "bcp47"
        assert caps["auto_detect"] is False
        assert sorted(caps["canonical_languages"]) == sorted(CANONICAL_LANGUAGES)

    def test_client_capabilities_method(self):
        from bud_waav import BudClient

        # The convenience accessor on the client returns the same shape (static,
        # no connection needed — the documented stand-in for /capabilities).
        caps = BudClient.capabilities("elevenlabs")
        assert caps["provider"] == "elevenlabs"
        assert caps["notation"] == "iso6391"


class TestLanguagePassthrough:
    """P2 backward-compat: the SDK does NOT map language client-side — the
    ``language`` field flows through ``_send_config`` UNCHANGED (the gateway maps
    it). A canonical token, an alias, and an arbitrary raw string all pass verbatim."""

    @staticmethod
    async def _captured_config(language: str) -> dict:
        import json
        from unittest.mock import MagicMock
        from bud_waav.types import STTConfig, TTSConfig
        from bud_waav.ws.session import WebSocketSession

        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram", language=language),
            tts_config=TTSConfig(provider="deepgram"),
        )
        sent: list[str] = []

        async def mock_send(data):
            sent.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send
        await session._send_config()
        assert len(sent) == 1
        return json.loads(sent[0])

    def test_canonical_token_passes_through_unchanged(self):
        import asyncio

        config = asyncio.run(self._captured_config("en-US"))
        # Verbatim — the SDK must NOT downgrade/normalize; the gateway does.
        assert config["stt_config"]["language"] == "en-US"

    def test_alias_token_passes_through_unchanged(self):
        import asyncio

        # The live-proven reversed `us-en` is forwarded as-is; the GATEWAY folds it.
        config = asyncio.run(self._captured_config("us-en"))
        assert config["stt_config"]["language"] == "us-en"

    def test_arbitrary_raw_string_passes_through(self):
        import asyncio

        # Forward-compat: a not-yet-known token still reaches the wire (the gateway
        # surfaces a config_warning rather than the SDK rejecting it).
        config = asyncio.run(self._captured_config("brand-new-locale-x"))
        assert config["stt_config"]["language"] == "brand-new-locale-x"
