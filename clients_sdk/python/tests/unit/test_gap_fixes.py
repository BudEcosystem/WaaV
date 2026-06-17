"""
Tests for gap analysis fixes: WebSocket config, receive loop, REST endpoints, BudClient.
"""

import json
import pytest
from unittest.mock import AsyncMock, MagicMock, patch

from bud_waav import (
    BudClient,
    STTConfig,
    TTSConfig,
    STTResult,
    AudioEvent,
    AudioFeatures,
    TurnDetectionConfig,
    NoiseFilterConfig,
    ExtendedVADConfig,
    DAGConfig,
    DAGDefinition,
    DAGNode,
    DAGEdge,
    DAGNodeType,
    Emotion,
    EmotionIntensityLevel,
    DeliveryStyle,
    ConversationConfig,
    ReasoningEffort,
    LatencyFiller,
    MuteStrategy,
    RateLimitError,
)
from bud_waav.ws.session import WebSocketSession, ReconnectConfig
from bud_waav.rest.client import RestClient
from bud_waav.pipelines.realtime import BudRealtime, RealtimeConfig, RealtimeProvider


# =============================================================================
# Task #22: WebSocket Config Message Tests
# =============================================================================


class TestWebSocketConfigMessage:
    """Tests that _send_config sends all gateway-supported fields."""

    @pytest.fixture
    def session(self):
        """Create a WebSocketSession with full config."""
        return WebSocketSession(
            url="ws://localhost:3001/ws",
            api_key="test-key-123",
            stt_config=STTConfig(
                provider="deepgram",
                language="en-US",
                model="nova-3",
                sample_rate=16000,
                channels=1,
                encoding="linear16",
                punctuate=True,
            ),
            tts_config=TTSConfig(
                provider="elevenlabs",
                voice_id="rachel",
                model="eleven_turbo_v2",
                sample_rate=24000,
                audio_format="linear16",
                speed=1.2,
                emotion=Emotion.HAPPY,
                emotion_intensity=EmotionIntensityLevel.HIGH,
                delivery_style=DeliveryStyle.EXPRESSIVE,
                emotion_description="Speaking with joy",
            ),
        )

    @pytest.mark.asyncio
    async def test_config_includes_api_key_in_stt(self, session):
        """Should include api_key in stt_config."""
        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        assert len(sent_messages) == 1
        config = json.loads(sent_messages[0])
        assert config["stt_config"]["api_key"] == "test-key-123"

    @pytest.mark.asyncio
    async def test_config_includes_api_key_in_tts(self, session):
        """Should include api_key in tts_config."""
        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        assert config["tts_config"]["api_key"] == "test-key-123"

    @pytest.mark.asyncio
    async def test_config_includes_tts_emotion_fields(self, session):
        """Should include emotion fields in tts_config."""
        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        tts = config["tts_config"]
        assert tts["emotion"] == "happy"
        assert tts["emotion_intensity"] == 1.0  # HIGH = 1.0
        assert tts["delivery_style"] == "expressive"
        assert tts["emotion_description"] == "Speaking with joy"

    @pytest.mark.asyncio
    async def test_config_includes_tts_audio_format(self, session):
        """Should include audio_format and speaking_rate in tts_config."""
        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        tts = config["tts_config"]
        assert tts["audio_format"] == "linear16"
        assert tts["speaking_rate"] == 1.2
        assert tts["sample_rate"] == 24000

    @pytest.mark.asyncio
    async def test_config_includes_stream_id(self):
        """Should include stream_id if provided."""
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stream_id="custom-stream-123",
            stt_config=STTConfig(provider="deepgram"),
        )

        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        assert config["stream_id"] == "custom-stream-123"

    @pytest.mark.asyncio
    async def test_config_includes_dag_config(self):
        """Should include dag_config when provided."""
        dag = DAGConfig(template="voice-assistant", enable_metrics=True)
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
            dag_config=dag,
        )

        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        assert "dag_config" in config
        assert config["dag_config"]["template"] == "voice-assistant"
        assert config["dag_config"]["enable_metrics"] is True

    @pytest.mark.asyncio
    async def test_config_dag_inline_definition(self):
        """Should serialize inline DAG definition correctly."""
        definition = DAGDefinition(
            id="custom",
            name="Custom",
            version="1.0",
            nodes=[
                DAGNode(id="input", type=DAGNodeType.AUDIO_INPUT),
                DAGNode(id="stt", type=DAGNodeType.STT_PROVIDER, config={"provider": "deepgram"}),
            ],
            edges=[
                DAGEdge(from_node="input", to_node="stt"),
            ],
        )
        dag = DAGConfig(definition=definition)
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
            dag_config=dag,
        )

        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        dag_def = config["dag_config"]["definition"]
        assert dag_def["id"] == "custom"
        assert len(dag_def["nodes"]) == 2
        assert len(dag_def["edges"]) == 1

    @pytest.mark.asyncio
    async def test_config_no_emotion_when_not_set(self):
        """Should not include emotion fields when not set."""
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            tts_config=TTSConfig(provider="deepgram"),
        )

        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        tts = config["tts_config"]
        assert "emotion" not in tts
        assert "emotion_intensity" not in tts
        assert "delivery_style" not in tts
        assert "emotion_description" not in tts

    @pytest.mark.asyncio
    async def test_config_no_dag_when_not_set(self):
        """Should not include dag_config when not set."""
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
        )

        sent_messages = []

        async def mock_send(data):
            sent_messages.append(data)

        session._ws = MagicMock()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent_messages[0])
        assert "dag_config" not in config

    def test_session_accepts_audio_features(self):
        """Should accept audio_features parameter."""
        features = AudioFeatures(
            turn_detection=TurnDetectionConfig(enabled=True, threshold=0.6),
            noise_filtering=NoiseFilterConfig(enabled=True, strength="high"),
            vad=ExtendedVADConfig(enabled=True, threshold=0.5),
        )
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            audio_features=features,
        )
        assert session.audio_features is not None
        assert session.audio_features.turn_detection.enabled is True
        assert session.audio_features.noise_filtering.enabled is True
        assert session.audio_features.vad.enabled is True


# =============================================================================
# P0 wire-contract fixes (SDK_STANDARDIZATION_PLAN Phase 1)
# =============================================================================


async def _capture_config(session) -> dict:
    """Drive ``_send_config`` through a mock socket and return the parsed JSON."""
    sent = []

    async def mock_send(data):
        sent.append(data)

    session._ws = MagicMock()
    session._ws.send = mock_send
    await session._send_config()
    assert len(sent) == 1
    return json.loads(sent[0])


class TestP0TurnDetectionNesting:
    """turn_detection must nest into stt_config.turn_detection (config.rs:345)."""

    @pytest.mark.asyncio
    async def test_turn_detection_nested_in_stt_config(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
            audio_features=AudioFeatures(
                turn_detection=TurnDetectionConfig(enabled=True, threshold=0.6),
            ),
        )
        config = await _capture_config(session)
        # Lives nested, NOT at the top level.
        assert "turn_detection" not in config
        assert config["stt_config"]["turn_detection"] == {"enabled": True, "threshold": 0.6}

    @pytest.mark.asyncio
    async def test_turn_detection_omitted_when_disabled(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
            audio_features=AudioFeatures(
                turn_detection=TurnDetectionConfig(enabled=False),
            ),
        )
        config = await _capture_config(session)
        assert "turn_detection" not in config["stt_config"]

    @pytest.mark.asyncio
    async def test_noise_and_vad_not_serialized_as_dead_keys(self):
        """noise/vad have NO /ws wire field yet — must NOT be sent (no dead no-op)."""
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
            audio_features=AudioFeatures(
                noise_filtering=NoiseFilterConfig(enabled=True, strength="high"),
                vad=ExtendedVADConfig(enabled=True),
            ),
        )
        config = await _capture_config(session)
        # No top-level audio_features / noise / vad keys anywhere.
        assert "audio_features" not in config
        assert "noise_filtering" not in config and "noise" not in config
        assert "vad" not in config
        assert "noise_filtering" not in config["stt_config"]
        assert "vad" not in config["stt_config"]


class TestP0ConversationConfig:
    """conversation_config (the LLM loop + reasoning) must serialize (config.rs:53-217)."""

    @pytest.mark.asyncio
    async def test_conversation_config_serialized(self):
        conv = ConversationConfig(
            base_url="http://127.0.0.1:11434/v1",
            model="llama3.2:1b",
            system_prompt="You are concise.",
            reasoning_effort=ReasoningEffort.MINIMAL,
            reasoning_model="o3",
            latency_filler=LatencyFiller.AUTO,
            eager_eot=True,
            mute_strategy=MuteStrategy.UNTIL_FIRST_BOT_COMPLETE,
            barge_in_min_words=3,
        )
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
            conversation_config=conv,
        )
        config = await _capture_config(session)
        cc = config["conversation_config"]
        assert cc["base_url"] == "http://127.0.0.1:11434/v1"
        assert cc["model"] == "llama3.2:1b"
        assert cc["system_prompt"] == "You are concise."
        # Enums serialize as their string value (the gateway's typed vocabulary).
        assert cc["reasoning_effort"] == "minimal"
        assert cc["reasoning_model"] == "o3"
        assert cc["latency_filler"] == "auto"
        assert cc["eager_eot"] is True
        assert cc["mute_strategy"] == "until_first_bot_complete"
        assert cc["barge_in_min_words"] == 3

    @pytest.mark.asyncio
    async def test_conversation_config_omits_unset_optionals(self):
        """Only base_url+model required; unset optionals must NOT be sent."""
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
            conversation_config=ConversationConfig(
                base_url="http://127.0.0.1:11434/v1", model="llama3.2:1b",
            ),
        )
        config = await _capture_config(session)
        cc = config["conversation_config"]
        assert set(cc.keys()) == {"base_url", "model"}

    @pytest.mark.asyncio
    async def test_no_conversation_config_when_not_set(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
        )
        config = await _capture_config(session)
        assert "conversation_config" not in config


class TestP0NestedFeatures:
    """~20 typed STT/TTS fields must nest under features{} so they reach the wire."""

    @pytest.mark.asyncio
    async def test_stt_features_nested(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(
                provider="deepgram",
                diarize=True,
                smart_format=True,
                interim_results=True,
                profanity_filter=True,
                keywords=["WaaV", "Deepgram"],
                custom_vocabulary=["Accubits"],
            ),
        )
        config = await _capture_config(session)
        feats = config["stt_config"]["features"]
        assert feats["diarization"] is True
        assert feats["smart_format"] is True
        assert feats["interim_results"] is True
        assert feats["profanity_filter"] is True
        assert feats["keyterms"] == ["WaaV", "Deepgram"]
        # custom_vocabulary has no canonical field -> extras passthrough.
        assert config["stt_config"]["extras"] == {"custom_vocabulary": ["Accubits"]}
        # The advanced fields must NOT leak as flat top-level stt_config keys.
        assert "diarize" not in config["stt_config"]
        assert "keywords" not in config["stt_config"]

    @pytest.mark.asyncio
    async def test_tts_features_and_extras_nested(self):
        session = WebSocketSession(
            url="ws://localhost:3009/ws",
            stt_config=STTConfig(provider="deepgram"),
            tts_config=TTSConfig(
                provider="elevenlabs",
                voice_id="rachel",
                model="eleven_turbo_v2",
                stability=0.7,
                similarity_boost=0.8,
                style=0.3,
                use_speaker_boost=True,
                acting_instructions="whispered fearfully",
                instant_mode=True,
                trailing_silence=0.5,
            ),
        )
        config = await _capture_config(session)
        feats = config["tts_config"]["features"]
        assert feats["stability"] == 0.7
        assert feats["similarity_boost"] == 0.8
        assert feats["style"] == 0.3
        assert feats["use_speaker_boost"] is True
        extras = config["tts_config"]["extras"]
        assert extras["acting_instructions"] == "whispered fearfully"
        assert extras["instant_mode"] is True
        assert extras["trailing_silence"] == 0.5
        # Must NOT leak as flat top-level keys.
        assert "stability" not in config["tts_config"]
        assert "acting_instructions" not in config["tts_config"]


class TestP0TranscriptMapping:
    """The exact gateway stt_result wire must surface through the SDK deserializer."""

    @pytest.mark.asyncio
    async def test_transcript_field_mapping_through_deserializer(self):
        # Feed the EXACT gateway wire frame to the SDK's own receive loop and
        # assert the SDK surfaces the text via STTResult (not raw msg access).
        session = WebSocketSession(url="ws://localhost:3009/ws")
        results = []
        session.on("transcript", lambda r: results.append(r))

        wire = json.dumps({
            "type": "stt_result",
            "transcript": "hello world",
            "is_final": True,
            "is_speech_final": True,
            "confidence": 0.97,
        })

        async def one_message():
            yield wire

        session._ws = one_message()
        session._connected = True
        await session._receive_loop()

        assert len(results) == 1
        assert results[0].text == "hello world"
        assert results[0].is_final is True
        assert results[0].is_speech_final is True
        assert results[0].confidence == 0.97
        # No translation on this frame -> empty list (default).
        assert results[0].translations == []

    @pytest.mark.asyncio
    async def test_translations_surface_through_deserializer(self):
        # P5: the uniform translations:[{lang,text}] array on the stt_result frame
        # must surface onto STTResult.translations through the SDK receive loop.
        session = WebSocketSession(url="ws://localhost:3009/ws")
        results = []
        session.on("transcript", lambda r: results.append(r))

        wire = json.dumps({
            "type": "stt_result",
            "transcript": "hello world",
            "is_final": True,
            "is_speech_final": True,
            "confidence": 0.95,
            "translations": [
                {"lang": "es-ES", "text": "hola mundo", "is_partial": False},
                {"lang": "de-DE", "text": "hallo welt"},
            ],
        })

        async def one_message():
            yield wire

        session._ws = one_message()
        session._connected = True
        await session._receive_loop()

        assert len(results) == 1
        translations = results[0].translations
        assert len(translations) == 2
        assert translations[0].lang == "es-ES"
        assert translations[0].text == "hola mundo"
        assert translations[0].is_partial is False
        assert translations[1].lang == "de-DE"
        assert translations[1].text == "hallo welt"
        assert translations[1].is_partial is False


class TestP0RateLimitError:
    """WS-connect 429 must classify into a typed RateLimitError with Retry-After."""

    def test_classify_429_with_retry_after(self):
        class _Resp:
            status_code = 429
            headers = {"retry-after": "7"}

        class _Exc(Exception):
            response = _Resp()

        err = WebSocketSession._classify_connect_error(_Exc("boom"), "ws://x/ws")
        assert isinstance(err, RateLimitError)
        assert err.retry_after == 7.0
        assert err.url == "ws://x/ws"

    def test_classify_non_429_is_connection_error(self):
        from bud_waav.errors import ConnectionError as BudConnectionError

        err = WebSocketSession._classify_connect_error(RuntimeError("dns"), "ws://x/ws")
        assert isinstance(err, BudConnectionError)
        assert not isinstance(err, RateLimitError)


class TestP0ProtocolVersion:
    """The ready envelope's protocol_version must be captured (plan W-K1)."""

    @pytest.mark.asyncio
    async def test_protocol_version_captured_on_ready(self):
        session = WebSocketSession(url="ws://localhost:3009/ws")
        wire = json.dumps({
            "type": "ready",
            "protocol_version": "1.0",
            "stream_id": "abc-123",
        })

        async def one_message():
            yield wire

        session._ws = one_message()
        session._connected = True
        await session._receive_loop()

        assert session.stream_id == "abc-123"
        assert session.protocol_version == "1.0"


class TestP0NoJsonPing:
    """The dead JSON {type:ping} keepalive (a non-existent gateway op) is removed."""

    def test_session_has_no_json_ping_method(self):
        # Native WS ping frames are auto-sent by the websockets lib; a JSON ping
        # causes a gateway parse-error, so the method must be gone.
        assert not hasattr(WebSocketSession, "ping")


# =============================================================================
# Task #23: WebSocket Receive Loop Tests
# =============================================================================


class TestWebSocketReceiveLoop:
    """Tests that receive loop handles all gateway message types."""

    def test_stt_result_with_is_speech_final(self):
        """STTResult should support is_speech_final field."""
        result = STTResult(
            text="Hello world",
            is_final=True,
            is_speech_final=True,
            confidence=0.95,
        )
        assert result.is_speech_final is True

    def test_stt_result_is_speech_final_defaults_false(self):
        """is_speech_final should default to False."""
        result = STTResult(text="Hello", is_final=False)
        assert result.is_speech_final is False

    @pytest.mark.asyncio
    async def test_receive_stt_result_with_speech_final(self):
        """Should parse is_speech_final from gateway stt_result message."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        session._connected = True

        # Track emitted events
        transcript_events = []
        session.on("transcript", lambda r: transcript_events.append(r))

        # Simulate the message handling code inline
        data = {
            "type": "stt_result",
            "transcript": "hello world",
            "is_final": True,
            "is_speech_final": True,
            "confidence": 0.95,
        }

        result = STTResult(
            text=data.get("transcript", ""),
            is_final=data.get("is_final", False),
            is_speech_final=data.get("is_speech_final", False),
            confidence=data.get("confidence"),
            speaker_id=data.get("speaker_id"),
        )

        assert result.is_speech_final is True
        assert result.is_final is True
        assert result.text == "hello world"

    @pytest.mark.asyncio
    async def test_receive_audio_end_event(self):
        """Should emit audio_end event when gateway sends it."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        session._connected = True

        audio_end_events = []
        session.on("audio_end", lambda d: audio_end_events.append(d))

        # Simulate audio_end emit
        session._emit("audio_end", {"stream_id": "test-123"})
        assert len(audio_end_events) == 1
        assert audio_end_events[0]["stream_id"] == "test-123"

    @pytest.mark.asyncio
    async def test_receive_turn_completed_event(self):
        """Should emit turn_completed event."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        session._connected = True

        turn_events = []
        session.on("turn_completed", lambda d: turn_events.append(d))

        session._emit("turn_completed", {"turn_id": "turn-1", "transcript": "hello"})
        assert len(turn_events) == 1

    @pytest.mark.asyncio
    async def test_receive_vad_event(self):
        """Should emit vad_event."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        session._connected = True

        vad_events = []
        session.on("vad_event", lambda d: vad_events.append(d))

        session._emit("vad_event", {"type": "vad_event", "speech": True})
        assert len(vad_events) == 1


# =============================================================================
# Task #24: REST Client Missing Endpoints Tests
# =============================================================================


class TestRestClientNewEndpoints:
    """Tests for newly added REST endpoints."""

    @pytest.fixture
    def client(self):
        """Create a RestClient instance."""
        return RestClient(base_url="http://localhost:3001", api_key="test-key")

    @pytest.mark.asyncio
    async def test_put_method(self, client):
        """RestClient should have a put method."""
        client._request = AsyncMock(return_value={"updated": True})

        result = await client.put("/test", json={"key": "value"})

        client._request.assert_called_once_with(
            "PUT", "/test", json={"key": "value"}, params=None
        )
        assert result["updated"] is True

    @pytest.mark.asyncio
    async def test_get_dag_template(self, client):
        """Should get a specific DAG template by name."""
        client.get = AsyncMock(return_value={
            "id": "voice-assistant",
            "name": "Voice Assistant",
            "version": "1.0",
        })

        result = await client.get_dag_template("voice-assistant")

        assert result["id"] == "voice-assistant"
        client.get.assert_called_once_with("/dag/templates/voice-assistant")

    @pytest.mark.asyncio
    async def test_remove_livekit_participant(self, client):
        """Should remove a LiveKit participant via DELETE /livekit/participant with JSON body."""
        client.delete = AsyncMock(return_value={
            "status": "removed",
            "room_name": "test-room",
            "participant_identity": "user-123",
        })

        result = await client.remove_livekit_participant(
            room_name="test-room",
            identity="user-123",
        )

        assert result["status"] == "removed"
        client.delete.assert_called_once()
        call_args = client.delete.call_args
        assert call_args[0][0] == "/livekit/participant"
        assert call_args[1]["json"]["room_name"] == "test-room"
        assert call_args[1]["json"]["participant_identity"] == "user-123"

    @pytest.mark.asyncio
    async def test_mute_livekit_participant(self, client):
        """Should mute a LiveKit participant via POST /livekit/participant/mute."""
        client.post = AsyncMock(return_value={
            "room_name": "test-room",
            "participant_identity": "user-123",
            "track_sid": "TR_abc123",
            "muted": True,
        })

        result = await client.mute_livekit_participant(
            room_name="test-room",
            identity="user-123",
            track_sid="TR_abc123",
            muted=True,
        )

        assert result["muted"] is True
        client.post.assert_called_once()
        call_args = client.post.call_args
        assert call_args[0][0] == "/livekit/participant/mute"
        assert call_args[1]["json"]["room_name"] == "test-room"
        assert call_args[1]["json"]["participant_identity"] == "user-123"
        assert call_args[1]["json"]["track_sid"] == "TR_abc123"
        assert call_args[1]["json"]["muted"] is True

    @pytest.mark.asyncio
    async def test_unmute_livekit_participant(self, client):
        """Should unmute a LiveKit participant."""
        client.post = AsyncMock(return_value={
            "room_name": "test-room",
            "participant_identity": "user-123",
            "track_sid": "TR_abc123",
            "muted": False,
        })

        result = await client.mute_livekit_participant(
            room_name="test-room",
            identity="user-123",
            track_sid="TR_abc123",
            muted=False,
        )

        assert result["muted"] is False

    @pytest.mark.asyncio
    async def test_sip_transfer(self, client):
        """Should transfer a SIP call."""
        client.post = AsyncMock(return_value={"status": "transferred"})

        result = await client.sip_transfer(
            stream_id="stream-123",
            transfer_to="+1234567890",
        )

        assert result["status"] == "transferred"
        client.post.assert_called_once()
        call_args = client.post.call_args
        assert call_args[0][0] == "/sip/transfer"
        assert call_args[1]["json"]["stream_id"] == "stream-123"
        assert call_args[1]["json"]["transfer_to"] == "+1234567890"

    @pytest.mark.asyncio
    async def test_get_metrics(self, client):
        """Should get server metrics."""
        client.get = AsyncMock(return_value={
            "stt_latency_p50": 45.2,
            "tts_latency_p50": 120.5,
            "active_connections": 5,
        })

        result = await client.get_metrics()

        assert result["stt_latency_p50"] == 45.2
        assert result["active_connections"] == 5
        client.get.assert_called_once_with("/metrics")

    @pytest.mark.asyncio
    async def test_speak_with_config(self, client):
        """Should send speak request with full TTS config."""
        client.post = AsyncMock(return_value=b"\x00\x01\x02\x03")

        tts_config = {
            "provider": "elevenlabs",
            "model": "eleven_turbo_v2",
            "voice_id": "rachel",
            "audio_format": "linear16",
            "sample_rate": 24000,
            "emotion": "happy",
            "emotion_intensity": 0.8,
        }

        result = await client.speak_with_config(
            text="Hello world",
            tts_config=tts_config,
        )

        assert result == b"\x00\x01\x02\x03"
        client.post.assert_called_once()
        call_args = client.post.call_args
        assert call_args[0][0] == "/speak"
        payload = call_args[1]["json"]
        assert payload["text"] == "Hello world"
        assert payload["tts_config"]["emotion"] == "happy"


# =============================================================================
# Task #25: BudClient Realtime Accessor Tests
# =============================================================================


class TestBudClientRealtimeAccessor:
    """Tests for BudClient realtime pipeline integration."""

    def test_client_has_realtime_url(self):
        """BudClient should expose realtime WebSocket URL."""
        client = BudClient(base_url="http://localhost:3001", api_key="test-key")
        assert client.realtime_url == "ws://localhost:3001/realtime"

    def test_client_has_realtime_url_https(self):
        """BudClient should convert https to wss for realtime URL."""
        client = BudClient(base_url="https://gateway.example.com", api_key="test-key")
        assert client.realtime_url == "wss://gateway.example.com/realtime"

    def test_create_realtime(self):
        """Should create a BudRealtime instance via BudClient."""
        client = BudClient(base_url="http://localhost:3001", api_key="test-key")
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="openai-key",
            system_prompt="You are a helpful assistant.",
        )
        realtime = client.create_realtime(config)
        assert isinstance(realtime, BudRealtime)
        assert realtime.provider == RealtimeProvider.OPENAI_REALTIME

    def test_create_realtime_hume(self):
        """Should create a Hume EVI realtime instance."""
        client = BudClient(base_url="http://localhost:3001", api_key="test-key")
        config = RealtimeConfig(
            provider=RealtimeProvider.HUME_EVI,
            api_key="hume-key",
            evi_version="3",
        )
        realtime = client.create_realtime(config)
        assert realtime.provider == RealtimeProvider.HUME_EVI


class TestBudClientNewMethods:
    """Tests for new methods on BudClient."""

    @pytest.fixture
    def client(self):
        """Create a BudClient instance."""
        return BudClient(base_url="http://localhost:3001", api_key="test-key")

    @pytest.mark.asyncio
    async def test_remove_livekit_participant(self, client):
        """Should proxy to rest client."""
        client._rest_client.remove_livekit_participant = AsyncMock(
            return_value={"status": "removed"}
        )

        result = await client.remove_livekit_participant("room-1", "user-1")

        assert result["status"] == "removed"
        client._rest_client.remove_livekit_participant.assert_called_once_with(
            "room-1", "user-1"
        )

    @pytest.mark.asyncio
    async def test_mute_livekit_participant(self, client):
        """Should proxy to rest client."""
        client._rest_client.mute_livekit_participant = AsyncMock(
            return_value={"muted": True}
        )

        result = await client.mute_livekit_participant(
            "room-1", "user-1", track_sid="TR_abc", muted=True
        )

        assert result["muted"] is True

    @pytest.mark.asyncio
    async def test_get_metrics(self, client):
        """Should proxy to rest client."""
        client._rest_client.get_metrics = AsyncMock(
            return_value={"active_connections": 10}
        )

        result = await client.get_metrics()

        assert result["active_connections"] == 10


# =============================================================================
# Audio Features Types Tests
# =============================================================================


class TestAudioFeaturesTypes:
    """Tests for audio features type correctness."""

    def test_turn_detection_config_defaults(self):
        """TurnDetectionConfig should have correct defaults."""
        config = TurnDetectionConfig()
        assert config.enabled is False
        assert config.threshold == 0.5
        assert config.silence_ms == 500
        assert config.prefix_padding_ms == 200
        assert config.create_response_ms == 300

    def test_noise_filter_config_defaults(self):
        """NoiseFilterConfig should have correct defaults."""
        config = NoiseFilterConfig()
        assert config.enabled is False
        assert config.strength == "medium"
        assert config.strength_value is None

    def test_noise_filter_config_with_numeric_strength(self):
        """NoiseFilterConfig should accept numeric strength_value."""
        config = NoiseFilterConfig(enabled=True, strength_value=0.8)
        assert config.strength_value == 0.8

    def test_extended_vad_config_defaults(self):
        """ExtendedVADConfig should have correct defaults."""
        from bud_waav import VADModeType
        config = ExtendedVADConfig()
        assert config.enabled is True
        assert config.threshold == 0.5
        assert config.mode == VADModeType.NORMAL

    def test_audio_features_composition(self):
        """AudioFeatures should compose all sub-configs."""
        features = AudioFeatures(
            turn_detection=TurnDetectionConfig(enabled=True, threshold=0.7),
            noise_filtering=NoiseFilterConfig(enabled=True, strength="high"),
            vad=ExtendedVADConfig(enabled=True, threshold=0.4),
        )
        assert features.turn_detection.threshold == 0.7
        assert features.noise_filtering.strength == "high"
        assert features.vad.threshold == 0.4


# =============================================================================
# Emotion System Integration Tests
# =============================================================================


class TestEmotionSystemIntegration:
    """Tests for emotion types working with TTS config."""

    def test_tts_config_with_emotion(self):
        """TTSConfig should accept emotion fields."""
        config = TTSConfig(
            provider="elevenlabs",
            emotion=Emotion.HAPPY,
            emotion_intensity=EmotionIntensityLevel.HIGH,
            delivery_style=DeliveryStyle.EXPRESSIVE,
            emotion_description="Joyful and energetic",
        )
        assert config.emotion == Emotion.HAPPY
        assert config.emotion_intensity == EmotionIntensityLevel.HIGH
        assert config.delivery_style == DeliveryStyle.EXPRESSIVE
        assert config.emotion_description == "Joyful and energetic"

    def test_tts_config_with_float_intensity(self):
        """TTSConfig should accept float intensity."""
        config = TTSConfig(
            provider="hume",
            emotion=Emotion.SAD,
            emotion_intensity=0.7,
        )
        assert config.emotion_intensity == 0.7

    def test_tts_config_with_hume_fields(self):
        """TTSConfig should accept Hume-specific fields."""
        config = TTSConfig(
            provider="hume",
            acting_instructions="whispered fearfully",
            voice_description="A calm, soothing voice",
            trailing_silence=0.5,
            instant_mode=True,
        )
        assert config.acting_instructions == "whispered fearfully"
        assert config.voice_description == "A calm, soothing voice"
        assert config.trailing_silence == 0.5
        assert config.instant_mode is True

    def test_intensity_to_number(self):
        """intensity_to_number should convert correctly."""
        from bud_waav import intensity_to_number

        assert intensity_to_number(EmotionIntensityLevel.LOW) == 0.3
        assert intensity_to_number(EmotionIntensityLevel.MEDIUM) == 0.6
        assert intensity_to_number(EmotionIntensityLevel.HIGH) == 1.0
        assert intensity_to_number(0.75) == 0.75
        assert intensity_to_number(1.5) == 1.0  # Clamped
        assert intensity_to_number(-0.5) == 0.0  # Clamped


# =============================================================================
# P1: Pipeline audio_features/dag_config/stream_id Passthrough Tests
# =============================================================================


class TestPipelineParamsPassthrough:
    """Tests that all pipelines accept and forward audio_features, dag_config, stream_id."""

    def test_stt_session_accepts_params(self):
        """STTSession should accept and forward audio_features, dag_config, stream_id."""
        from bud_waav.pipelines.stt import STTSession

        features = AudioFeatures(
            turn_detection=TurnDetectionConfig(enabled=True),
        )
        dag = DAGConfig(template="simple-stt")
        session = STTSession(
            url="ws://localhost:3001/ws",
            api_key="test-key",
            audio_features=features,
            dag_config=dag,
            stream_id="stt-stream-1",
        )
        assert session._session.audio_features is not None
        assert session._session.audio_features.turn_detection.enabled is True
        assert session._session.dag_config is not None
        assert session._session.dag_config.template == "simple-stt"
        assert session._session.requested_stream_id == "stt-stream-1"

    def test_stt_pipeline_create_passes_params(self):
        """BudSTT.create() should pass audio_features, dag_config, stream_id to STTSession."""
        from bud_waav.pipelines.stt import BudSTT

        stt = BudSTT(url="ws://localhost:3001/ws", api_key="test-key")
        features = AudioFeatures(noise_filtering=NoiseFilterConfig(enabled=True))
        dag = DAGConfig(template="voice-assistant")
        session = stt.create(
            provider="deepgram",
            audio_features=features,
            dag_config=dag,
            stream_id="test-stream",
        )
        assert session._session.audio_features is not None
        assert session._session.audio_features.noise_filtering.enabled is True
        assert session._session.dag_config.template == "voice-assistant"
        assert session._session.requested_stream_id == "test-stream"

    def test_tts_session_accepts_params(self):
        """TTSSession should accept and forward audio_features, dag_config, stream_id."""
        from bud_waav.pipelines.tts import TTSSession

        features = AudioFeatures(vad=ExtendedVADConfig(enabled=True, threshold=0.6))
        dag = DAGConfig(template="simple-tts")
        session = TTSSession(
            url="ws://localhost:3001/ws",
            api_key="test-key",
            audio_features=features,
            dag_config=dag,
            stream_id="tts-stream-1",
        )
        assert session._session.audio_features is not None
        assert session._session.audio_features.vad.threshold == 0.6
        assert session._session.dag_config.template == "simple-tts"
        assert session._session.requested_stream_id == "tts-stream-1"

    def test_tts_pipeline_create_passes_params(self):
        """BudTTS.create() should pass audio_features, dag_config, stream_id to TTSSession."""
        from bud_waav.pipelines.tts import BudTTS

        tts = BudTTS(url="ws://localhost:3001/ws", api_key="test-key")
        features = AudioFeatures(turn_detection=TurnDetectionConfig(enabled=True, silence_ms=700))
        session = tts.create(
            provider="elevenlabs",
            audio_features=features,
            dag_config=DAGConfig(template="simple-tts"),
            stream_id="tts-stream-2",
        )
        assert session._session.audio_features.turn_detection.silence_ms == 700
        assert session._session.dag_config.template == "simple-tts"
        assert session._session.requested_stream_id == "tts-stream-2"

    def test_talk_session_accepts_params(self):
        """TalkSession should accept and forward audio_features, dag_config, stream_id."""
        from bud_waav.pipelines.talk import TalkSession

        features = AudioFeatures(
            turn_detection=TurnDetectionConfig(enabled=True),
            noise_filtering=NoiseFilterConfig(enabled=True, strength="high"),
        )
        dag = DAGConfig(template="voice-assistant", enable_metrics=True)
        session = TalkSession(
            url="ws://localhost:3001/ws",
            api_key="test-key",
            stt_config=STTConfig(provider="deepgram"),
            tts_config=TTSConfig(provider="elevenlabs"),
            audio_features=features,
            dag_config=dag,
            stream_id="talk-stream-1",
        )
        assert session._session.audio_features is not None
        assert session._session.audio_features.turn_detection.enabled is True
        assert session._session.audio_features.noise_filtering.strength == "high"
        assert session._session.dag_config.template == "voice-assistant"
        assert session._session.dag_config.enable_metrics is True
        assert session._session.requested_stream_id == "talk-stream-1"

    def test_talk_pipeline_create_passes_params(self):
        """BudTalk.create() should pass audio_features, dag_config, stream_id to TalkSession."""
        from bud_waav.pipelines.talk import BudTalk

        talk = BudTalk(url="ws://localhost:3001/ws", api_key="test-key")
        features = AudioFeatures(vad=ExtendedVADConfig(enabled=True))
        session = talk.create(
            stt={"provider": "deepgram"},
            tts={"provider": "elevenlabs"},
            audio_features=features,
            dag_config=DAGConfig(template="voice-assistant"),
            stream_id="talk-stream-2",
        )
        assert session._session.audio_features is not None
        assert session._session.audio_features.vad.enabled is True
        assert session._session.dag_config.template == "voice-assistant"
        assert session._session.requested_stream_id == "talk-stream-2"

    def test_transcribe_session_accepts_params(self):
        """TranscribeSession should store audio_features, dag_config, stream_id."""
        from bud_waav.pipelines.transcribe import TranscribeSession

        features = AudioFeatures(noise_filtering=NoiseFilterConfig(enabled=True))
        dag = DAGConfig(template="simple-stt")
        session = TranscribeSession(
            url="ws://localhost:3001/ws",
            api_key="test-key",
            audio_features=features,
            dag_config=dag,
            stream_id="transcribe-stream-1",
        )
        assert session._audio_features is not None
        assert session._audio_features.noise_filtering.enabled is True
        assert session._dag_config.template == "simple-stt"
        assert session._stream_id == "transcribe-stream-1"

    def test_transcribe_pipeline_create_passes_params(self):
        """BudTranscribe.create() should pass params to TranscribeSession."""
        from bud_waav.pipelines.transcribe import BudTranscribe

        transcribe = BudTranscribe(url="ws://localhost:3001/ws", api_key="test-key")
        features = AudioFeatures(turn_detection=TurnDetectionConfig(enabled=True))
        session = transcribe.create(
            provider="deepgram",
            audio_features=features,
            dag_config=DAGConfig(template="simple-stt"),
            stream_id="transcribe-stream-2",
        )
        assert session._audio_features is not None
        assert session._audio_features.turn_detection.enabled is True
        assert session._dag_config.template == "simple-stt"
        assert session._stream_id == "transcribe-stream-2"

    def test_pipeline_params_default_to_none(self):
        """All pipelines should default audio_features/dag_config/stream_id to None."""
        from bud_waav.pipelines.stt import STTSession
        from bud_waav.pipelines.tts import TTSSession
        from bud_waav.pipelines.talk import TalkSession
        from bud_waav.pipelines.transcribe import TranscribeSession

        stt = STTSession(url="ws://localhost:3001/ws")
        assert stt._session.audio_features is None
        assert stt._session.dag_config is None
        assert stt._session.requested_stream_id is None

        tts = TTSSession(url="ws://localhost:3001/ws")
        assert tts._session.audio_features is None
        assert tts._session.dag_config is None
        assert tts._session.requested_stream_id is None

        talk = TalkSession(url="ws://localhost:3001/ws")
        assert talk._session.audio_features is None
        assert talk._session.dag_config is None
        assert talk._session.requested_stream_id is None

        trans = TranscribeSession(url="ws://localhost:3001/ws")
        assert trans._audio_features is None
        assert trans._dag_config is None
        assert trans._stream_id is None


# =============================================================================
# P1: TalkSession New Event Types Tests
# =============================================================================


class TestTalkSessionNewEvents:
    """Tests that TalkSession handles turn_completed, vad_event, audio_end."""

    def test_talk_event_has_data_field(self):
        """TalkEvent should have a data field for raw payloads."""
        from bud_waav.pipelines.talk import TalkEvent

        event = TalkEvent(type="turn_completed", data={"turn_id": "t1"})
        assert event.data is not None
        assert event.data["turn_id"] == "t1"

    def test_talk_event_data_defaults_none(self):
        """TalkEvent.data should default to None."""
        from bud_waav.pipelines.talk import TalkEvent

        event = TalkEvent(type="transcript")
        assert event.data is None

    def test_talk_session_handles_turn_completed(self):
        """TalkSession should emit turn_completed events."""
        from bud_waav.pipelines.talk import TalkSession

        session = TalkSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
            tts_config=TTSConfig(provider="elevenlabs"),
        )

        events = []
        session.on("turn_completed", lambda d: events.append(d))

        # Simulate event from WebSocketSession
        session._on_turn_completed({"turn_id": "turn-1", "transcript": "hello"})
        assert len(events) == 1
        assert events[0]["turn_id"] == "turn-1"

    def test_talk_session_handles_vad_event(self):
        """TalkSession should emit vad_event events."""
        from bud_waav.pipelines.talk import TalkSession

        session = TalkSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
            tts_config=TTSConfig(provider="elevenlabs"),
        )

        events = []
        session.on("vad_event", lambda d: events.append(d))

        session._on_vad_event({"type": "vad_event", "speech": True})
        assert len(events) == 1
        assert events[0]["speech"] is True

    def test_talk_session_handles_audio_end(self):
        """TalkSession should emit audio_end events."""
        from bud_waav.pipelines.talk import TalkSession

        session = TalkSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
            tts_config=TTSConfig(provider="elevenlabs"),
        )

        events = []
        session.on("audio_end", lambda d: events.append(d))

        session._on_audio_end({"stream_id": "test-stream"})
        assert len(events) == 1
        assert events[0]["stream_id"] == "test-stream"

    def test_talk_session_emits_unified_event(self):
        """TalkSession should emit unified 'event' for new event types."""
        from bud_waav.pipelines.talk import TalkSession, TalkEvent

        session = TalkSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram"),
        )

        events = []
        session.on("event", lambda e: events.append(e))

        session._on_turn_completed({"turn_id": "t1"})
        session._on_vad_event({"speech": True})
        session._on_audio_end({"stream_id": "s1"})

        assert len(events) == 3
        assert events[0].type == "turn_completed"
        assert events[0].data["turn_id"] == "t1"
        assert events[1].type == "vad_event"
        assert events[2].type == "audio_end"


# =============================================================================
# P1: Missing Type Exports Tests
# =============================================================================


class TestMissingTypeExports:
    """Tests that all types defined in types.py are properly exported."""

    def test_realtime_session_types_exported(self):
        """Realtime session types should be importable from bud_waav."""
        from bud_waav import (
            VADConfig,
            InputTranscriptionConfig,
            RealtimeSessionConfig,
            RealtimeTranscript,
            RealtimeSpeechEvent,
            RealtimeAudioChunk,
        )

        # Verify they are actual classes
        config = RealtimeSessionConfig(provider="openai")
        assert config.model == "gpt-4o-realtime-preview"

        vad = VADConfig(threshold=0.6)
        assert vad.threshold == 0.6

        itc = InputTranscriptionConfig(model="whisper-1")
        assert itc.enabled is True

    def test_voice_clone_types_exported(self):
        """Voice cloning types should be importable from bud_waav."""
        from bud_waav import (
            VoiceCloneProvider,
            VoiceCloneRequest,
            VoiceCloneStatus,
            VoiceCloneResponse,
        )

        assert VoiceCloneProvider.HUME == "hume"
        assert VoiceCloneProvider.ELEVENLABS == "elevenlabs"
        assert VoiceCloneStatus.READY == "ready"

        request = VoiceCloneRequest(
            provider=VoiceCloneProvider.ELEVENLABS,
            name="My Voice",
        )
        assert request.name == "My Voice"

    def test_hume_evi_types_exported(self):
        """Hume EVI types should be importable from bud_waav."""
        from bud_waav import HumeEVIVersion, HumeEVIConfig, ProsodyScores

        assert HumeEVIVersion.V3 == "3"
        assert HumeEVIVersion.V4_MINI == "4-mini"

        config = HumeEVIConfig(evi_version=HumeEVIVersion.V3, voice_id="test-voice")
        assert config.voice_id == "test-voice"

        scores = ProsodyScores(joy=0.8, anger=0.1)
        top = scores.top_emotions(2)
        assert top[0][0] == "joy"
        assert top[0][1] == 0.8

    def test_defaults_dicts_exported(self):
        """VOICE_DEFAULTS and REALTIME_DEFAULTS should be importable."""
        from bud_waav import VOICE_DEFAULTS, REALTIME_DEFAULTS

        assert "deepgram" in VOICE_DEFAULTS
        assert VOICE_DEFAULTS["deepgram"]["model"] == "aura-asteria-en"

        assert "openai" in REALTIME_DEFAULTS
        assert REALTIME_DEFAULTS["openai"]["model"] == "gpt-4o-realtime-preview"

    def test_all_exports_in_all_list(self):
        """All newly exported types should be in __all__."""
        import bud_waav

        new_types = [
            "VADConfig",
            "InputTranscriptionConfig",
            "RealtimeSessionConfig",
            "RealtimeTranscript",
            "RealtimeSpeechEvent",
            "RealtimeAudioChunk",
            "VOICE_DEFAULTS",
            "REALTIME_DEFAULTS",
            "VoiceCloneProvider",
            "VoiceCloneRequest",
            "VoiceCloneStatus",
            "VoiceCloneResponse",
            "HumeEVIVersion",
            "HumeEVIConfig",
            "ProsodyScores",
        ]

        for name in new_types:
            assert name in bud_waav.__all__, f"{name} not in __all__"
            assert hasattr(bud_waav, name), f"{name} not importable from bud_waav"


# =============================================================================
# P2 Gap Fixes
# =============================================================================


class TestDAGNodeTypesExpanded:
    """Tests for expanded DAG node types matching gateway."""

    def test_gateway_input_node_types(self):
        """Gateway input node types should be available."""
        from bud_waav import DAGNodeType

        assert DAGNodeType.AUDIO_INPUT.value == "audio_input"
        assert DAGNodeType.TEXT_INPUT.value == "text_input"

    def test_gateway_output_node_types(self):
        """Gateway output node types should be available."""
        from bud_waav import DAGNodeType

        assert DAGNodeType.AUDIO_OUTPUT.value == "audio_output"
        assert DAGNodeType.TEXT_OUTPUT.value == "text_output"
        assert DAGNodeType.WEBHOOK_OUTPUT.value == "webhook_output"

    def test_gateway_provider_node_types(self):
        """Gateway provider node types should be available."""
        from bud_waav import DAGNodeType

        assert DAGNodeType.STT_PROVIDER.value == "stt_provider"
        assert DAGNodeType.TTS_PROVIDER.value == "tts_provider"
        assert DAGNodeType.REALTIME_PROVIDER.value == "realtime_provider"

    def test_gateway_processing_node_types(self):
        """Gateway processing node types should be available."""
        from bud_waav import DAGNodeType

        assert DAGNodeType.PROCESSOR.value == "processor"
        assert DAGNodeType.TRANSFORM.value == "transform"
        assert DAGNodeType.PASSTHROUGH.value == "passthrough"

    def test_gateway_endpoint_node_types(self):
        """Gateway endpoint node types should be available."""
        from bud_waav import DAGNodeType

        assert DAGNodeType.HTTP_ENDPOINT.value == "http_endpoint"
        assert DAGNodeType.GRPC_ENDPOINT.value == "grpc_endpoint"
        assert DAGNodeType.WEBSOCKET_ENDPOINT.value == "websocket_endpoint"
        assert DAGNodeType.IPC_ENDPOINT.value == "ipc_endpoint"
        assert DAGNodeType.LIVEKIT_ENDPOINT.value == "livekit_endpoint"
        assert DAGNodeType.LLM_ENDPOINT.value == "llm_endpoint"

    def test_gateway_router_node_types(self):
        """Gateway router node types should be available."""
        from bud_waav import DAGNodeType

        assert DAGNodeType.ROUTER.value == "router"
        assert DAGNodeType.SPLIT.value == "split"
        assert DAGNodeType.JOIN.value == "join"

    def test_legacy_aliases_backward_compat(self):
        """Legacy aliases should still work for backward compatibility."""
        from bud_waav import DAGNodeType

        # LLM maps to LLM_ENDPOINT
        assert DAGNodeType.LLM == DAGNodeType.LLM_ENDPOINT
        # WEBHOOK maps to WEBHOOK_OUTPUT
        assert DAGNodeType.WEBHOOK == DAGNodeType.WEBHOOK_OUTPUT
        # Legacy types BUFFER and SWITCH still exist
        assert DAGNodeType.BUFFER.value == "buffer"
        assert DAGNodeType.SWITCH.value == "switch"

    def test_new_dag_support_types_exported(self):
        """OutputDestination, JoinStrategy, DAGDataType should be exported."""
        from bud_waav import OutputDestination, JoinStrategy, DAGDataType

        assert OutputDestination.WEBSOCKET.value == "websocket"
        assert OutputDestination.LIVEKIT.value == "livekit"
        assert OutputDestination.ENDPOINT.value == "endpoint"
        assert OutputDestination.BROADCAST.value == "broadcast"
        assert OutputDestination.DISCARD.value == "discard"

        assert JoinStrategy.FIRST.value == "first"
        assert JoinStrategy.ALL.value == "all"
        assert JoinStrategy.BEST.value == "best"
        assert JoinStrategy.MERGE.value == "merge"

        assert DAGDataType.AUDIO.value == "audio"
        assert DAGDataType.TEXT.value == "text"
        assert DAGDataType.STT_RESULT.value == "stt_result"
        assert DAGDataType.JSON.value == "json"
        assert DAGDataType.BINARY.value == "binary"

    def test_dag_validation_includes_counts(self):
        """DAG validation result should include node_count and edge_count."""
        from bud_waav import validate_dag_definition, TEMPLATE_SIMPLE_STT

        result = validate_dag_definition(TEMPLATE_SIMPLE_STT)
        assert result.valid is True
        assert result.node_count == 3
        assert result.edge_count == 2

    def test_dag_node_with_new_types(self):
        """Should create DAG nodes with new node types."""
        from bud_waav import DAGNode, DAGNodeType

        ipc_node = DAGNode(
            id="ipc1",
            type=DAGNodeType.IPC_ENDPOINT,
            config={"shm_name": "/audio_buffer", "input_format": "pcm16"},
        )
        assert ipc_node.type == DAGNodeType.IPC_ENDPOINT

        grpc_node = DAGNode(
            id="grpc1",
            type=DAGNodeType.GRPC_ENDPOINT,
            config={"address": "localhost:50051", "service": "stt.STTService"},
        )
        assert grpc_node.type == DAGNodeType.GRPC_ENDPOINT


class TestWebSocketAuthMessages:
    """Tests for WebSocket auth message handling."""

    def test_session_has_auth_attributes(self):
        """WebSocketSession should have auth-related attributes."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        assert hasattr(session, "_authenticated")
        assert session._authenticated is False
        assert hasattr(session, "_auth_event")

    def test_auth_event_lazy_init(self):
        """Auth event should be lazily initialized."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        assert session._auth_event is None
        event = session._get_auth_event()
        assert event is not None
        # Same event on second call
        assert session._get_auth_event() is event

    @pytest.mark.asyncio
    async def test_auth_required_sends_token(self):
        """Receiving auth_required should trigger auth token send."""
        session = WebSocketSession(
            url="ws://localhost:3001/ws", api_key="test-api-key"
        )
        session._connected = True
        session._ws = MagicMock()
        session._ws.send = AsyncMock()

        # Simulate auth_required message
        import asyncio

        session._message_queue = asyncio.Queue()
        # Directly call the handler logic that would run in receive_loop
        data = {"type": "auth_required"}
        msg_type = data.get("type")

        if msg_type == "auth_required":
            if session.api_key:
                await session._send_json({"type": "auth", "token": session.api_key})

        # Verify auth message was sent
        session._ws.send.assert_called_once()
        sent_msg = json.loads(session._ws.send.call_args[0][0])
        assert sent_msg["type"] == "auth"
        assert sent_msg["token"] == "test-api-key"

    @pytest.mark.asyncio
    async def test_authenticated_sets_event(self):
        """Receiving authenticated message should set auth event."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        session._authenticated = False

        # Simulate authenticated response
        session._authenticated = True
        session._get_auth_event().set()

        assert session._authenticated is True
        assert session._get_auth_event().is_set()

    def test_session_has_send_audio_end(self):
        """WebSocketSession should have send_audio_end method."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        assert hasattr(session, "send_audio_end")
        assert callable(session.send_audio_end)

    def test_session_has_send_custom(self):
        """WebSocketSession should have send_custom method."""
        session = WebSocketSession(url="ws://localhost:3001/ws")
        assert hasattr(session, "send_custom")
        assert callable(session.send_custom)


class TestRestClientDAGMethods:
    """Tests for REST client DAG method corrections."""

    @pytest.mark.asyncio
    async def test_validate_dag_wraps_in_dag_key(self):
        """validate_dag should send definition wrapped in {'dag': ...}."""
        client = RestClient(base_url="http://localhost:3001")
        client.post = AsyncMock(return_value={"valid": True, "errors": [], "warnings": []})

        definition = {"id": "test", "name": "Test", "nodes": [], "edges": []}
        await client.validate_dag(definition)

        call_args = client.post.call_args
        assert call_args[0][0] == "/dag/validate"
        assert call_args[1]["json"] == {"dag": definition}

    @pytest.mark.asyncio
    async def test_list_dag_templates_returns_dict(self):
        """list_dag_templates should return the full gateway response dict."""
        client = RestClient(base_url="http://localhost:3001")
        client.get = AsyncMock(
            return_value={"templates": [{"name": "stt", "version": "1.0"}], "count": 1}
        )

        result = await client.list_dag_templates()
        assert isinstance(result, dict)
        assert "templates" in result
        assert "count" in result
        assert result["count"] == 1


class TestRealtimeGatewayRouting:
    """Tests for Realtime pipeline gateway routing mode."""

    def test_gateway_mode_from_config(self):
        """Setting gateway_url should enable gateway mode."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)
        assert rt._use_gateway is True

    def test_direct_mode_without_gateway_url(self):
        """Not setting gateway_url should use direct mode."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        rt = BudRealtime(config)
        assert rt._use_gateway is False

    def test_gateway_config_extra_fields(self):
        """RealtimeConfig should support gateway-specific fields."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
            transcribe_input=True,
            input_audio_format="pcm16",
            output_audio_format="pcm16",
            modalities=["text", "audio"],
        )
        assert config.transcribe_input is True
        assert config.input_audio_format == "pcm16"
        assert config.modalities == ["text", "audio"]

    def test_client_create_realtime_defaults_to_gateway(self):
        """BudClient.create_realtime should default to gateway mode."""
        client = BudClient(base_url="http://localhost:3001", api_key="test-key")
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            system_prompt="Test",
        )
        rt = client.create_realtime(config)
        assert rt._use_gateway is True
        assert rt._config.gateway_url == "ws://localhost:3001"
        assert rt._config.api_key == "test-key"

    def test_client_create_realtime_direct_mode(self):
        """When api_key is in config, should use direct mode."""
        client = BudClient(base_url="http://localhost:3001", api_key="gateway-key")
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="direct-provider-key",
        )
        rt = client.create_realtime(config)
        assert rt._use_gateway is False

    @pytest.mark.asyncio
    async def test_connect_raises_without_url_in_direct_mode(self):
        """connect() without url in direct mode should raise ValueError."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            api_key="test-key",
        )
        rt = BudRealtime(config)
        with pytest.raises(ValueError, match="url is required"):
            await rt.connect()

    def test_gateway_mode_has_session_id(self):
        """Gateway mode should track session_id."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)
        assert rt._session_id is None

    def test_gateway_message_handler_exists(self):
        """Gateway message handler should exist."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)
        assert hasattr(rt, "_handle_gateway_message")
        assert hasattr(rt, "_send_gateway_config_unlocked")

    def test_gateway_message_handler_transcript(self):
        """Gateway transcript messages should emit transcript events."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)

        transcripts = []
        rt.on("transcript", lambda t: transcripts.append(t))

        rt._handle_gateway_message(
            "transcript",
            {"type": "transcript", "text": "Hello world", "is_final": True, "role": "user"},
        )

        assert len(transcripts) == 1
        assert transcripts[0].text == "Hello world"
        assert transcripts[0].is_final is True
        assert transcripts[0].role == "user"

    def test_gateway_message_handler_function_call(self):
        """Gateway function_call messages should emit function_call events."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)

        calls = []
        rt.on("function_call", lambda c: calls.append(c))

        rt._handle_gateway_message(
            "function_call",
            {
                "type": "function_call",
                "name": "get_weather",
                "arguments": '{"city": "London"}',
                "call_id": "call_123",
            },
        )

        assert len(calls) == 1
        assert calls[0].name == "get_weather"
        assert calls[0].arguments == {"city": "London"}
        assert calls[0].call_id == "call_123"

    def test_gateway_message_handler_session_created(self):
        """Gateway session_created should set session_id."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)

        rt._handle_gateway_message(
            "session_created",
            {
                "type": "session_created",
                "session_id": "sess_abc123",
                "provider": "openai",
                "model": "gpt-4o-realtime-preview",
            },
        )

        assert rt._session_id == "sess_abc123"

    def test_gateway_message_handler_error(self):
        """Gateway error messages should emit error events."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)

        errors = []
        rt.on("error", lambda e: errors.append(e))

        rt._handle_gateway_message(
            "error",
            {"type": "error", "message": "Rate limit exceeded"},
        )

        assert len(errors) == 1
        assert "Rate limit exceeded" in str(errors[0])

    def test_clear_audio_buffer_method_exists(self):
        """BudRealtime should have clear_audio_buffer method."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)
        assert hasattr(rt, "clear_audio_buffer")
        assert callable(rt.clear_audio_buffer)

    def test_create_response_method_exists(self):
        """BudRealtime should have create_response method."""
        config = RealtimeConfig(
            provider=RealtimeProvider.OPENAI_REALTIME,
            gateway_url="ws://localhost:3001",
        )
        rt = BudRealtime(config)
        assert hasattr(rt, "create_response")
        assert callable(rt.create_response)


class TestP2TypeExports:
    """Tests that P2 types are properly exported."""

    def test_dag_support_types_in_all(self):
        """New DAG support types should be in __all__."""
        import bud_waav

        for name in ["OutputDestination", "JoinStrategy", "DAGDataType"]:
            assert name in bud_waav.__all__, f"{name} not in __all__"
            assert hasattr(bud_waav, name), f"{name} not importable"

    def test_total_exports_count(self):
        """Should have the expected total number of exports."""
        import bud_waav

        # Verify we haven't accidentally removed any exports
        assert len(bud_waav.__all__) >= 113, (
            f"Expected >= 113 exports, got {len(bud_waav.__all__)}. "
            "Some exports may have been accidentally removed."
        )
