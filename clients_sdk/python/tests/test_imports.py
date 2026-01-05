"""Test that all imports work correctly."""

import pytest


def test_import_main_client():
    """Test importing the main BudClient."""
    from bud_foundry import BudClient
    assert BudClient is not None


def test_import_config_types():
    """Test importing configuration types."""
    from bud_foundry import STTConfig, TTSConfig, LiveKitConfig, FeatureFlags
    assert STTConfig is not None
    assert TTSConfig is not None
    assert LiveKitConfig is not None
    assert FeatureFlags is not None


def test_import_result_types():
    """Test importing result types."""
    from bud_foundry import STTResult, TranscriptEvent, AudioEvent, Voice
    assert STTResult is not None
    assert TranscriptEvent is not None
    assert AudioEvent is not None
    assert Voice is not None


def test_import_error_types():
    """Test importing error types."""
    from bud_foundry import (
        BudError,
        ConnectionError,
        TimeoutError,
        APIError,
        STTError,
        TTSError,
    )
    assert BudError is not None


def test_import_pipeline_classes():
    """Test importing pipeline classes."""
    from bud_foundry import BudSTT, BudTTS, BudTalk, BudTranscribe
    assert BudSTT is not None
    assert BudTTS is not None
    assert BudTalk is not None
    assert BudTranscribe is not None


def test_import_utilities():
    """Test importing utility classes."""
    from bud_foundry import RestClient, WebSocketSession, AudioProcessor
    assert RestClient is not None
    assert WebSocketSession is not None
    assert AudioProcessor is not None


def test_create_stt_config():
    """Test creating STT config."""
    from bud_foundry import STTConfig
    
    config = STTConfig(
        provider="deepgram",
        language="en-US",
        model="nova-3",
    )
    assert config.provider == "deepgram"
    assert config.language == "en-US"
    assert config.model == "nova-3"


def test_create_tts_config():
    """Test creating TTS config."""
    from bud_foundry import TTSConfig
    
    config = TTSConfig(
        provider="elevenlabs",
        voice="rachel",
        sample_rate=24000,
    )
    assert config.provider == "elevenlabs"
    assert config.voice == "rachel"
    assert config.sample_rate == 24000


def test_create_feature_flags():
    """Test creating feature flags."""
    from bud_foundry import FeatureFlags
    
    flags = FeatureFlags(
        vad=True,
        noise_cancellation=True,
        speaker_diarization=True,
    )
    assert flags.vad is True
    assert flags.noise_cancellation is True
    assert flags.speaker_diarization is True


def test_create_client():
    """Test creating the main client."""
    from bud_foundry import BudClient
    
    client = BudClient(
        base_url="http://localhost:3001",
        api_key="test-key",
    )
    assert client.base_url == "http://localhost:3001"
    assert client.api_key == "test-key"
    assert client.stt is not None
    assert client.tts is not None
    assert client.talk is not None
    assert client.transcribe is not None


def test_audio_processor_float_to_int():
    """Test audio conversion."""
    from bud_foundry import AudioProcessor
    import struct
    
    # Create float32 samples
    floats = [0.5, -0.5, 1.0, -1.0]
    float_bytes = struct.pack("<4f", *floats)
    
    # Convert to int16
    int16_bytes = AudioProcessor.float32_to_int16(float_bytes)
    
    # Verify
    int16_samples = struct.unpack("<4h", int16_bytes)
    assert int16_samples[0] == int(0.5 * 32767)
    assert int16_samples[1] == int(-0.5 * 32767)


def test_audio_processor_silence_detection():
    """Test silence detection."""
    from bud_foundry import AudioProcessor
    import struct
    
    # Create silent audio
    silent_samples = [0] * 100
    silent_bytes = struct.pack(f"<{len(silent_samples)}h", *silent_samples)
    
    assert AudioProcessor.detect_silence(silent_bytes, threshold=0.01) is True
    
    # Create non-silent audio
    loud_samples = [16384] * 100
    loud_bytes = struct.pack(f"<{len(loud_samples)}h", *loud_samples)
    
    assert AudioProcessor.detect_silence(loud_bytes, threshold=0.01) is False
