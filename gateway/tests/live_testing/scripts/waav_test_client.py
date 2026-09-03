import os
#!/usr/bin/env python3
"""
WaaV Gateway WebSocket Test Client

Comprehensive test client for live testing of WaaV Gateway:
- STT (Speech-to-Text) with Deepgram
- TTS (Text-to-Speech) with Deepgram
- Noise Filtering (DeepFilterNet)
- Turn Detection (text-based and audio-based)
"""

import asyncio
import json
import time
import wave
import struct
import sys
import logging
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional, List, Dict, Any, Callable
from datetime import datetime
import statistics

# Third-party imports
try:
    import websockets
    from websockets.exceptions import ConnectionClosed
except ImportError:
    print("Please install websockets: pip install websockets")
    sys.exit(1)

try:
    import numpy as np
except ImportError:
    print("Please install numpy: pip install numpy")
    sys.exit(1)

# Logging setup
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s.%(msecs)03d - %(levelname)s - %(message)s',
    datefmt='%H:%M:%S'
)
logger = logging.getLogger(__name__)

# Configuration
GATEWAY_URL = "ws://localhost:3001/ws"
DEEPGRAM_API_KEY = os.environ.get("DEEPGRAM_API_KEY", "")

# Audio constants
SAMPLE_RATE = 16000
CHANNELS = 1
CHUNK_SIZE = 3200  # 100ms at 16kHz (16000 * 0.1 * 2 bytes)


@dataclass
class TestResult:
    """Container for test results."""
    test_name: str
    success: bool
    duration_ms: float
    message: str = ""
    details: Dict[str, Any] = field(default_factory=dict)
    errors: List[str] = field(default_factory=list)
    latencies: List[float] = field(default_factory=list)

    def to_dict(self) -> dict:
        result = {
            "test_name": self.test_name,
            "success": self.success,
            "duration_ms": round(self.duration_ms, 2),
            "message": self.message,
        }
        if self.details:
            result["details"] = self.details
        if self.errors:
            result["errors"] = self.errors
        if self.latencies:
            result["latency_stats"] = {
                "min_ms": round(min(self.latencies), 2),
                "max_ms": round(max(self.latencies), 2),
                "mean_ms": round(statistics.mean(self.latencies), 2),
                "p50_ms": round(statistics.median(self.latencies), 2),
                "p99_ms": round(sorted(self.latencies)[int(len(self.latencies) * 0.99)] if len(self.latencies) > 1 else self.latencies[0], 2),
            }
        return result


class WaavTestClient:
    """WebSocket client for testing WaaV Gateway."""

    def __init__(self, gateway_url: str = GATEWAY_URL, api_key: str = DEEPGRAM_API_KEY):
        self.gateway_url = gateway_url
        self.api_key = api_key
        self.ws: Optional[websockets.WebSocketClientProtocol] = None
        self.stream_id: Optional[str] = None
        self.results: List[TestResult] = []

        # Message collectors
        self.stt_results: List[Dict] = []
        self.tts_audio: List[bytes] = []
        self.turn_events: List[Dict] = []
        self.errors: List[Dict] = []
        self.all_messages: List[Dict] = []

        # Timing
        self.message_timestamps: Dict[str, float] = {}

    async def connect(self) -> bool:
        """Establish WebSocket connection."""
        try:
            logger.info(f"Connecting to {self.gateway_url}...")
            self.ws = await websockets.connect(
                self.gateway_url,
                max_size=10 * 1024 * 1024,  # 10MB max message size
                ping_interval=20,
                ping_timeout=60
            )
            logger.info("WebSocket connection established")
            return True
        except Exception as e:
            logger.error(f"Connection failed: {e}")
            return False

    async def disconnect(self):
        """Close WebSocket connection."""
        if self.ws:
            await self.ws.close()
            self.ws = None
            logger.info("Disconnected from gateway")

    async def send_config(self, stt_enabled: bool = True, tts_enabled: bool = True,
                          stream_id: Optional[str] = None,
                          extra_config: Optional[Dict] = None) -> bool:
        """Send configuration message to gateway.

        Note: Gateway requires TTS config when audio=true, so we always provide both.
        """
        config = {
            "type": "config",
            "audio": stt_enabled,
        }

        if stream_id:
            config["stream_id"] = stream_id

        # STT config (always needed when audio=true)
        if stt_enabled:
            config["stt_config"] = {
                "provider": "deepgram",
                "api_key": self.api_key,
                "language": "en",
                "sample_rate": SAMPLE_RATE,
                "channels": CHANNELS,
                "punctuation": True,
                "encoding": "linear16",
                "model": "nova-2",
                "interim_results": True,
                "endpointing": 500,
            }

        # TTS config (always required by gateway when audio=true)
        # Always provide TTS config since gateway requires it
        config["tts_config"] = {
            "provider": "deepgram",
            "api_key": self.api_key,
            "voice_id": "aura-asteria-en",
            "sample_rate": 24000,
            "model": "aura-asteria-en",
        }

        if extra_config:
            config.update(extra_config)

        try:
            logger.info(f"Sending config: {json.dumps(config, indent=2)}")
            await self.ws.send(json.dumps(config))

            # Wait for ready message
            response = await asyncio.wait_for(self.ws.recv(), timeout=10)
            data = json.loads(response)

            if data.get("type") == "ready":
                self.stream_id = data.get("stream_id")
                logger.info(f"Received ready message. Stream ID: {self.stream_id}")
                return True
            elif data.get("type") == "error":
                logger.error(f"Config error: {data.get('message')}")
                return False
            else:
                logger.warning(f"Unexpected response: {data}")
                return False

        except asyncio.TimeoutError:
            logger.error("Timeout waiting for ready message")
            return False
        except Exception as e:
            logger.error(f"Config error: {e}")
            return False

    async def send_audio_file(self, filepath: Path, chunk_delay_ms: float = 100) -> int:
        """Stream audio file to gateway in chunks."""
        if not self.ws:
            raise RuntimeError("Not connected")

        if not filepath.exists():
            raise FileNotFoundError(f"Audio file not found: {filepath}")

        # Read WAV file
        with wave.open(str(filepath), 'rb') as wav_file:
            sample_rate = wav_file.getframerate()
            channels = wav_file.getnchannels()
            sample_width = wav_file.getsampwidth()
            n_frames = wav_file.getnframes()
            audio_data = wav_file.readframes(n_frames)

        logger.info(f"Streaming audio: {filepath.name}")
        logger.info(f"  Sample rate: {sample_rate}Hz, Channels: {channels}, "
                   f"Duration: {n_frames / sample_rate:.2f}s")

        # Stream in chunks
        bytes_sent = 0
        chunk_size = int(sample_rate * 0.1 * sample_width * channels)  # 100ms chunks

        self.message_timestamps["audio_start"] = time.time()

        for i in range(0, len(audio_data), chunk_size):
            chunk = audio_data[i:i + chunk_size]
            await self.ws.send(chunk)
            bytes_sent += len(chunk)
            await asyncio.sleep(chunk_delay_ms / 1000)

        self.message_timestamps["audio_end"] = time.time()
        logger.info(f"Sent {bytes_sent} bytes of audio")

        # Signal end of audio to trigger CloseStream and get speech_final
        audio_end_msg = {"type": "audio_end"}
        await self.ws.send(json.dumps(audio_end_msg))
        logger.debug("Sent audio_end signal to finalize STT stream")

        return bytes_sent

    async def send_audio_bytes(self, audio_data: bytes, chunk_delay_ms: float = 100) -> int:
        """Stream raw audio bytes to gateway."""
        if not self.ws:
            raise RuntimeError("Not connected")

        bytes_sent = 0
        self.message_timestamps["audio_start"] = time.time()

        for i in range(0, len(audio_data), CHUNK_SIZE):
            chunk = audio_data[i:i + CHUNK_SIZE]
            await self.ws.send(chunk)
            bytes_sent += len(chunk)
            await asyncio.sleep(chunk_delay_ms / 1000)

        self.message_timestamps["audio_end"] = time.time()

        # Signal end of audio to trigger CloseStream and get speech_final
        audio_end_msg = {"type": "audio_end"}
        await self.ws.send(json.dumps(audio_end_msg))
        logger.debug("Sent audio_end signal to finalize STT stream")

        return bytes_sent

    async def send_speak(self, text: str, flush: bool = True,
                         allow_interruption: bool = True) -> None:
        """Send TTS speak command."""
        if not self.ws:
            raise RuntimeError("Not connected")

        message = {
            "type": "speak",
            "text": text,
            "flush": flush,
            "allow_interruption": allow_interruption
        }

        self.message_timestamps["speak_sent"] = time.time()
        logger.info(f"Sending speak: '{text[:50]}...' " if len(text) > 50 else f"Sending speak: '{text}'")
        await self.ws.send(json.dumps(message))

    async def send_clear(self) -> None:
        """Send clear/interrupt command."""
        if not self.ws:
            raise RuntimeError("Not connected")

        await self.ws.send(json.dumps({"type": "clear"}))
        logger.info("Sent clear command")

    async def collect_messages(self, timeout_sec: float = 10.0,
                               stop_on_final: bool = True) -> List[Dict]:
        """Collect messages from gateway."""
        messages = []
        self.stt_results = []
        self.tts_audio = []
        self.turn_events = []
        self.errors = []

        start_time = time.time()

        while True:
            try:
                remaining_timeout = timeout_sec - (time.time() - start_time)
                if remaining_timeout <= 0:
                    break

                msg = await asyncio.wait_for(self.ws.recv(), timeout=remaining_timeout)

                if isinstance(msg, bytes):
                    # Binary message (TTS audio)
                    self.tts_audio.append(msg)
                    self.message_timestamps["tts_audio_received"] = time.time()
                    logger.info(f"Received TTS audio: {len(msg)} bytes")
                    messages.append({"type": "tts_audio", "size": len(msg)})
                else:
                    # JSON message
                    data = json.loads(msg)
                    data["_received_at"] = time.time()
                    messages.append(data)
                    self.all_messages.append(data)

                    msg_type = data.get("type", "unknown")

                    if msg_type == "stt_result":
                        self.stt_results.append(data)
                        is_final = data.get("is_final", False)
                        is_speech_final = data.get("is_speech_final", False)
                        transcript = data.get("transcript", "")
                        confidence = data.get("confidence", 0)

                        # Calculate latency from audio end
                        if "audio_end" in self.message_timestamps:
                            latency = (data["_received_at"] - self.message_timestamps["audio_end"]) * 1000
                            data["_latency_ms"] = latency

                        logger.info(f"STT: '{transcript}' "
                                   f"[final={is_final}, speech_final={is_speech_final}, conf={confidence:.2f}]")

                        if stop_on_final and is_speech_final and transcript:
                            logger.info("Received final STT result, stopping collection")
                            break

                    elif msg_type == "turn_complete" or msg_type == "turn_detected":
                        self.turn_events.append(data)
                        logger.info(f"Turn event: {data}")

                    elif msg_type == "tts_playback_complete":
                        logger.info(f"TTS playback complete: {data}")
                        if "speak_sent" in self.message_timestamps:
                            latency = (data["_received_at"] - self.message_timestamps["speak_sent"]) * 1000
                            data["_latency_ms"] = latency

                    elif msg_type == "error":
                        self.errors.append(data)
                        logger.error(f"Error from gateway: {data.get('message')}")

                    else:
                        logger.debug(f"Received message: {msg_type}")

            except asyncio.TimeoutError:
                logger.info("Message collection timeout")
                break
            except ConnectionClosed:
                logger.warning("WebSocket connection closed")
                break
            except Exception as e:
                logger.error(f"Error receiving message: {e}")
                break

        return messages

    def reset_collectors(self):
        """Reset message collectors for new test."""
        self.stt_results = []
        self.tts_audio = []
        self.turn_events = []
        self.errors = []
        self.all_messages = []
        self.message_timestamps = {}


class WaavTestRunner:
    """Test runner for WaaV Gateway live tests."""

    def __init__(self, audio_dir: Path):
        self.audio_dir = audio_dir
        self.results: List[TestResult] = []
        self.client = WaavTestClient()

    async def run_all_tests(self) -> List[TestResult]:
        """Run all test suites."""
        logger.info("=" * 70)
        logger.info("WaaV Gateway Live Testing Suite")
        logger.info("=" * 70)

        # Test suites
        test_suites = [
            ("Connection Tests", self.test_connection),
            ("STT Tests", self.test_stt),
            ("TTS Tests", self.test_tts),
            ("Noise Filter Tests", self.test_noise_filter),
            ("Turn Detection Tests", self.test_turn_detection),
            ("Integration Tests", self.test_integration),
        ]

        for suite_name, test_func in test_suites:
            logger.info(f"\n{'='*70}")
            logger.info(f"Running: {suite_name}")
            logger.info("=" * 70)
            try:
                await test_func()
            except Exception as e:
                logger.error(f"Test suite '{suite_name}' failed: {e}")
                self.results.append(TestResult(
                    test_name=suite_name,
                    success=False,
                    duration_ms=0,
                    message=f"Suite failed: {e}",
                    errors=[str(e)]
                ))

        return self.results

    async def test_connection(self):
        """Test basic WebSocket connection."""
        start = time.time()

        # Test 1: Basic connection
        connected = await self.client.connect()
        self.results.append(TestResult(
            test_name="connection_basic",
            success=connected,
            duration_ms=(time.time() - start) * 1000,
            message="WebSocket connection established" if connected else "Connection failed"
        ))

        if not connected:
            return

        # Test 2: Send config and receive ready
        start = time.time()
        config_ok = await self.client.send_config(stt_enabled=True, tts_enabled=True)
        self.results.append(TestResult(
            test_name="connection_config",
            success=config_ok,
            duration_ms=(time.time() - start) * 1000,
            message="Config accepted, ready received" if config_ok else "Config failed",
            details={"stream_id": self.client.stream_id} if config_ok else {}
        ))

        await self.client.disconnect()

    async def test_stt(self):
        """Test Speech-to-Text functionality."""
        # Find audio files
        audio_files = list(self.audio_dir.glob("*.wav"))
        if not audio_files:
            logger.warning("No audio files found for STT testing")
            return

        # Connect and configure
        if not await self.client.connect():
            self.results.append(TestResult(
                test_name="stt_connection",
                success=False,
                duration_ms=0,
                message="Failed to connect"
            ))
            return

        if not await self.client.send_config(stt_enabled=True, tts_enabled=True):
            self.results.append(TestResult(
                test_name="stt_config",
                success=False,
                duration_ms=0,
                message="Failed to configure"
            ))
            await self.client.disconnect()
            return

        # Test with various audio files
        # Test with real speech files (CMU Arctic, TTS-generated, OSR)
        test_files = [
            # CMU Arctic real speech
            "cmu_bdl_arctic_a0001.wav",
            "cmu_clb_arctic_a0001.wav",
            # TTS-generated real speech
            "tts_hello_world.wav",
            "tts_statement.wav",
            # Open Speech Repository
            "osr_american_sample.wav",
        ]

        for test_file in test_files:
            filepath = self.audio_dir / test_file
            if not filepath.exists():
                logger.warning(f"Test file not found: {test_file}")
                continue

            self.client.reset_collectors()
            start = time.time()

            try:
                # Stream audio
                bytes_sent = await self.client.send_audio_file(filepath)

                # Collect results
                messages = await self.client.collect_messages(timeout_sec=15, stop_on_final=True)

                # Analyze results
                final_transcripts = [r for r in self.client.stt_results if r.get("is_final")]
                interim_results = [r for r in self.client.stt_results if not r.get("is_final")]

                full_transcript = " ".join(r.get("transcript", "") for r in final_transcripts if r.get("transcript"))

                # Calculate latencies
                latencies = [r.get("_latency_ms", 0) for r in self.client.stt_results if "_latency_ms" in r]

                success = bool(final_transcripts) and bool(full_transcript.strip())

                self.results.append(TestResult(
                    test_name=f"stt_{test_file.replace('.wav', '')}",
                    success=success,
                    duration_ms=(time.time() - start) * 1000,
                    message=f"Transcript: '{full_transcript[:100]}...'" if len(full_transcript) > 100 else f"Transcript: '{full_transcript}'",
                    details={
                        "bytes_sent": bytes_sent,
                        "final_results": len(final_transcripts),
                        "interim_results": len(interim_results),
                        "transcript_length": len(full_transcript),
                    },
                    latencies=latencies,
                    errors=self.client.errors
                ))

            except Exception as e:
                self.results.append(TestResult(
                    test_name=f"stt_{test_file.replace('.wav', '')}",
                    success=False,
                    duration_ms=(time.time() - start) * 1000,
                    message=f"Error: {e}",
                    errors=[str(e)]
                ))

        await self.client.disconnect()

    async def test_tts(self):
        """Test Text-to-Speech functionality."""
        # Connect and configure
        if not await self.client.connect():
            self.results.append(TestResult(
                test_name="tts_connection",
                success=False,
                duration_ms=0,
                message="Failed to connect"
            ))
            return

        # Note: Gateway requires audio=true for TTS to work
        if not await self.client.send_config(stt_enabled=True, tts_enabled=True):
            self.results.append(TestResult(
                test_name="tts_config",
                success=False,
                duration_ms=0,
                message="Failed to configure"
            ))
            await self.client.disconnect()
            return

        # Test phrases
        test_phrases = [
            ("short_phrase", "Hello, how are you?"),
            ("medium_phrase", "The quick brown fox jumps over the lazy dog. This is a test of the text to speech system."),
            ("long_phrase", "In the beginning, there was silence. Then came the first sound, a whisper of creation that echoed through the void. From this whisper, the universe unfurled its cosmic tapestry, weaving stars and galaxies into existence."),
            ("numbers", "The year is 2024. The temperature is 72 degrees. Call me at 555-1234."),
            ("special_chars", "Hello! How are you doing? I'm fine, thank you. Let's test: $100, 50%, and more..."),
        ]

        for test_name, text in test_phrases:
            self.client.reset_collectors()
            start = time.time()

            try:
                # Send TTS request
                await self.client.send_speak(text)

                # Collect audio response
                messages = await self.client.collect_messages(timeout_sec=30, stop_on_final=False)

                # Analyze results
                total_audio_bytes = sum(len(chunk) for chunk in self.client.tts_audio)

                success = total_audio_bytes > 0

                self.results.append(TestResult(
                    test_name=f"tts_{test_name}",
                    success=success,
                    duration_ms=(time.time() - start) * 1000,
                    message=f"Received {total_audio_bytes} bytes of audio" if success else "No audio received",
                    details={
                        "text_length": len(text),
                        "audio_bytes": total_audio_bytes,
                        "audio_chunks": len(self.client.tts_audio),
                    },
                    errors=[str(e) for e in self.client.errors]
                ))

                # Save audio for manual verification
                if self.client.tts_audio:
                    output_path = self.audio_dir / f"tts_output_{test_name}.raw"
                    with open(output_path, 'wb') as f:
                        for chunk in self.client.tts_audio:
                            f.write(chunk)
                    logger.info(f"Saved TTS audio to {output_path}")

            except Exception as e:
                self.results.append(TestResult(
                    test_name=f"tts_{test_name}",
                    success=False,
                    duration_ms=(time.time() - start) * 1000,
                    message=f"Error: {e}",
                    errors=[str(e)]
                ))

        await self.client.disconnect()

    async def test_noise_filter(self):
        """Test noise filtering with DeepFilterNet."""
        # Use real speech with added noise for more realistic testing
        noisy_files = [
            ("real_cmu_bdl_arctic_a0001_snr20.wav", "Real speech 20dB SNR (light noise)"),
            ("real_cmu_bdl_arctic_a0001_snr10.wav", "Real speech 10dB SNR (moderate noise)"),
            ("real_cmu_bdl_arctic_a0001_snr5.wav", "Real speech 5dB SNR (heavy noise)"),
            ("real_tts_statement_snr10.wav", "TTS speech 10dB SNR"),
            ("white_noise.wav", "Pure white noise"),
        ]

        # Connect
        if not await self.client.connect():
            self.results.append(TestResult(
                test_name="noise_filter_connection",
                success=False,
                duration_ms=0,
                message="Failed to connect"
            ))
            return

        if not await self.client.send_config(stt_enabled=True, tts_enabled=True):
            await self.client.disconnect()
            return

        for filename, description in noisy_files:
            filepath = self.audio_dir / filename
            if not filepath.exists():
                logger.warning(f"Noisy audio not found: {filename}")
                continue

            self.client.reset_collectors()
            start = time.time()

            try:
                bytes_sent = await self.client.send_audio_file(filepath)
                messages = await self.client.collect_messages(timeout_sec=15, stop_on_final=True)

                # For noisy audio, we expect the noise filter to help
                # Check if we got any transcription (success if the filter helps)
                transcripts = [r.get("transcript", "") for r in self.client.stt_results if r.get("is_final")]
                full_transcript = " ".join(transcripts)

                # For pure noise, we expect no transcript
                if "white_noise" in filename:
                    success = len(full_transcript.strip()) == 0
                    message = "Correctly filtered out pure noise" if success else f"Unexpected transcript: {full_transcript}"
                else:
                    success = len(full_transcript.strip()) > 0
                    message = f"Transcript: '{full_transcript}'" if success else "No transcript (noise filter may have removed too much)"

                self.results.append(TestResult(
                    test_name=f"noise_filter_{filename.replace('.wav', '')}",
                    success=success,
                    duration_ms=(time.time() - start) * 1000,
                    message=message,
                    details={
                        "description": description,
                        "bytes_sent": bytes_sent,
                        "results_count": len(self.client.stt_results),
                        "transcript_length": len(full_transcript),
                    }
                ))

            except Exception as e:
                self.results.append(TestResult(
                    test_name=f"noise_filter_{filename.replace('.wav', '')}",
                    success=False,
                    duration_ms=(time.time() - start) * 1000,
                    message=f"Error: {e}",
                    errors=[str(e)]
                ))

        await self.client.disconnect()

    async def test_turn_detection(self):
        """Test turn detection (both text-based and audio-based)."""
        # Test with speech that has natural pauses
        test_file = self.audio_dir / "speech_with_pauses.wav"

        if not test_file.exists():
            logger.warning("Speech with pauses file not found")
            return

        # Connect
        if not await self.client.connect():
            return

        if not await self.client.send_config(stt_enabled=True, tts_enabled=True):
            await self.client.disconnect()
            return

        self.client.reset_collectors()
        start = time.time()

        try:
            bytes_sent = await self.client.send_audio_file(test_file)
            messages = await self.client.collect_messages(timeout_sec=20, stop_on_final=True)

            # Look for turn events
            turn_events = [m for m in messages if m.get("type") in ["turn_complete", "turn_detected", "speech_final"]]

            # Check for is_speech_final flags
            speech_final_results = [r for r in self.client.stt_results if r.get("is_speech_final")]

            success = len(speech_final_results) > 0 or len(turn_events) > 0

            self.results.append(TestResult(
                test_name="turn_detection_speech_pauses",
                success=success,
                duration_ms=(time.time() - start) * 1000,
                message=f"Detected {len(speech_final_results)} speech finals, {len(turn_events)} turn events",
                details={
                    "speech_final_count": len(speech_final_results),
                    "turn_event_count": len(turn_events),
                    "total_stt_results": len(self.client.stt_results),
                }
            ))

        except Exception as e:
            self.results.append(TestResult(
                test_name="turn_detection_speech_pauses",
                success=False,
                duration_ms=(time.time() - start) * 1000,
                message=f"Error: {e}",
                errors=[str(e)]
            ))

        await self.client.disconnect()

    async def test_integration(self):
        """Test end-to-end integration: Audio -> STT -> TTS response."""
        # Use real speech file for integration test
        test_file = self.audio_dir / "tts_hello_world.wav"

        if not test_file.exists():
            # Fallback to CMU Arctic sample
            test_file = self.audio_dir / "cmu_bdl_arctic_a0001.wav"

        if not test_file.exists():
            logger.warning("Real speech file not found for integration test")
            return

        # Connect with both STT and TTS
        if not await self.client.connect():
            return

        if not await self.client.send_config(stt_enabled=True, tts_enabled=True):
            await self.client.disconnect()
            return

        self.client.reset_collectors()
        start = time.time()

        try:
            # Step 1: Send audio for STT
            logger.info("[Integration] Step 1: Sending audio for STT...")
            bytes_sent = await self.client.send_audio_file(test_file)

            # Step 2: Collect STT results
            logger.info("[Integration] Step 2: Waiting for STT results...")
            await self.client.collect_messages(timeout_sec=10, stop_on_final=True)

            stt_transcript = " ".join(
                r.get("transcript", "") for r in self.client.stt_results if r.get("is_final")
            )
            logger.info(f"[Integration] STT Result: '{stt_transcript}'")

            stt_latencies = [r.get("_latency_ms", 0) for r in self.client.stt_results if "_latency_ms" in r]

            # Step 3: Send TTS response based on transcript
            self.client.reset_collectors()
            response_text = f"You said: {stt_transcript}" if stt_transcript else "I didn't catch that. Could you repeat?"

            logger.info(f"[Integration] Step 3: Sending TTS response: '{response_text}'")
            await self.client.send_speak(response_text)

            # Step 4: Collect TTS audio
            logger.info("[Integration] Step 4: Waiting for TTS audio...")
            await self.client.collect_messages(timeout_sec=15, stop_on_final=False)

            total_audio = sum(len(c) for c in self.client.tts_audio)

            # Calculate end-to-end latency
            end_time = time.time()
            total_duration = (end_time - start) * 1000

            success = bool(stt_transcript) and total_audio > 0

            self.results.append(TestResult(
                test_name="integration_e2e",
                success=success,
                duration_ms=total_duration,
                message=f"E2E: Audio -> STT ('{stt_transcript[:50]}...') -> TTS ({total_audio} bytes)",
                details={
                    "stt_transcript": stt_transcript,
                    "stt_results_count": len(self.client.stt_results),
                    "tts_response": response_text,
                    "tts_audio_bytes": total_audio,
                },
                latencies=stt_latencies
            ))

        except Exception as e:
            self.results.append(TestResult(
                test_name="integration_e2e",
                success=False,
                duration_ms=(time.time() - start) * 1000,
                message=f"Error: {e}",
                errors=[str(e)]
            ))

        await self.client.disconnect()

    def print_results(self):
        """Print test results summary."""
        print("\n" + "=" * 70)
        print("TEST RESULTS SUMMARY")
        print("=" * 70)

        passed = sum(1 for r in self.results if r.success)
        failed = len(self.results) - passed

        for result in self.results:
            status = "PASS" if result.success else "FAIL"
            print(f"\n[{status}] {result.test_name}")
            print(f"  Duration: {result.duration_ms:.2f}ms")
            print(f"  Message: {result.message}")
            if result.details:
                for k, v in result.details.items():
                    print(f"  {k}: {v}")
            if hasattr(result, 'latencies') and result.latencies:
                stats = result.to_dict().get("latency_stats", {})
                if stats:
                    print(f"  Latency: min={stats['min_ms']}ms, p50={stats['p50_ms']}ms, p99={stats['p99_ms']}ms, max={stats['max_ms']}ms")
            if result.errors:
                print(f"  Errors: {result.errors}")

        print("\n" + "=" * 70)
        print(f"TOTAL: {passed} passed, {failed} failed out of {len(self.results)} tests")
        print("=" * 70)

    def save_results(self, output_path: Path):
        """Save results to JSON file."""
        results_dict = {
            "timestamp": datetime.now().isoformat(),
            "summary": {
                "total": len(self.results),
                "passed": sum(1 for r in self.results if r.success),
                "failed": sum(1 for r in self.results if not r.success),
            },
            "results": [r.to_dict() for r in self.results]
        }

        with open(output_path, 'w') as f:
            json.dump(results_dict, f, indent=2)

        logger.info(f"Results saved to {output_path}")


async def main():
    """Main entry point."""
    import argparse

    parser = argparse.ArgumentParser(description="WaaV Gateway Live Testing")
    parser.add_argument("--audio-dir", type=Path,
                       default=Path(__file__).parent.parent / "audio",
                       help="Directory containing test audio files")
    parser.add_argument("--results-dir", type=Path,
                       default=Path(__file__).parent.parent / "results",
                       help="Directory for test results")
    parser.add_argument("--gateway", type=str, default=GATEWAY_URL,
                       help="Gateway WebSocket URL")
    args = parser.parse_args()

    # Ensure directories exist
    args.audio_dir.mkdir(parents=True, exist_ok=True)
    args.results_dir.mkdir(parents=True, exist_ok=True)

    # Check for audio files
    audio_files = list(args.audio_dir.glob("*.wav"))
    if not audio_files:
        logger.warning(f"No audio files found in {args.audio_dir}")
        logger.info("Run audio_generator.py first to create test audio files")
        return

    # Run tests
    runner = WaavTestRunner(args.audio_dir)
    await runner.run_all_tests()

    # Print and save results
    runner.print_results()

    results_file = args.results_dir / f"test_results_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
    runner.save_results(results_file)


if __name__ == "__main__":
    asyncio.run(main())
