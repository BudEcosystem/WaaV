"""D8 opus transport-codec negotiation (Python SDK).

The Python SDK is negotiation-AWARE: it sends ``stt_config.audio_in_codec`` /
``tts_config.audio_out_codec`` when requested and parses the effective codecs echoed on ``ready`` so
a caller can detect a downgrade. (It does not yet opus-encode/decode itself — those fields are opt-in
for callers who handle opus, matching the light-client scope.)
"""

import inspect

from bud_waav.types import STTConfig, TTSConfig
from bud_waav.ws.session import WebSocketSession


def test_stt_audio_in_codec_model_field() -> None:
    assert STTConfig(provider="deepgram", audio_in_codec="opus").audio_in_codec == "opus"
    # Default is None → the wire field is omitted (backward compatible / linear16).
    assert STTConfig(provider="deepgram").audio_in_codec is None


def test_tts_audio_out_codec_model_field() -> None:
    assert TTSConfig(provider="deepgram", audio_out_codec="opus").audio_out_codec == "opus"
    assert TTSConfig(provider="deepgram").audio_out_codec is None


def test_codec_fields_serialized_in_send_config() -> None:
    # Mirrors test_alias.py: assert the wire serializer emits the codec keys (gated on being set).
    s = WebSocketSession(url="ws://x", audio=False)
    src = inspect.getsource(s._send_config)
    assert 'stt_dict["audio_in_codec"] = self.stt_config.audio_in_codec' in src
    assert 'tts_dict["audio_out_codec"] = self.tts_config.audio_out_codec' in src
    # Gated on being requested (opt-in), never unconditional.
    assert 'getattr(self.stt_config, "audio_in_codec", None)' in src
    assert 'getattr(self.tts_config, "audio_out_codec", None)' in src


def test_ready_echo_properties() -> None:
    s = WebSocketSession(url="ws://x", audio=False)
    # Absent until a `ready` ack carries the echo.
    assert s.audio_in_codec is None
    assert s.audio_out_codec is None
    # Simulate the gateway echo: we asked for opus uplink but it downgraded to linear16.
    s._audio_in_codec = "linear16"
    s._audio_out_codec = "opus"
    assert s.audio_in_codec == "linear16"
    assert s.audio_out_codec == "opus"
