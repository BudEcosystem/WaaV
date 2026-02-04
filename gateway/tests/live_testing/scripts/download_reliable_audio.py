#!/usr/bin/env python3
"""
Download Reliable Audio Samples

Uses more reliable sources for real audio samples:
- CMU Arctic (speech synthesis corpus)
- Open Speech and Language Resources
- Direct WAV files that don't require conversion
"""

import subprocess
import tempfile
import logging
from pathlib import Path
import os
import time

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

AUDIO_DIR = Path(__file__).parent.parent / "audio"
TARGET_SAMPLE_RATE = 16000


def ensure_audio_dir():
    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    return AUDIO_DIR


def download_with_wget(url: str, output_path: Path, timeout: int = 60) -> bool:
    """Download using wget with resume support."""
    try:
        result = subprocess.run(
            ["wget", "-q", "-O", str(output_path), "--timeout", str(timeout), url],
            capture_output=True,
            text=True,
            timeout=timeout + 30
        )
        return result.returncode == 0 and output_path.exists() and output_path.stat().st_size > 1000
    except Exception as e:
        logger.error(f"wget failed: {e}")
        return False


def convert_audio(input_path: Path, output_path: Path, max_duration: int = 10) -> bool:
    """Convert audio to 16kHz mono WAV."""
    try:
        cmd = [
            "ffmpeg", "-y",
            "-i", str(input_path),
            "-ar", str(TARGET_SAMPLE_RATE),
            "-ac", "1",
            "-sample_fmt", "s16",
            "-t", str(max_duration),
            str(output_path)
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        return result.returncode == 0 and output_path.exists()
    except Exception as e:
        logger.error(f"Conversion failed: {e}")
        return False


# Reliable audio sources - CMU Arctic direct WAV files
CMU_ARCTIC_VOICES = [
    ("bdl", "male", "American English"),
    ("clb", "female", "American English"),
    ("slt", "female", "American English"),
    ("rms", "male", "American English"),
]

CMU_ARCTIC_SENTENCES = [
    "arctic_a0001",
    "arctic_a0002",
    "arctic_a0003",
    "arctic_a0004",
    "arctic_a0005",
]


def download_cmu_arctic_samples():
    """Download CMU Arctic speech samples - these are reliable WAV files."""
    ensure_audio_dir()
    downloaded = []

    base_url = "http://www.festvox.org/cmu_arctic/cmu_arctic/cmu_us_{voice}_arctic/wav/{sentence}.wav"

    # Download a few samples from different voices
    for voice_code, gender, accent in CMU_ARCTIC_VOICES[:2]:  # Just first 2 voices
        for sentence in CMU_ARCTIC_SENTENCES[:2]:  # Just first 2 sentences
            name = f"cmu_{voice_code}_{sentence}"
            url = base_url.format(voice=voice_code, sentence=sentence)
            output_wav = AUDIO_DIR / f"{name}.wav"

            if output_wav.exists():
                logger.info(f"Already exists: {output_wav.name}")
                downloaded.append(output_wav)
                continue

            logger.info(f"Downloading: {name} ({gender}, {accent})")

            # Download directly to temp file first
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
                tmp_path = Path(tmp.name)

            try:
                if download_with_wget(url, tmp_path):
                    # Convert to standard format
                    if convert_audio(tmp_path, output_wav):
                        downloaded.append(output_wav)
                        logger.info(f"Success: {name}")
                    else:
                        # Try direct copy if conversion fails (already in right format)
                        import shutil
                        shutil.copy(tmp_path, output_wav)
                        if output_wav.exists():
                            downloaded.append(output_wav)
                            logger.info(f"Success (direct copy): {name}")
            finally:
                if tmp_path.exists():
                    tmp_path.unlink()

            time.sleep(0.5)  # Be polite to servers

    return downloaded


def download_openslr_samples():
    """
    Download from OpenSLR - free speech and language resources.
    Using mini_librispeech for quick download.
    """
    ensure_audio_dir()
    downloaded = []

    # Mini LibriSpeech dev-clean-2 (small sample)
    # This is a direct link to a single FLAC file
    sample_urls = [
        # These are direct links to individual audio files
        ("https://www.voiptroubleshooter.com/open_speech/american/OSR_us_000_0010_8k.wav",
         "osr_american_sample", "Open Speech Repository - American English"),
    ]

    for url, name, description in sample_urls:
        output_wav = AUDIO_DIR / f"{name}.wav"

        if output_wav.exists():
            logger.info(f"Already exists: {output_wav.name}")
            downloaded.append(output_wav)
            continue

        logger.info(f"Downloading: {description}")

        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            tmp_path = Path(tmp.name)

        try:
            if download_with_wget(url, tmp_path):
                if convert_audio(tmp_path, output_wav):
                    downloaded.append(output_wav)
                    logger.info(f"Success: {name}")
        finally:
            if tmp_path.exists():
                tmp_path.unlink()

    return downloaded


def download_tatoeba_samples():
    """Download Tatoeba sentence audio samples."""
    ensure_audio_dir()
    downloaded = []

    # Tatoeba has CC-licensed audio from native speakers
    tatoeba_samples = [
        ("https://audio.tatoeba.org/sentences/eng/1.mp3", "tatoeba_1", "I won!"),
        ("https://audio.tatoeba.org/sentences/eng/9.mp3", "tatoeba_9", "Relax."),
        ("https://audio.tatoeba.org/sentences/eng/10.mp3", "tatoeba_10", "Smile!"),
        ("https://audio.tatoeba.org/sentences/eng/20.mp3", "tatoeba_20", "Who?"),
        ("https://audio.tatoeba.org/sentences/eng/34.mp3", "tatoeba_34", "I slept."),
        ("https://audio.tatoeba.org/sentences/eng/60.mp3", "tatoeba_60", "I'm cold."),
        ("https://audio.tatoeba.org/sentences/eng/100.mp3", "tatoeba_100", "I'm here."),
        ("https://audio.tatoeba.org/sentences/eng/240.mp3", "tatoeba_240", "Who is he?"),
        ("https://audio.tatoeba.org/sentences/eng/280.mp3", "tatoeba_280", "Run!"),
        ("https://audio.tatoeba.org/sentences/eng/316.mp3", "tatoeba_316", "Thank you."),
    ]

    for url, name, text in tatoeba_samples:
        output_wav = AUDIO_DIR / f"{name}.wav"

        if output_wav.exists():
            logger.info(f"Already exists: {output_wav.name} ('{text}')")
            downloaded.append(output_wav)
            continue

        logger.info(f"Downloading: '{text}'")

        with tempfile.NamedTemporaryFile(suffix=".mp3", delete=False) as tmp:
            tmp_path = Path(tmp.name)

        try:
            if download_with_wget(url, tmp_path, timeout=30):
                if convert_audio(tmp_path, output_wav):
                    downloaded.append(output_wav)
                    logger.info(f"Success: {name} ('{text}')")
        finally:
            if tmp_path.exists():
                tmp_path.unlink()

        time.sleep(0.3)

    return downloaded


def create_real_speech_with_tts():
    """
    Create real speech samples using local TTS if available.
    Falls back to espeak if available.
    """
    ensure_audio_dir()
    downloaded = []

    # Check for espeak
    try:
        result = subprocess.run(["espeak", "--version"], capture_output=True, timeout=5)
        has_espeak = result.returncode == 0
    except:
        has_espeak = False

    if not has_espeak:
        logger.info("espeak not available, skipping TTS-generated samples")
        return downloaded

    phrases = [
        ("tts_hello_world", "Hello world. How are you doing today?"),
        ("tts_numbers", "The year is twenty twenty four. I am number one."),
        ("tts_question", "What time is it? Where are we going?"),
        ("tts_statement", "The quick brown fox jumps over the lazy dog."),
        ("tts_greeting", "Good morning! Welcome to the voice assistant."),
    ]

    for name, text in phrases:
        output_wav = AUDIO_DIR / f"{name}.wav"

        if output_wav.exists():
            logger.info(f"Already exists: {output_wav.name}")
            downloaded.append(output_wav)
            continue

        logger.info(f"Generating TTS: '{text[:40]}...'")

        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            tmp_path = Path(tmp.name)

        try:
            # Generate with espeak
            result = subprocess.run(
                ["espeak", "-w", str(tmp_path), "-v", "en-us", text],
                capture_output=True,
                text=True,
                timeout=30
            )

            if result.returncode == 0 and tmp_path.exists():
                # Convert to standard format
                if convert_audio(tmp_path, output_wav):
                    downloaded.append(output_wav)
                    logger.info(f"Success: {name}")
        finally:
            if tmp_path.exists():
                tmp_path.unlink()

    return downloaded


def main():
    """Main entry point."""
    logger.info("=" * 60)
    logger.info("Downloading Reliable Real Audio Samples")
    logger.info("=" * 60)

    all_downloaded = []

    # 1. CMU Arctic
    logger.info("\n[1/4] CMU Arctic Speech Synthesis Corpus...")
    all_downloaded.extend(download_cmu_arctic_samples())

    # 2. OpenSLR samples
    logger.info("\n[2/4] Open Speech Repository...")
    all_downloaded.extend(download_openslr_samples())

    # 3. Tatoeba samples
    logger.info("\n[3/4] Tatoeba Sentence Audio...")
    all_downloaded.extend(download_tatoeba_samples())

    # 4. TTS-generated samples
    logger.info("\n[4/4] TTS-Generated Samples...")
    all_downloaded.extend(create_real_speech_with_tts())

    # Summary
    logger.info("\n" + "=" * 60)
    logger.info("DOWNLOAD SUMMARY")
    logger.info("=" * 60)

    total_size = 0
    for filepath in sorted(set(all_downloaded)):
        if filepath.exists():
            size = filepath.stat().st_size
            total_size += size
            logger.info(f"  {filepath.name}: {size/1024:.1f} KB")

    logger.info("=" * 60)
    logger.info(f"Total: {len(set(all_downloaded))} files, {total_size/1024:.1f} KB")
    logger.info(f"Location: {AUDIO_DIR}")

    # List all available audio files
    all_wavs = list(AUDIO_DIR.glob("*.wav"))
    logger.info(f"\nTotal audio files available: {len(all_wavs)}")


if __name__ == "__main__":
    main()
