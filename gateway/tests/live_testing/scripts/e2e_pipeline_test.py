import os
#!/usr/bin/env python3
"""
WaaV Gateway End-to-End Pipeline Testing

Tests the complete audio processing pipeline with component-level metrics:
- Audio Input → Noise Filter → STT → Turn Detection → TTS → Audio Output

Measures:
- Component latency (per stage)
- End-to-end latency
- STT accuracy (WER - Word Error Rate)
- Noise filter effectiveness
- Turn detection accuracy
- TTS quality metrics
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
from typing import Optional, List, Dict, Any, Tuple
from datetime import datetime
import statistics
import difflib

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

logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s.%(msecs)03d - %(levelname)s - %(message)s',
    datefmt='%H:%M:%S'
)
logger = logging.getLogger(__name__)

# Configuration
GATEWAY_URL = "ws://localhost:3001/ws"
DEEPGRAM_API_KEY = os.environ.get("DEEPGRAM_API_KEY", "")

SAMPLE_RATE = 16000
CHANNELS = 1
CHUNK_SIZE = 3200  # 100ms chunks


@dataclass
class ComponentMetrics:
    """Metrics for a single pipeline component."""
    name: str
    latency_ms: List[float] = field(default_factory=list)
    success_count: int = 0
    failure_count: int = 0
    accuracy: Optional[float] = None
    extra_metrics: Dict[str, Any] = field(default_factory=dict)

    @property
    def avg_latency(self) -> float:
        return statistics.mean(self.latency_ms) if self.latency_ms else 0

    @property
    def p50_latency(self) -> float:
        return statistics.median(self.latency_ms) if self.latency_ms else 0

    @property
    def p99_latency(self) -> float:
        if not self.latency_ms:
            return 0
        sorted_latencies = sorted(self.latency_ms)
        idx = int(len(sorted_latencies) * 0.99)
        return sorted_latencies[min(idx, len(sorted_latencies) - 1)]

    @property
    def success_rate(self) -> float:
        total = self.success_count + self.failure_count
        return self.success_count / total if total > 0 else 0

    def to_dict(self) -> dict:
        result = {
            "name": self.name,
            "success_rate": round(self.success_rate * 100, 2),
            "success_count": self.success_count,
            "failure_count": self.failure_count,
        }
        if self.latency_ms:
            result["latency"] = {
                "avg_ms": round(self.avg_latency, 2),
                "p50_ms": round(self.p50_latency, 2),
                "p99_ms": round(self.p99_latency, 2),
                "min_ms": round(min(self.latency_ms), 2),
                "max_ms": round(max(self.latency_ms), 2),
            }
        if self.accuracy is not None:
            result["accuracy"] = round(self.accuracy * 100, 2)
        if self.extra_metrics:
            result["extra"] = self.extra_metrics
        return result


@dataclass
class PipelineMetrics:
    """Complete pipeline metrics."""
    test_name: str
    timestamp: str = field(default_factory=lambda: datetime.now().isoformat())

    # Component metrics
    connection: ComponentMetrics = field(default_factory=lambda: ComponentMetrics("connection"))
    noise_filter: ComponentMetrics = field(default_factory=lambda: ComponentMetrics("noise_filter"))
    stt: ComponentMetrics = field(default_factory=lambda: ComponentMetrics("stt"))
    turn_detection: ComponentMetrics = field(default_factory=lambda: ComponentMetrics("turn_detection"))
    tts: ComponentMetrics = field(default_factory=lambda: ComponentMetrics("tts"))

    # End-to-end metrics
    e2e_latency_ms: List[float] = field(default_factory=list)
    total_audio_sent_bytes: int = 0
    total_audio_received_bytes: int = 0

    def to_dict(self) -> dict:
        e2e_stats = {}
        if self.e2e_latency_ms:
            e2e_stats = {
                "avg_ms": round(statistics.mean(self.e2e_latency_ms), 2),
                "p50_ms": round(statistics.median(self.e2e_latency_ms), 2),
                "p99_ms": round(sorted(self.e2e_latency_ms)[int(len(self.e2e_latency_ms) * 0.99)], 2) if len(self.e2e_latency_ms) > 1 else round(self.e2e_latency_ms[0], 2),
                "min_ms": round(min(self.e2e_latency_ms), 2),
                "max_ms": round(max(self.e2e_latency_ms), 2),
            }

        return {
            "test_name": self.test_name,
            "timestamp": self.timestamp,
            "components": {
                "connection": self.connection.to_dict(),
                "noise_filter": self.noise_filter.to_dict(),
                "stt": self.stt.to_dict(),
                "turn_detection": self.turn_detection.to_dict(),
                "tts": self.tts.to_dict(),
            },
            "end_to_end": {
                "latency": e2e_stats,
                "total_audio_sent_bytes": self.total_audio_sent_bytes,
                "total_audio_received_bytes": self.total_audio_received_bytes,
            }
        }


def calculate_wer(reference: str, hypothesis: str) -> Tuple[float, Dict]:
    """
    Calculate Word Error Rate (WER) between reference and hypothesis.
    Returns WER (0.0 = perfect, 1.0 = completely wrong) and detailed stats.
    """
    ref_words = reference.lower().split()
    hyp_words = hypothesis.lower().split()

    if not ref_words:
        return 0.0 if not hyp_words else 1.0, {"ref_words": 0, "hyp_words": len(hyp_words)}

    # Use dynamic programming for edit distance
    d = [[0] * (len(hyp_words) + 1) for _ in range(len(ref_words) + 1)]

    for i in range(len(ref_words) + 1):
        d[i][0] = i
    for j in range(len(hyp_words) + 1):
        d[0][j] = j

    for i in range(1, len(ref_words) + 1):
        for j in range(1, len(hyp_words) + 1):
            if ref_words[i-1] == hyp_words[j-1]:
                d[i][j] = d[i-1][j-1]
            else:
                d[i][j] = min(d[i-1][j] + 1,      # deletion
                             d[i][j-1] + 1,      # insertion
                             d[i-1][j-1] + 1)    # substitution

    wer = d[len(ref_words)][len(hyp_words)] / len(ref_words)

    return wer, {
        "ref_words": len(ref_words),
        "hyp_words": len(hyp_words),
        "edit_distance": d[len(ref_words)][len(hyp_words)],
    }


class E2EPipelineTestClient:
    """End-to-end pipeline test client with component-level metrics."""

    def __init__(self, gateway_url: str = GATEWAY_URL, api_key: str = DEEPGRAM_API_KEY):
        self.gateway_url = gateway_url
        self.api_key = api_key
        self.ws: Optional[websockets.WebSocketClientProtocol] = None
        self.stream_id: Optional[str] = None

        # Timing tracking
        self.timestamps: Dict[str, float] = {}

        # Message collection
        self.stt_results: List[Dict] = []
        self.tts_audio_chunks: List[bytes] = []
        self.turn_events: List[Dict] = []
        self.errors: List[Dict] = []

    async def connect(self) -> float:
        """Connect and return connection latency in ms."""
        start = time.time()
        self.ws = await websockets.connect(
            self.gateway_url,
            max_size=10 * 1024 * 1024,
            ping_interval=20,
            ping_timeout=60
        )
        return (time.time() - start) * 1000

    async def disconnect(self):
        """Close connection."""
        if self.ws:
            await self.ws.close()
            self.ws = None

    async def send_config(self, use_dag: bool = False) -> Tuple[bool, float]:
        """
        Send configuration and return (success, config_latency_ms).

        If use_dag=True, sends a DAG pipeline configuration.
        """
        start = time.time()

        if use_dag:
            config = self._build_dag_config()
        else:
            config = self._build_standard_config()

        await self.ws.send(json.dumps(config))

        try:
            response = await asyncio.wait_for(self.ws.recv(), timeout=30)
            data = json.loads(response)

            latency = (time.time() - start) * 1000

            if data.get("type") == "ready":
                self.stream_id = data.get("stream_id")
                return True, latency
            else:
                logger.error(f"Config failed: {data}")
                return False, latency

        except asyncio.TimeoutError:
            return False, (time.time() - start) * 1000

    def _build_standard_config(self) -> dict:
        """Build standard WebSocket configuration."""
        return {
            "type": "config",
            "audio": True,
            "stt_config": {
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
            },
            "tts_config": {
                "provider": "deepgram",
                "api_key": self.api_key,
                "voice_id": "aura-asteria-en",
                "sample_rate": 24000,
                "model": "aura-asteria-en",
            }
        }

    def _build_dag_config(self) -> dict:
        """Build DAG pipeline configuration for end-to-end testing."""
        return {
            "type": "config",
            "audio": True,
            "stt_config": {
                "provider": "deepgram",
                "api_key": self.api_key,
                "language": "en",
                "sample_rate": SAMPLE_RATE,
                "channels": CHANNELS,
                "punctuation": True,
                "encoding": "linear16",
                "model": "nova-2",
                "interim_results": True,
            },
            "tts_config": {
                "provider": "deepgram",
                "api_key": self.api_key,
                "voice_id": "aura-asteria-en",
                "sample_rate": 24000,
                "model": "aura-asteria-en",
            },
            "dag_config": {
                "id": "e2e-test-pipeline",
                "name": "E2E Test Pipeline",
                "version": "1.0.0",
                "nodes": [
                    {
                        "id": "audio_input",
                        "type": "audio_input"
                    },
                    {
                        "id": "noise_filter",
                        "type": "processor",
                        "plugin": "deepfilter_net"
                    },
                    {
                        "id": "stt",
                        "type": "stt_provider",
                        "provider": "deepgram",
                        "model": "nova-2"
                    },
                    {
                        "id": "turn_detector",
                        "type": "processor",
                        "plugin": "turn_detect"
                    },
                    {
                        "id": "tts",
                        "type": "tts_provider",
                        "provider": "deepgram",
                        "voice_id": "aura-asteria-en"
                    },
                    {
                        "id": "audio_output",
                        "type": "audio_output",
                        "destination": "web_socket"
                    }
                ],
                "edges": [
                    {"from": "audio_input", "to": "noise_filter"},
                    {"from": "noise_filter", "to": "stt"},
                    {"from": "stt", "to": "turn_detector"},
                    {
                        "from": "turn_detector",
                        "to": "tts",
                        "condition": "is_speech_final"
                    },
                    {"from": "tts", "to": "audio_output"}
                ],
                "entry_node": "audio_input",
                "exit_nodes": ["audio_output"]
            }
        }

    async def send_audio_with_timing(self, filepath: Path) -> Tuple[int, float]:
        """
        Send audio file and return (bytes_sent, send_duration_ms).
        """
        with wave.open(str(filepath), 'rb') as wav:
            audio_data = wav.readframes(wav.getnframes())

        self.timestamps["audio_send_start"] = time.time()

        bytes_sent = 0
        chunk_size = int(SAMPLE_RATE * 0.1 * 2)  # 100ms chunks

        for i in range(0, len(audio_data), chunk_size):
            chunk = audio_data[i:i + chunk_size]
            await self.ws.send(chunk)
            bytes_sent += len(chunk)
            await asyncio.sleep(0.1)  # Real-time pacing

        self.timestamps["audio_send_end"] = time.time()
        send_duration = (self.timestamps["audio_send_end"] - self.timestamps["audio_send_start"]) * 1000

        # Signal end of audio to trigger CloseStream and get speech_final
        audio_end_msg = {"type": "audio_end"}
        await self.ws.send(json.dumps(audio_end_msg))
        logger.debug("Sent audio_end signal to finalize STT stream")

        return bytes_sent, send_duration

    async def collect_pipeline_results(self, timeout_sec: float = 20.0) -> Dict[str, Any]:
        """
        Collect results from the pipeline with timing information.
        Returns structured results with component timings.
        """
        self.stt_results = []
        self.tts_audio_chunks = []
        self.turn_events = []
        self.errors = []

        results = {
            "stt_first_interim_at": None,
            "stt_first_final_at": None,
            "stt_speech_final_at": None,
            "tts_first_audio_at": None,
            "tts_complete_at": None,
            "transcripts": [],
            "turn_events": [],
        }

        start_time = time.time()

        while True:
            try:
                remaining = timeout_sec - (time.time() - start_time)
                if remaining <= 0:
                    break

                msg = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 5.0))
                recv_time = time.time()

                if isinstance(msg, bytes):
                    # TTS audio
                    self.tts_audio_chunks.append(msg)
                    if results["tts_first_audio_at"] is None:
                        results["tts_first_audio_at"] = recv_time
                else:
                    data = json.loads(msg)
                    msg_type = data.get("type", "unknown")

                    if msg_type == "stt_result":
                        self.stt_results.append(data)
                        is_final = data.get("is_final", False)
                        is_speech_final = data.get("is_speech_final", False)

                        if results["stt_first_interim_at"] is None:
                            results["stt_first_interim_at"] = recv_time

                        if is_final and results["stt_first_final_at"] is None:
                            results["stt_first_final_at"] = recv_time

                        if is_speech_final:
                            results["stt_speech_final_at"] = recv_time
                            if data.get("transcript"):
                                results["transcripts"].append(data.get("transcript"))

                    elif msg_type in ["turn_complete", "turn_detected"]:
                        self.turn_events.append(data)
                        results["turn_events"].append({
                            "type": msg_type,
                            "time": recv_time,
                        })

                    elif msg_type == "tts_playback_complete":
                        results["tts_complete_at"] = recv_time

                    elif msg_type == "error":
                        self.errors.append(data)
                        logger.error(f"Pipeline error: {data.get('message')}")

            except asyncio.TimeoutError:
                continue
            except ConnectionClosed:
                break

        results["total_tts_bytes"] = sum(len(c) for c in self.tts_audio_chunks)
        return results

    def reset(self):
        """Reset state for new test."""
        self.timestamps = {}
        self.stt_results = []
        self.tts_audio_chunks = []
        self.turn_events = []
        self.errors = []


class E2EPipelineTestRunner:
    """Runner for end-to-end pipeline tests."""

    def __init__(self, audio_dir: Path, results_dir: Path):
        self.audio_dir = audio_dir
        self.results_dir = results_dir
        self.client = E2EPipelineTestClient()

        # Test samples with expected transcripts
        self.test_samples = [
            {
                "file": "tts_hello_world.wav",
                "expected_transcript": "Hello world. How are you doing today?",
                "description": "TTS-generated clear speech"
            },
            {
                "file": "tts_statement.wav",
                "expected_transcript": "The quick brown fox jumps over the lazy dog.",
                "description": "TTS-generated statement"
            },
            {
                "file": "cmu_bdl_arctic_a0001.wav",
                "expected_transcript": "Author of the danger trail, Philip Steels and others.",
                "description": "CMU Arctic male voice"
            },
            {
                "file": "cmu_clb_arctic_a0001.wav",
                "expected_transcript": "Author of the danger trail, Philip Steels and others.",
                "description": "CMU Arctic female voice"
            },
            {
                "file": "osr_american_sample.wav",
                "expected_transcript": "How are you doing today? The quick brown fox jumps over the lazy dog.",
                "description": "Open Speech Repository sample"
            },
            {
                "file": "real_cmu_bdl_arctic_a0001_snr10.wav",
                "expected_transcript": "Author of the danger trail, Philip Steels and others.",
                "description": "Real speech with 10dB SNR noise"
            },
        ]

    async def run_full_pipeline_test(self, use_dag: bool = False) -> PipelineMetrics:
        """
        Run complete end-to-end pipeline test.

        Pipeline: Audio → [Noise Filter] → STT → [Turn Detection] → TTS → Output
        """
        mode = "DAG" if use_dag else "Standard"
        logger.info(f"\n{'='*70}")
        logger.info(f"Running Full Pipeline Test ({mode} Mode)")
        logger.info("="*70)

        metrics = PipelineMetrics(test_name=f"full_pipeline_{mode.lower()}")

        # Test connection
        try:
            conn_latency = await self.client.connect()
            metrics.connection.latency_ms.append(conn_latency)
            metrics.connection.success_count += 1
            logger.info(f"[Connection] Connected in {conn_latency:.2f}ms")
        except Exception as e:
            metrics.connection.failure_count += 1
            logger.error(f"[Connection] Failed: {e}")
            return metrics

        # Test configuration
        try:
            success, config_latency = await self.client.send_config(use_dag=use_dag)
            if success:
                metrics.connection.latency_ms.append(config_latency)
                logger.info(f"[Config] Configured in {config_latency:.2f}ms, stream_id={self.client.stream_id}")
            else:
                metrics.connection.failure_count += 1
                logger.error("[Config] Failed")
                await self.client.disconnect()
                return metrics
        except Exception as e:
            metrics.connection.failure_count += 1
            logger.error(f"[Config] Error: {e}")
            await self.client.disconnect()
            return metrics

        # Process each test sample
        for sample in self.test_samples:
            filepath = self.audio_dir / sample["file"]
            if not filepath.exists():
                logger.warning(f"[Skip] {sample['file']} not found")
                continue

            logger.info(f"\n[Test] {sample['description']}: {sample['file']}")
            self.client.reset()

            try:
                # Record E2E start time
                e2e_start = time.time()

                # Send audio
                bytes_sent, send_duration = await self.client.send_audio_with_timing(filepath)
                metrics.total_audio_sent_bytes += bytes_sent
                logger.info(f"  [Audio] Sent {bytes_sent} bytes in {send_duration:.2f}ms")

                # Collect results
                results = await self.client.collect_pipeline_results(timeout_sec=20)

                # Calculate component latencies
                audio_send_end = self.client.timestamps.get("audio_send_end", e2e_start)

                # STT latency (time from audio end to first interim result)
                if results["stt_first_interim_at"]:
                    stt_interim_latency = (results["stt_first_interim_at"] - audio_send_end) * 1000
                    metrics.stt.latency_ms.append(stt_interim_latency)
                    logger.info(f"  [STT] First interim: {stt_interim_latency:.2f}ms")

                # STT final latency
                if results["stt_first_final_at"]:
                    stt_final_latency = (results["stt_first_final_at"] - audio_send_end) * 1000
                    logger.info(f"  [STT] First final: {stt_final_latency:.2f}ms")

                # STT accuracy (WER)
                full_transcript = " ".join(results["transcripts"])
                if full_transcript:
                    wer, wer_details = calculate_wer(sample["expected_transcript"], full_transcript)
                    metrics.stt.extra_metrics.setdefault("wer_samples", []).append({
                        "file": sample["file"],
                        "wer": wer,
                        "expected": sample["expected_transcript"],
                        "actual": full_transcript,
                    })
                    metrics.stt.success_count += 1
                    logger.info(f"  [STT] Transcript: '{full_transcript[:60]}...' (WER: {wer:.2%})")
                else:
                    metrics.stt.failure_count += 1
                    logger.warning(f"  [STT] No transcript received")

                # Turn detection
                if results["stt_speech_final_at"]:
                    metrics.turn_detection.success_count += 1
                    if results["stt_first_final_at"]:
                        turn_latency = (results["stt_speech_final_at"] - results["stt_first_final_at"]) * 1000
                        metrics.turn_detection.latency_ms.append(turn_latency)
                        logger.info(f"  [Turn] Speech final detected, latency: {turn_latency:.2f}ms")
                else:
                    metrics.turn_detection.failure_count += 1

                # TTS latency and metrics
                if results["tts_first_audio_at"]:
                    tts_latency = (results["tts_first_audio_at"] - (results["stt_speech_final_at"] or audio_send_end)) * 1000
                    metrics.tts.latency_ms.append(tts_latency)
                    metrics.tts.success_count += 1
                    metrics.total_audio_received_bytes += results["total_tts_bytes"]
                    logger.info(f"  [TTS] First audio: {tts_latency:.2f}ms, total: {results['total_tts_bytes']} bytes")

                # E2E latency (audio send end to TTS first audio)
                if results["tts_first_audio_at"]:
                    e2e_latency = (results["tts_first_audio_at"] - audio_send_end) * 1000
                    metrics.e2e_latency_ms.append(e2e_latency)
                    logger.info(f"  [E2E] Total latency: {e2e_latency:.2f}ms")

            except Exception as e:
                logger.error(f"  [Error] {e}")
                metrics.stt.failure_count += 1

        # Calculate overall STT accuracy
        wer_samples = metrics.stt.extra_metrics.get("wer_samples", [])
        if wer_samples:
            avg_wer = statistics.mean([s["wer"] for s in wer_samples])
            metrics.stt.accuracy = 1.0 - avg_wer  # Convert WER to accuracy

        await self.client.disconnect()
        return metrics

    async def run_noise_filter_test(self) -> PipelineMetrics:
        """
        Test noise filter effectiveness by comparing clean vs noisy audio transcription.
        """
        logger.info(f"\n{'='*70}")
        logger.info("Running Noise Filter Effectiveness Test")
        logger.info("="*70)

        metrics = PipelineMetrics(test_name="noise_filter_effectiveness")

        # Pairs of clean and noisy audio
        test_pairs = [
            {
                "clean": "cmu_bdl_arctic_a0001.wav",
                "noisy_20db": "real_cmu_bdl_arctic_a0001_snr20.wav",
                "noisy_10db": "real_cmu_bdl_arctic_a0001_snr10.wav",
                "noisy_5db": "real_cmu_bdl_arctic_a0001_snr5.wav",
                "expected": "Author of the danger trail, Philip Steels and others.",
            },
            {
                "clean": "tts_statement.wav",
                "noisy_10db": "real_tts_statement_snr10.wav",
                "expected": "The quick brown fox jumps over the lazy dog.",
            },
        ]

        try:
            await self.client.connect()
            success, _ = await self.client.send_config(use_dag=False)
            if not success:
                logger.error("Failed to configure")
                return metrics

            for pair in test_pairs:
                results = {}

                for noise_level in ["clean", "noisy_20db", "noisy_10db", "noisy_5db"]:
                    if noise_level not in pair:
                        continue

                    filepath = self.audio_dir / pair[noise_level]
                    if not filepath.exists():
                        continue

                    self.client.reset()

                    logger.info(f"\n[Test] {noise_level}: {pair[noise_level]}")

                    _, _ = await self.client.send_audio_with_timing(filepath)
                    pipeline_results = await self.client.collect_pipeline_results(timeout_sec=15)

                    transcript = " ".join(pipeline_results["transcripts"])
                    wer, _ = calculate_wer(pair["expected"], transcript)

                    results[noise_level] = {
                        "transcript": transcript,
                        "wer": wer,
                        "accuracy": 1.0 - wer,
                    }

                    logger.info(f"  Transcript: '{transcript[:50]}...' (WER: {wer:.2%})")

                    if noise_level != "clean":
                        if transcript:
                            metrics.noise_filter.success_count += 1
                        else:
                            metrics.noise_filter.failure_count += 1

                # Compare clean vs noisy
                if "clean" in results and results["clean"]["transcript"]:
                    clean_wer = results["clean"]["wer"]
                    for noise_level in ["noisy_20db", "noisy_10db", "noisy_5db"]:
                        if noise_level in results and results[noise_level]["transcript"]:
                            noisy_wer = results[noise_level]["wer"]
                            improvement = clean_wer - noisy_wer
                            logger.info(f"  {noise_level} vs clean: WER diff = {improvement:+.2%}")

                metrics.noise_filter.extra_metrics.setdefault("test_pairs", []).append(results)

            # Calculate overall noise filter effectiveness
            pairs_data = metrics.noise_filter.extra_metrics.get("test_pairs", [])
            if pairs_data:
                noisy_accuracies = []
                for pair_result in pairs_data:
                    for k, v in pair_result.items():
                        if k.startswith("noisy") and v.get("accuracy"):
                            noisy_accuracies.append(v["accuracy"])
                if noisy_accuracies:
                    metrics.noise_filter.accuracy = statistics.mean(noisy_accuracies)

        finally:
            await self.client.disconnect()

        return metrics

    async def run_latency_breakdown_test(self) -> Dict[str, Any]:
        """
        Detailed latency breakdown test measuring each component.
        """
        logger.info(f"\n{'='*70}")
        logger.info("Running Latency Breakdown Test")
        logger.info("="*70)

        results = {
            "connection_latency_ms": [],
            "config_latency_ms": [],
            "stt_interim_latency_ms": [],
            "stt_final_latency_ms": [],
            "turn_detection_latency_ms": [],
            "tts_first_byte_latency_ms": [],
            "e2e_latency_ms": [],
        }

        # Run multiple iterations for statistical significance
        iterations = 3
        test_file = self.audio_dir / "tts_hello_world.wav"

        if not test_file.exists():
            test_file = self.audio_dir / "cmu_clb_arctic_a0001.wav"

        if not test_file.exists():
            logger.error("No test audio file found")
            return results

        for i in range(iterations):
            logger.info(f"\n[Iteration {i+1}/{iterations}]")

            try:
                # Connection
                conn_start = time.time()
                await self.client.connect()
                conn_latency = (time.time() - conn_start) * 1000
                results["connection_latency_ms"].append(conn_latency)
                logger.info(f"  Connection: {conn_latency:.2f}ms")

                # Config
                config_start = time.time()
                success, _ = await self.client.send_config()
                config_latency = (time.time() - config_start) * 1000
                results["config_latency_ms"].append(config_latency)
                logger.info(f"  Config: {config_latency:.2f}ms")

                if not success:
                    await self.client.disconnect()
                    continue

                self.client.reset()

                # Send audio
                bytes_sent, _ = await self.client.send_audio_with_timing(test_file)
                audio_end = self.client.timestamps["audio_send_end"]

                # Collect with timing
                pipeline_results = await self.client.collect_pipeline_results(timeout_sec=20)

                # Calculate latencies
                if pipeline_results["stt_first_interim_at"]:
                    interim_lat = (pipeline_results["stt_first_interim_at"] - audio_end) * 1000
                    results["stt_interim_latency_ms"].append(interim_lat)
                    logger.info(f"  STT Interim: {interim_lat:.2f}ms")

                if pipeline_results["stt_first_final_at"]:
                    final_lat = (pipeline_results["stt_first_final_at"] - audio_end) * 1000
                    results["stt_final_latency_ms"].append(final_lat)
                    logger.info(f"  STT Final: {final_lat:.2f}ms")

                if pipeline_results["stt_speech_final_at"] and pipeline_results["stt_first_final_at"]:
                    turn_lat = (pipeline_results["stt_speech_final_at"] - pipeline_results["stt_first_final_at"]) * 1000
                    results["turn_detection_latency_ms"].append(turn_lat)
                    logger.info(f"  Turn Detection: {turn_lat:.2f}ms")

                if pipeline_results["tts_first_audio_at"]:
                    tts_lat = (pipeline_results["tts_first_audio_at"] - (pipeline_results["stt_speech_final_at"] or audio_end)) * 1000
                    results["tts_first_byte_latency_ms"].append(tts_lat)
                    logger.info(f"  TTS First Byte: {tts_lat:.2f}ms")

                    e2e_lat = (pipeline_results["tts_first_audio_at"] - audio_end) * 1000
                    results["e2e_latency_ms"].append(e2e_lat)
                    logger.info(f"  E2E: {e2e_lat:.2f}ms")

                await self.client.disconnect()

                # Small delay between iterations
                await asyncio.sleep(1)

            except Exception as e:
                logger.error(f"  Error: {e}")
                try:
                    await self.client.disconnect()
                except:
                    pass

        # Calculate statistics
        summary = {}
        for key, values in results.items():
            if values:
                summary[key] = {
                    "avg": round(statistics.mean(values), 2),
                    "min": round(min(values), 2),
                    "max": round(max(values), 2),
                    "p50": round(statistics.median(values), 2),
                    "samples": len(values),
                }

        logger.info("\n" + "="*70)
        logger.info("LATENCY SUMMARY")
        logger.info("="*70)
        for key, stats in summary.items():
            logger.info(f"{key}: avg={stats['avg']}ms, p50={stats['p50']}ms, min={stats['min']}ms, max={stats['max']}ms")

        return {"raw": results, "summary": summary}

    async def run_all_tests(self) -> Dict[str, Any]:
        """Run all end-to-end tests and return comprehensive results."""
        all_results = {
            "timestamp": datetime.now().isoformat(),
            "tests": {}
        }

        # 1. Full pipeline test (standard mode)
        pipeline_metrics = await self.run_full_pipeline_test(use_dag=False)
        all_results["tests"]["full_pipeline_standard"] = pipeline_metrics.to_dict()

        # 2. Noise filter effectiveness
        noise_metrics = await self.run_noise_filter_test()
        all_results["tests"]["noise_filter"] = noise_metrics.to_dict()

        # 3. Latency breakdown
        latency_results = await self.run_latency_breakdown_test()
        all_results["tests"]["latency_breakdown"] = latency_results

        # Save results
        results_file = self.results_dir / f"e2e_results_{datetime.now().strftime('%Y%m%d_%H%M%S')}.json"
        with open(results_file, 'w') as f:
            json.dump(all_results, f, indent=2)

        logger.info(f"\nResults saved to: {results_file}")

        return all_results


async def main():
    import argparse

    parser = argparse.ArgumentParser(description="WaaV Gateway E2E Pipeline Testing")
    parser.add_argument("--audio-dir", type=Path,
                       default=Path(__file__).parent.parent / "audio")
    parser.add_argument("--results-dir", type=Path,
                       default=Path(__file__).parent.parent / "results")
    parser.add_argument("--gateway", type=str, default=GATEWAY_URL)
    parser.add_argument("--test", choices=["all", "pipeline", "noise", "latency"],
                       default="all", help="Test to run")
    args = parser.parse_args()

    args.results_dir.mkdir(parents=True, exist_ok=True)

    runner = E2EPipelineTestRunner(args.audio_dir, args.results_dir)

    if args.test == "all":
        results = await runner.run_all_tests()
    elif args.test == "pipeline":
        metrics = await runner.run_full_pipeline_test()
        results = metrics.to_dict()
    elif args.test == "noise":
        metrics = await runner.run_noise_filter_test()
        results = metrics.to_dict()
    elif args.test == "latency":
        results = await runner.run_latency_breakdown_test()

    print("\n" + "="*70)
    print("FINAL RESULTS")
    print("="*70)
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    asyncio.run(main())
