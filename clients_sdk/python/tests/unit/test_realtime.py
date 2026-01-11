"""
Tests for BudRealtime pipeline.
"""

import pytest

from bud_foundry.pipelines.realtime import (
    BudRealtime,
    RealtimeConfig,
    RealtimeProvider,
    RealtimeState,
    ToolDefinition,
    FunctionCallEvent,
    TranscriptEvent,
    AudioEvent,
    EmotionEvent,
    StateChangeEvent,
    TurnDetectionConfig,
)


class TestRealtimeConfig:
    """Tests for RealtimeConfig dataclass."""

    def test_create_openai_config(self):
        """Should create config for OpenAI Realtime."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
            system_prompt="You are a helpful assistant.",
        )
        assert config.provider == RealtimeProvider.OPENAI_REALTIME
        assert config.api_key == "test-key"
        assert config.system_prompt == "You are a helpful assistant."

    def test_create_hume_config(self):
        """Should create config for Hume EVI."""
        config = RealtimeConfig(
            provider=RealtimeProvider.HUME_EVI,
            api_key="test-key",
            evi_version="3",
            verbose_transcription=True,
        )
        assert config.provider == RealtimeProvider.HUME_EVI
        assert config.evi_version == "3"
        assert config.verbose_transcription is True

    def test_turn_detection_config(self):
        """Should accept turn detection configuration."""
        turn_detection = TurnDetectionConfig(
            enabled=True,
            threshold=0.5,
            silence_ms=500,
        )
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
            turn_detection=turn_detection,
        )
        assert config.turn_detection is not None
        assert config.turn_detection.enabled is True
        assert config.turn_detection.threshold == 0.5


class TestToolDefinition:
    """Tests for ToolDefinition dataclass."""

    def test_create_tool(self):
        """Should create a tool definition."""
        tool = ToolDefinition(
            name="get_weather",
            description="Get the current weather",
            parameters={
                "type": "object",
                "properties": {
                    "location": {"type": "string", "description": "City name"},
                },
                "required": ["location"],
            },
        )
        assert tool.name == "get_weather"
        assert tool.description == "Get the current weather"
        assert tool.type == "function"


class TestEventDataclasses:
    """Tests for event dataclasses."""

    def test_function_call_event(self):
        """Should create function call event."""
        event = FunctionCallEvent(
            name="get_weather",
            arguments={"location": "Tokyo"},
            call_id="call_123",
        )
        assert event.name == "get_weather"
        assert event.arguments == {"location": "Tokyo"}
        assert event.call_id == "call_123"

    def test_transcript_event(self):
        """Should create transcript event."""
        event = TranscriptEvent(
            text="Hello, how can I help you?",
            is_final=True,
            role="assistant",
        )
        assert event.text == "Hello, how can I help you?"
        assert event.is_final is True
        assert event.role == "assistant"

    def test_audio_event(self):
        """Should create audio event."""
        event = AudioEvent(audio=b"\x00\x01\x02", sample_rate=24000)
        assert event.audio == b"\x00\x01\x02"
        assert event.sample_rate == 24000

    def test_emotion_event(self):
        """Should create emotion event."""
        event = EmotionEvent(
            emotions={"happy": 0.8, "sad": 0.1, "neutral": 0.1},
            dominant="happy",
            confidence=0.8,
        )
        assert event.emotions["happy"] == 0.8
        assert event.dominant == "happy"
        assert event.confidence == 0.8

    def test_state_change_event(self):
        """Should create state change event."""
        event = StateChangeEvent(
            previous_state=RealtimeState.CONNECTING,
            current_state=RealtimeState.CONNECTED,
        )
        assert event.previous_state == RealtimeState.CONNECTING
        assert event.current_state == RealtimeState.CONNECTED


class TestBudRealtimeInitialization:
    """Tests for BudRealtime initialization."""

    def test_create_with_openai(self):
        """Should create instance with OpenAI provider."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)
        assert realtime.provider == RealtimeProvider.OPENAI_REALTIME
        assert realtime.state == RealtimeState.DISCONNECTED
        assert realtime.connected is False

    def test_create_with_hume(self):
        """Should create instance with Hume provider."""
        config = RealtimeConfig(
            provider=RealtimeProvider.HUME_EVI,
            api_key="test-key",
        )
        realtime = BudRealtime(config)
        assert realtime.provider == RealtimeProvider.HUME_EVI

    def test_default_model_openai(self):
        """Should set default model for OpenAI."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)
        assert realtime.config.model == "gpt-4o-realtime-preview"

    def test_default_evi_version_hume(self):
        """Should set default EVI version for Hume."""
        config = RealtimeConfig(
            provider=RealtimeProvider.HUME_EVI,
            api_key="test-key",
        )
        realtime = BudRealtime(config)
        assert realtime.config.evi_version == "3"


class TestBudRealtimeEventHandlers:
    """Tests for event handling."""

    def test_register_handler(self):
        """Should register event handlers."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        handler_called = []

        def handler(event):
            handler_called.append(event)

        realtime.on("transcript", handler)
        realtime._emit(
            "transcript",
            TranscriptEvent(text="test", is_final=True),
        )

        assert len(handler_called) == 1
        assert handler_called[0].text == "test"

    def test_unregister_handler(self):
        """Should unregister event handlers."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        handler_called = []

        def handler(event):
            handler_called.append(event)

        realtime.on("transcript", handler)
        realtime.off("transcript", handler)
        realtime._emit(
            "transcript",
            TranscriptEvent(text="test", is_final=True),
        )

        assert len(handler_called) == 0

    def test_unregister_all_handlers(self):
        """Should unregister all handlers for an event."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        handler_called = []

        realtime.on("transcript", lambda e: handler_called.append(1))
        realtime.on("transcript", lambda e: handler_called.append(2))
        realtime.off("transcript")  # Remove all
        realtime._emit(
            "transcript",
            TranscriptEvent(text="test", is_final=True),
        )

        assert len(handler_called) == 0


class TestBudRealtimeTools:
    """Tests for tool management."""

    @pytest.mark.asyncio
    async def test_add_tool(self):
        """Should add tools."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        tool = ToolDefinition(
            name="test_tool",
            description="A test tool",
            parameters={"type": "object", "properties": {}},
        )

        await realtime.add_tool(tool)

        assert len(realtime.tools) == 1
        assert realtime.tools[0].name == "test_tool"

    @pytest.mark.asyncio
    async def test_remove_tool(self):
        """Should remove tools by name."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        realtime._tools = [
            ToolDefinition(
                name="tool1",
                description="Tool 1",
                parameters={},
            ),
            ToolDefinition(
                name="tool2",
                description="Tool 2",
                parameters={},
            ),
        ]

        await realtime.remove_tool("tool1")

        assert len(realtime.tools) == 1
        assert realtime.tools[0].name == "tool2"

    def test_tools_list_is_copy(self):
        """tools property should return a copy."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        realtime._tools = [
            ToolDefinition(
                name="tool1",
                description="Tool 1",
                parameters={},
            ),
        ]

        tools = realtime.tools
        tools.append(
            ToolDefinition(name="added", description="", parameters={})
        )

        # Original should be unchanged
        assert len(realtime._tools) == 1


class TestBudRealtimeStateManagement:
    """Tests for state management."""

    def test_initial_state(self):
        """Initial state should be disconnected."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)
        assert realtime.state == RealtimeState.DISCONNECTED

    def test_state_change_event(self):
        """Should emit state change events."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        state_changes = []

        def handler(event):
            state_changes.append(event)

        realtime.on("state_change", handler)
        realtime._set_state(RealtimeState.CONNECTING)

        assert len(state_changes) == 1
        assert state_changes[0].previous_state == RealtimeState.DISCONNECTED
        assert state_changes[0].current_state == RealtimeState.CONNECTING


class TestBudRealtimeMessageHandling:
    """Tests for message handling."""

    def test_handle_openai_audio_delta(self):
        """Should handle OpenAI audio delta messages."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        audio_events = []
        realtime.on("audio", lambda e: audio_events.append(e))

        import base64

        audio_data = b"\x00\x01\x02\x03"
        message = {
            "type": "response.audio.delta",
            "delta": base64.b64encode(audio_data).decode("utf-8"),
        }

        realtime._handle_openai_message("response.audio.delta", message)

        assert len(audio_events) == 1
        assert audio_events[0].audio == audio_data

    def test_handle_openai_transcript(self):
        """Should handle OpenAI transcript messages."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        transcript_events = []
        realtime.on("transcript", lambda e: transcript_events.append(e))

        message = {
            "type": "response.audio_transcript.done",
            "transcript": "Hello, world!",
        }

        realtime._handle_openai_message("response.audio_transcript.done", message)

        assert len(transcript_events) == 1
        assert transcript_events[0].text == "Hello, world!"
        assert transcript_events[0].is_final is True
        assert transcript_events[0].role == "assistant"

    def test_handle_hume_audio(self):
        """Should handle Hume audio messages."""
        config = RealtimeConfig(
            provider=RealtimeProvider.HUME_EVI,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        audio_events = []
        realtime.on("audio", lambda e: audio_events.append(e))

        import base64

        audio_data = b"\x00\x01\x02\x03"
        message = {
            "type": "audio",
            "data": base64.b64encode(audio_data).decode("utf-8"),
        }

        realtime._handle_hume_message("audio", message)

        assert len(audio_events) == 1
        assert audio_events[0].audio == audio_data

    def test_handle_hume_emotion(self):
        """Should handle Hume emotion messages."""
        config = RealtimeConfig(
            provider=RealtimeProvider.HUME_EVI,
            api_key="test-key",
        )
        realtime = BudRealtime(config)

        emotion_events = []
        realtime.on("emotion", lambda e: emotion_events.append(e))

        message = {
            "type": "assistant_message",
            "message": {"content": "Hello!"},
            "models": {
                "prosody": {
                    "scores": {"happy": 0.8, "sad": 0.1, "neutral": 0.1}
                }
            },
        }

        realtime._handle_hume_message("assistant_message", message)

        assert len(emotion_events) == 1
        assert emotion_events[0].dominant == "happy"
        assert emotion_events[0].confidence == 0.8
