#!/usr/bin/env python3
"""
Audio Generator for WaaV Gateway Live Testing

Generates synthetic audio samples and downloads real audio from the web
for comprehensive testing of STT, TTS, noise filtering, and turn detection.
"""

import numpy as np
import struct
import wave
import os
import subprocess
import hashlib
from pathlib import Path
from typing import Tuple, Optional
import logging

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Audio constants
SAMPLE_RATE = 16000  # 16kHz for STT
CHANNELS = 1  # Mono
SAMPLE_WIDTH = 2  # 16-bit

# Output directory
AUDIO_DIR = Path(__file__).parent.parent / "audio"


def ensure_audio_dir():
    """Ensure audio directory exists."""
    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    return AUDIO_DIR


def pcm_to_wav(pcm_data: bytes, sample_rate: int = SAMPLE_RATE,
               channels: int = CHANNELS, sample_width: int = SAMPLE_WIDTH) -> bytes:
    """Convert raw PCM data to WAV format."""
    import io
    buffer = io.BytesIO()
    with wave.open(buffer, 'wb') as wav_file:
        wav_file.setnchannels(channels)
        wav_file.setsampwidth(sample_width)
        wav_file.setframerate(sample_rate)
        wav_file.writeframes(pcm_data)
    return buffer.getvalue()


def save_audio(samples: np.ndarray, filename: str, as_wav: bool = True) -> Path:
    """Save audio samples to file."""
    ensure_audio_dir()

    # Ensure int16 format
    if samples.dtype != np.int16:
        samples = np.clip(samples * 32767, -32768, 32767).astype(np.int16)

    pcm_data = samples.tobytes()

    if as_wav:
        filepath = AUDIO_DIR / f"{filename}.wav"
        with wave.open(str(filepath), 'wb') as wav_file:
            wav_file.setnchannels(CHANNELS)
            wav_file.setsampwidth(SAMPLE_WIDTH)
            wav_file.setframerate(SAMPLE_RATE)
            wav_file.writeframes(pcm_data)
    else:
        filepath = AUDIO_DIR / f"{filename}.pcm"
        with open(filepath, 'wb') as f:
            f.write(pcm_data)

    logger.info(f"Saved audio: {filepath} ({len(samples) / SAMPLE_RATE:.2f}s)")
    return filepath


def generate_silence(duration_sec: float) -> np.ndarray:
    """Generate silent audio."""
    num_samples = int(duration_sec * SAMPLE_RATE)
    return np.zeros(num_samples, dtype=np.int16)


def generate_white_noise(duration_sec: float, amplitude: float = 0.1, seed: int = 42) -> np.ndarray:
    """Generate deterministic white noise."""
    np.random.seed(seed)
    num_samples = int(duration_sec * SAMPLE_RATE)
    noise = np.random.randn(num_samples) * amplitude
    return np.clip(noise * 32767, -32768, 32767).astype(np.int16)


def generate_sine_wave(duration_sec: float, frequency: float = 440.0,
                       amplitude: float = 0.5) -> np.ndarray:
    """Generate a sine wave tone."""
    num_samples = int(duration_sec * SAMPLE_RATE)
    t = np.arange(num_samples) / SAMPLE_RATE
    signal = amplitude * np.sin(2 * np.pi * frequency * t)
    return np.clip(signal * 32767, -32768, 32767).astype(np.int16)


def generate_speech_simulation(duration_sec: float = 3.0) -> np.ndarray:
    """
    Generate speech-like audio with frequency variations and harmonics.
    This simulates the spectral characteristics of human speech.
    """
    num_samples = int(duration_sec * SAMPLE_RATE)
    t = np.arange(num_samples) / SAMPLE_RATE

    # Simulate fundamental frequency variation (F0 contour)
    # Human speech F0 typically ranges from 85-255 Hz
    f0_base = 150  # Base fundamental frequency
    f0_variation = 50 * np.sin(2 * np.pi * 3 * t)  # Slow pitch variation
    f0 = f0_base + f0_variation

    # Generate fundamental with harmonics (formant-like structure)
    signal = np.zeros(num_samples, dtype=np.float64)

    # Fundamental
    phase = np.cumsum(2 * np.pi * f0 / SAMPLE_RATE)
    signal += 0.3 * np.sin(phase)

    # 2nd harmonic
    signal += 0.2 * np.sin(2 * phase)

    # 3rd harmonic
    signal += 0.15 * np.sin(3 * phase)

    # 4th harmonic
    signal += 0.1 * np.sin(4 * phase)

    # 5th harmonic (adds brightness)
    signal += 0.05 * np.sin(5 * phase)

    # Add amplitude modulation (syllable-like structure)
    syllable_rate = 4  # Syllables per second (typical speech rate)
    envelope = 0.5 + 0.5 * np.sin(2 * np.pi * syllable_rate * t)
    signal *= envelope

    # Add slight noise for naturalness
    noise = np.random.randn(num_samples) * 0.02
    signal += noise

    # Normalize
    signal = signal / np.max(np.abs(signal)) * 0.7

    return np.clip(signal * 32767, -32768, 32767).astype(np.int16)


def generate_speech_with_pauses(duration_sec: float = 5.0,
                                 pause_ratio: float = 0.3) -> np.ndarray:
    """Generate speech-like audio with natural pauses."""
    num_samples = int(duration_sec * SAMPLE_RATE)

    # Generate base speech
    speech = generate_speech_simulation(duration_sec)

    # Add pauses
    segment_duration = 0.5  # seconds
    segment_samples = int(segment_duration * SAMPLE_RATE)

    for i in range(0, num_samples, segment_samples):
        if np.random.random() < pause_ratio:
            end_idx = min(i + segment_samples, num_samples)
            speech[i:end_idx] = 0

    return speech


def generate_noisy_speech(duration_sec: float = 3.0, snr_db: float = 10.0) -> np.ndarray:
    """Generate speech-like audio with additive noise at specified SNR."""
    speech = generate_speech_simulation(duration_sec)
    speech_float = speech.astype(np.float64) / 32767.0

    # Calculate speech power
    speech_power = np.mean(speech_float ** 2)

    # Calculate required noise power for target SNR
    noise_power = speech_power / (10 ** (snr_db / 10))
    noise_amplitude = np.sqrt(noise_power)

    # Generate noise
    noise = np.random.randn(len(speech)) * noise_amplitude

    # Mix
    mixed = speech_float + noise

    # Normalize to prevent clipping
    max_val = np.max(np.abs(mixed))
    if max_val > 0.95:
        mixed = mixed * 0.95 / max_val

    return np.clip(mixed * 32767, -32768, 32767).astype(np.int16)


def download_real_audio_samples():
    """
    Download real audio samples from public domain sources for testing.
    Uses freely available speech samples.
    """
    ensure_audio_dir()

    # List of public domain audio samples (LibriSpeech, Common Voice samples, etc.)
    # These are direct links to small audio files that can be used for testing
    audio_sources = [
        {
            "name": "librivox_sample_1",
            "url": "https://www.archive.org/download/count_monte_cristo_0711_librivox/count_of_monte_cristo_001_dumas_64kb.mp3",
            "description": "Count of Monte Cristo LibriVox sample"
        },
        {
            "name": "sample_speech_1",
            "url": "https://www2.cs.uic.edu/~i101/SoundFiles/gettysburg10.wav",
            "description": "Gettysburg Address sample"
        },
        {
            "name": "sample_speech_2",
            "url": "https://www2.cs.uic.edu/~i101/SoundFiles/preamble10.wav",
            "description": "Preamble to the Constitution"
        },
    ]

    downloaded = []

    for source in audio_sources:
        output_path = AUDIO_DIR / f"{source['name']}_original"
        converted_path = AUDIO_DIR / f"{source['name']}.wav"

        # Skip if already exists
        if converted_path.exists():
            logger.info(f"Already exists: {converted_path}")
            downloaded.append(converted_path)
            continue

        try:
            logger.info(f"Downloading: {source['description']}")

            # Determine output extension from URL
            ext = source['url'].split('.')[-1].split('?')[0]
            original_file = str(output_path) + f".{ext}"

            # Download using curl
            result = subprocess.run(
                ["curl", "-L", "-o", original_file, source['url']],
                capture_output=True,
                text=True,
                timeout=60
            )

            if result.returncode != 0:
                logger.error(f"Download failed: {result.stderr}")
                continue

            # Convert to 16kHz mono WAV using ffmpeg
            logger.info(f"Converting to 16kHz mono WAV: {converted_path}")
            result = subprocess.run(
                [
                    "ffmpeg", "-y", "-i", original_file,
                    "-ar", str(SAMPLE_RATE),
                    "-ac", "1",
                    "-sample_fmt", "s16",
                    "-t", "10",  # Limit to 10 seconds
                    str(converted_path)
                ],
                capture_output=True,
                text=True,
                timeout=60
            )

            if result.returncode == 0:
                downloaded.append(converted_path)
                logger.info(f"Successfully converted: {converted_path}")
                # Clean up original
                os.remove(original_file)
            else:
                logger.error(f"Conversion failed: {result.stderr}")

        except subprocess.TimeoutExpired:
            logger.error(f"Timeout downloading/converting: {source['name']}")
        except Exception as e:
            logger.error(f"Error processing {source['name']}: {e}")

    return downloaded


def download_from_openslr():
    """
    Download samples from OpenSLR (Open Speech and Language Resources).
    These are high-quality speech datasets.
    """
    ensure_audio_dir()

    # We'll use a small sample from LibriSpeech test-clean
    # Direct download of a small WAV file
    sample_url = "https://www.openslr.org/resources/12/test-clean.tar.gz"

    # For quick testing, let's use Google's speech samples
    google_samples = [
        "https://storage.googleapis.com/tfhub-model-hosting/download/google/wiki40b-lm-en/2/wiki40b-lm-en-wiki40b-lm-en-2.zip"
    ]

    # Instead, let's create more realistic synthetic samples
    logger.info("Creating enhanced synthetic speech samples...")

    samples_created = []

    # Create samples with different characteristics
    configs = [
        ("clean_speech_short", 1.0, None, "Short clean speech simulation"),
        ("clean_speech_medium", 3.0, None, "Medium clean speech simulation"),
        ("clean_speech_long", 5.0, None, "Long clean speech simulation"),
        ("noisy_speech_snr20", 3.0, 20.0, "Speech with light noise (20dB SNR)"),
        ("noisy_speech_snr10", 3.0, 10.0, "Speech with moderate noise (10dB SNR)"),
        ("noisy_speech_snr5", 3.0, 5.0, "Speech with heavy noise (5dB SNR)"),
        ("speech_with_pauses", 5.0, None, "Speech with natural pauses"),
        ("silence_200ms", 0.2, None, "200ms silence"),
        ("silence_1s", 1.0, None, "1 second silence"),
    ]

    for name, duration, snr, description in configs:
        filepath = AUDIO_DIR / f"{name}.wav"

        if filepath.exists():
            logger.info(f"Already exists: {filepath}")
            samples_created.append(filepath)
            continue

        logger.info(f"Generating: {description}")

        if "silence" in name:
            samples = generate_silence(duration)
        elif snr is not None:
            samples = generate_noisy_speech(duration, snr)
        elif "pauses" in name:
            samples = generate_speech_with_pauses(duration)
        else:
            samples = generate_speech_simulation(duration)

        save_audio(samples, name)
        samples_created.append(filepath)

    return samples_created


def create_all_test_audio():
    """Create all test audio files needed for comprehensive testing."""
    logger.info("=" * 60)
    logger.info("Creating Test Audio Files for WaaV Gateway Live Testing")
    logger.info("=" * 60)

    all_files = []

    # 1. Generate synthetic samples
    logger.info("\n[1] Generating synthetic audio samples...")
    synthetic_files = download_from_openslr()
    all_files.extend(synthetic_files)

    # 2. Download real audio from the web
    logger.info("\n[2] Downloading real audio samples from the web...")
    real_files = download_real_audio_samples()
    all_files.extend(real_files)

    # 3. Create additional test-specific samples
    logger.info("\n[3] Creating additional test-specific samples...")

    # Tone for testing audio pipeline
    tone_440 = generate_sine_wave(2.0, 440.0, 0.5)
    save_audio(tone_440, "tone_440hz")
    all_files.append(AUDIO_DIR / "tone_440hz.wav")

    # Chirp (frequency sweep) for testing frequency response
    logger.info("Generating chirp signal...")
    t = np.arange(int(3.0 * SAMPLE_RATE)) / SAMPLE_RATE
    start_freq, end_freq = 100, 4000
    chirp = 0.5 * np.sin(2 * np.pi * (start_freq + (end_freq - start_freq) * t / 6) * t)
    chirp_samples = np.clip(chirp * 32767, -32768, 32767).astype(np.int16)
    save_audio(chirp_samples, "chirp_100_4000hz")
    all_files.append(AUDIO_DIR / "chirp_100_4000hz.wav")

    # White noise only
    noise = generate_white_noise(2.0, 0.3)
    save_audio(noise, "white_noise")
    all_files.append(AUDIO_DIR / "white_noise.wav")

    logger.info("\n" + "=" * 60)
    logger.info(f"Created {len(all_files)} test audio files in {AUDIO_DIR}")
    logger.info("=" * 60)

    # Print summary
    logger.info("\nTest Audio Files Summary:")
    for f in sorted(all_files):
        if f.exists():
            size_kb = f.stat().st_size / 1024
            logger.info(f"  - {f.name}: {size_kb:.1f} KB")

    return all_files


if __name__ == "__main__":
    create_all_test_audio()
