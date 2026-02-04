#!/usr/bin/env python3
"""
Download Real Audio Samples from the Web

Downloads high-quality speech samples from various public domain sources
for comprehensive WaaV Gateway testing.
"""

import os
import subprocess
import tempfile
import hashlib
import logging
from pathlib import Path
from typing import List, Tuple, Optional
import urllib.request
import ssl

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Output directory
AUDIO_DIR = Path(__file__).parent.parent / "audio"

# Target audio format
TARGET_SAMPLE_RATE = 16000
TARGET_CHANNELS = 1
MAX_DURATION_SEC = 10


def ensure_audio_dir():
    """Ensure audio directory exists."""
    AUDIO_DIR.mkdir(parents=True, exist_ok=True)
    return AUDIO_DIR


def check_ffmpeg():
    """Check if ffmpeg is available."""
    try:
        result = subprocess.run(["ffmpeg", "-version"], capture_output=True, text=True)
        return result.returncode == 0
    except FileNotFoundError:
        return False


def download_file(url: str, output_path: Path, timeout: int = 120) -> bool:
    """Download a file from URL."""
    try:
        logger.info(f"Downloading: {url}")

        # Use curl for better reliability
        result = subprocess.run(
            ["curl", "-L", "-o", str(output_path), "-m", str(timeout),
             "--retry", "3", "--retry-delay", "2", url],
            capture_output=True,
            text=True,
            timeout=timeout + 30
        )

        if result.returncode == 0 and output_path.exists() and output_path.stat().st_size > 0:
            logger.info(f"Downloaded: {output_path.name} ({output_path.stat().st_size / 1024:.1f} KB)")
            return True
        else:
            logger.error(f"Download failed: {result.stderr}")
            return False

    except subprocess.TimeoutExpired:
        logger.error(f"Download timeout: {url}")
        return False
    except Exception as e:
        logger.error(f"Download error: {e}")
        return False


def convert_to_wav(input_path: Path, output_path: Path,
                   sample_rate: int = TARGET_SAMPLE_RATE,
                   channels: int = TARGET_CHANNELS,
                   max_duration: int = MAX_DURATION_SEC) -> bool:
    """Convert audio file to WAV format suitable for STT."""
    try:
        logger.info(f"Converting: {input_path.name} -> {output_path.name}")

        cmd = [
            "ffmpeg", "-y", "-i", str(input_path),
            "-ar", str(sample_rate),
            "-ac", str(channels),
            "-sample_fmt", "s16",
            "-t", str(max_duration),
            str(output_path)
        ]

        result = subprocess.run(cmd, capture_output=True, text=True, timeout=60)

        if result.returncode == 0 and output_path.exists():
            logger.info(f"Converted: {output_path.name} ({output_path.stat().st_size / 1024:.1f} KB)")
            return True
        else:
            logger.error(f"Conversion failed: {result.stderr}")
            return False

    except subprocess.TimeoutExpired:
        logger.error("Conversion timeout")
        return False
    except Exception as e:
        logger.error(f"Conversion error: {e}")
        return False


def get_audio_info(filepath: Path) -> Optional[dict]:
    """Get audio file information using ffprobe."""
    try:
        cmd = [
            "ffprobe", "-v", "quiet", "-print_format", "json",
            "-show_format", "-show_streams", str(filepath)
        ]
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)

        if result.returncode == 0:
            import json
            return json.loads(result.stdout)
        return None
    except Exception:
        return None


# Public domain audio sources
AUDIO_SOURCES = [
    # LibriVox - Public domain audiobooks
    {
        "name": "librivox_alice",
        "url": "https://archive.org/download/alice_in_wonderland_librivox/alice_in_wonderland_01_carroll.mp3",
        "description": "Alice in Wonderland - Chapter 1 (LibriVox)",
        "expected_content": "Alice was beginning to get very tired"
    },
    {
        "name": "librivox_sherlock",
        "url": "https://archive.org/download/adventures_of_sherlock_holmes_0711_librivox/adventuresofsherlock_01_doyle.mp3",
        "description": "Sherlock Holmes - A Scandal in Bohemia (LibriVox)",
        "expected_content": "To Sherlock Holmes she is always the woman"
    },
    {
        "name": "librivox_pride",
        "url": "https://archive.org/download/pride_and_prejudice_librivox/prideandprejudice_01_austen.mp3",
        "description": "Pride and Prejudice - Chapter 1 (LibriVox)",
        "expected_content": "It is a truth universally acknowledged"
    },

    # UIC Speech samples
    {
        "name": "gettysburg",
        "url": "https://www2.cs.uic.edu/~i101/SoundFiles/gettysburg10.wav",
        "description": "Gettysburg Address excerpt",
        "expected_content": "Four score and seven years ago"
    },
    {
        "name": "preamble",
        "url": "https://www2.cs.uic.edu/~i101/SoundFiles/preamble10.wav",
        "description": "US Constitution Preamble",
        "expected_content": "We the people of the United States"
    },

    # Open Speech Repository samples
    {
        "name": "cmu_arctic_male",
        "url": "http://www.festvox.org/cmu_arctic/cmu_arctic/cmu_us_bdl_arctic/wav/arctic_a0001.wav",
        "description": "CMU Arctic - Male voice sample",
        "expected_content": "Author of the danger trail"
    },
    {
        "name": "cmu_arctic_female",
        "url": "http://www.festvox.org/cmu_arctic/cmu_arctic/cmu_us_clb_arctic/wav/arctic_a0001.wav",
        "description": "CMU Arctic - Female voice sample",
        "expected_content": "Author of the danger trail"
    },

    # VCTK samples (if accessible)
    {
        "name": "voxforge_sample",
        "url": "http://www.repository.voxforge1.org/downloads/SpeechCorpus/Trunk/Audio/Main/16kHz_16bit/",
        "description": "VoxForge speech sample",
        "skip": True  # Directory listing, not direct file
    },
]

# Additional diverse speech samples
DIVERSE_SOURCES = [
    # Mozilla Common Voice samples (need to check URLs)
    {
        "name": "tatoeba_sample_1",
        "url": "https://audio.tatoeba.org/sentences/eng/5849997.mp3",
        "description": "Tatoeba - English sentence sample",
    },
    {
        "name": "tatoeba_sample_2",
        "url": "https://audio.tatoeba.org/sentences/eng/2093.mp3",
        "description": "Tatoeba - I love you",
    },
    {
        "name": "tatoeba_sample_3",
        "url": "https://audio.tatoeba.org/sentences/eng/4815.mp3",
        "description": "Tatoeba - Hello",
    },
]


def download_all_samples() -> List[Path]:
    """Download all audio samples."""
    ensure_audio_dir()

    if not check_ffmpeg():
        logger.error("ffmpeg not found. Please install ffmpeg.")
        return []

    downloaded = []

    all_sources = AUDIO_SOURCES + DIVERSE_SOURCES

    for source in all_sources:
        if source.get("skip"):
            continue

        name = source["name"]
        url = source["url"]
        description = source.get("description", name)

        output_wav = AUDIO_DIR / f"{name}.wav"

        # Skip if already exists
        if output_wav.exists():
            logger.info(f"Already exists: {output_wav.name}")
            downloaded.append(output_wav)
            continue

        # Determine file extension from URL
        ext = url.split('.')[-1].split('?')[0].lower()
        if ext not in ['wav', 'mp3', 'flac', 'ogg', 'm4a', 'aac']:
            ext = 'mp3'  # Default

        # Create temporary file for download
        with tempfile.NamedTemporaryFile(suffix=f".{ext}", delete=False) as tmp:
            tmp_path = Path(tmp.name)

        try:
            # Download
            if download_file(url, tmp_path):
                # Convert to standard WAV
                if convert_to_wav(tmp_path, output_wav):
                    downloaded.append(output_wav)
                    logger.info(f"Success: {name} - {description}")
                else:
                    logger.warning(f"Failed to convert: {name}")
            else:
                logger.warning(f"Failed to download: {name}")

        except Exception as e:
            logger.error(f"Error processing {name}: {e}")

        finally:
            # Clean up temp file
            if tmp_path.exists():
                tmp_path.unlink()

    return downloaded


def create_summary(downloaded: List[Path]):
    """Create a summary of downloaded files."""
    logger.info("\n" + "=" * 60)
    logger.info("DOWNLOAD SUMMARY")
    logger.info("=" * 60)

    total_size = 0
    for filepath in sorted(downloaded):
        if filepath.exists():
            size = filepath.stat().st_size
            total_size += size

            # Get duration if possible
            info = get_audio_info(filepath)
            duration = "unknown"
            if info and "format" in info:
                dur = info["format"].get("duration")
                if dur:
                    duration = f"{float(dur):.2f}s"

            logger.info(f"  {filepath.name}: {size/1024:.1f} KB, {duration}")

    logger.info("=" * 60)
    logger.info(f"Total: {len(downloaded)} files, {total_size/1024:.1f} KB")
    logger.info(f"Location: {AUDIO_DIR}")
    logger.info("=" * 60)


def main():
    """Main entry point."""
    logger.info("=" * 60)
    logger.info("Downloading Real Audio Samples for WaaV Gateway Testing")
    logger.info("=" * 60)

    downloaded = download_all_samples()
    create_summary(downloaded)

    if downloaded:
        logger.info("\nAudio files are ready for testing!")
        logger.info("Run: python waav_test_client.py")
    else:
        logger.warning("\nNo audio files were downloaded.")
        logger.info("Check your internet connection and try again.")


if __name__ == "__main__":
    main()
