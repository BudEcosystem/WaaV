#!/usr/bin/env python3
"""
WaaV Gateway Full End-to-End Pipeline Test

Tests the COMPLETE audio processing pipeline:
Audio Input → Noise Filter → STT → Turn Detection → [Send TTS] → TTS → Audio Output

Measures ALL components with accurate latency tracking.
"""

import asyncio
import json
import time
import wave
import sys
import logging
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional, List, Dict, Any
from datetime import datetime
import statistics

try:
    import websockets
    from websockets.exceptions import ConnectionClosed
except ImportError:
    print("pip install websockets")
    sys.exit(1)

try:
    import numpy as np
except ImportError:
    print("pip install numpy")
    sys.exit(1)

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s.%(msecs)03d - %(levelname)s - %(message)s',
    datefmt='%H:%M:%S'
)
logger = logging.getLogger(__name__)

GATEWAY_URL = "ws://localhost:3001/ws"
DEEPGRAM_API_KEY = "a89c460d1b43a8b5d67beea50545650ed62eb030"
SAMPLE_RATE = 16000


@dataclass
class E2ETestResult:
    """Result of a single E2E test run."""
    test_name: str
    audio_file: str
    success: bool

    # Component latencies (all in milliseconds)
    connection_latency_ms: float = 0
    config_latency_ms: float = 0
    audio_send_duration_ms: float = 0
    stt_first_result_latency_ms: float = 0
    stt_final_result_latency_ms: float = 0
    speech_final_latency_ms: float = 0
    tts_speak_to_first_audio_ms: float = 0
    tts_total_duration_ms: float = 0
    e2e_audio_to_audio_latency_ms: float = 0

    # Accuracy metrics
    stt_transcript: str = ""
    stt_confidence: float = 0
    expected_contains: str = ""
    transcript_matches: bool = False

    # Volume metrics
    audio_bytes_sent: int = 0
    audio_bytes_received: int = 0
    stt_result_count: int = 0

    errors: List[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "test_name": self.test_name,
            "audio_file": self.audio_file,
            "success": self.success,
            "latencies": {
                "connection_ms": round(self.connection_latency_ms, 2),
                "config_ms": round(self.config_latency_ms, 2),
                "audio_send_ms": round(self.audio_send_duration_ms, 2),
                "stt_first_result_ms": round(self.stt_first_result_latency_ms, 2),
                "stt_final_result_ms": round(self.stt_final_result_latency_ms, 2),
                "speech_final_ms": round(self.speech_final_latency_ms, 2),
                "tts_first_audio_ms": round(self.tts_speak_to_first_audio_ms, 2),
                "tts_total_ms": round(self.tts_total_duration_ms, 2),
                "e2e_audio_to_audio_ms": round(self.e2e_audio_to_audio_latency_ms, 2),
            },
            "accuracy": {
                "transcript": self.stt_transcript,
                "confidence": round(self.stt_confidence, 3),
                "expected_contains": self.expected_contains,
                "matches": self.transcript_matches,
            },
            "volume": {
                "audio_sent_bytes": self.audio_bytes_sent,
                "audio_received_bytes": self.audio_bytes_received,
                "stt_results": self.stt_result_count,
            },
            "errors": self.errors,
        }


class FullE2ETestClient:
    """Complete end-to-end test client."""

    def __init__(self, gateway_url: str = GATEWAY_URL, api_key: str = DEEPGRAM_API_KEY):
        self.gateway_url = gateway_url
        self.api_key = api_key
        self.ws = None
        self.stream_id = None

    async def run_full_e2e_test(self, audio_file: Path, expected_contains: str,
                                 tts_response: str = None) -> E2ETestResult:
        """
        Run a complete end-to-end test:
        1. Connect
        2. Configure (STT + TTS)
        3. Send audio
        4. Receive STT results
        5. Send TTS speak command
        6. Receive TTS audio
        7. Measure all latencies
        """
        result = E2ETestResult(
            test_name=f"e2e_{audio_file.stem}",
            audio_file=str(audio_file),
            expected_contains=expected_contains,
            success=False,
        )

        timestamps = {}

        try:
            # 1. Connect
            timestamps["connect_start"] = time.time()
            self.ws = await websockets.connect(
                self.gateway_url,
                max_size=10 * 1024 * 1024,
                ping_interval=20,
                ping_timeout=60
            )
            timestamps["connect_end"] = time.time()
            result.connection_latency_ms = (timestamps["connect_end"] - timestamps["connect_start"]) * 1000
            logger.info(f"  [1/6] Connected: {result.connection_latency_ms:.2f}ms")

            # 2. Configure
            timestamps["config_start"] = time.time()
            config = {
                "type": "config",
                "audio": True,
                "stt_config": {
                    "provider": "deepgram",
                    "api_key": self.api_key,
                    "language": "en",
                    "sample_rate": SAMPLE_RATE,
                    "channels": 1,
                    "punctuation": True,
                    "encoding": "linear16",
                    "model": "nova-2",
                    "interim_results": True,
                    "endpointing": 500,
                },
                "tts_config": {
                    "provider": "deepgram",
                    "api_key": self.api_key,
                    "voice_id": "aura-asteria-en",
                    "sample_rate": 24000,
                    "model": "aura-asteria-en",
                }
            }
            await self.ws.send(json.dumps(config))

            # Wait for ready
            response = await asyncio.wait_for(self.ws.recv(), timeout=30)
            data = json.loads(response)
            timestamps["config_end"] = time.time()

            if data.get("type") != "ready":
                result.errors.append(f"Config failed: {data}")
                return result

            self.stream_id = data.get("stream_id")
            result.config_latency_ms = (timestamps["config_end"] - timestamps["config_start"]) * 1000
            logger.info(f"  [2/6] Configured: {result.config_latency_ms:.2f}ms")

            # 3. Send audio
            with wave.open(str(audio_file), 'rb') as wav:
                audio_data = wav.readframes(wav.getnframes())
                sample_rate = wav.getframerate()

            timestamps["audio_send_start"] = time.time()
            chunk_size = int(sample_rate * 0.1 * 2)  # 100ms chunks

            for i in range(0, len(audio_data), chunk_size):
                chunk = audio_data[i:i + chunk_size]
                await self.ws.send(chunk)
                result.audio_bytes_sent += len(chunk)
                await asyncio.sleep(0.1)  # Real-time pacing

            timestamps["audio_send_end"] = time.time()
            result.audio_send_duration_ms = (timestamps["audio_send_end"] - timestamps["audio_send_start"]) * 1000
            logger.info(f"  [3/6] Audio sent: {result.audio_bytes_sent} bytes in {result.audio_send_duration_ms:.2f}ms")

            # Signal end of audio to trigger CloseStream and get speech_final
            audio_end_msg = {"type": "audio_end"}
            await self.ws.send(json.dumps(audio_end_msg))
            logger.debug("  Sent audio_end signal to finalize STT stream")

            # 4. Collect STT results
            stt_results = []
            final_transcript = ""
            timestamps["stt_first_result"] = None
            timestamps["stt_final_result"] = None
            timestamps["speech_final"] = None

            collect_start = time.time()
            while time.time() - collect_start < 15:
                try:
                    msg = await asyncio.wait_for(self.ws.recv(), timeout=2.0)
                    recv_time = time.time()

                    if isinstance(msg, bytes):
                        # Unexpected binary before TTS - might be leftover
                        continue

                    data = json.loads(msg)
                    msg_type = data.get("type")

                    if msg_type == "stt_result":
                        result.stt_result_count += 1
                        stt_results.append(data)

                        if timestamps["stt_first_result"] is None:
                            timestamps["stt_first_result"] = recv_time
                            result.stt_first_result_latency_ms = (recv_time - timestamps["audio_send_end"]) * 1000

                        if data.get("is_final"):
                            if timestamps["stt_final_result"] is None:
                                timestamps["stt_final_result"] = recv_time
                                result.stt_final_result_latency_ms = (recv_time - timestamps["audio_send_end"]) * 1000

                            transcript = data.get("transcript", "")
                            if transcript:
                                final_transcript = transcript
                                result.stt_confidence = data.get("confidence", 0)

                        if data.get("is_speech_final"):
                            timestamps["speech_final"] = recv_time
                            result.speech_final_latency_ms = (recv_time - timestamps["audio_send_end"]) * 1000

                            # Got speech final, proceed to TTS
                            break

                    elif msg_type == "error":
                        result.errors.append(data.get("message", "Unknown error"))

                except asyncio.TimeoutError:
                    if timestamps["speech_final"]:
                        break
                    continue

            result.stt_transcript = final_transcript
            result.transcript_matches = expected_contains.lower() in final_transcript.lower() if final_transcript else False

            logger.info(f"  [4/6] STT complete: '{final_transcript[:50]}...' (latency: first={result.stt_first_result_latency_ms:.2f}ms, final={result.stt_final_result_latency_ms:.2f}ms)")

            # 5. Send TTS speak command
            tts_text = tts_response or f"You said: {final_transcript}" if final_transcript else "I didn't understand that."

            timestamps["tts_speak_sent"] = time.time()
            speak_msg = {
                "type": "speak",
                "text": tts_text,
                "flush": True,
                "allow_interruption": True
            }
            await self.ws.send(json.dumps(speak_msg))
            logger.info(f"  [5/6] TTS speak sent: '{tts_text[:40]}...'")

            # 6. Collect TTS audio
            tts_audio_chunks = []
            timestamps["tts_first_audio"] = None
            timestamps["tts_complete"] = None

            collect_start = time.time()
            while time.time() - collect_start < 30:
                try:
                    msg = await asyncio.wait_for(self.ws.recv(), timeout=5.0)
                    recv_time = time.time()

                    if isinstance(msg, bytes):
                        tts_audio_chunks.append(msg)
                        result.audio_bytes_received += len(msg)

                        if timestamps["tts_first_audio"] is None:
                            timestamps["tts_first_audio"] = recv_time
                            result.tts_speak_to_first_audio_ms = (recv_time - timestamps["tts_speak_sent"]) * 1000

                    else:
                        data = json.loads(msg)
                        if data.get("type") == "tts_playback_complete":
                            timestamps["tts_complete"] = recv_time
                            result.tts_total_duration_ms = (recv_time - timestamps["tts_speak_sent"]) * 1000
                            break
                        elif data.get("type") == "error":
                            result.errors.append(f"TTS: {data.get('message')}")

                except asyncio.TimeoutError:
                    if tts_audio_chunks:
                        timestamps["tts_complete"] = time.time()
                        result.tts_total_duration_ms = (timestamps["tts_complete"] - timestamps["tts_speak_sent"]) * 1000
                        break
                    continue

            logger.info(f"  [6/6] TTS complete: {result.audio_bytes_received} bytes (first_audio={result.tts_speak_to_first_audio_ms:.2f}ms, total={result.tts_total_duration_ms:.2f}ms)")

            # Calculate E2E latency (audio send end to TTS first audio)
            if timestamps.get("tts_first_audio"):
                result.e2e_audio_to_audio_latency_ms = (timestamps["tts_first_audio"] - timestamps["audio_send_end"]) * 1000

            # Determine success
            result.success = (
                result.stt_transcript != "" and
                result.audio_bytes_received > 0
            )

        except Exception as e:
            result.errors.append(str(e))
            logger.error(f"  [Error] {e}")

        finally:
            if self.ws:
                await self.ws.close()
                self.ws = None

        return result


async def run_comprehensive_e2e_tests(audio_dir: Path, results_dir: Path) -> Dict[str, Any]:
    """Run comprehensive E2E tests with all metrics."""

    logger.info("=" * 70)
    logger.info("WaaV Gateway - Full End-to-End Pipeline Test")
    logger.info("=" * 70)
    logger.info("Pipeline: Audio → Noise Filter → STT → Turn Detection → TTS → Audio Out")
    logger.info("=" * 70)

    # Test cases with expected content (partial match)
    test_cases = [
        {
            "file": "tts_hello_world.wav",
            "expected_contains": "hello",
            "description": "TTS-generated: Hello world",
            "tts_response": "Hello to you too!"
        },
        {
            "file": "tts_question.wav",
            "expected_contains": "time",
            "description": "TTS-generated: Question",
            "tts_response": "I can help you with that."
        },
        {
            "file": "cmu_clb_arctic_a0001.wav",
            "expected_contains": "author",
            "description": "CMU Arctic: Author of the danger trail",
            "tts_response": "That's an interesting book reference."
        },
        {
            "file": "osr_american_sample.wav",
            "expected_contains": "birch",
            "description": "OSR: Harvard Sentences",
            "tts_response": "I heard you mention a birch canoe."
        },
        {
            "file": "real_cmu_bdl_arctic_a0001_snr10.wav",
            "expected_contains": "author",
            "description": "Noisy (10dB SNR): Author passage",
            "tts_response": "Even with noise, I understood you."
        },
    ]

    client = FullE2ETestClient()
    results = []

    for i, test_case in enumerate(test_cases, 1):
        audio_file = audio_dir / test_case["file"]
        if not audio_file.exists():
            logger.warning(f"[{i}/{len(test_cases)}] Skip: {test_case['file']} not found")
            continue

        logger.info(f"\n[{i}/{len(test_cases)}] Testing: {test_case['description']}")

        result = await client.run_full_e2e_test(
            audio_file,
            test_case["expected_contains"],
            test_case.get("tts_response")
        )
        results.append(result)

        status = "PASS" if result.success else "FAIL"
        logger.info(f"  Result: {status}")

        # Brief delay between tests
        await asyncio.sleep(1)

    # Generate summary
    summary = {
        "timestamp": datetime.now().isoformat(),
        "total_tests": len(results),
        "passed": sum(1 for r in results if r.success),
        "failed": sum(1 for r in results if not r.success),
    }

    # Aggregate latencies
    if results:
        latency_fields = [
            "connection_latency_ms",
            "config_latency_ms",
            "stt_first_result_latency_ms",
            "stt_final_result_latency_ms",
            "speech_final_latency_ms",
            "tts_speak_to_first_audio_ms",
            "tts_total_duration_ms",
            "e2e_audio_to_audio_latency_ms",
        ]

        summary["latency_aggregates"] = {}
        for field in latency_fields:
            values = [getattr(r, field) for r in results if getattr(r, field) > 0]
            if values:
                summary["latency_aggregates"][field] = {
                    "avg_ms": round(statistics.mean(values), 2),
                    "min_ms": round(min(values), 2),
                    "max_ms": round(max(values), 2),
                    "p50_ms": round(statistics.median(values), 2),
                    "samples": len(values),
                }

    # Full results
    full_results = {
        "summary": summary,
        "tests": [r.to_dict() for r in results],
    }

    # Save results
    results_file = results_dir / f"full_e2e_results_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
    with open(results_file, 'w') as f:
        json.dump(full_results, f, indent=2)

    # Print summary
    logger.info("\n" + "=" * 70)
    logger.info("SUMMARY")
    logger.info("=" * 70)
    logger.info(f"Total: {summary['total_tests']}, Passed: {summary['passed']}, Failed: {summary['failed']}")

    if "latency_aggregates" in summary:
        logger.info("\nLatency Summary (averages):")
        for field, stats in summary["latency_aggregates"].items():
            logger.info(f"  {field}: avg={stats['avg_ms']}ms, p50={stats['p50_ms']}ms, min={stats['min_ms']}ms, max={stats['max_ms']}ms")

    logger.info(f"\nResults saved to: {results_file}")

    return full_results


async def main():
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--audio-dir", type=Path, default=Path(__file__).parent.parent / "audio")
    parser.add_argument("--results-dir", type=Path, default=Path(__file__).parent.parent / "results")
    args = parser.parse_args()

    args.results_dir.mkdir(parents=True, exist_ok=True)

    results = await run_comprehensive_e2e_tests(args.audio_dir, args.results_dir)
    print("\n" + json.dumps(results, indent=2))


if __name__ == "__main__":
    asyncio.run(main())
