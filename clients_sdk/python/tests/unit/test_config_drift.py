"""SDK ↔ gateway config DRIFT GUARD (SDK_STANDARDIZATION_PLAN P1, deliverable #1).

The non-negotiable P1 deliverable: a test that FAILS if the gateway ``/ws`` config
gains or renames a field the SDK cannot reach. It loads the committed, wire-honest
``gateway/docs/openapi.yaml`` (which ``gateway/tests/openapi_drift.rs`` keeps
byte-identical to the live Rust wire structs) and asserts, per config schema, that
the SDK's declared reachable-property set (``bud_waav.config_mirror.SDK_CONFIG_REACH``)
COVERS every property the schema exposes.

A new gateway field with no SDK mapping → this test fails with the exact missing
dotted path, so "feature exists server-side but unreachable from the SDK" becomes a
build failure instead of a silent no-op.

No third-party YAML dependency: a tiny indentation-based extractor reads just the
``properties:`` keys of each schema (the OpenAPI emitted by utoipa is regular —
4-space schema name, 6-space ``properties:``, 8-space property keys).
"""

from __future__ import annotations

from pathlib import Path

import pytest

from bud_waav.config_mirror import (
    ADAPTER_KINDS,
    SDK_CONFIG_REACH,
)

# gateway/docs/openapi.yaml relative to this test file
# (clients_sdk/python/tests/unit/ -> repo root is 4 parents up).
_REPO_ROOT = Path(__file__).resolve().parents[4]
_OPENAPI_PATH = _REPO_ROOT / "gateway" / "docs" / "openapi.yaml"

# Gateway schema name -> the SDK_CONFIG_REACH key that mirrors it. The envelope
# (IncomingMessage::Config branch) is mapped separately below because it is a
# oneOf branch, not a top-level component schema.
_SCHEMA_TO_REACH = {
    "STTWebSocketConfig": "STTWebSocketConfig",
    "TurnDetectionWsConfig": "TurnDetectionWsConfig",
    "SttFeatures": "SttFeatures",
    "TTSWebSocketConfig": "TTSWebSocketConfig",
    "TtsFeatures": "TtsFeatures",
    "LiveKitWebSocketConfig": "LiveKitWebSocketConfig",
    "DAGWebSocketConfig": "DAGWebSocketConfig",
    "ConversationWebSocketConfig": "ConversationWebSocketConfig",
}


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def extract_schema_properties(text: str, schema: str) -> set[str]:
    """Return the ``properties:`` key names of one component schema.

    Dependency-free reader of the (regular) utoipa-emitted OpenAPI YAML:
    component schemas are 4-space indented, their ``properties:`` map is 6-space
    indented, and each property name is an 8-space-indented ``key:``. We locate
    the schema block, then the ``properties:`` line inside it, then collect the
    keys directly under it until the indentation returns to the properties level
    or shallower.
    """
    lines = text.splitlines()
    # Locate the schema header line: exactly 4-space indent, "<schema>:".
    start = None
    for i, line in enumerate(lines):
        if _indent(line) == 4 and line.strip() == f"{schema}:":
            start = i
            break
    if start is None:
        raise AssertionError(
            f"schema {schema!r} not found in {_OPENAPI_PATH} — did the gateway rename it?"
        )

    # Walk the schema body (indent > 4) to find the 6-space "properties:" line.
    props_line = None
    for i in range(start + 1, len(lines)):
        line = lines[i]
        if line.strip() == "":
            continue
        if _indent(line) <= 4:  # next schema started
            break
        if _indent(line) == 6 and line.strip() == "properties:":
            props_line = i
            break
    if props_line is None:
        return set()

    # Collect 8-space-indented property keys under properties:.
    props: set[str] = set()
    for i in range(props_line + 1, len(lines)):
        line = lines[i]
        if line.strip() == "":
            continue
        ind = _indent(line)
        if ind <= 6:  # left the properties: map
            break
        if ind == 8 and line.rstrip().endswith(":"):
            props.add(line.strip()[:-1])
    return props


def extract_config_envelope_properties(text: str) -> set[str]:
    """Extract the ``config`` branch properties of the ``IncomingMessage`` oneOf.

    The envelope is not a standalone component schema; it is the discriminated
    ``type: config`` branch inside ``IncomingMessage``'s ``oneOf``. We find the
    branch whose ``type`` const is ``config`` and read its ``properties:`` keys.
    """
    lines = text.splitlines()
    # Find the IncomingMessage schema, then scan its oneOf branches for the one
    # whose properties include a `type:` enum/const of "config".
    start = None
    for i, line in enumerate(lines):
        if _indent(line) == 4 and line.strip() == "IncomingMessage:":
            start = i
            break
    assert start is not None, "IncomingMessage schema missing from openapi.yaml"

    end = len(lines)
    for i in range(start + 1, len(lines)):
        if _indent(lines[i]) <= 4 and lines[i].strip():
            end = i
            break
    block = "\n".join(lines[start:end])

    # The config branch is the one mentioning the conversation_config ref (unique
    # to the config envelope). Collect property keys appearing at the branch's
    # property indent within a window around that anchor.
    assert "conversation_config:" in block, (
        "IncomingMessage no longer carries a config branch with conversation_config"
    )

    # Collect every property key that appears as `<8-or-more-space>name:` inside
    # the config branch. The config branch is the oneOf entry containing the
    # config-only fields; we gather keys between the branch start and the next
    # branch (a line beginning a new `- ` list item at the oneOf item indent).
    props: set[str] = set()
    blk_lines = block.splitlines()
    # Find the properties: line that has conversation_config under it.
    for i, line in enumerate(blk_lines):
        if line.strip() == "conversation_config:":
            anchor = i
            break
    else:  # pragma: no cover - guarded by the assert above
        return props
    anchor_ind = _indent(blk_lines[anchor])
    # Walk up to the enclosing `properties:` then collect sibling keys.
    # Collect all keys at anchor_ind within the contiguous run around the anchor.
    j = anchor
    while j >= 0 and (_indent(blk_lines[j]) >= anchor_ind or blk_lines[j].strip() == ""):
        line = blk_lines[j]
        if _indent(line) == anchor_ind and line.rstrip().endswith(":"):
            props.add(line.strip()[:-1])
        j -= 1
    j = anchor + 1
    while j < len(blk_lines) and (
        _indent(blk_lines[j]) >= anchor_ind or blk_lines[j].strip() == ""
    ):
        line = blk_lines[j]
        if _indent(line) == anchor_ind and line.rstrip().endswith(":"):
            props.add(line.strip()[:-1])
        j += 1
    return props


@pytest.fixture(scope="module")
def openapi_text() -> str:
    if not _OPENAPI_PATH.exists():
        pytest.skip(f"committed gateway spec not found at {_OPENAPI_PATH}")
    return _OPENAPI_PATH.read_text()


class TestConfigDriftGuard:
    """SDK config reach must COVER every gateway config-schema property."""

    @pytest.mark.parametrize("schema", sorted(_SCHEMA_TO_REACH))
    def test_sdk_covers_gateway_schema(self, openapi_text: str, schema: str):
        gateway_props = extract_schema_properties(openapi_text, schema)
        assert gateway_props, f"extractor found no properties for {schema} (parser/spec drift)"

        reach = SDK_CONFIG_REACH[_SCHEMA_TO_REACH[schema]]
        missing = gateway_props - reach
        assert not missing, (
            f"DRIFT: gateway schema `{schema}` exposes config propert"
            f"{'y' if len(missing) == 1 else 'ies'} {sorted(missing)} the SDK cannot "
            f"reach. Add {'it' if len(missing) == 1 else 'them'} to "
            f"bud_waav.config_mirror.SDK_CONFIG_REACH['{_SCHEMA_TO_REACH[schema]}'] AND "
            f"serialize {'it' if len(missing) == 1 else 'them'} in ws/session.py / types.py."
        )

    def test_sdk_covers_config_envelope(self, openapi_text: str):
        env_props = extract_config_envelope_properties(openapi_text)
        assert env_props, "could not extract config-envelope properties (parser/spec drift)"
        reach = SDK_CONFIG_REACH["ConfigEnvelope"]
        missing = env_props - reach
        assert not missing, (
            f"DRIFT: the config envelope exposes propert"
            f"{'y' if len(missing) == 1 else 'ies'} {sorted(missing)} the SDK cannot reach. "
            f"Update SDK_CONFIG_REACH['ConfigEnvelope'] and ws/session.py::_send_config."
        )

    def test_reach_keys_have_no_typos_vs_schemas(self, openapi_text: str):
        """Every reach entry must NOT claim a property the gateway schema lacks.

        Coverage allows the SDK to expose convenience fields with no 1:1 gateway
        property (those ride through features/extras) — but a name in a reach set
        that *matches no* gateway property is almost always a typo or a removed
        field, so flag any reach name that is neither a real gateway property nor
        an explicitly-allowed SDK convenience alias.
        """
        # Reach names intentionally NOT 1:1 gateway properties (they are nested
        # block refs or escape hatches that the SDK populates structurally).
        allowed_non_props = {"features", "extras"}
        for schema, reach_key in _SCHEMA_TO_REACH.items():
            gateway_props = extract_schema_properties(openapi_text, schema)
            stray = SDK_CONFIG_REACH[reach_key] - gateway_props - allowed_non_props
            assert not stray, (
                f"reach set for {reach_key} names {sorted(stray)} which the gateway "
                f"schema {schema} no longer exposes — stale mapping (field removed/renamed?)."
            )


class TestAdapterKindUnion:
    """provider_kind / reasoning_provider_kind are nullable string in OpenAPI.

    The typed value space (openai|anthropic|gemini) is hand-authored from
    adapter.rs:40 because utoipa renders it as a plain string. Assert the union
    and that both fields are reachable.
    """

    def test_adapter_kind_values(self):
        assert ADAPTER_KINDS == ("openai", "anthropic", "gemini")

    def test_provider_kind_fields_reachable(self):
        reach = SDK_CONFIG_REACH["ConversationWebSocketConfig"]
        assert "provider_kind" in reach
        assert "reasoning_provider_kind" in reach


class TestExtractorSanity:
    """Guard the dependency-free YAML extractor itself."""

    def test_extractor_reads_known_props(self, openapi_text: str):
        conv = extract_schema_properties(openapi_text, "ConversationWebSocketConfig")
        # The two required fields and a representative reasoning field must parse.
        assert {"base_url", "model", "reasoning_model"} <= conv
        # 29-field flagship surface — assert the count matches the digest so a
        # parser regression (under/over-collecting) is caught.
        assert len(conv) == 29, f"expected 29 conversation props, parsed {len(conv)}: {sorted(conv)}"

    def test_extractor_missing_schema_raises(self, openapi_text: str):
        with pytest.raises(AssertionError):
            extract_schema_properties(openapi_text, "NoSuchSchema__")
