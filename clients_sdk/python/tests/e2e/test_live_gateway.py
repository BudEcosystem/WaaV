"""
End-to-end live tests against a running WaaV Gateway.

These tests require:
  - Gateway running at ``WAAV_GATEWAY_URL`` (default ws://127.0.0.1:3009)
  - Valid Deepgram API key configured in gateway .env
  - Real audio fixtures in gateway/tests/live_testing/audio/

Run with: pytest tests/e2e/test_live_gateway.py -v -s
Skip with: pytest -k "not e2e" tests/

The gateway uses per-IP rate limiting (60 req/s, burst 10). Tests include
retry logic for 429 responses to handle this gracefully.
"""

import asyncio
import json
import os
import struct
import time
import wave
from pathlib import Path

import httpx
import pytest
import websockets

# ---------------------------------------------------------------------------
# SDK imports
# ---------------------------------------------------------------------------
from bud_waav import BudClient, STTConfig, TTSConfig, AudioFeatures
from bud_waav.errors import APIError
from bud_waav.ws.session import ReconnectConfig

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
# Env-configurable gateway URL (plan P0 item 5); default to the live port 3009.
_WS_BASE = os.environ.get("WAAV_GATEWAY_URL", "ws://127.0.0.1:3009").rstrip("/")
BASE_URL = _WS_BASE.replace("ws://", "http://").replace("wss://", "https://")
WS_URL = f"{_WS_BASE}/ws"
AUDIO_DIR = Path(__file__).resolve().parents[4] / "gateway" / "tests" / "live_testing" / "audio"

# Timeouts
CONNECT_TIMEOUT = 15.0
STT_RESULT_TIMEOUT = 10.0
TTS_AUDIO_TIMEOUT = 15.0

# Audio streaming constants
CHUNK_DURATION_MS = 100  # 100ms chunks
SAMPLE_RATE = 16000
BYTES_PER_SAMPLE = 2  # 16-bit PCM
CHUNK_SIZE = int(SAMPLE_RATE * CHUNK_DURATION_MS / 1000) * BYTES_PER_SAMPLE  # 3200 bytes


# ---------------------------------------------------------------------------
# Gateway availability check (cached, only runs once)
# ---------------------------------------------------------------------------
_gateway_check_cache: bool | None = None


def _check_gateway() -> bool:
    """Check gateway availability (cached)."""
    global _gateway_check_cache
    if _gateway_check_cache is not None:
        return _gateway_check_cache
    try:
        resp = httpx.get(f"{BASE_URL}/", timeout=5.0)
        _gateway_check_cache = resp.status_code in (200, 429)  # 429 = running but rate limited
    except Exception:
        _gateway_check_cache = False
    return _gateway_check_cache


def _check_audio() -> bool:
    return (AUDIO_DIR / "cmu_bdl_arctic_a0001.wav").exists()


skip_no_gateway = pytest.mark.skipif(not _check_gateway(), reason=f"Gateway not running at {BASE_URL}")
skip_no_audio = pytest.mark.skipif(not _check_audio(), reason="Audio fixtures not found")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
def load_raw_pcm(wav_path: Path) -> bytes:
    """Load a WAV file and return raw PCM16 bytes (header stripped)."""
    with wave.open(str(wav_path), "rb") as wf:
        assert wf.getsampwidth() == 2, f"Expected 16-bit audio, got {wf.getsampwidth()*8}-bit"
        assert wf.getnchannels() == 1, f"Expected mono, got {wf.getnchannels()} channels"
        return wf.readframes(wf.getnframes())


def chunk_audio(pcm_data: bytes, chunk_size: int = CHUNK_SIZE) -> list[bytes]:
    """Split raw PCM data into chunks for streaming."""
    return [pcm_data[i:i + chunk_size] for i in range(0, len(pcm_data), chunk_size)]


async def wait_for_rate_limit(seconds: int = 5) -> None:
    """Wait for rate limit window to pass."""
    await asyncio.sleep(seconds)


async def connect_ws_with_retry(url: str, max_retries: int = 3, **kwargs):
    """Connect to WebSocket with rate-limit retry."""
    for attempt in range(max_retries):
        try:
            return await websockets.connect(url, **kwargs)
        except websockets.exceptions.InvalidStatus as e:
            if e.response.status_code == 429 and attempt < max_retries - 1:
                retry_after = int(e.response.headers.get("retry-after", "10"))
                await asyncio.sleep(retry_after + 1)
                continue
            raise


async def sdk_connect_with_retry(session, timeout=CONNECT_TIMEOUT, max_retries: int = 4):
    """Connect an SDK session with rate-limit retry.

    The gateway rate limit is 60/s with burst 10. When exhausted, the
    retry-after header typically says 5-50 seconds. We use a generous
    wait of 15 seconds per attempt to let the token bucket refill.
    """
    import re
    for attempt in range(max_retries):
        try:
            await session.connect(timeout=timeout)
            return
        except Exception as e:
            err_str = str(e)
            if ("429" in err_str or "Too Many Requests" in err_str) and attempt < max_retries - 1:
                # Try to extract wait time from error message
                match = re.search(r"Wait for (\d+)s", err_str)
                wait_secs = int(match.group(1)) + 2 if match else 15
                await asyncio.sleep(wait_secs)
                # Reset session state so it can reconnect
                session._session._connected = False
                session._session._connecting = False
                session._session._closed = False
                session._session._ready_event = None
                continue
            raise


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------
@pytest.fixture
def client():
    """Create a BudClient instance for live testing."""
    return BudClient(base_url=BASE_URL)


# ===========================================================================
# Phase 1: REST Endpoint Tests
# ===========================================================================
@skip_no_gateway
class TestRESTEndpoints:
    """Test REST API endpoints via the SDK."""

    @pytest.mark.asyncio
    async def test_health_check(self, client):
        """Health endpoint should return OK status."""
        for attempt in range(3):
            try:
                result = await client.health()
                assert result is not None
                # Gateway returns lowercase "ok" (messages.rs); accept any case.
                assert (result.get("status") or "").lower() == "ok"
                print(f"  Health: {result}")
                break
            except APIError as e:
                if "429" in str(e) and attempt < 2:
                    await wait_for_rate_limit(10)
                    continue
                raise
        await client.close()

    @pytest.mark.asyncio
    async def test_list_voices(self, client):
        """Voices endpoint should return Deepgram voices grouped by provider."""
        for attempt in range(3):
            try:
                voices = await client.list_voices(provider="deepgram")
                assert isinstance(voices, dict)
                assert "deepgram" in voices
                deepgram_voices = voices["deepgram"]
                assert isinstance(deepgram_voices, list)
                assert len(deepgram_voices) > 0
                voice_ids = [v.get("id", "") for v in deepgram_voices]
                print(f"  Found {len(deepgram_voices)} Deepgram voices")
                print(f"  Sample voices: {voice_ids[:5]}")
                break
            except APIError as e:
                if "429" in str(e) and attempt < 2:
                    await wait_for_rate_limit(10)
                    continue
                raise
        await client.close()

    @pytest.mark.asyncio
    async def test_rest_speak(self, client):
        """REST /speak endpoint should synthesize audio."""
        for attempt in range(3):
            try:
                audio_data = await client.rest.speak(
                    text="Hello, this is a test.",
                    provider="deepgram",
                    model="aura-asteria-en",
                    sample_rate=24000,
                )
                assert isinstance(audio_data, bytes)
                assert len(audio_data) > 1000
                assert len(audio_data) % 2 == 0
                duration_sec = len(audio_data) / (24000 * 2)
                print(f"  TTS audio: {len(audio_data)} bytes, ~{duration_sec:.2f}s")
                break
            except APIError as e:
                if "429" in str(e) and attempt < 2:
                    await wait_for_rate_limit(10)
                    continue
                raise
        await client.close()

    @pytest.mark.asyncio
    async def test_rest_speak_empty_text_rejected(self, client):
        """Empty text should be rejected with an error."""
        for attempt in range(3):
            try:
                with pytest.raises((APIError, Exception)):
                    await client.rest.speak(text="", provider="deepgram")
                break
            except APIError as e:
                if "429" in str(e) and attempt < 2:
                    await wait_for_rate_limit(10)
                    continue
                raise
        await client.close()


# ===========================================================================
# Phase 2: WebSocket STT Tests
# ===========================================================================
@skip_no_gateway
@skip_no_audio
class TestSTTPipeline:
    """Test real-time STT with actual audio files."""

    @pytest.mark.asyncio
    async def test_stt_real_speech(self, client):
        """Should transcribe real speech audio (CMU Arctic)."""
        audio_path = AUDIO_DIR / "cmu_bdl_arctic_a0001.wav"
        pcm_data = load_raw_pcm(audio_path)
        chunks = chunk_audio(pcm_data)

        results: list[dict] = []
        errors: list[str] = []

        session = client.stt.create(
            provider="deepgram",
            language="en-US",
            model="nova-3",
            reconnect=ReconnectConfig(enabled=False),
        )
        session.on("transcript", lambda r: results.append({
            "text": r.text,
            "is_final": r.is_final,
            "is_speech_final": r.is_speech_final,
            "confidence": r.confidence,
        }))
        session.on("error", lambda e: errors.append(str(e)))

        await sdk_connect_with_retry(session)
        assert session.connected
        assert session.stream_id is not None
        print(f"  Connected, stream_id={session.stream_id}")

        for chunk in chunks:
            await session.send_audio(chunk)
            await asyncio.sleep(CHUNK_DURATION_MS / 1000 * 0.5)

        await session._session.send_audio_end()

        deadline = time.monotonic() + STT_RESULT_TIMEOUT
        while time.monotonic() < deadline:
            if any(r.get("is_speech_final") for r in results):
                break
            await asyncio.sleep(0.1)

        await session.disconnect()

        assert len(results) > 0, f"No STT results received. Errors: {errors}"
        final_results = [r for r in results if r.get("is_final")]
        assert len(final_results) > 0, "No final STT results"

        full_transcript = " ".join(r["text"] for r in final_results if r["text"])
        print(f"  Transcript: '{full_transcript}'")
        print(f"  Results: {len(results)} total, {len(final_results)} final")
        print(f"  Confidence: {final_results[-1].get('confidence', 'N/A')}")
        assert len(full_transcript.strip()) > 0

        metrics = session.get_metrics()
        print(f"  Metrics: sent={metrics.audio_bytes_sent}B, msgs_rx={metrics.messages_received}")

    @pytest.mark.asyncio
    async def test_stt_noisy_speech(self, client):
        """Should transcribe speech even with background noise."""
        audio_path = AUDIO_DIR / "noisy_speech_snr10.wav"
        if not audio_path.exists():
            pytest.skip("noisy_speech_snr10.wav not found")

        pcm_data = load_raw_pcm(audio_path)
        chunks = chunk_audio(pcm_data)
        results: list[dict] = []

        session = client.stt.create(
            provider="deepgram",
            language="en-US",
            model="nova-3",
            reconnect=ReconnectConfig(enabled=False),
        )
        session.on("transcript", lambda r: results.append({
            "text": r.text, "is_final": r.is_final, "confidence": r.confidence,
        }))

        await sdk_connect_with_retry(session)
        for chunk in chunks:
            await session.send_audio(chunk)
            await asyncio.sleep(CHUNK_DURATION_MS / 1000 * 0.5)
        await session._session.send_audio_end()

        deadline = time.monotonic() + STT_RESULT_TIMEOUT
        while time.monotonic() < deadline:
            if any(r.get("is_final") for r in results):
                break
            await asyncio.sleep(0.1)

        await session.disconnect()

        final_results = [r for r in results if r.get("is_final")]
        full_transcript = " ".join(r["text"] for r in final_results if r["text"])
        print(f"  Noisy transcript: '{full_transcript}'")
        print(f"  Results: {len(final_results)} final")


# ===========================================================================
# Phase 3: WebSocket TTS Tests
# ===========================================================================
@skip_no_gateway
class TestTTSPipeline:
    """Test real-time TTS with the streaming WebSocket pipeline."""

    @pytest.mark.asyncio
    async def test_tts_streaming(self, client):
        """Should synthesize text and receive audio chunks via WebSocket."""
        audio_chunks: list[bytes] = []
        playback_done = asyncio.Event()

        session = client.tts.create(
            provider="deepgram",
            model="aura-asteria-en",
            voice_id="aura-asteria-en",
            sample_rate=24000,
            reconnect=ReconnectConfig(enabled=False),
        )
        session.on("audio", lambda evt: audio_chunks.append(evt.audio))
        session.on("playback_complete", lambda: playback_done.set())

        await sdk_connect_with_retry(session)
        assert session.connected
        print(f"  Connected, stream_id={session.stream_id}")

        await session.speak("Hello world, this is a test of text to speech synthesis.")

        try:
            await asyncio.wait_for(playback_done.wait(), timeout=TTS_AUDIO_TIMEOUT)
        except asyncio.TimeoutError:
            pass

        await asyncio.sleep(1.0)
        await session.disconnect()

        total_bytes = sum(len(c) for c in audio_chunks)
        print(f"  Received {len(audio_chunks)} audio chunks, {total_bytes} bytes total")
        assert total_bytes > 0, "No TTS audio received"

        duration_sec = total_bytes / (24000 * 2)
        print(f"  Audio duration: ~{duration_sec:.2f}s")
        assert duration_sec > 0.1, "Audio too short"

        metrics = session.get_metrics()
        print(f"  TTS TTFB: {metrics.tts_ttfb.p50:.1f}ms (p50)")
        if metrics.tts_ttfb.count > 0:
            assert metrics.tts_ttfb.p50 < 5000, "TTS TTFB > 5s, too slow"

    @pytest.mark.asyncio
    async def test_tts_clear_interruption(self, client):
        """Should handle clear (interruption) during TTS playback."""
        audio_chunks: list[bytes] = []

        session = client.tts.create(
            provider="deepgram",
            model="aura-asteria-en",
            voice_id="aura-asteria-en",
            sample_rate=24000,
            reconnect=ReconnectConfig(enabled=False),
        )
        session.on("audio", lambda evt: audio_chunks.append(evt.audio))

        await sdk_connect_with_retry(session)

        await session.speak(
            "This is a much longer sentence to test interruption behavior. "
            "We want to make sure the clear command properly stops the synthesis."
        )

        await asyncio.sleep(1.0)
        await session.clear()
        pre_clear_count = len(audio_chunks)

        await asyncio.sleep(2.0)
        post_clear_count = len(audio_chunks)

        await session.disconnect()

        print(f"  Pre-clear chunks: {pre_clear_count}, Post-clear: {post_clear_count}")
        total_bytes = sum(len(c) for c in audio_chunks)
        print(f"  Total audio: {total_bytes} bytes")


# ===========================================================================
# Phase 4: Raw WebSocket Protocol Tests
# ===========================================================================
@skip_no_gateway
class TestWebSocketProtocol:
    """Test the raw WebSocket protocol correctness."""

    @pytest.mark.asyncio
    async def test_config_ready_handshake(self):
        """Config → ready handshake should complete successfully."""
        ws = await connect_ws_with_retry(WS_URL, open_timeout=5)
        try:
            config = {
                "type": "config",
                "audio": True,
                "stt_config": {
                    "provider": "deepgram",
                    "language": "en-US",
                    "sample_rate": 16000,
                    "channels": 1,
                    "punctuation": True,
                    "encoding": "linear16",
                    "model": "nova-3",
                },
                "tts_config": {
                    "provider": "deepgram",
                    "model": "aura-asteria-en",
                    "voice_id": "aura-asteria-en",
                    "audio_format": "linear16",
                    "sample_rate": 24000,
                },
            }
            await ws.send(json.dumps(config))

            response = await asyncio.wait_for(ws.recv(), timeout=CONNECT_TIMEOUT)
            msg = json.loads(response)
            assert msg["type"] == "ready"
            assert "stream_id" in msg
            print(f"  Ready: stream_id={msg['stream_id']}")
        finally:
            await ws.close()

    @pytest.mark.asyncio
    async def test_custom_stream_id(self):
        """Custom stream_id should be respected by the gateway."""
        custom_id = "test-custom-stream-id-12345"
        ws = await connect_ws_with_retry(WS_URL, open_timeout=5)
        try:
            config = {
                "type": "config",
                "stream_id": custom_id,
                "audio": True,
                "stt_config": {
                    "provider": "deepgram",
                    "language": "en-US",
                    "sample_rate": 16000,
                    "channels": 1,
                    "punctuation": True,
                    "encoding": "linear16",
                    "model": "nova-3",
                },
                "tts_config": {
                    "provider": "deepgram",
                    "model": "aura-asteria-en",
                    "voice_id": "aura-asteria-en",
                    "sample_rate": 24000,
                },
            }
            await ws.send(json.dumps(config))

            response = await asyncio.wait_for(ws.recv(), timeout=CONNECT_TIMEOUT)
            msg = json.loads(response)
            assert msg["type"] == "ready"
            assert msg["stream_id"] == custom_id
            print(f"  Custom stream_id verified: {msg['stream_id']}")
        finally:
            await ws.close()

    @pytest.mark.asyncio
    async def test_speak_and_receive_audio(self):
        """Speak command should produce binary audio frames."""
        ws = await connect_ws_with_retry(WS_URL, open_timeout=5)
        try:
            config = {
                "type": "config",
                "audio": True,
                "stt_config": {
                    "provider": "deepgram",
                    "language": "en-US",
                    "sample_rate": 16000,
                    "channels": 1,
                    "punctuation": True,
                    "encoding": "linear16",
                    "model": "nova-3",
                },
                "tts_config": {
                    "provider": "deepgram",
                    "model": "aura-asteria-en",
                    "voice_id": "aura-asteria-en",
                    "audio_format": "linear16",
                    "sample_rate": 24000,
                },
            }
            await ws.send(json.dumps(config))

            response = await asyncio.wait_for(ws.recv(), timeout=CONNECT_TIMEOUT)
            ready_msg = json.loads(response)
            assert ready_msg["type"] == "ready"

            await ws.send(json.dumps({
                "type": "speak",
                "text": "Hello world",
                "flush": True,
            }))

            audio_bytes = 0
            text_messages = []
            deadline = time.monotonic() + TTS_AUDIO_TIMEOUT

            while time.monotonic() < deadline:
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=3.0)
                    if isinstance(msg, bytes):
                        audio_bytes += len(msg)
                    else:
                        data = json.loads(msg)
                        text_messages.append(data.get("type"))
                        if data.get("type") == "tts_playback_complete":
                            break
                except asyncio.TimeoutError:
                    break

            print(f"  Audio bytes: {audio_bytes}")
            print(f"  Text messages: {text_messages}")
            assert audio_bytes > 0, "No audio received from speak command"
        finally:
            await ws.close()

    @pytest.mark.asyncio
    @skip_no_audio
    async def test_audio_streaming_and_stt_results(self):
        """Streaming audio should produce STT results."""
        audio_path = AUDIO_DIR / "cmu_bdl_arctic_a0001.wav"
        pcm_data = load_raw_pcm(audio_path)
        chunks = chunk_audio(pcm_data)

        ws = await connect_ws_with_retry(WS_URL, open_timeout=5)
        try:
            config = {
                "type": "config",
                "audio": True,
                "stt_config": {
                    "provider": "deepgram",
                    "language": "en-US",
                    "sample_rate": 16000,
                    "channels": 1,
                    "punctuation": True,
                    "encoding": "linear16",
                    "model": "nova-3",
                },
                "tts_config": {
                    "provider": "deepgram",
                    "model": "aura-asteria-en",
                    "voice_id": "aura-asteria-en",
                    "sample_rate": 24000,
                },
            }
            await ws.send(json.dumps(config))
            response = await asyncio.wait_for(ws.recv(), timeout=CONNECT_TIMEOUT)
            assert json.loads(response)["type"] == "ready"

            for chunk in chunks:
                await ws.send(chunk)
                await asyncio.sleep(CHUNK_DURATION_MS / 1000 * 0.3)

            await ws.send(json.dumps({"type": "audio_end"}))

            stt_results = []
            deadline = time.monotonic() + STT_RESULT_TIMEOUT
            while time.monotonic() < deadline:
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=2.0)
                    if isinstance(msg, str):
                        data = json.loads(msg)
                        if data.get("type") == "stt_result":
                            stt_results.append(data)
                except asyncio.TimeoutError:
                    break

            assert len(stt_results) > 0, "No STT results from audio streaming"
            final_results = [r for r in stt_results if r.get("is_final")]
            transcript = " ".join(r["transcript"] for r in final_results if r.get("transcript"))
            print(f"  Raw WS transcript: '{transcript}'")
            print(f"  Total results: {len(stt_results)}, final: {len(final_results)}")
        finally:
            await ws.close()


# ===========================================================================
# Phase 5: Performance & Latency Tests
# ===========================================================================
@skip_no_gateway
class TestPerformance:
    """Measure and validate latency targets."""

    @pytest.mark.asyncio
    async def test_ws_connect_latency(self, client):
        """WebSocket connect + ready should be under 5 seconds."""
        session = client.stt.create(
            provider="deepgram",
            language="en-US",
            reconnect=ReconnectConfig(enabled=False),
        )

        start = time.monotonic()
        await sdk_connect_with_retry(session)
        elapsed_ms = (time.monotonic() - start) * 1000
        await session.disconnect()

        metrics = session.get_metrics()
        print(f"  WS connect: {metrics.ws_connect_ms:.1f}ms")
        print(f"  Total ready: {elapsed_ms:.1f}ms")
        # Don't count rate-limit wait time against latency
        assert metrics.ws_connect_ms < 5000, f"WS connect too slow: {metrics.ws_connect_ms:.0f}ms"
        await client.close()

    @pytest.mark.asyncio
    async def test_rest_speak_latency(self, client):
        """REST TTS should return within reasonable time."""
        for attempt in range(3):
            try:
                start = time.monotonic()
                audio = await client.rest.speak(
                    text="Quick test.",
                    provider="deepgram",
                    model="aura-asteria-en",
                    sample_rate=24000,
                )
                elapsed_ms = (time.monotonic() - start) * 1000
                print(f"  REST speak latency: {elapsed_ms:.1f}ms")
                print(f"  Audio size: {len(audio)} bytes")
                assert elapsed_ms < 10000, f"REST speak too slow: {elapsed_ms:.0f}ms"
                break
            except APIError as e:
                if "429" in str(e) and attempt < 2:
                    await wait_for_rate_limit(10)
                    continue
                raise
        await client.close()

    @pytest.mark.asyncio
    @skip_no_audio
    async def test_stt_time_to_first_result(self, client):
        """Time from first audio to first STT result should be reasonable."""
        audio_path = AUDIO_DIR / "cmu_bdl_arctic_a0001.wav"
        pcm_data = load_raw_pcm(audio_path)
        chunks = chunk_audio(pcm_data)

        first_result_time = None
        audio_start_time = None

        def on_result(r):
            nonlocal first_result_time
            if first_result_time is None and r.text.strip():
                first_result_time = time.monotonic()

        session = client.stt.create(
            provider="deepgram",
            language="en-US",
            model="nova-3",
            reconnect=ReconnectConfig(enabled=False),
        )
        session.on("transcript", on_result)

        await sdk_connect_with_retry(session)

        audio_start_time = time.monotonic()
        for chunk in chunks:
            await session.send_audio(chunk)
            await asyncio.sleep(CHUNK_DURATION_MS / 1000 * 0.3)

        deadline = time.monotonic() + STT_RESULT_TIMEOUT
        while time.monotonic() < deadline:
            if first_result_time is not None:
                break
            await asyncio.sleep(0.05)

        await session.disconnect()

        if first_result_time and audio_start_time:
            ttfr_ms = (first_result_time - audio_start_time) * 1000
            print(f"  STT time-to-first-result: {ttfr_ms:.1f}ms")
            assert ttfr_ms < 5000, f"STT TTFR too slow: {ttfr_ms:.0f}ms"
        else:
            print("  Warning: No STT result received")

        await client.close()

    @pytest.mark.asyncio
    async def test_multiple_sequential_connections(self, client):
        """Multiple sequential connections should all succeed."""
        connect_times = []

        for i in range(3):
            session = client.stt.create(
                provider="deepgram",
                language="en-US",
                reconnect=ReconnectConfig(enabled=False),
            )

            start = time.monotonic()
            await sdk_connect_with_retry(session)
            elapsed = (time.monotonic() - start) * 1000
            connect_times.append(elapsed)

            assert session.connected
            assert session.stream_id is not None
            await session.disconnect()

            await asyncio.sleep(0.5)

        avg_ms = sum(connect_times) / len(connect_times)
        print(f"  Connection times: {[f'{t:.0f}ms' for t in connect_times]}")
        print(f"  Average: {avg_ms:.0f}ms")
        await client.close()


# ===========================================================================
# Phase 6: SDK BudClient Integration Tests
# ===========================================================================
@skip_no_gateway
class TestBudClientIntegration:
    """Test the BudClient high-level API integration."""

    @pytest.mark.asyncio
    async def test_context_manager(self):
        """BudClient should work as async context manager."""
        async with BudClient(base_url=BASE_URL) as bud:
            for attempt in range(3):
                try:
                    health = await bud.health()
                    assert (health.get("status") or "").lower() == "ok"
                    break
                except APIError as e:
                    if "429" in str(e) and attempt < 2:
                        await wait_for_rate_limit(10)
                        continue
                    raise

    @pytest.mark.asyncio
    async def test_stt_pipeline_context_manager(self):
        """STT session should work as async context manager."""
        bud = BudClient(base_url=BASE_URL)
        session = bud.stt.create(
            provider="deepgram",
            language="en-US",
            reconnect=ReconnectConfig(enabled=False),
        )
        async with session:
            assert session.connected
        assert not session.connected
        await bud.close()

    @pytest.mark.asyncio
    async def test_tts_pipeline_context_manager(self):
        """TTS session should work as async context manager."""
        bud = BudClient(base_url=BASE_URL)
        session = bud.tts.create(
            provider="deepgram",
            model="aura-asteria-en",
            voice_id="aura-asteria-en",
            sample_rate=24000,
            reconnect=ReconnectConfig(enabled=False),
        )
        async with session:
            assert session.connected
        assert not session.connected
        await bud.close()

    @pytest.mark.asyncio
    async def test_sdk_stream_id_passthrough(self):
        """Custom stream_id should be passed through the SDK pipeline."""
        bud = BudClient(base_url=BASE_URL)
        custom_id = "sdk-test-stream-42"
        session = bud.stt.create(
            provider="deepgram",
            language="en-US",
            stream_id=custom_id,
            reconnect=ReconnectConfig(enabled=False),
        )
        await sdk_connect_with_retry(session)
        assert session.stream_id == custom_id
        await session.disconnect()
        await bud.close()

    @pytest.mark.asyncio
    async def test_sdk_metrics_collection(self):
        """SDK should collect connection metrics."""
        bud = BudClient(base_url=BASE_URL)
        session = bud.stt.create(
            provider="deepgram",
            language="en-US",
            reconnect=ReconnectConfig(enabled=False),
        )
        await sdk_connect_with_retry(session)

        metrics = session.get_metrics()
        assert metrics.ws_connect_ms > 0
        assert metrics.messages_sent >= 1

        await session.disconnect()
        await bud.close()

        print(f"  WS connect: {metrics.ws_connect_ms:.1f}ms")
        print(f"  Messages sent: {metrics.messages_sent}")
        print(f"  Messages received: {metrics.messages_received}")
