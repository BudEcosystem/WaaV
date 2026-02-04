"""
Voice Activity Detection (VAD) for Bud WaaV SDK.

Energy-based VAD with RMS calculation, configurable thresholds,
pre-speech buffering, and speech/silence state tracking.
"""

import math
import struct
import time
from collections import deque
from dataclasses import dataclass, field
from enum import Enum
from typing import Callable, Optional


class VADState(Enum):
    """VAD state machine states."""

    SILENCE = "silence"
    SPEECH = "speech"
    UNCERTAIN = "uncertain"


@dataclass
class VADConfig:
    """Configuration for Voice Activity Detection.

    Attributes:
        energy_threshold_db: Energy threshold in dB for speech detection.
            Frames above this are considered potential speech. Default -35 dB.
        min_speech_duration_ms: Minimum consecutive speech duration (ms) before
            confirming speech onset. Prevents false triggers. Default 250ms.
        min_silence_duration_ms: Minimum consecutive silence duration (ms) before
            confirming speech end. Allows for natural pauses. Default 500ms.
        pre_speech_buffer_ms: Audio to retain before confirmed speech start (ms).
            Captures the onset that triggered detection. Default 300ms.
        smoothing_factor: Exponential smoothing for energy calculation.
            Higher values = more smoothing (0.0-1.0). Default 0.95.
        sample_rate: Audio sample rate in Hz. Default 16000.
        sample_width: Bytes per sample. 2 for int16, 4 for float32. Default 2.
        frame_duration_ms: Expected frame duration in ms. Default 20ms.
    """

    energy_threshold_db: float = -35.0
    min_speech_duration_ms: int = 250
    min_silence_duration_ms: int = 500
    pre_speech_buffer_ms: int = 300
    smoothing_factor: float = 0.95
    sample_rate: int = 16000
    sample_width: int = 2
    frame_duration_ms: int = 20


@dataclass
class VADFrame:
    """Result of processing a single audio frame.

    Attributes:
        state: Current VAD state after processing this frame.
        energy_db: Measured energy in dB for this frame.
        smoothed_energy_db: Exponentially smoothed energy in dB.
        is_speech: True if current state is SPEECH.
        timestamp_ms: Monotonic timestamp when frame was processed.
    """

    state: VADState
    energy_db: float
    smoothed_energy_db: float
    is_speech: bool
    timestamp_ms: float


class VoiceActivityDetector:
    """Energy-based Voice Activity Detector.

    Uses RMS energy with exponential smoothing and a state machine
    to detect speech onset and offset. Supports pre-speech buffering
    to capture audio before speech was confirmed.

    The state machine transitions:
      SILENCE → UNCERTAIN (energy above threshold)
      UNCERTAIN → SPEECH (energy sustained for min_speech_duration_ms)
      UNCERTAIN → SILENCE (energy drops before min_speech_duration_ms)
      SPEECH → UNCERTAIN (energy below threshold)
      UNCERTAIN → SILENCE (energy low for min_silence_duration_ms)

    Example::

        vad = VoiceActivityDetector(VADConfig(
            energy_threshold_db=-35.0,
            min_speech_duration_ms=250,
        ))

        vad.on_speech_start = lambda: print("Speech started")
        vad.on_speech_end = lambda pre_speech: print(f"Speech ended, {len(pre_speech)} pre-speech bytes")

        for chunk in audio_chunks:
            frame = vad.process(chunk)
            if frame.is_speech:
                # Process speech audio
                pass
    """

    def __init__(self, config: Optional[VADConfig] = None) -> None:
        self._config = config or VADConfig()
        self._state = VADState.SILENCE
        self._smoothed_energy_db: float = -96.0  # Near silence floor
        self._frames_in_state: int = 0
        self._total_frames: int = 0
        self._was_speaking: bool = False  # Track previous speech state for uncertain→silence

        # Pre-speech circular buffer
        max_pre_speech_frames = max(
            1,
            self._config.pre_speech_buffer_ms // self._config.frame_duration_ms,
        )
        self._pre_speech_buffer: deque[bytes] = deque(maxlen=max_pre_speech_frames)

        # Event callbacks
        self.on_speech_start: Optional[Callable[[], None]] = None
        self.on_speech_end: Optional[Callable[[bytes], None]] = None
        self.on_frame: Optional[Callable[[VADFrame], None]] = None

    @property
    def state(self) -> VADState:
        """Current VAD state."""
        return self._state

    @property
    def is_speaking(self) -> bool:
        """Whether speech is currently detected."""
        return self._state == VADState.SPEECH

    @property
    def config(self) -> VADConfig:
        """Current VAD configuration."""
        return self._config

    def set_threshold(self, energy_threshold_db: float) -> None:
        """Update the energy threshold at runtime.

        Args:
            energy_threshold_db: New threshold in dB.
        """
        self._config.energy_threshold_db = energy_threshold_db

    def reset(self) -> None:
        """Reset VAD state to initial silence."""
        self._state = VADState.SILENCE
        self._smoothed_energy_db = -96.0
        self._frames_in_state = 0
        self._total_frames = 0
        self._was_speaking = False
        self._pre_speech_buffer.clear()

    def process(self, audio: bytes) -> VADFrame:
        """Process an audio frame and update VAD state.

        Uses frame-counted timing (frames * frame_duration_ms) instead of
        wall-clock time, which is more correct for audio processing and
        deterministic in tests.

        Args:
            audio: Raw PCM audio bytes (int16 or float32 depending on config).

        Returns:
            VADFrame with detection results for this frame.
        """
        self._total_frames += 1
        self._frames_in_state += 1
        now_ms = _monotonic_ms()
        energy_db = _calculate_energy_db(audio, self._config.sample_width)

        # Exponential smoothing
        alpha = 1.0 - self._config.smoothing_factor
        self._smoothed_energy_db = (
            alpha * energy_db + self._config.smoothing_factor * self._smoothed_energy_db
        )

        is_above_threshold = self._smoothed_energy_db > self._config.energy_threshold_db
        duration_in_state_ms = self._frames_in_state * self._config.frame_duration_ms

        if self._state == VADState.SILENCE:
            if is_above_threshold:
                self._state = VADState.UNCERTAIN
                self._frames_in_state = 0
                self._was_speaking = False
            # Buffer audio for pre-speech capture
            self._pre_speech_buffer.append(audio)

        elif self._state == VADState.UNCERTAIN:
            if not self._was_speaking:
                # Transitioning from silence → uncertain → maybe speech
                if is_above_threshold:
                    self._pre_speech_buffer.append(audio)
                    if duration_in_state_ms >= self._config.min_speech_duration_ms:
                        self._state = VADState.SPEECH
                        self._frames_in_state = 0
                        self._was_speaking = True
                        if self.on_speech_start:
                            self.on_speech_start()
                else:
                    # Dropped back below threshold before confirmation
                    self._state = VADState.SILENCE
                    self._frames_in_state = 0
            else:
                # Transitioning from speech → uncertain → maybe silence
                if not is_above_threshold:
                    if duration_in_state_ms >= self._config.min_silence_duration_ms:
                        pre_speech = b"".join(self._pre_speech_buffer)
                        self._state = VADState.SILENCE
                        self._frames_in_state = 0
                        self._was_speaking = False
                        self._pre_speech_buffer.clear()
                        if self.on_speech_end:
                            self.on_speech_end(pre_speech)
                else:
                    # Energy came back up, return to speech
                    self._state = VADState.SPEECH
                    self._frames_in_state = 0

        elif self._state == VADState.SPEECH:
            if not is_above_threshold:
                self._state = VADState.UNCERTAIN
                self._frames_in_state = 0

        frame = VADFrame(
            state=self._state,
            energy_db=energy_db,
            smoothed_energy_db=self._smoothed_energy_db,
            is_speech=self._state == VADState.SPEECH,
            timestamp_ms=now_ms,
        )

        if self.on_frame:
            self.on_frame(frame)

        return frame

    def get_pre_speech_audio(self) -> bytes:
        """Get buffered pre-speech audio.

        Returns:
            Concatenated pre-speech audio bytes, or empty bytes if none buffered.
        """
        return b"".join(self._pre_speech_buffer)


def _monotonic_ms() -> float:
    """Get monotonic time in milliseconds."""
    return time.monotonic() * 1000.0


def _calculate_energy_db(audio: bytes, sample_width: int = 2) -> float:
    """Calculate frame energy in dB.

    Uses RMS (Root Mean Square) of the audio samples, converted to dB scale.
    Returns -96.0 dB for silence (zero energy).

    Args:
        audio: Raw PCM audio bytes.
        sample_width: Bytes per sample (2=int16, 4=float32).

    Returns:
        Energy level in dB. Typical range: -96 (silence) to 0 (full scale).
    """
    fmt = "h" if sample_width == 2 else "f"
    num_samples = len(audio) // sample_width

    if num_samples == 0:
        return -96.0

    samples = struct.unpack(f"<{num_samples}{fmt}", audio)

    # RMS calculation
    sum_sq = sum(s * s for s in samples)
    rms = (sum_sq / num_samples) ** 0.5

    # Normalize int16 to 0-1 range
    if sample_width == 2:
        rms /= 32767.0

    if rms < 1e-10:
        return -96.0

    return 20.0 * math.log10(rms)
