#!/usr/bin/env python3
"""
WaaV Gateway DAG Pipeline Test

Tests the DAG (Directed Acyclic Graph) routing feature for custom pipelines.
"""

import asyncio
import json
import time
import wave
import sys
import logging
from pathlib import Path
from datetime import datetime

try:
    import websockets
except ImportError:
    print("pip install websockets")
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


async def test_dag_pipeline(audio_file: Path) -> dict:
    """
    Test a custom DAG pipeline configuration.

    Pipeline: Audio → STT → Transform → TTS → Output
    """
    logger.info("=" * 70)
    logger.info("Testing DAG Pipeline Configuration")
    logger.info("=" * 70)

    result = {
        "success": False,
        "dag_accepted": False,
        "stt_results": [],
        "tts_received": False,
        "errors": [],
        "latencies": {},
    }

    timestamps = {}

    try:
        # Connect
        timestamps["connect_start"] = time.time()
        ws = await websockets.connect(
            GATEWAY_URL,
            max_size=10 * 1024 * 1024,
            ping_interval=20,
            ping_timeout=60
        )
        timestamps["connect_end"] = time.time()
        result["latencies"]["connection_ms"] = (timestamps["connect_end"] - timestamps["connect_start"]) * 1000
        logger.info(f"[1] Connected: {result['latencies']['connection_ms']:.2f}ms")

        # Send DAG config
        timestamps["config_start"] = time.time()
        dag_config = {
            "type": "config",
            "audio": True,
            "stt_config": {
                "provider": "deepgram",
                "api_key": DEEPGRAM_API_KEY,
                "language": "en",
                "sample_rate": SAMPLE_RATE,
                "channels": 1,
                "punctuation": True,
                "encoding": "linear16",
                "model": "nova-2",
                "interim_results": True,
            },
            "tts_config": {
                "provider": "deepgram",
                "api_key": DEEPGRAM_API_KEY,
                "voice_id": "aura-asteria-en",
                "sample_rate": 24000,
                "model": "aura-asteria-en",
            },
            # DAG Pipeline Definition
            "dag_config": {
                "id": "test-voice-pipeline",
                "name": "Test Voice Pipeline",
                "version": "1.0.0",
                "nodes": [
                    {
                        "id": "audio_input",
                        "type": "audio_input"
                    },
                    {
                        "id": "stt",
                        "type": "stt_provider",
                        "provider": "deepgram",
                        "model": "nova-2",
                        "language": "en-US"
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
                    {
                        "from": "audio_input",
                        "to": "stt"
                    },
                    {
                        "from": "stt",
                        "to": "tts",
                        "condition": "is_speech_final"
                    },
                    {
                        "from": "tts",
                        "to": "audio_output"
                    }
                ],
                "entry_node": "audio_input",
                "exit_nodes": ["audio_output"]
            }
        }

        await ws.send(json.dumps(dag_config))
        logger.info("[2] Sent DAG config")

        # Wait for response
        response = await asyncio.wait_for(ws.recv(), timeout=30)
        timestamps["config_end"] = time.time()
        data = json.loads(response)

        result["latencies"]["config_ms"] = (timestamps["config_end"] - timestamps["config_start"]) * 1000

        if data.get("type") == "ready":
            result["dag_accepted"] = True
            logger.info(f"[3] DAG accepted: stream_id={data.get('stream_id')}")
        elif data.get("type") == "error":
            result["errors"].append(f"DAG rejected: {data.get('message')}")
            logger.error(f"[3] DAG rejected: {data.get('message')}")
            # Try without DAG config as fallback
            logger.info("[3b] Trying without DAG config (standard mode)...")
            standard_config = {
                "type": "config",
                "audio": True,
                "stt_config": dag_config["stt_config"],
                "tts_config": dag_config["tts_config"],
            }
            await ws.send(json.dumps(standard_config))
            response = await asyncio.wait_for(ws.recv(), timeout=30)
            data = json.loads(response)
            if data.get("type") == "ready":
                logger.info(f"[3b] Standard mode ready: stream_id={data.get('stream_id')}")
            else:
                result["errors"].append(f"Standard mode also failed: {data}")
                return result

        # Send audio
        if not audio_file.exists():
            result["errors"].append(f"Audio file not found: {audio_file}")
            return result

        with wave.open(str(audio_file), 'rb') as wav:
            audio_data = wav.readframes(wav.getnframes())

        timestamps["audio_start"] = time.time()
        chunk_size = int(SAMPLE_RATE * 0.1 * 2)
        bytes_sent = 0

        for i in range(0, len(audio_data), chunk_size):
            chunk = audio_data[i:i + chunk_size]
            await ws.send(chunk)
            bytes_sent += len(chunk)
            await asyncio.sleep(0.1)

        timestamps["audio_end"] = time.time()
        result["latencies"]["audio_send_ms"] = (timestamps["audio_end"] - timestamps["audio_start"]) * 1000
        logger.info(f"[4] Audio sent: {bytes_sent} bytes")

        # Signal end of audio to trigger CloseStream and get speech_final
        audio_end_msg = {"type": "audio_end"}
        await ws.send(json.dumps(audio_end_msg))
        logger.debug("[4b] Sent audio_end signal to finalize STT stream")

        # Collect results
        tts_bytes = 0
        collect_start = time.time()

        while time.time() - collect_start < 30:
            try:
                msg = await asyncio.wait_for(ws.recv(), timeout=5.0)

                if isinstance(msg, bytes):
                    tts_bytes += len(msg)
                    if not result["tts_received"]:
                        result["tts_received"] = True
                        result["latencies"]["tts_first_audio_ms"] = (time.time() - timestamps["audio_end"]) * 1000
                else:
                    data = json.loads(msg)
                    if data.get("type") == "stt_result":
                        result["stt_results"].append({
                            "transcript": data.get("transcript", ""),
                            "is_final": data.get("is_final", False),
                            "is_speech_final": data.get("is_speech_final", False),
                        })
                        if data.get("is_final") and data.get("transcript"):
                            logger.info(f"[5] STT: '{data.get('transcript')}'")
                    elif data.get("type") == "tts_playback_complete":
                        logger.info(f"[6] TTS complete: {tts_bytes} bytes")
                        break
                    elif data.get("type") == "error":
                        result["errors"].append(data.get("message"))

            except asyncio.TimeoutError:
                if tts_bytes > 0:
                    break
                continue

        result["latencies"]["total_tts_bytes"] = tts_bytes
        result["success"] = len(result["stt_results"]) > 0 or tts_bytes > 0

        await ws.close()

    except Exception as e:
        result["errors"].append(str(e))
        logger.error(f"Error: {e}")

    return result


async def main():
    audio_dir = Path(__file__).parent.parent / "audio"

    # Test files
    test_files = [
        audio_dir / "tts_hello_world.wav",
        audio_dir / "cmu_clb_arctic_a0001.wav",
    ]

    for audio_file in test_files:
        if audio_file.exists():
            logger.info(f"\nTesting with: {audio_file.name}")
            result = await test_dag_pipeline(audio_file)

            print("\n" + "=" * 70)
            print("RESULT")
            print("=" * 70)
            print(json.dumps(result, indent=2))
            break

    # Also test the REST API for DAG validation
    logger.info("\n" + "=" * 70)
    logger.info("Testing DAG REST API")
    logger.info("=" * 70)

    import subprocess

    # Check templates endpoint
    result = subprocess.run(
        ["curl", "-s", "http://localhost:3001/dag/templates"],
        capture_output=True,
        text=True
    )
    logger.info(f"GET /dag/templates: {result.stdout}")

    # Test DAG validation
    dag_def = {
        "dag": {
            "id": "test",
            "name": "Test",
            "version": "1.0.0",
            "nodes": [
                {"id": "input", "type": "audio_input"},
                {"id": "output", "type": "audio_output"}
            ],
            "edges": [
                {"from": "input", "to": "output"}
            ],
            "entry_node": "input",
            "exit_nodes": ["output"]
        }
    }

    result = subprocess.run(
        ["curl", "-s", "-X", "POST", "http://localhost:3001/dag/validate",
         "-H", "Content-Type: application/json",
         "-d", json.dumps(dag_def)],
        capture_output=True,
        text=True
    )
    logger.info(f"POST /dag/validate: {result.stdout}")


if __name__ == "__main__":
    asyncio.run(main())
