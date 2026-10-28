"""Full-Duplex-Bench inference for Bud WaaV interaction mode.

For each sample directory (with input.wav), open a real /ws session on the gateway with interaction
mode on, stream input.wav in real time as 20 ms frames, and write the agent's audio as it is heard
by the client into output.wav (time-aligned with input.wav, same duration). The gateway paces the
agent's audio at playback speed, so a frame is placed at the time it arrives (after the previous
frame, never overlapping). Every event is kept in events.jsonl beside it.

usage: fdb_waav.py --root DIR [--concurrency 4] [--limit N] [--overwrite] [--input input.wav --output output.wav]
"""
import argparse
import asyncio
import json
import os
import re
import sys
import time
import wave
from pathlib import Path

import numpy as np
import websockets

CALLER_RATE, AGENT_RATE, FRAME_MS = 16000, 24000, 20
CALLER_FRAME = CALLER_RATE * FRAME_MS // 1000  # samples
GATEWAY = os.environ.get("WAAV_WS", "ws://127.0.0.1:4410/ws")
GW_LOG = Path(os.environ.get("WAAV_GW_LOG", "/home/bud/bench/waav/logs/gateway.log"))
SYSTEM_PROMPT = ("You are a friendly, helpful voice assistant talking with someone on a call. "
                 "Reply briefly and naturally, in one or two short spoken sentences.")


def read_wav(path: Path) -> np.ndarray:
    """Mono int16 at CALLER_RATE (any WAV: float ones are decoded by ffmpeg)."""
    try:
        w = wave.open(str(path))
    except wave.Error:
        import subprocess
        raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", str(path), "-ac", "1", "-ar", str(CALLER_RATE),
                              "-f", "s16le", "-"], capture_output=True, check=True).stdout
        return np.frombuffer(raw, dtype="<i2").copy()
    with w:
        rate, ch, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if width == 2:
        x = np.frombuffer(raw, dtype="<i2").astype(np.float32)
    elif width == 4:
        x = np.frombuffer(raw, dtype="<i4").astype(np.float32) / 65536.0
    else:
        raise ValueError(f"{path}: sample width {width}")
    if ch > 1:
        x = x.reshape(-1, ch).mean(axis=1)
    if rate != CALLER_RATE:
        n = int(round(len(x) * CALLER_RATE / rate))
        x = np.interp(np.linspace(0, len(x) - 1, n), np.arange(len(x)), x)
    return np.clip(x, -32768, 32767).astype("<i2")


def config(args, stream_id: str) -> dict:
    tts = {"provider": "deepgram", "model": args.tts_model, "voice_id": args.tts_model,
           "audio_format": "linear16", "sample_rate": AGENT_RATE, "features": {"streaming": True}}
    return {
        "type": "config", "stream_id": stream_id,
        "stt_config": {"provider": "deepgram", "model": "nova-3", "language": "en", "sample_rate": CALLER_RATE,
                       "channels": 1, "punctuation": True, "encoding": "linear16"},
        "tts_config": tts,
        "conversation_config": {"base_url": "http://127.0.0.1:19742/v1", "model": "qwen3b",
                                "api_key": "local-dummy", "system_prompt": SYSTEM_PROMPT},
        "interaction": True,
    }


def warm_lines(since: int) -> int:
    """How many warm-up lines the gateway's debug log reports since `since` (any session: the log
    line carries no session id; with several sessions this waits for all of them)."""
    try:
        with open(GW_LOG, "rb") as f:
            f.seek(since)
            tail = f.read().decode("utf-8", "replace")
    except OSError:
        return 0
    return tail.count("warm-up line synthesised")


async def wait_warm(since: int, timeout: float = 30.0, hold: float = 2.5):
    """Wait until no warm-up line was synthesised for `hold` seconds (at least 3 s in all): a
    deployed agent's session is open and warm before the caller speaks."""
    start = last = time.monotonic()
    seen = 0
    while time.monotonic() - start < timeout:
        n = warm_lines(since)
        if n != seen:
            seen, last = n, time.monotonic()
        if time.monotonic() - last > hold and time.monotonic() - start > 3.0:
            break
        await asyncio.sleep(0.1)
    return time.monotonic() - start


async def run_one(args, sample: Path, sem: asyncio.Semaphore):
    out_wav = sample / args.output
    if out_wav.exists() and not args.overwrite:
        return "skip"
    async with sem:
        loop = asyncio.get_running_loop()
        pcm = await loop.run_in_executor(None, read_wav, sample / args.input)
        duration = len(pcm) / CALLER_RATE
        stream_id = f"fdb-{sample.parent.name}-{sample.name}-{int(time.time() * 1000) % 10**8}"
        stream_id = re.sub(r"[^A-Za-z0-9_-]", "_", stream_id)
        log_mark = GW_LOG.stat().st_size if GW_LOG.exists() else 0
        # keep the front model resident: an idle server is paged out and its next request is slow
        def keep_alive():
            try:
                import urllib.request
                urllib.request.urlopen(urllib.request.Request(
                    os.environ.get("WAAV_FRONT", "http://127.0.0.1:19743") + "/completion",
                    data=b'{"prompt":"Hi","n_predict":1}', headers={"content-type": "application/json"}), timeout=15).read()
            except Exception:
                pass
        await loop.run_in_executor(None, keep_alive)
        events, frames = [], []
        async with websockets.connect(GATEWAY, max_size=None, ping_interval=None) as ws:
            await ws.send(json.dumps(config(args, stream_id)))
            t_open = time.monotonic()
            state = {"t0": None}

            async def reader():
                async for msg in ws:
                    at = time.monotonic()
                    if isinstance(msg, bytes):
                        frames.append((at, msg))
                    else:
                        events.append((at, json.loads(msg)))

            rd = asyncio.create_task(reader())
            # ready
            while not any(e.get("type") in ("ready", "error") for _, e in events):
                if time.monotonic() - t_open > 60:
                    raise RuntimeError("no ready")
                await asyncio.sleep(0.01)
            ready = next(e for _, e in events if e.get("type") in ("ready", "error"))
            if ready["type"] != "ready" or "interaction" not in ready:
                rd.cancel()
                raise RuntimeError(f"not in interaction mode: {ready} {[e for _, e in events if e.get('type') == 'config_warning']}")
            warm = await wait_warm(log_mark)
            frames.clear()  # nothing before the input starts belongs to output.wav
            t0 = time.monotonic()
            t0_wall = time.time()
            state["t0"] = t0
            sent = 0
            for i in range(0, len(pcm), CALLER_FRAME):
                frame = pcm[i:i + CALLER_FRAME]
                if len(frame) < CALLER_FRAME:
                    frame = np.pad(frame, (0, CALLER_FRAME - len(frame)))
                await ws.send(frame.tobytes())
                sent += 1
                delay = t0 + sent * FRAME_MS / 1000 - time.monotonic()
                if delay > 0:
                    await asyncio.sleep(delay)
            # the input's own trailing silence is part of it; listen a little past its end
            await asyncio.sleep(max(0.0, t0 + duration + args.tail - time.monotonic()))
            rd.cancel()
        def write_outputs():
            # place the agent's audio where it was heard
            out = np.zeros(int(duration * AGENT_RATE), dtype=np.int16)
            cursor = 0
            for at, data in frames:
                s = np.frombuffer(data, dtype="<i2")
                start = max(int(round((at - t0) * AGENT_RATE)), cursor)
                end = min(start + len(s), len(out))
                if start < len(out) and end > start:
                    out[start:end] = s[:end - start]
                cursor = start + len(s)
            with wave.open(str(out_wav), "wb") as w:
                w.setnchannels(1)
                w.setsampwidth(2)
                w.setframerate(AGENT_RATE)
                w.writeframes(out.tobytes())
            with open(sample / (args.output.rsplit(".", 1)[0] + "_events.jsonl"), "w") as f:
                f.write(json.dumps({"t": 0.0, "type": "_meta", "stream_id": stream_id, "warm_s": round(warm, 3),
                                    "duration_s": duration, "t0_after_open_s": round(t0 - t_open, 3), "t0_wall": t0_wall}) + "\n")
                for at, e in events:
                    f.write(json.dumps({"t": round(at - t0, 4), **e}) + "\n")
        await loop.run_in_executor(None, write_outputs)
        heard = sum(1 for at, _ in frames if at - t0 < duration)
        return f"ok {duration:.1f}s frames={heard} warm={warm:.1f}s"


async def main():
    p = argparse.ArgumentParser()
    p.add_argument("--root", required=True)
    p.add_argument("--concurrency", type=int, default=4)
    p.add_argument("--limit", type=int, default=0)
    p.add_argument("--overwrite", action="store_true")
    p.add_argument("--input", default="input.wav")
    p.add_argument("--output", default="output.wav")
    p.add_argument("--tail", type=float, default=0.0)
    p.add_argument("--tts-model", default="aura-2-thalia-en")
    args = p.parse_args()
    samples = sorted(d for d in Path(args.root).iterdir() if (d / args.input).exists())
    if args.limit:
        samples = samples[:args.limit]
    sem = asyncio.Semaphore(args.concurrency)

    async def guarded(s):
        for attempt in range(3):
            try:
                return s, await run_one(args, s, sem)
            except Exception as e:  # noqa: BLE001
                err = f"error {type(e).__name__}: {e}"[:300]
                await asyncio.sleep(2)
        return s, err

    done = 0
    for coro in asyncio.as_completed([guarded(s) for s in samples]):
        s, res = await coro
        done += 1
        print(f"[{done}/{len(samples)}] {s.name}: {res}", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
