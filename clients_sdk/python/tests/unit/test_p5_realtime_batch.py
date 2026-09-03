"""P5 SDK tests: gateway-native realtime client + translation block + batched STT.

Covers the three P5 SDK deliverables:
  1. TRANSLATION: ``STTConfig.translation`` reaches ``stt_config.translation`` on the
     /ws wire (via ``_send_config``), and on the batch envelope.
  2. BATCH: ``bud.transcribe_batch`` builds the right ``POST /transcribe/batch`` body
     (audio source, flattened StandardSTTConfig, batch-only knobs, translation) and
     polls ``GET /transcribe/batch/{job_id}`` until terminal.
  3. REALTIME: ``bud.realtime(...)`` builds the gateway PROVIDER-AGNOSTIC ``/realtime``
     config message (NOT an OpenAI-native ``session.update``) and works for all 12
     providers; the unified event stream + callbacks decode the gateway IN messages.
"""

import json

import pytest

from bud_waav import (
    BudClient,
    STTConfig,
    TranslationConfig,
    GatewayRealtime,
    GatewayRealtimeConfig,
    GatewayTurnDetection,
    REALTIME_PROVIDERS,
    get_supported_realtime_providers,
)
from bud_waav.ws.session import WebSocketSession


# =============================================================================
# (1) TRANSLATION — canonical block on the STT surface
# =============================================================================


class TestTranslationBlock:
    def test_translation_to_wire_targets(self):
        tc = TranslationConfig(target_languages=["es-ES", "de-DE"], partials=True)
        assert tc.to_wire() == {
            "target_languages": ["es-ES", "de-DE"],
            "partials": True,
        }

    def test_translation_to_wire_english_fast_path(self):
        tc = TranslationConfig(translate_to_english=True)
        assert tc.to_wire() == {"translate_to_english": True}

    def test_translation_empty_is_omitted(self):
        assert TranslationConfig().to_wire() == {}

    @pytest.mark.asyncio
    async def test_translation_reaches_stt_config_on_ws(self):
        """STTConfig.translation must nest under stt_config.translation on /ws."""
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(
                provider="speechmatics",
                language="en-US",
                translation=TranslationConfig(
                    target_languages=["es-ES", "de-DE"], partials=True
                ),
            ),
            audio=True,
        )
        sent = []

        async def mock_send(data):
            sent.append(data)

        session._ws = type("W", (), {})()
        session._ws.send = mock_send

        await session._send_config()

        config = json.loads(sent[0])
        translation = config["stt_config"]["translation"]
        assert translation["target_languages"] == ["es-ES", "de-DE"]
        assert translation["partials"] is True

    @pytest.mark.asyncio
    async def test_no_translation_key_when_unset(self):
        session = WebSocketSession(
            url="ws://localhost:3001/ws",
            stt_config=STTConfig(provider="deepgram", language="en-US"),
            audio=True,
        )
        sent = []

        async def mock_send(data):
            sent.append(data)

        session._ws = type("W", (), {})()
        session._ws.send = mock_send

        await session._send_config()
        config = json.loads(sent[0])
        assert "translation" not in config["stt_config"]


# =============================================================================
# (2) BATCHED / ASYNC STT — envelope + poll-until-done
# =============================================================================


class _FakeRest:
    """Captures submit_batch / get_batch calls and scripts poll responses."""

    def __init__(self, submit_response, poll_responses=None):
        self.submit_response = submit_response
        self.poll_responses = list(poll_responses or [])
        self.submit_calls = []
        self.get_calls = []

    async def submit_batch(self, **kwargs):
        self.submit_calls.append(kwargs)
        return self.submit_response

    async def get_batch(self, job_id):
        self.get_calls.append(job_id)
        return self.poll_responses.pop(0)


def _client_with_rest(rest):
    bud = BudClient(base_url="http://127.0.0.1:9")
    bud._rest_client = rest
    return bud


class TestBatchEnvelope:
    @pytest.mark.asyncio
    async def test_submit_body_url_and_flattened_config(self):
        rest = _FakeRest(submit_response={"job_id": "j1", "status": "completed", "result": {"text": "hi"}})
        bud = _client_with_rest(rest)

        out = await bud.transcribe_batch(
            audio="https://example.com/a.wav",
            provider="deepgram",
            language="en-US",
            model="nova-3",
            batch={"summarize": True, "detect_language": True, "alternatives": 3},
            translation={"target_languages": ["es-ES"]},
        )

        assert out["status"] == "completed"
        call = rest.submit_calls[0]
        # URL audio source, NOT inline bytes.
        assert call["url"] == "https://example.com/a.wav"
        assert call["audio_base64"] is None
        # StandardSTTConfig flattened (reused verbatim).
        assert call["stt_config"]["provider"] == "deepgram"
        assert call["stt_config"]["model"] == "nova-3"
        # Batch-only knobs (the streaming-drop set) carried through.
        assert call["batch"] == {"summarize": True, "detect_language": True, "alternatives": 3}
        # Translation block.
        assert call["translation"] == {"target_languages": ["es-ES"]}

    @pytest.mark.asyncio
    async def test_submit_inline_bytes(self):
        rest = _FakeRest(submit_response={"job_id": "j2", "status": "completed", "result": {}})
        bud = _client_with_rest(rest)

        await bud.transcribe_batch(
            audio=b"\x00\x01\x02\x03",
            content_type="audio/wav",
            provider="openai",
        )
        call = rest.submit_calls[0]
        assert call["url"] is None
        assert call["audio_base64"] == "AAECAw=="  # base64 of bytes 00 01 02 03
        assert call["content_type"] == "audio/wav"

    @pytest.mark.asyncio
    async def test_poll_until_completed(self):
        rest = _FakeRest(
            submit_response={"job_id": "abc", "status": "queued"},
            poll_responses=[
                {"job_id": "abc", "status": "processing"},
                {"job_id": "abc", "status": "completed", "result": {"text": "done"}},
            ],
        )
        bud = _client_with_rest(rest)

        out = await bud.transcribe_batch(
            audio="https://x/a.wav", poll_interval=0.0
        )
        assert out["status"] == "completed"
        assert out["result"]["text"] == "done"
        assert rest.get_calls == ["abc", "abc"]

    @pytest.mark.asyncio
    async def test_callback_mode_returns_ack_no_poll(self):
        rest = _FakeRest(submit_response={"job_id": "cb", "status": "queued"})
        bud = _client_with_rest(rest)

        out = await bud.transcribe_batch(
            audio="https://x/a.wav",
            callback_url="https://my.app/webhook",
            callback_method="PUT",
        )
        # Webhook mode: immediate ack, no GET polling.
        assert out == {"job_id": "cb", "status": "queued"}
        assert rest.get_calls == []
        assert rest.submit_calls[0]["callback_url"] == "https://my.app/webhook"
        assert rest.submit_calls[0]["callback_method"] == "PUT"

    @pytest.mark.asyncio
    async def test_typed_stt_config_and_translation_reused(self):
        rest = _FakeRest(submit_response={"job_id": "j", "status": "completed", "result": {}})
        bud = _client_with_rest(rest)

        cfg = STTConfig(
            provider="assemblyai",
            language="en-US",
            diarize=True,
            translation=TranslationConfig(target_languages=["fr-FR"]),
        )
        await bud.transcribe_batch(audio="https://x/a.wav", config=cfg)
        call = rest.submit_calls[0]
        # Diarization (a SttFeatures field) survives via the flattened config.
        assert call["stt_config"]["diarize"] is True
        assert call["stt_config"]["provider"] == "assemblyai"
        # translation pulled off the typed STTConfig when not passed separately.
        assert call["translation"] == {"target_languages": ["fr-FR"]}
        # translation must NOT be duplicated inside the flattened stt_config.
        assert "translation" not in call["stt_config"]


# =============================================================================
# (3) GATEWAY-NATIVE REALTIME — provider-agnostic /realtime config + events
# =============================================================================


class TestGatewayRealtimeConfig:
    def test_all_twelve_providers_supported(self):
        expected = {
            "openai", "hume", "azure", "grok", "inworld", "deepgram",
            "elevenlabs", "gemini", "ultravox", "nova_sonic", "speechmatics", "yandex",
        }
        assert set(REALTIME_PROVIDERS) == expected
        assert set(get_supported_realtime_providers()) == expected

    def test_bud_realtime_builds_gateway_config_not_openai_native(self):
        """The headline assertion: bud.realtime emits the gateway /realtime config
        message (type=config + provider-agnostic fields), NOT OpenAI's
        session.update wire."""
        bud = BudClient(base_url="http://127.0.0.1:3009")
        session = bud.realtime(
            provider="openai",
            voice="alloy",
            instructions="You are helpful.",
            temperature=0.7,
            max_response_tokens=512,
            modalities=["text", "audio"],
        )
        assert isinstance(session, GatewayRealtime)
        msg = session.config.to_config_message()

        # Gateway provider-agnostic envelope.
        assert msg["type"] == "config"
        assert msg["provider"] == "openai"
        assert msg["voice"] == "alloy"
        assert msg["instructions"] == "You are helpful."
        assert msg["temperature"] == 0.7
        assert msg["max_response_tokens"] == 512
        assert msg["modalities"] == ["text", "audio"]

        # NOT the OpenAI-native protocol.
        assert msg["type"] != "session.update"
        assert "session" not in msg
        assert "max_response_output_tokens" not in msg  # OpenAI-native name

        # Routes to the gateway /realtime endpoint.
        assert session._url == "ws://127.0.0.1:3009/realtime"

    def test_realtime_works_for_every_provider(self):
        bud = BudClient(base_url="http://127.0.0.1:3009")
        for provider in REALTIME_PROVIDERS:
            session = bud.realtime(provider=provider)
            msg = session.config.to_config_message()
            assert msg["provider"] == provider
            assert msg["type"] == "config"

    def test_unknown_provider_rejected(self):
        bud = BudClient(base_url="http://127.0.0.1:3009")
        with pytest.raises(ValueError):
            bud.realtime(provider="not-a-provider")

    def test_turn_detection_and_tools_wire(self):
        bud = BudClient(base_url="http://127.0.0.1:3009")
        session = bud.realtime(
            provider="deepgram",
            turn_detection={"mode": "server_vad", "threshold": 0.6, "silence_duration_ms": 400},
            tools=[{"name": "get_weather", "description": "wx", "parameters": {"type": "object"}}],
            reasoning_effort="low",
        )
        msg = session.config.to_config_message()
        assert msg["turn_detection"] == {
            "mode": "server_vad",
            "threshold": 0.6,
            "silence_duration_ms": 400,
        }
        # Tools use the gateway ToolConfig shape ({type, function:{...}}).
        assert msg["tools"][0] == {
            "type": "function",
            "function": {"name": "get_weather", "description": "wx", "parameters": {"type": "object"}},
        }
        assert msg["reasoning_effort"] == "low"

    def test_semantic_turn_detection(self):
        cfg = GatewayRealtimeConfig(
            provider="openai",
            turn_detection=GatewayTurnDetection(mode="semantic", eagerness="high"),
        )
        assert cfg.to_config_message()["turn_detection"] == {"mode": "semantic", "eagerness": "high"}

    def test_alias_field(self):
        cfg = GatewayRealtimeConfig(provider="openai", alias="support-bot")
        assert cfg.to_config_message()["alias"] == "support-bot"


class TestGatewayRealtimeMessages:
    """The gateway IN messages decode to the unified events (callbacks + stream)."""

    def _session(self):
        return GatewayRealtime(
            url="ws://x/realtime", config=GatewayRealtimeConfig(provider="openai")
        )

    @pytest.mark.asyncio
    async def test_dispatch_transcript(self):
        s = self._session()
        seen = []
        s.on("transcript", lambda e: seen.append(e))
        await s._dispatch({"type": "transcript", "text": "hello", "role": "user", "is_final": True})
        assert seen[0].text == "hello"
        assert seen[0].role == "user"
        assert seen[0].is_final is True
        # Also queued on the unified stream.
        ev = await s._queue.get()
        assert ev.type == "transcript" and ev.transcript.text == "hello"

    @pytest.mark.asyncio
    async def test_dispatch_session_created(self):
        s = self._session()
        seen = []
        s.on("session_created", lambda e: seen.append(e))
        await s._dispatch({
            "type": "session_created", "session_id": "sess_1",
            "provider": "openai", "model": "gpt-realtime",
        })
        assert s.session_id == "sess_1"
        assert seen[0].provider == "openai"
        assert seen[0].model == "gpt-realtime"

    @pytest.mark.asyncio
    async def test_dispatch_function_call(self):
        s = self._session()
        seen = []
        s.on("function_call", lambda e: seen.append(e))
        await s._dispatch({
            "type": "function_call", "call_id": "c1", "name": "wx",
            "arguments": '{"city": "Tokyo"}',
        })
        assert seen[0].name == "wx"
        assert seen[0].arguments == {"city": "Tokyo"}
        assert seen[0].call_id == "c1"

    @pytest.mark.asyncio
    async def test_dispatch_error_and_speech_and_response(self):
        s = self._session()
        errs, speech, resp = [], [], []
        s.on("error", lambda e: errs.append(e))
        s.on("speech_event", lambda e: speech.append(e))
        s.on("response_done", lambda e: resp.append(e))
        await s._dispatch({"type": "error", "code": "no_key", "message": "API key not configured"})
        await s._dispatch({"type": "speech_event", "event": "started", "audio_ms": 120})
        await s._dispatch({"type": "response_done", "response_id": "r1"})
        assert errs[0].message == "API key not configured" and errs[0].code == "no_key"
        assert speech[0].event == "started" and speech[0].audio_ms == 120
        assert resp[0].response_id == "r1"

    @pytest.mark.asyncio
    async def test_binary_audio_frame_emits_audio_event(self):
        s = self._session()
        seen = []
        s.on("audio", lambda e: seen.append(e))
        await s._handle_raw(b"\x10\x20\x30")
        assert seen[0].audio == b"\x10\x20\x30"
        ev = await s._queue.get()
        assert ev.type == "audio" and ev.audio.audio == b"\x10\x20\x30"
