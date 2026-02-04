#!/usr/bin/env python3
"""
Create noisy versions of real speech samples for noise filter testing.
"""

import numpy as np
import wave
from pathlib import Path
import logging

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

AUDIO_DIR = Path(__file__).parent.parent / "audio"


def load_wav(filepath: Path) -> tuple:
    """Load WAV file and return samples and params."""
    with wave.open(str(filepath), 'rb') as wav:
        params = wav.getparams()
        frames = wav.readframes(params.nframes)
        samples = np.frombuffer(frames, dtype=np.int16)
    return samples, params


def save_wav(filepath: Path, samples: np.ndarray, params):
    """Save samples to WAV file."""
    with wave.open(str(filepath), 'wb') as wav:
        wav.setparams(params)
        wav.writeframes(samples.astype(np.int16).tobytes())


def add_noise(samples: np.ndarray, snr_db: float) -> np.ndarray:
    """Add Gaussian noise at specified SNR."""
    samples_float = samples.astype(np.float64)

    # Calculate signal power
    signal_power = np.mean(samples_float ** 2)

    # Calculate noise power for target SNR
    noise_power = signal_power / (10 ** (snr_db / 10))
    noise_std = np.sqrt(noise_power)

    # Generate and add noise
    noise = np.random.randn(len(samples_float)) * noise_std
    noisy = samples_float + noise

    # Clip to int16 range
    noisy = np.clip(noisy, -32768, 32767)

    return noisy.astype(np.int16)


def main():
    # Use real speech sample as base
    base_files = [
        "cmu_bdl_arctic_a0001.wav",
        "tts_statement.wav",
    ]

    snr_levels = [20, 10, 5]  # dB

    for base_file in base_files:
        base_path = AUDIO_DIR / base_file
        if not base_path.exists():
            logger.warning(f"Base file not found: {base_file}")
            continue

        samples, params = load_wav(base_path)
        base_name = base_file.replace('.wav', '')

        for snr in snr_levels:
            output_name = f"real_{base_name}_snr{snr}.wav"
            output_path = AUDIO_DIR / output_name

            if output_path.exists():
                logger.info(f"Already exists: {output_name}")
                continue

            noisy_samples = add_noise(samples, snr)
            save_wav(output_path, noisy_samples, params)
            logger.info(f"Created: {output_name}")


if __name__ == "__main__":
    main()
