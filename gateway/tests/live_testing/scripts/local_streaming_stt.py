#!/usr/bin/env python3
"""
A local streaming speech recogniser that speaks the Deepgram live wire.

The gateway's Deepgram provider connects here when a session sets
`stt_config.extras.endpoint_override` to `ws://127.0.0.1:<port>` (the gateway needs
WAAV_ALLOW_LOOPBACK_ENDPOINTS=1). Audio comes in as binary PCM16 frames on `/v1/listen`; this
server transcribes it with an OpenAI-compatible Whisper (`/v1/audio/transcriptions`) or, with
`--engine parakeet`, in this process with NVIDIA Parakeet TDT 0.6B v3 (int8 ONNX on the CPU, through
the `onnx-asr` package and the model files in the Hugging Face cache; nothing is downloaded):

- while speech is going on, the utterance so far is transcribed again every `--interim-ms` and
  sent as an interim result (`is_final: false`),
- after `--final-silence-ms` without speech the whole utterance is transcribed once more and sent
  as `is_final: true, speech_final: true`. A connection that asks for a longer `endpointing`
  (the Deepgram query parameter, in ms) gets that instead.

Every result carries the confidence 0.9. Whisper's own word probabilities are no measure of
how clear the audio was (clear one-word clips score 0.50 to 0.65, speech played backwards 0.45
to 0.77 and more), so they are not passed on as a recogniser's confidence.

Speech is found with an energy gate on 20 ms frames, which is enough for clean test audio. Of the
client's text messages only `Finalize` and `CloseStream` are acted on (`KeepAlive` is ignored).
Standard library plus `websockets` (Parakeet: `onnx-asr`, `numpy`). Loopback only.

Why Parakeet (r7-fix, 21 Oct): Whisper small.en shares the GPU with the front model and mishears
the first half second of "Stop talking." ("ZOB", "Zop Talk"); live its words came 0.82-0.86 s after
the voice. Parakeet on the CPU hears "Stop." from 0.3 s of audio and returns nothing (not an
invented sentence) on shorter audio, so its first interim can come after 100 ms of voice.
"""

import argparse
import asyncio
import io
import json
import math
import os
import re
import struct
import time
import urllib.parse
import urllib.request
import wave

FRAME_MS = 20
# What the gate keeps from before the first voiced frame.
PREROLL_MS = 200
# The longest stretch handed to Whisper (its window is 30 s).
WINDOW_S = 28
# Less voiced audio than this is not worth a request; an interim needs a little more.
MIN_VOICED_MS = 100
# Whisper invents a sentence on less audio; Parakeet returns nothing on it.
MIN_INTERIM_MS = {"whisper": 250, "parakeet": 100}
PARAKEET = "models--istupakov--parakeet-tdt-0.6b-v3-onnx"


def parakeet(threads: int):
    """Parakeet TDT 0.6B v3 (int8) from the local Hugging Face cache, as `transcribe` takes it."""
    import glob

    import numpy as np
    import onnx_asr
    import onnxruntime

    hub = os.environ.get("HF_HUB_CACHE") or os.path.expanduser("~/.cache/huggingface/hub")
    found = sorted(glob.glob(f"{hub}/{PARAKEET}/snapshots/*/encoder-model.int8.onnx"))
    if not found:
        raise SystemExit(f"--engine parakeet: no {PARAKEET} snapshot with the int8 model under {hub}")
    options = onnxruntime.SessionOptions()
    # Two threads: on the shared 20-core box (load 15-20) 4 and 8 threads were slower.
    options.intra_op_num_threads = threads
    options.log_severity_level = 3
    model = onnx_asr.load_model("nemo-parakeet-tdt-0.6b-v3", os.path.dirname(found[-1]),
                                quantization="int8", sess_options=options)

    def run(pcm: bytes, rate: int) -> str:
        audio = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768.0
        return model.recognize(audio, sample_rate=rate) if len(audio) else ""

    return run


def transcribe(audio_url: str, pcm: bytes, rate: int, single_pass: bool = False) -> str:
    """What the speech-to-text server at `audio_url` hears in this PCM16 mono audio.

    `single_pass` switches Whisper's temperature fallback off: on the first few hundred
    milliseconds of an utterance the fallback takes up to 2 s and invents a sentence."""
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(pcm)
    boundary = "live" + os.urandom(8).hex()
    fields = {"model": "whisper-1", "response_format": "json"}
    if single_pass:
        fields.update(temperature="0", temperature_inc="0")
    parts = [
        *(f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{value}\r\n'.encode()
          for name, value in fields.items()),
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="audio.wav"\r\n'
        f'Content-Type: audio/wav\r\n\r\n'.encode() + buffer.getvalue() + b"\r\n",
        f"--{boundary}--\r\n".encode(),
    ]
    request = urllib.request.Request(
        f"{audio_url}/v1/audio/transcriptions", data=b"".join(parts),
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.loads(response.read()).get("text", "").strip()
    except Exception as error:
        return f"(not transcribed: {error})"


def words_only(text: str) -> str:
    """Whisper's text without its non-speech marks ("[BLANK_AUDIO]", "(silence)")."""
    return " ".join(re.sub(r"\[[^\]]*\]|\([^)]*\)", " ", text).split())


def stamp() -> str:
    """The time of day to the millisecond, as in the gateway's log."""
    now = time.time()
    return time.strftime("%H:%M:%S", time.gmtime(now)) + f".{int(now * 1000) % 1000:03d}"


def frame_db(frame: bytes) -> float:
    count = len(frame) // 2
    if count == 0:
        return -100.0
    samples = struct.unpack(f"<{count}h", frame[:count * 2])
    rms = math.sqrt(sum(s * s for s in samples) / count)
    return 20 * math.log10(rms / 32768.0) if rms > 0 else -100.0


class Stream:
    """One `/v1/listen` connection."""

    def __init__(self, ws, rate: int, final_silence_ms: int, args):
        self.ws = ws
        self.rate = rate
        self.final_silence_ms = final_silence_ms
        self.args = args
        self.frame_bytes = rate * FRAME_MS // 1000 * 2
        self.pending = bytearray()
        self.preroll = bytearray()
        # The utterance in progress (None between utterances), its voiced time, the silence
        # since its last voiced frame, and whether it grew since the last interim.
        self.speech = None
        self.voiced_ms = 0
        self.silence_ms = 0
        self.fresh = False
        self.finalize = False
        self.sent_s = 0.0
        self.start_s = 0.0

    def feed(self, data: bytes):
        self.pending += data
        while len(self.pending) >= self.frame_bytes:
            frame = bytes(self.pending[:self.frame_bytes])
            del self.pending[:self.frame_bytes]
            self.sent_s += FRAME_MS / 1000
            if frame_db(frame) > self.args.gate_db:
                if self.speech is None:
                    self.speech = bytearray(self.preroll)
                    self.start_s = self.sent_s - len(self.preroll) / 2 / self.rate
                    self.voiced_ms = 0
                    if self.args.verbose:
                        print(f"{stamp()} speech from {self.sent_s:.2f} s of the stream", flush=True)
                self.voiced_ms += FRAME_MS
                self.silence_ms = 0
                self.fresh = True
            elif self.speech is not None:
                self.silence_ms += FRAME_MS
            if self.speech is not None:
                self.speech += frame
            else:
                self.preroll += frame
                del self.preroll[:-self.rate * PREROLL_MS // 1000 * 2]

    async def hear(self, pcm: bytes, interim: bool) -> str:
        pcm = pcm[-self.rate * WINDOW_S * 2:]
        began, voiced_ms = time.monotonic(), self.voiced_ms
        if self.args.model is not None:
            text = words_only(await asyncio.to_thread(self.args.model, pcm, self.rate))
        else:
            text = words_only(await asyncio.to_thread(transcribe, self.args.whisper, pcm, self.rate, interim))
        if self.args.verbose:
            print(f"{stamp()} {'interim' if interim else 'final  '} {text!r} "
                  f"({len(pcm) // 2 * 1000 // self.rate} ms of audio, {voiced_ms} ms voiced, heard in "
                  f"{(time.monotonic() - began) * 1000:.0f} ms)", flush=True)
        return text

    async def send(self, text: str, final: bool, seconds: float, from_finalize: bool = False):
        result = {
            "type": "Results", "channel_index": [0, 1], "start": round(self.start_s, 3),
            "duration": round(seconds, 3), "is_final": final, "speech_final": final,
            "channel": {"alternatives": [{"transcript": text, "confidence": 0.9, "words": []}]},
        }
        if from_finalize:
            result["from_finalize"] = True
        await self.ws.send(json.dumps(result))

    async def end_utterance(self, from_finalize: bool = False):
        """The final result of the utterance in progress."""
        pcm, voiced_ms = bytes(self.speech or b""), self.voiced_ms
        self.speech, self.preroll, self.fresh, self.finalize = None, bytearray(), False, False
        text = await self.hear(pcm, False) if voiced_ms >= MIN_VOICED_MS else ""
        await self.send(text, True, len(pcm) / 2 / self.rate, from_finalize)

    async def work(self):
        """Transcribes beside the socket, so audio is read while Whisper runs."""
        last_interim = 0.0
        while True:
            await asyncio.sleep(FRAME_MS / 1000)
            if self.finalize:
                await self.end_utterance(from_finalize=True)
            if self.speech is None:
                continue
            if self.silence_ms >= self.final_silence_ms:
                await self.end_utterance()
            elif (self.fresh and self.voiced_ms >= MIN_INTERIM_MS[self.args.engine]
                  and time.monotonic() - last_interim >= self.args.interim_ms / 1000):
                self.fresh = False
                last_interim = time.monotonic()
                pcm = bytes(self.speech)
                text = await self.hear(pcm, True)
                if text:
                    await self.send(text, False, len(pcm) / 2 / self.rate)

    async def run(self):
        worker = asyncio.create_task(self.work())
        try:
            async for message in self.ws:
                if isinstance(message, bytes):
                    self.feed(message)
                    continue
                kind = json.loads(message).get("type")
                if kind == "Finalize":
                    self.finalize = True
                elif kind == "CloseStream":
                    break
        finally:
            worker.cancel()
            try:
                await worker
            except asyncio.CancelledError:
                pass
        if self.speech is not None:
            try:
                await self.end_utterance()
            except Exception:
                pass  # the client did not wait for it


async def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", type=int, default=8126)
    parser.add_argument("--engine", choices=("whisper", "parakeet"), default="whisper",
                        help="whisper: the server at --whisper; parakeet: Parakeet TDT 0.6B v3 in this process")
    parser.add_argument("--threads", type=int, default=2, help="--engine parakeet: onnxruntime threads")
    parser.add_argument("--whisper", default="http://127.0.0.1:8124",
                        help="base URL of the OpenAI-compatible transcription server")
    # 100: Whisper's own time (100 to 300 ms an interim, one at a time) is the cadence. Live S1 at
    # 350 had the cue's words come 961 ms after the voice when the first interim (260 ms voiced)
    # was misheard; at 200, "Stop talking." was misheard twice ("ZOB", "ZOPTALK", until 580 ms of
    # it) and its words came 883 to 1101 ms after the voice: an interim began 200 ms after the one
    # before began (r5-latency).
    parser.add_argument("--interim-ms", type=int, default=100)
    parser.add_argument("--final-silence-ms", type=int, default=500)
    parser.add_argument("--gate-db", type=float, default=-45.0,
                        help="a 20 ms frame above this level (dBFS) is speech")
    parser.add_argument("--verbose", action="store_true", help="print every result")
    args = parser.parse_args()
    args.model = parakeet(args.threads) if args.engine == "parakeet" else None
    if args.model is not None:
        args.model(b"\0\0" * 8000, 16000)  # the first run of a session is slow

    import websockets

    async def handler(ws):
        request = getattr(ws, "request", None)
        path = request.path if request is not None else getattr(ws, "path", "")
        query = urllib.parse.parse_qs(urllib.parse.urlsplit(path).query)
        rate = int(query.get("sample_rate", ["16000"])[0])
        endpointing = int(query.get("endpointing", ["0"])[0])
        await Stream(ws, rate, max(args.final_silence_ms, endpointing), args).run()

    async with websockets.serve(handler, "127.0.0.1", args.port, max_size=None):
        print(f"local streaming speech-to-text ({args.engine}) on ws://127.0.0.1:{args.port}/v1/listen", flush=True)
        await asyncio.Future()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
