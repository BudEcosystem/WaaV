"""
Audio processing utilities for Bud Foundry SDK
"""

import struct
from typing import Union


class AudioProcessor:
    """Audio processing utilities for PCM conversion."""

    @staticmethod
    def float32_to_int16(audio: bytes) -> bytes:
        """
        Convert float32 PCM audio to int16 PCM.

        Args:
            audio: Float32 PCM audio bytes

        Returns:
            Int16 PCM audio bytes
        """
        # Unpack as float32
        num_samples = len(audio) // 4
        floats = struct.unpack(f"<{num_samples}f", audio)

        # Convert to int16
        int16_samples = []
        for f in floats:
            # Clamp to [-1, 1]
            f = max(-1.0, min(1.0, f))
            # Scale to int16 range
            int16_samples.append(int(f * 32767))

        return struct.pack(f"<{num_samples}h", *int16_samples)

    @staticmethod
    def int16_to_float32(audio: bytes) -> bytes:
        """
        Convert int16 PCM audio to float32 PCM.

        Args:
            audio: Int16 PCM audio bytes

        Returns:
            Float32 PCM audio bytes
        """
        # Unpack as int16
        num_samples = len(audio) // 2
        int16s = struct.unpack(f"<{num_samples}h", audio)

        # Convert to float32
        floats = [i / 32767.0 for i in int16s]

        return struct.pack(f"<{num_samples}f", *floats)

    @staticmethod
    def resample(
        audio: bytes,
        from_rate: int,
        to_rate: int,
        sample_width: int = 2,
    ) -> bytes:
        """
        Resample audio using linear interpolation.

        Args:
            audio: Audio bytes
            from_rate: Source sample rate
            to_rate: Target sample rate
            sample_width: Bytes per sample (2 for int16)

        Returns:
            Resampled audio bytes
        """
        if from_rate == to_rate:
            return audio

        # Unpack samples
        format_char = "h" if sample_width == 2 else "f"
        num_samples = len(audio) // sample_width
        samples = list(struct.unpack(f"<{num_samples}{format_char}", audio))

        # Calculate resampling ratio
        ratio = to_rate / from_rate
        new_length = int(num_samples * ratio)

        # Linear interpolation
        new_samples = []
        for i in range(new_length):
            pos = i / ratio
            idx = int(pos)
            frac = pos - idx

            if idx + 1 < num_samples:
                sample = samples[idx] * (1 - frac) + samples[idx + 1] * frac
            else:
                sample = samples[idx]

            if sample_width == 2:
                new_samples.append(int(sample))
            else:
                new_samples.append(sample)

        return struct.pack(f"<{new_length}{format_char}", *new_samples)

    @staticmethod
    def stereo_to_mono(audio: bytes, sample_width: int = 2) -> bytes:
        """
        Convert stereo audio to mono by averaging channels.

        Args:
            audio: Stereo audio bytes
            sample_width: Bytes per sample (2 for int16)

        Returns:
            Mono audio bytes
        """
        format_char = "h" if sample_width == 2 else "f"
        num_samples = len(audio) // sample_width
        samples = list(struct.unpack(f"<{num_samples}{format_char}", audio))

        # Average pairs
        mono_samples = []
        for i in range(0, num_samples, 2):
            if i + 1 < num_samples:
                avg = (samples[i] + samples[i + 1]) / 2
            else:
                avg = samples[i]

            if sample_width == 2:
                mono_samples.append(int(avg))
            else:
                mono_samples.append(avg)

        return struct.pack(f"<{len(mono_samples)}{format_char}", *mono_samples)

    @staticmethod
    def chunk_audio(
        audio: bytes,
        chunk_size: int,
        sample_width: int = 2,
    ) -> list[bytes]:
        """
        Split audio into chunks.

        Args:
            audio: Audio bytes
            chunk_size: Size of each chunk in bytes
            sample_width: Bytes per sample

        Returns:
            List of audio chunks
        """
        # Align to sample boundaries
        aligned_size = (chunk_size // sample_width) * sample_width
        if aligned_size == 0:
            aligned_size = sample_width

        chunks = []
        for i in range(0, len(audio), aligned_size):
            chunks.append(audio[i:i + aligned_size])

        return chunks

    @staticmethod
    def calculate_duration(
        audio: bytes,
        sample_rate: int,
        sample_width: int = 2,
        channels: int = 1,
    ) -> float:
        """
        Calculate audio duration in seconds.

        Args:
            audio: Audio bytes
            sample_rate: Sample rate in Hz
            sample_width: Bytes per sample
            channels: Number of channels

        Returns:
            Duration in seconds
        """
        num_samples = len(audio) // (sample_width * channels)
        return num_samples / sample_rate

    @staticmethod
    def calculate_rms(audio: bytes, sample_width: int = 2) -> float:
        """
        Calculate RMS (Root Mean Square) of audio.

        Args:
            audio: Audio bytes
            sample_width: Bytes per sample

        Returns:
            RMS value (0.0 to 1.0 for normalized audio)
        """
        format_char = "h" if sample_width == 2 else "f"
        num_samples = len(audio) // sample_width

        if num_samples == 0:
            return 0.0

        samples = struct.unpack(f"<{num_samples}{format_char}", audio)

        # Calculate RMS
        sum_squares = sum(s * s for s in samples)
        rms: float = float((sum_squares / num_samples) ** 0.5)

        # Normalize to 0-1 range for int16
        if sample_width == 2:
            rms = rms / 32767.0

        return rms

    @staticmethod
    def detect_silence(
        audio: bytes,
        threshold: float = 0.01,
        sample_width: int = 2,
    ) -> bool:
        """
        Detect if audio chunk is silence.

        Args:
            audio: Audio bytes
            threshold: RMS threshold below which is considered silence
            sample_width: Bytes per sample

        Returns:
            True if audio is silence
        """
        rms = AudioProcessor.calculate_rms(audio, sample_width)
        return rms < threshold

    @staticmethod
    def normalize(
        audio: bytes,
        target_level: float = 0.9,
        sample_width: int = 2,
    ) -> bytes:
        """
        Normalize audio to a target level.

        Args:
            audio: Audio bytes
            target_level: Target peak level (0.0 to 1.0)
            sample_width: Bytes per sample

        Returns:
            Normalized audio bytes
        """
        format_char = "h" if sample_width == 2 else "f"
        num_samples = len(audio) // sample_width

        if num_samples == 0:
            return audio

        samples = list(struct.unpack(f"<{num_samples}{format_char}", audio))

        # Find peak
        max_val = 32767.0 if sample_width == 2 else 1.0
        peak = max(abs(s) for s in samples) if samples else 0

        if peak == 0:
            return audio

        # Calculate gain
        target_peak = target_level * max_val
        gain = target_peak / peak

        # Apply gain
        normalized = []
        for s in samples:
            new_val = s * gain
            if sample_width == 2:
                new_val = max(-32768, min(32767, int(new_val)))
            normalized.append(new_val)

        return struct.pack(f"<{num_samples}{format_char}", *normalized)
