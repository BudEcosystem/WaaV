"""
Tests for DAG routing types and validation.
"""

import pytest

from bud_foundry import (
    DAGNodeType,
    DAGNode,
    DAGEdge,
    DAGDefinition,
    DAGConfig,
    DAGValidationResult,
    validate_dag_definition,
    TEMPLATE_SIMPLE_STT,
    TEMPLATE_SIMPLE_TTS,
    TEMPLATE_VOICE_ASSISTANT,
    BUILTIN_TEMPLATES,
    get_builtin_template,
)


class TestDAGNodeType:
    """Tests for DAG node types."""

    def test_all_node_types_defined(self):
        """All node types should be defined."""
        expected_types = [
            "audio_input",
            "audio_output",
            "stt_provider",
            "tts_provider",
            "llm",
            "http_endpoint",
            "webhook",
            "transform",
        ]
        for node_type in expected_types:
            assert DAGNodeType(node_type).value == node_type


class TestDAGNode:
    """Tests for DAG node model."""

    def test_create_node(self):
        """Should create a node with required fields."""
        node = DAGNode(id="node1", type=DAGNodeType.STT_PROVIDER)
        assert node.id == "node1"
        assert node.type == DAGNodeType.STT_PROVIDER
        assert node.config is None

    def test_create_node_with_config(self):
        """Should create a node with config."""
        node = DAGNode(
            id="stt1",
            type=DAGNodeType.STT_PROVIDER,
            config={"provider": "deepgram", "model": "nova-2"},
        )
        assert node.config == {"provider": "deepgram", "model": "nova-2"}


class TestDAGEdge:
    """Tests for DAG edge model."""

    def test_create_edge(self):
        """Should create an edge with required fields."""
        edge = DAGEdge(from_node="node1", to_node="node2")
        assert edge.from_node == "node1"
        assert edge.to_node == "node2"
        assert edge.condition is None

    def test_create_edge_with_condition(self):
        """Should create an edge with condition."""
        edge = DAGEdge(
            from_node="node1",
            to_node="node2",
            condition="result.confidence > 0.9",
        )
        assert edge.condition == "result.confidence > 0.9"

    def test_edge_alias(self):
        """Should support 'from' and 'to' aliases."""
        # Test with alias
        edge = DAGEdge.model_validate({"from": "node1", "to": "node2"})
        assert edge.from_node == "node1"
        assert edge.to_node == "node2"


class TestDAGDefinition:
    """Tests for DAG definition model."""

    def test_create_definition(self):
        """Should create a definition with required fields."""
        definition = DAGDefinition(
            id="dag1",
            name="Test DAG",
            version="1.0.0",
            nodes=[
                DAGNode(id="input", type=DAGNodeType.AUDIO_INPUT),
                DAGNode(id="stt", type=DAGNodeType.STT_PROVIDER),
            ],
            edges=[
                DAGEdge(from_node="input", to_node="stt"),
            ],
        )
        assert definition.id == "dag1"
        assert definition.name == "Test DAG"
        assert definition.version == "1.0.0"
        assert len(definition.nodes) == 2
        assert len(definition.edges) == 1


class TestDAGConfig:
    """Tests for DAG config model."""

    def test_create_config_with_template(self):
        """Should create config with template name."""
        config = DAGConfig(template="voice_assistant")
        assert config.template == "voice_assistant"
        assert config.definition is None

    def test_create_config_with_definition(self):
        """Should create config with inline definition."""
        definition = DAGDefinition(
            id="custom",
            name="Custom DAG",
            version="1.0.0",
            nodes=[
                DAGNode(id="input", type=DAGNodeType.AUDIO_INPUT),
            ],
            edges=[],
        )
        config = DAGConfig(definition=definition)
        assert config.template is None
        assert config.definition == definition


class TestDAGValidation:
    """Tests for DAG validation."""

    def test_validate_valid_dag(self):
        """Valid DAG should pass validation."""
        definition = DAGDefinition(
            id="valid_dag",
            name="Valid DAG",
            version="1.0.0",
            nodes=[
                DAGNode(id="input", type=DAGNodeType.AUDIO_INPUT),
                DAGNode(id="stt", type=DAGNodeType.STT_PROVIDER),
                DAGNode(id="output", type=DAGNodeType.AUDIO_OUTPUT),
            ],
            edges=[
                DAGEdge(from_node="input", to_node="stt"),
                DAGEdge(from_node="stt", to_node="output"),
            ],
        )
        result = validate_dag_definition(definition)
        assert result.valid is True
        assert len(result.errors) == 0

    def test_validate_empty_dag(self):
        """Empty DAG should have a warning (valid but with warnings)."""
        definition = DAGDefinition(
            id="empty",
            name="Empty DAG",
            version="1.0.0",
            nodes=[],
            edges=[],
        )
        result = validate_dag_definition(definition)
        # Empty DAG is valid but has a warning
        assert result.valid is True
        assert any("no nodes" in w.lower() for w in result.warnings)

    def test_validate_duplicate_node_ids(self):
        """Duplicate node IDs should fail validation."""
        definition = DAGDefinition(
            id="duplicate",
            name="Duplicate DAG",
            version="1.0.0",
            nodes=[
                DAGNode(id="node1", type=DAGNodeType.AUDIO_INPUT),
                DAGNode(id="node1", type=DAGNodeType.STT_PROVIDER),  # Duplicate
            ],
            edges=[],
        )
        result = validate_dag_definition(definition)
        assert result.valid is False
        assert any("duplicate" in e.lower() for e in result.errors)

    def test_validate_missing_edge_nodes(self):
        """Edges referencing non-existent nodes should fail."""
        definition = DAGDefinition(
            id="missing",
            name="Missing Nodes DAG",
            version="1.0.0",
            nodes=[
                DAGNode(id="node1", type=DAGNodeType.AUDIO_INPUT),
            ],
            edges=[
                DAGEdge(from_node="node1", to_node="nonexistent"),
            ],
        )
        result = validate_dag_definition(definition)
        assert result.valid is False
        assert any("nonexistent" in e for e in result.errors)

    def test_validate_cycle_detection(self):
        """Cycles should be detected and fail validation."""
        definition = DAGDefinition(
            id="cycle",
            name="Cycle DAG",
            version="1.0.0",
            nodes=[
                DAGNode(id="a", type=DAGNodeType.AUDIO_INPUT),
                DAGNode(id="b", type=DAGNodeType.STT_PROVIDER),
                DAGNode(id="c", type=DAGNodeType.TTS_PROVIDER),
            ],
            edges=[
                DAGEdge(from_node="a", to_node="b"),
                DAGEdge(from_node="b", to_node="c"),
                DAGEdge(from_node="c", to_node="a"),  # Creates cycle
            ],
        )
        result = validate_dag_definition(definition)
        assert result.valid is False
        assert any("cycle" in e.lower() for e in result.errors)


class TestBuiltinTemplates:
    """Tests for builtin DAG templates."""

    def test_simple_stt_template(self):
        """Simple STT template should be valid."""
        assert TEMPLATE_SIMPLE_STT is not None
        result = validate_dag_definition(TEMPLATE_SIMPLE_STT)
        assert result.valid is True

    def test_simple_tts_template(self):
        """Simple TTS template should be valid."""
        assert TEMPLATE_SIMPLE_TTS is not None
        result = validate_dag_definition(TEMPLATE_SIMPLE_TTS)
        assert result.valid is True

    def test_voice_assistant_template(self):
        """Voice assistant template should be valid."""
        assert TEMPLATE_VOICE_ASSISTANT is not None
        result = validate_dag_definition(TEMPLATE_VOICE_ASSISTANT)
        assert result.valid is True

    def test_builtin_templates_dict(self):
        """All templates should be in BUILTIN_TEMPLATES."""
        # Template keys use hyphens
        assert "simple-stt" in BUILTIN_TEMPLATES
        assert "simple-tts" in BUILTIN_TEMPLATES
        assert "voice-assistant" in BUILTIN_TEMPLATES

    def test_get_builtin_template(self):
        """get_builtin_template should return correct templates."""
        # Template keys use hyphens
        template = get_builtin_template("simple-stt")
        assert template is not None
        assert template.id == "simple-stt"

        template = get_builtin_template("nonexistent")
        assert template is None
