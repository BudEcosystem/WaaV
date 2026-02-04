"""
Tests for Voice Activity Detection (VAD) module.
"""

import struct
import math
import pytest

from bud_waav.audio.vad import (
    VADConfig,
    VADFrame,
    VADState,
    VoiceActivityDetector,
    _calculate_energy_db,
)


def _generate_silence(num_samples: int = 320, sample_width: int = 2) -> bytes:
    """Generate silence (zero samples)."""
    if sample_width == 2:
        return struct.pack(f"<{num_samples}h", *([0] * num_samples))
    return struct.pack(f"<{num_samples}f", *([0.0] * num_samples))


def _generate_tone(
    num_samples: int = 320,
    amplitude: float = 0.5,
    frequency: float = 440.0,
    sample_rate: int = 16000,
    sample_width: int = 2,
) -> bytes:
    """Generate a sine tone at given amplitude."""
    samples = []
    for i in range(num_samples):
        t = i / sample_rate
        value = amplitude * math.sin(2 * math.pi * frequency * t)
        if sample_width == 2:
            samples.append(int(value * 32767))
        else:
            samples.append(value)

    fmt = "h" if sample_width == 2 else "f"
    return struct.pack(f"<{num_samples}{fmt}", *samples)


class TestCalculateEnergyDb:
    """Tests for the energy calculation function."""

    def test_silence_returns_floor(self):
        """Silence should return near -96 dB."""
        audio = _generate_silence()
        db = _calculate_energy_db(audio)
        assert db == -96.0

    def test_empty_audio_returns_floor(self):
        """Empty audio should return -96 dB."""
        db = _calculate_energy_db(b"")
        assert db == -96.0

    def test_full_scale_returns_near_zero(self):
        """Full scale signal should return near 0 dB."""
        # All samples at max value
        num_samples = 320
        samples = [32767] * num_samples
        audio = struct.pack(f"<{num_samples}h", *samples)
        db = _calculate_energy_db(audio)
        assert db == pytest.approx(0.0, abs=0.01)

    def test_loud_tone_above_quiet_tone(self):
        """Louder tone should have higher dB than quieter tone."""
        loud = _generate_tone(amplitude=0.5)
        quiet = _generate_tone(amplitude=0.01)
        db_loud = _calculate_energy_db(loud)
        db_quiet = _calculate_energy_db(quiet)
        assert db_loud > db_quiet

    def test_float32_audio(self):
        """Should handle float32 audio."""
        audio = _generate_tone(amplitude=0.5, sample_width=4)
        db = _calculate_energy_db(audio, sample_width=4)
        assert -10 < db < 0  # Reasonable range for 0.5 amplitude

    def test_sine_amplitude_to_db(self):
        """RMS of sine wave at amplitude A is A/sqrt(2), dB = 20*log10(A/sqrt(2))."""
        amp = 0.5
        audio = _generate_tone(amplitude=amp, num_samples=16000)
        db = _calculate_energy_db(audio)
        expected_db = 20 * math.log10(amp / math.sqrt(2))
        assert db == pytest.approx(expected_db, abs=0.5)


class TestVADConfig:
    """Tests for VAD configuration."""

    def test_defaults(self):
        """Should have reasonable defaults."""
        config = VADConfig()
        assert config.energy_threshold_db == -35.0
        assert config.min_speech_duration_ms == 250
        assert config.min_silence_duration_ms == 500
        assert config.pre_speech_buffer_ms == 300
        assert config.smoothing_factor == 0.95
        assert config.sample_rate == 16000
        assert config.sample_width == 2
        assert config.frame_duration_ms == 20

    def test_custom_config(self):
        """Should accept custom values."""
        config = VADConfig(
            energy_threshold_db=-30.0,
            min_speech_duration_ms=100,
            min_silence_duration_ms=200,
        )
        assert config.energy_threshold_db == -30.0
        assert config.min_speech_duration_ms == 100


class TestVoiceActivityDetector:
    """Tests for the VAD state machine."""

    def test_initial_state_is_silence(self):
        """Should start in SILENCE state."""
        vad = VoiceActivityDetector()
        assert vad.state == VADState.SILENCE
        assert not vad.is_speaking

    def test_silence_stays_silent(self):
        """Silence input should keep SILENCE state."""
        vad = VoiceActivityDetector(VADConfig(smoothing_factor=0.0))
        for _ in range(50):
            frame = vad.process(_generate_silence())
            assert frame.state == VADState.SILENCE
            assert not frame.is_speech

    def test_loud_signal_transitions_to_speech(self):
        """Loud signal sustained should transition through UNCERTAIN to SPEECH."""
        config = VADConfig(
            energy_threshold_db=-40.0,
            min_speech_duration_ms=50,
            smoothing_factor=0.0,
            frame_duration_ms=20,
        )
        vad = VoiceActivityDetector(config)

        seen_states = set()
        for _ in range(20):
            frame = vad.process(_generate_tone(amplitude=0.5))
            seen_states.add(frame.state)

        assert VADState.SPEECH in seen_states

    def test_speech_to_silence_transition(self):
        """Speech followed by silence should transition back to SILENCE."""
        config = VADConfig(
            energy_threshold_db=-40.0,
            min_speech_duration_ms=10,
            min_silence_duration_ms=10,
            smoothing_factor=0.0,
            frame_duration_ms=20,
        )
        vad = VoiceActivityDetector(config)

        # Drive to SPEECH state
        for _ in range(20):
            vad.process(_generate_tone(amplitude=0.5))

        assert vad.is_speaking

        # Drive to SILENCE
        for _ in range(20):
            vad.process(_generate_silence())

        assert not vad.is_speaking

    def test_on_speech_start_callback(self):
        """on_speech_start callback should fire when speech is confirmed."""
        config = VADConfig(
            energy_threshold_db=-40.0,
            min_speech_duration_ms=10,
            smoothing_factor=0.0,
        )
        vad = VoiceActivityDetector(config)

        start_count = 0

        def on_start():
            nonlocal start_count
            start_count += 1

        vad.on_speech_start = on_start

        for _ in range(20):
            vad.process(_generate_tone(amplitude=0.5))

        assert start_count >= 1

    def test_on_speech_end_callback_with_pre_speech(self):
        """on_speech_end callback should provide pre-speech audio."""
        config = VADConfig(
            energy_threshold_db=-40.0,
            min_speech_duration_ms=10,
            min_silence_duration_ms=10,
            smoothing_factor=0.0,
            pre_speech_buffer_ms=100,
            frame_duration_ms=20,
        )
        vad = VoiceActivityDetector(config)

        end_data: list[bytes] = []

        def on_end(pre_speech: bytes):
            end_data.append(pre_speech)

        vad.on_speech_end = on_end

        # Drive to speech
        for _ in range(20):
            vad.process(_generate_tone(amplitude=0.5))
        # Drive to silence
        for _ in range(20):
            vad.process(_generate_silence())

        assert len(end_data) >= 1
        assert len(end_data[0]) > 0  # Pre-speech audio should not be empty

    def test_on_frame_callback(self):
        """on_frame callback should fire for every frame."""
        vad = VoiceActivityDetector()
        frames: list[VADFrame] = []
        vad.on_frame = lambda f: frames.append(f)

        for _ in range(5):
            vad.process(_generate_silence())

        assert len(frames) == 5
        for f in frames:
            assert isinstance(f, VADFrame)
            assert f.timestamp_ms > 0

    def test_reset(self):
        """reset() should return to initial SILENCE state."""
        config = VADConfig(
            energy_threshold_db=-40.0,
            min_speech_duration_ms=10,
            smoothing_factor=0.0,
        )
        vad = VoiceActivityDetector(config)

        # Drive to speech
        for _ in range(20):
            vad.process(_generate_tone(amplitude=0.5))

        vad.reset()
        assert vad.state == VADState.SILENCE
        assert not vad.is_speaking

    def test_set_threshold(self):
        """set_threshold() should update threshold at runtime."""
        vad = VoiceActivityDetector()
        assert vad.config.energy_threshold_db == -35.0

        vad.set_threshold(-20.0)
        assert vad.config.energy_threshold_db == -20.0

    def test_get_pre_speech_audio(self):
        """get_pre_speech_audio() should return buffered audio."""
        config = VADConfig(
            pre_speech_buffer_ms=100,
            frame_duration_ms=20,
        )
        vad = VoiceActivityDetector(config)

        # Process some silence frames (these get buffered)
        for _ in range(5):
            vad.process(_generate_silence())

        pre_speech = vad.get_pre_speech_audio()
        assert len(pre_speech) > 0

    def test_pre_speech_buffer_size_limited(self):
        """Pre-speech buffer should not grow beyond configured max."""
        config = VADConfig(
            pre_speech_buffer_ms=60,  # 3 frames at 20ms each
            frame_duration_ms=20,
        )
        vad = VoiceActivityDetector(config)

        # Process many silence frames
        for _ in range(100):
            vad.process(_generate_silence(num_samples=320))

        pre_speech = vad.get_pre_speech_audio()
        # Should be limited to ~3 frames worth
        max_bytes = 3 * 320 * 2  # 3 frames * 320 samples * 2 bytes/sample
        assert len(pre_speech) <= max_bytes

    def test_frame_energy_values_correct(self):
        """Frame should contain correct energy values."""
        vad = VoiceActivityDetector(VADConfig(smoothing_factor=0.0))

        frame = vad.process(_generate_silence())
        assert frame.energy_db == -96.0

        frame = vad.process(_generate_tone(amplitude=0.5, num_samples=16000))
        assert frame.energy_db > -10  # Loud signal

    def test_uncertain_state_exists(self):
        """UNCERTAIN state should appear during transitions."""
        config = VADConfig(
            energy_threshold_db=-40.0,
            min_speech_duration_ms=100,
            smoothing_factor=0.0,
        )
        vad = VoiceActivityDetector(config)

        # First frame above threshold → UNCERTAIN
        frame = vad.process(_generate_tone(amplitude=0.5))
        assert frame.state == VADState.UNCERTAIN
