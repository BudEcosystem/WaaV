#!/usr/bin/env python3
"""
Deepgram matrix for interaction mode, against a running gateway.

Parts (``--parts``, default ``A,B,C,E,D``; D runs last because a refused Voice Agent setting
can open the gateway's Deepgram circuit breaker):

A  voices: one ``/ws`` session per Deepgram voice speaks the language pack's listening sounds
   and one acknowledgement; each clip is checked for length after trimming silence (<= 600 ms
   for a listening sound), for silence, and by a round trip through Deepgram's own nova-3 in the
   voice's language (prerecorded ``/v1/listen``, called directly) to catch spelled-out sounds.
B  speech-to-text: every streaming model x each of its base languages that has a FLEURS clip is
   streamed in real time through a ``/ws`` session (``--concurrency``): word error rate
   (character error rate for unsegmented scripts), first-interim and final latency, whether the
   confidence is a real value, and the language ``stt_result`` names (``detected_language``).
   ``--multi-languages`` also streams those clips through nova-3 with ``language: "multi"``, whose
   results must name the clip's language. Prerecorded-only models: a session asking for
   interaction mode (it must get ``interaction_surface_unavailable`` reason ``stt_file_only``),
   a mode-off session, and ``POST /transcribe/batch``.
C  interaction smoke: each streaming English model runs the question turn with
   ``"interaction": true`` and must show ``task_created``, an acknowledgement or a relay, and
   ``task_result``. ``--smoke-languages`` runs the same turn in each of those languages with
   nova-3, an Aura-2 voice of the language and the language's spoken request (the gateway needs
   that language's pack); it must also keep the mode on (no ``interaction_language_step_down``).
D  Voice Agent: through the gateway's ``/realtime`` (provider ``deepgram``) for each think model
   (the gateway names the model's vendor), a function call (the gateway must run with the
   ``deepgram_tools`` provider fix to send the session's functions), an interaction-mode session
   whose think model delegates a task (``--delegate-think``; it must show ``task_created`` and
   ``task_result``), a voice spread with each voice's language, a Flux listen model
   (``transcription_model``) and the EU region (``region: "eu"``); directly against
   ``agent.deepgram.com`` a listen-model x voice spread, every think model with its own
   provider type, a function round trip, and the InjectAgentMessage / InjectUserMessage probe
   (every frame sent and received is recorded). ``--no-direct`` skips the direct probes.
L  listening sounds through the gateway (``--listen-voices``, ``voice:language``): one interaction-mode
   ``/ws`` session per Aura-2 voice (nova-3 in its language finalising after 1,400 ms of silence,
   the model front: the gateway needs ``WAAV_INTERACTION_FRONT_URL``) hears one long sentence in that language, synthesised in one piece by Aura-2
   (so its comma pauses sound unfinished) with each pause stretched to 1,200 ms; the gateway's bank is
   made from the pack's words in that voice and checked (``--gateway-log``: its ``interaction
   listening sounds ready`` and ``... refused`` lines); a listening sound must start in the turn, be
   audible, at most 600 ms of sound and, sent to nova-3 in the voice's language, not spelled out.
E  endpoint types through the gateway: Flux (``/v2/listen``), the EU host for speech-to-text and
   streaming text-to-speech, prerecorded ``/transcribe/batch``, ``POST /speak`` (REST) and the
   ``/v1/speak`` WebSocket (``tts_config.features.streaming``). ``--e-repeats`` N: each of
   streaming STT (nova-3, Flux), REST and WebSocket TTS, prerecorded STT and the Voice Agent runs
   N times through the gateway and N times against Deepgram directly with the same audio, text
   and query (``case`` ``overhead_*``: gateway minus raw latency); ``/v1/realtime`` is probed
   without a credential (it serves deployments of Bud mode only).
S  stop cue (the live script's S1): in an interaction-mode session the agent says a long line,
   1.5 s into it the caller says the cue; ``cut_ms`` is the time from the cue's first voiced
   frame to ``agent_audio_stopped``, ``words_ms`` from that frame to the first ``stt_result``
   holding a cue word (``--stop-sets model:language:runs``; English cues are the live script's
   caller clips in ``--caller-dir``, other languages their spoken ``stop`` request). The model front
   decides the cue (no phrase list), so the gateway needs ``WAAV_INTERACTION_FRONT_URL``.

Widened coverage (final matrix): A also says ``--voice-repeats`` sentences of the voice's language
per route (``/ws`` over Deepgram REST, ``/ws`` over the ``/v1/speak`` WebSocket, ``POST /speak``):
first audio, synthesis time and a round trip through nova-3 scored by word (or character) error
rate. B adds Flux (``--flux-pairs``), latency repeats (``--latency-pairs``, ``--latency-repeats``)
and per-model caps of the prerecorded pairs (``--file-cap``, ``--file-full-models``). C runs
``--smoke-turns`` turns per session and counts listening sounds. D gives every think model a
second, tool-calling turn (``--think-tools``), probes ``text`` injection and the refused
injection (``--inject-think``, ``--user-text-url``: a gateway with ``deepgram_user_text``). Every
row records the one-minute load average (``load1``); each part its load samples.

The gateway must run with DEEPGRAM_API_KEY in its environment. This script reads
DEEPGRAM_API_KEY from its own environment only for the direct calls (A's round trip, D's direct
probes, the balance check); without it those are skipped.

Budget: every item is priced before it runs (``budget/pricing.json``) and charged to a ledger
shared across runs (``--ledger``). A part stops cleanly when the ledger total would pass
``--budget-usd`` (default 150); what ran and what was skipped is written to the results. The
project balance is read before each part and every ``--balance-every`` items when the key may
read it (``billing:read``); otherwise the estimate is the guard.

Data (``--data``, the matrix work directory): ``matrix.json``, ``data/fleurs/index.json``,
``data/requests/index.json``, ``packs/`` and ``budget/pricing.json``.

Self-contained on purpose: ``interaction_live_e2e.Session`` is bound to that script's stack and
its fixed OpenAI/Whisper provider configs, so only trivial audio helpers would be reused.
"""

import argparse
import asyncio
import base64
import io
import json
import math
import os
import re
import sys
import time
import unicodedata
import urllib.error
import urllib.request
import wave
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

import numpy as np
import websockets

REPO_GATEWAY = Path(__file__).resolve().parents[3]
EN_ASSETS = REPO_GATEWAY / "src/core/interaction/assets"
RATE = 16000
AGENT_RATE = 24000
FRAME_MS = 20
UNSEGMENTED = {"ja", "zh", "th", "ko", "lo", "km", "my", "yue", "bo"}
DG_API = "https://api.deepgram.com"
DG_AGENT = "wss://agent.deepgram.com/v1/agent/converse"
QUESTION = {"en": "What is the capital of France?"}
# One Aura-2 voice per language of the draft packs (Deepgram's catalogue, 2026-10-14).
PACK_VOICES = {"de": "aura-2-aurelia-de", "es": "aura-2-agustina-es", "fr": "aura-2-agathe-fr",
               "it": "aura-2-cesare-it", "nl": "aura-2-beatrix-nl", "ja": "aura-2-ama-ja",
               "en": "aura-2-thalia-en"}
INSTRUCTIONS = "You are a helpful phone assistant. Answer in one short sentence."


def now() -> float:
    return time.monotonic()


def ms(seconds: Optional[float]) -> Optional[int]:
    return None if seconds is None else round(seconds * 1000)


def load1() -> float:
    return round(os.getloadavg()[0], 1)


class LoadSampler:
    """The one-minute load average every `every` seconds while a part runs (the box is shared)."""

    def __init__(self, every: float = 10.0):
        self.every, self.samples, self.task = every, [], None

    async def _run(self):
        while True:
            self.samples.append(load1())
            await asyncio.sleep(self.every)

    def start(self) -> "LoadSampler":
        self.task = asyncio.create_task(self._run())
        return self

    def stop(self) -> dict:
        if self.task:
            self.task.cancel()
        s = self.samples or [load1()]
        return {"min": min(s), "mean": round(sum(s) / len(s), 1), "max": max(s), "samples": len(s),
                "over_20": sum(x > 20 for x in s)}


# ---- audio


def load_wav(path: Path) -> Tuple[bytes, int]:
    with wave.open(str(path), "rb") as w:
        if w.getnchannels() != 1 or w.getsampwidth() != 2:
            raise ValueError(f"{path}: mono 16-bit expected")
        return w.readframes(w.getnframes()), w.getframerate()


def resample(pcm: bytes, src: int, dst: int) -> bytes:
    if src == dst:
        return pcm
    x = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype="<i2").astype(np.float64)
    if x.size == 0:
        return b""
    n = int(x.size * dst / src)
    return np.interp(np.linspace(0, x.size - 1, n), np.arange(x.size), x).astype("<i2").tobytes()


def wav_bytes(pcm: bytes, rate: int) -> bytes:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(rate); w.writeframes(pcm)
    return buf.getvalue()


def dbfs(pcm: bytes) -> float:
    x = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype="<i2").astype(np.float64)
    if x.size == 0:
        return -100.0
    rms = math.sqrt(float(np.mean(x * x)))
    return max(-100.0, 20 * math.log10(rms / 32768.0)) if rms > 0 else -100.0


def voiced_span(pcm: bytes, rate: int, floor_db: float = -45.0) -> Tuple[float, float]:
    """(start, end) in seconds of the audio above `floor_db` in 10 ms frames."""
    step = rate // 100 * 2
    frames = [dbfs(pcm[i:i + step]) for i in range(0, len(pcm), step)]
    voiced = [i for i, d in enumerate(frames) if d > floor_db]
    if not voiced:
        return 0.0, 0.0
    return voiced[0] / 100, (voiced[-1] + 1) / 100


# ---- text scoring


def norm_text(text: str) -> str:
    text = unicodedata.normalize("NFKC", text).lower()
    text = "".join(" " if unicodedata.category(c)[0] in "PS" else c for c in text)
    return " ".join(text.split())


def edit_distance(a: List[str], b: List[str]) -> int:
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1]


def error_rate(reference: str, hypothesis: str, language: str) -> Tuple[str, Optional[float]]:
    ref, hyp = norm_text(reference), norm_text(hypothesis)
    if language.split("-")[0] in UNSEGMENTED:
        r, h, kind = list(ref.replace(" ", "")), list(hyp.replace(" ", "")), "cer"
    else:
        r, h, kind = ref.split(), hyp.split(), "wer"
    if not r:
        return kind, None
    return kind, round(edit_distance(r, h) / len(r), 4)


# ---- HTTP


def http(url: str, method: str = "GET", body: Any = None, headers: Optional[dict] = None,
         timeout: float = 120, ctype: str = "application/json") -> Tuple[int, bytes, dict]:
    data = body if isinstance(body, (bytes, type(None))) else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method,
                                 headers={"Content-Type": ctype, **(headers or {})})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read(), dict(r.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read(), dict(e.headers)
    except Exception as e:  # network
        return 0, f"{type(e).__name__}: {e}".encode(), {}


def http_timed(url: str, body: Any, headers: Optional[dict] = None, timeout: float = 60,
               ctype: str = "application/json") -> dict:
    """A POST whose response is read in chunks: status, time to the first body byte and to the
    last (ms), the body and its content type."""
    data = body if isinstance(body, bytes) else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method="POST", headers={"Content-Type": ctype, **(headers or {})})
    t, first, chunks = now(), None, []
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, ctype_out = r.status, r.headers.get("Content-Type")
            while True:
                chunk = r.read1(8192) if hasattr(r, "read1") else r.read(8192)
                if not chunk:
                    break
                first = first or now()
                chunks.append(chunk)
    except urllib.error.HTTPError as e:
        return {"status": e.code, "body": e.read(), "ttfb_ms": None, "total_ms": ms(now() - t), "content_type": None}
    except Exception as e:  # network
        return {"status": 0, "body": f"{type(e).__name__}: {e}".encode(), "ttfb_ms": None, "total_ms": ms(now() - t),
                "content_type": None}
    return {"status": status, "body": b"".join(chunks), "ttfb_ms": ms(first - t) if first else None,
            "total_ms": ms(now() - t), "content_type": ctype_out}


def pcm_of(body: bytes) -> bytes:
    """Raw linear16 from a body that may be a WAV file."""
    if body[:4] == b"RIFF":
        with wave.open(io.BytesIO(body), "rb") as w:
            return w.readframes(w.getnframes())
    return body


def pctl(values: List[float], p: float) -> Optional[float]:
    v = sorted(x for x in values if x is not None)
    if not v:
        return None
    k = (len(v) - 1) * p / 100
    lo, hi = math.floor(k), math.ceil(k)
    return round(v[lo] + (v[hi] - v[lo]) * (k - lo), 1)


def dg_key() -> Optional[str]:
    return os.environ.get("DEEPGRAM_API_KEY") or None


def dg_headers() -> dict:
    return {"Authorization": "Token " + (dg_key() or "")}


def dg_transcribe(pcm: bytes, rate: int, model: str, language: str,
                  smart: bool = True) -> Tuple[int, str, Optional[float]]:
    status, body, _ = http(f"{DG_API}/v1/listen?model={model}&language={language}&smart_format={str(smart).lower()}",
                           "POST", wav_bytes(pcm, rate), dg_headers(), ctype="audio/wav")
    if status != 200:
        return status, body[:200].decode("utf-8", "replace"), None
    alt = json.loads(body)["results"]["channels"][0]["alternatives"][0]
    return status, alt.get("transcript", ""), alt.get("confidence")


# ---- budget


class Budget:
    """A running estimate of Deepgram spend, shared across runs through a JSONL ledger."""

    def __init__(self, args, pricing: dict):
        self.limit = args.budget_usd
        self.prices = pricing
        self.ledger = Path(args.ledger)
        self.ledger.parent.mkdir(parents=True, exist_ok=True)
        self.before = sum(json.loads(l).get("usd", 0.0) for l in self.ledger.read_text().splitlines()
                          if l.strip()) if self.ledger.exists() else 0.0
        self.run = 0.0
        self.by_part: Dict[str, float] = {}
        self.items = 0
        self.balance_every = args.balance_every
        self.balance_notes: List[dict] = []
        self.start_balance: Optional[float] = None
        self.measured: Optional[float] = None
        self.stopped: Optional[str] = None
        self.dry = args.estimate_only

    @property
    def total(self) -> float:
        return self.before + self.run

    def stt_rate(self, model: str, streaming: bool = True) -> float:
        table = self.prices["stt_streaming_per_min" if streaming else "stt_prerecorded_per_min"]
        for key in ("flux-general-en", "nova-3", "nova-2", "whisper", "enhanced"):
            if model.startswith(key):
                return table.get(key, table["default"])
        if model.startswith("nova"):
            return table.get("nova-2", table["default"])
        return table["default"]

    def stt(self, model: str, seconds: float, streaming: bool = True) -> float:
        return seconds / 60 * self.stt_rate(model, streaming)

    def tts(self, chars: int, model: str = "aura-2") -> float:
        return chars / 1000 * self.prices["tts_per_1k_chars"]["aura-2" if "aura-2" in model else "aura"]

    def agent(self, seconds: float) -> float:
        return seconds / 60 * self.prices["agent_per_min"]["guard_rate"]

    def allow(self, part: str, estimate: float) -> bool:
        if self.stopped:
            return False
        if self.measured is not None and self.measured >= self.limit:
            self.stopped = f"measured spend {self.measured:.2f} reached {self.limit}"
            return False
        if self.total + estimate > self.limit:
            self.stopped = f"estimate {self.total:.2f} + {estimate:.3f} would pass {self.limit} (part {part})"
            return False
        return True

    def charge(self, part: str, usd: float, note: str = ""):
        self.run += usd
        self.by_part[part] = self.by_part.get(part, 0.0) + usd
        self.items += 1
        with self.ledger.open("a") as f:
            f.write(json.dumps({"at": time.time(), "part": f"matrix-{part}", "usd": round(usd, 6),
                                "note": note[:120]}) + "\n")
        if self.balance_every and self.items % self.balance_every == 0:
            self.check_balance(f"after {self.items} items")

    def check_balance(self, when: str):
        if not dg_key():
            return
        if self.balance_notes and self.balance_notes[-1].get("status") == 403:
            return  # the key cannot read it: once is enough
        status, body, _ = http(f"{DG_API}/v1/projects", headers=dg_headers(), timeout=20)
        note: Dict[str, Any] = {"when": when, "status": status}
        if status == 200:
            pid = json.loads(body)["projects"][0]["project_id"]
            status, body, _ = http(f"{DG_API}/v1/projects/{pid}/balances", headers=dg_headers(), timeout=20)
            note["status"] = status
            if status == 200:
                amount = sum(b.get("amount", 0.0) for b in json.loads(body).get("balances", []))
                note["balance"] = amount
                if self.start_balance is None:
                    self.start_balance = amount
                self.measured = self.before + (self.start_balance - amount)
            else:
                note["detail"] = json.loads(body).get("details", "") if body[:1] == b"{" else ""
        self.balance_notes.append(note)

    def summary(self) -> dict:
        return {"limit_usd": self.limit, "ledger": str(self.ledger), "spent_before_run_usd": round(self.before, 4),
                "this_run_usd": round(self.run, 4), "by_part_usd": {k: round(v, 4) for k, v in self.by_part.items()},
                "total_usd": round(self.total, 4), "measured_usd": self.measured,
                "balance_checks": self.balance_notes, "stopped": self.stopped}


# ---- a /ws session


class WsSession:
    def __init__(self, url: str, label: str):
        self.url, self.label = url, label
        self.events: List[Tuple[float, dict]] = []
        self.audio: List[Tuple[float, bytes]] = []
        self.ws = None
        self.reader: Optional[asyncio.Task] = None
        self.opened = 0.0
        self.ready: Optional[dict] = None
        self.closed_reason: Optional[str] = None

    async def open(self, config: dict, timeout: float = 30.0) -> "WsSession":
        self.ws = await websockets.connect(self.url, max_size=None, ping_interval=None, open_timeout=20)
        self.opened = now()
        await self.ws.send(json.dumps(config))
        self.reader = asyncio.create_task(self._read())
        found = await self.wait(lambda e: e.get("type") in ("ready", "error", "session_created"), timeout)
        if found and found[1].get("type") in ("ready", "session_created"):
            self.ready = found[1]
        return self

    async def _read(self):
        try:
            async for m in self.ws:
                if isinstance(m, bytes):
                    self.audio.append((now(), m))
                else:
                    try:
                        self.events.append((now(), json.loads(m)))
                    except ValueError:
                        self.events.append((now(), {"type": "?", "raw": m[:200]}))
        except Exception as e:
            self.closed_reason = f"{type(e).__name__}: {e}"[:200]
        else:
            self.closed_reason = "closed"

    async def wait(self, match, timeout: float, start: int = 0) -> Optional[Tuple[float, dict]]:
        deadline, i = now() + timeout, start
        while True:
            while i < len(self.events):
                at, e = self.events[i]
                if match(e):
                    return at, e
                i += 1
            if now() > deadline or (self.reader and self.reader.done()):
                return None
            await asyncio.sleep(0.01)

    async def send(self, msg: dict):
        await self.ws.send(json.dumps(msg))

    async def stream(self, pcm: bytes, rate: int, frame_ms: int = FRAME_MS) -> Tuple[float, float]:
        """Send `pcm` in real time; returns (start, end) send times."""
        size = rate * frame_ms // 1000 * 2
        start, sent = now(), 0
        for i in range(0, len(pcm), size):
            frame = pcm[i:i + size].ljust(size, b"\0")
            try:
                await self.ws.send(frame)
            except Exception:
                break
            sent += 1
            delay = start + sent * frame_ms / 1000 - now()
            if delay > 0:
                await asyncio.sleep(delay)
        return start, now()

    async def silence(self, seconds: float, rate: int, frame_ms: int = FRAME_MS):
        await self.stream(bytes(int(rate * seconds) * 2), rate, frame_ms)

    def of(self, kind: str) -> List[Tuple[float, dict]]:
        return [(a, e) for a, e in self.events if e.get("type") == kind]

    def warnings(self) -> List[dict]:
        return [e for _, e in self.events if e.get("type") in ("config_warning", "warning")]

    def errors(self) -> List[dict]:
        return [e for _, e in self.events if e.get("type") == "error"]

    def pcm(self, since: float = 0.0, until: float = float("inf")) -> bytes:
        return b"".join(d for a, d in self.audio if since <= a <= until)

    async def close(self):
        try:
            await asyncio.wait_for(self.ws.close(), 5)
        except Exception:
            pass
        if self.reader:
            self.reader.cancel()


def ws_url(base: str, path: str) -> str:
    return re.sub(r"^http", "ws", base.rstrip("/")) + path


def stt_cfg(model: str, language: str, extras: Optional[dict] = None) -> dict:
    cfg = {"provider": "deepgram", "model": model, "language": language, "sample_rate": RATE,
           "channels": 1, "punctuation": True, "encoding": "linear16"}
    if extras:
        cfg["extras"] = extras
    return cfg


def tts_cfg(voice: str, rate: int = RATE, features: Optional[dict] = None, extras: Optional[dict] = None) -> dict:
    cfg = {"provider": "deepgram", "model": voice, "voice_id": voice, "audio_format": "linear16",
           "sample_rate": rate}
    if features:
        cfg["features"] = features
    if extras:
        cfg["extras"] = extras
    return cfg


def error_text(events: List[dict]) -> str:
    return "; ".join(f"{e.get('code', '')}:{str(e.get('message', ''))[:160]}" for e in events)[:400]


# ---- data


class Data:
    def __init__(self, root: Path, packs: Path):
        self.root = root
        self.matrix = json.loads((root / "matrix.json").read_text())
        fl = root / "data/fleurs/index.json"
        index = json.loads(fl.read_text()) if fl.exists() else {}
        self.fleurs = {k: v for k, v in index.items() if isinstance(v, dict) and "wav" in v}
        rq = root / "data/requests/index.json"
        self.requests = json.loads(rq.read_text()) if rq.exists() else {}
        self.packs = packs

    def wav(self, rel: str) -> Path:
        p = Path(rel)
        return p if p.is_absolute() else self.root / p

    def bank(self, language: str) -> dict:
        path = EN_ASSETS / "phrase_banks.en.json" if language == "en" else self.packs / f"phrase_banks.{language}.json"
        return json.loads(path.read_text()) if path.exists() else {}

    def request(self, language: str, name: str) -> Optional[Path]:
        entry = self.requests.get(language, {}).get("items", {}).get(name)
        return self.wav(entry["wav"]) if entry else None

    def request_text(self, language: str, name: str) -> str:
        return self.requests.get(language, {}).get("items", {}).get(name, {}).get("text", "")


# ---- part A: voices


def spelled_out(text: str, said: str) -> bool:
    words = norm_text(text).split()
    letters = [w for w in words if len(w) == 1 and w.isalpha()]
    names = {"em", "aitch", "ay", "bee", "cee", "dee", "ee", "eff", "gee", "jay", "kay", "el", "en", "oh",
             "pee", "cue", "ar", "ess", "tee", "you", "vee", "ex", "why", "zed", "zee"}
    short = len(norm_text(said).replace(" ", "")) <= 4
    return len(letters) >= 2 or (short and len(words) >= 3 and sum(w in names for w in words) >= 2)


# Part A's sentences: 0-2 for the /ws routes, 3-5 for POST /speak (the gateway caches a
# synthesis by text, voice and features, so a route never repeats another route's text).
# Numbers are written as words; the round trip runs without smart_format.
SENTENCES = {
    "en": ["The weather in the mountains will be cold and windy tomorrow morning.",
           "Please remember to bring your passport and your boarding pass.",
           "Our office opens at nine and closes at half past five.",
           "The train to the city leaves from platform four every hour.",
           "She bought fresh bread, cheese and apples at the market.",
           "We will call you back as soon as the order is ready."],
    "de": ["Das Wetter in den Bergen wird morgen früh kalt und windig.",
           "Bitte denken Sie an Ihren Reisepass und Ihre Bordkarte.",
           "Unser Büro öffnet um neun Uhr und schließt um halb sechs.",
           "Der Zug in die Stadt fährt jede Stunde von Gleis vier ab.",
           "Sie kaufte frisches Brot, Käse und Äpfel auf dem Markt.",
           "Wir rufen Sie zurück, sobald die Bestellung fertig ist."],
    "es": ["El tiempo en las montañas será frío y con viento mañana por la mañana.",
           "Por favor, recuerde traer su pasaporte y su tarjeta de embarque.",
           "Nuestra oficina abre a las nueve y cierra a las cinco y media.",
           "El tren a la ciudad sale del andén cuatro cada hora.",
           "Ella compró pan fresco, queso y manzanas en el mercado.",
           "Le llamaremos en cuanto el pedido esté listo."],
    "fr": ["Demain matin, il fera froid et venteux dans les montagnes.",
           "N'oubliez pas d'apporter votre passeport et votre carte d'embarquement.",
           "Notre bureau ouvre à neuf heures et ferme à dix-sept heures trente.",
           "Le train pour la ville part du quai quatre toutes les heures.",
           "Elle a acheté du pain frais, du fromage et des pommes au marché.",
           "Nous vous rappellerons dès que la commande sera prête."],
    "it": ["Domani mattina in montagna farà freddo e ci sarà vento.",
           "Si ricordi di portare il passaporto e la carta d'imbarco.",
           "Il nostro ufficio apre alle nove e chiude alle cinque e mezza.",
           "Il treno per la città parte dal binario quattro ogni ora.",
           "Ha comprato pane fresco, formaggio e mele al mercato.",
           "La richiameremo appena l'ordine sarà pronto."],
    "nl": ["Morgenochtend wordt het koud en winderig in de bergen.",
           "Vergeet niet uw paspoort en uw instapkaart mee te nemen.",
           "Ons kantoor gaat om negen uur open en sluit om half zes.",
           "De trein naar de stad vertrekt elk uur vanaf spoor vier.",
           "Ze kocht vers brood, kaas en appels op de markt.",
           "We bellen u terug zodra de bestelling klaar is."],
    "ja": ["明日の朝、山の天気は寒くて風が強いでしょう。",
           "パスポートと搭乗券を忘れずにお持ちください。",
           "事務所は九時に開いて、五時半に閉まります。",
           "町へ行く電車は毎時四番線から出発します。",
           "彼女は市場で新鮮なパンとチーズとりんごを買いました。",
           "ご注文の準備ができ次第、折り返しお電話いたします。"],
}
WORDS_OK = 0.35  # a sentence's round trip: at most this error rate


def roundtrip(pcm: bytes, rate: int, text: str, language: str) -> dict:
    status, heard, conf = dg_transcribe(pcm, rate, "nova-3", language, smart=False)
    if status != 200:
        return {"status": status, "text": heard}
    kind, rate_ = error_rate(text, heard, language)
    return {"status": status, "text": heard, "metric": kind, "error_rate": rate_,
            "words_ok": rate_ is not None and rate_ <= WORDS_OK}


async def speak_sentences(args, s: WsSession, language: str, texts: List[str], route: str) -> List[dict]:
    clips = []
    for text in texts:
        mark = len(s.events)
        sent = now()
        await s.send({"type": "speak", "text": text, "flush": True})
        done = await s.wait(lambda e: e.get("type") in ("tts_playback_complete", "error"), 30, mark)
        done_at = done[0] if done else None
        await asyncio.sleep(0.15)
        pcm = s.pcm(sent, now())
        got = [a for a, _ in s.audio if a >= sent]
        clip: Dict[str, Any] = {"route": route, "kind": "sentence", "text": text, "load1": load1(),
                                "first_audio_ms": ms(got[0] - sent) if got else None,
                                "last_audio_ms": ms(got[-1] - sent) if got else None,
                                "complete_ms": ms(done_at - sent) if done_at else None,
                                "audio_ms": ms(len(pcm) / 2 / RATE)}
        if done and done[1].get("type") == "error":
            clip["error"] = error_text([done[1]])
        a, b = voiced_span(pcm, RATE)
        clip["trimmed_ms"] = ms(b - a)
        clip["silent"] = clip["trimmed_ms"] == 0
        if pcm and dg_key() and not clip["silent"]:
            clip["roundtrip"] = await asyncio.to_thread(roundtrip, pcm, RATE, text, language)
        clips.append(clip)
    return clips


async def voice_case(args, budget: Budget, data: Data, voice: str, info: dict) -> dict:
    out = await voice_sounds_case(args, budget, data, voice, info)
    language = info.get("base_language", "en")
    texts = SENTENCES.get(language, SENTENCES["en"])
    reps = max(1, args.voice_repeats)
    if not args.voice_sentences or out.get("skipped"):
        return out
    chars = sum(len(t) for t in texts[:reps])
    est = 3 * budget.tts(chars, voice) + budget.stt("nova-3", 3 * reps * 5, False) + budget.stt("nova-3", 20 * reps)
    if not budget.allow("A", est):
        out["sentences_skipped"] = budget.stopped
        return out
    sentences: List[dict] = []
    # /ws over Deepgram's REST /v1/speak (the default) and over its /v1/speak WebSocket
    for route, features in (("ws_rest", None), ("ws_streaming", {"streaming": True})):
        s = WsSession(ws_url(args.base_url, "/ws"), f"A-{route}-{voice}")
        try:
            await s.open({"type": "config", "stt_config": stt_cfg("nova-3", language),
                          "tts_config": tts_cfg(voice, RATE, features)})
            if not s.ready:
                sentences.append({"route": route, "error": error_text(s.errors()) or "no ready"})
            else:
                sentences += await speak_sentences(args, s, language, texts[:reps], route)
        finally:
            await s.close()
            budget.charge("A", budget.tts(chars, voice) + budget.stt("nova-3", max(0.0, now() - s.opened) if s.opened else 0)
                          + budget.stt("nova-3", reps * 5, False), f"{route} {voice}")
    # POST /speak: time to the first byte and to the last
    for text in texts[3:3 + reps]:
        r = await asyncio.to_thread(http_timed, args.base_url.rstrip("/") + "/speak",
                                    {"text": text, "tts_config": tts_cfg(voice)}, None, 60)
        clip = {"route": "rest_speak", "kind": "sentence", "text": text, "status": r["status"], "load1": load1(),
                "first_audio_ms": r["ttfb_ms"], "complete_ms": r["total_ms"], "content_type": r["content_type"]}
        if r["status"] == 200:
            pcm = pcm_of(r["body"])
            clip["audio_ms"] = ms(len(pcm) / 2 / RATE)
            a, b = voiced_span(pcm, RATE)
            clip["trimmed_ms"] = ms(b - a)
            clip["silent"] = clip["trimmed_ms"] == 0
            if dg_key() and not clip["silent"]:
                clip["roundtrip"] = await asyncio.to_thread(roundtrip, pcm, RATE, text, language)
        else:
            clip["error"] = r["body"][:200].decode("utf-8", "replace")
        budget.charge("A", budget.tts(len(text), voice) + budget.stt("nova-3", 5, False), f"rest {voice}")
        sentences.append(clip)
    out["sentences"] = sentences
    out["listening_pass"] = out.get("pass")
    out["sentences_pass"] = bool(sentences) and all(
        not c.get("error") and not c.get("silent") and (c.get("roundtrip") or {}).get("words_ok") for c in sentences)
    # The voice works when every sentence on every route is audible and heard as said; the
    # listening sounds' length is the pack's matter (listening_pass).
    out["pass"] = out["sentences_pass"]
    return out


async def voice_sounds_case(args, budget: Budget, data: Data, voice: str, info: dict) -> dict:
    language = info.get("base_language", "en")
    bank = data.bank(language).get("groups", {})
    sounds = [l["text"] for l in bank.get("backchannel", {}).get("lines", [])]
    maxc = data.bank(language).get("rules", {}).get("ack_clip_max_chars", 24)
    acks = [l["text"] for l in bank.get("ack", {}).get("lines", []) if len(l["text"]) <= maxc]
    lines = [("listening", t) for t in sounds] + [("ack", t) for t in acks[:1]]
    out: Dict[str, Any] = {"voice": voice, "language": language, "architecture": info.get("architecture"),
                           "pack": bool(bank), "clips": []}
    if not lines:
        out["error"] = f"no pack for {language}"
        return out
    chars = sum(len(t) for _, t in lines)
    session_s = 4 + 2.5 * len(lines)
    est = budget.tts(chars, voice) + budget.stt("nova-3", session_s) + budget.stt("nova-3", 2 * len(lines), False)
    if not budget.allow("A", est):
        out["skipped"] = budget.stopped
        return out
    s = WsSession(ws_url(args.base_url, "/ws"), f"A-{voice}")
    started = now()
    try:
        await s.open({"type": "config", "stt_config": stt_cfg("nova-3", language), "tts_config": tts_cfg(voice)})
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no ready"
            return out
        for kind, text in lines:
            mark = len(s.events)
            sent = now()
            await s.send({"type": "speak", "text": text, "flush": True})
            done = await s.wait(lambda e: e.get("type") in ("tts_playback_complete", "error"), 25, mark)
            await asyncio.sleep(0.15)
            pcm = s.pcm(sent, now())
            clip: Dict[str, Any] = {"kind": kind, "text": text, "raw_ms": ms(len(pcm) / 2 / RATE)}
            if done and done[1].get("type") == "error":
                clip["error"] = error_text([done[1]])
            first = next((a for a, d in s.audio if a >= sent), None)
            clip["first_audio_ms"] = ms(first - sent) if first else None
            a, b = voiced_span(pcm, RATE)
            clip["trimmed_ms"] = ms(b - a)
            clip["peak_dbfs"] = round(max((dbfs(pcm[i:i + 320]) for i in range(0, len(pcm), 320)), default=-100), 1)
            clip["silent"] = clip["trimmed_ms"] == 0
            if kind == "listening":
                clip["length_ok"] = 0 < clip["trimmed_ms"] <= 600
            if pcm and dg_key() and not clip["silent"]:
                status, heard, conf = dg_transcribe(pcm, RATE, "nova-3", language)
                clip["roundtrip"] = {"status": status, "text": heard, "confidence": conf}
                clip["spelled_out"] = spelled_out(heard, text) if status == 200 else None
            if args.keep_audio and pcm:
                d = Path(args.results) / "audio/A"
                d.mkdir(parents=True, exist_ok=True)
                (d / f"{voice}-{len(out['clips'])}.wav").write_bytes(wav_bytes(pcm, RATE))
            out["clips"].append(clip)
        out["warnings"] = [w.get("code") for w in s.warnings()]
    finally:
        await s.close()
        wall = now() - started
        budget.charge("A", budget.tts(chars, voice) + budget.stt("nova-3", wall)
                      + budget.stt("nova-3", sum(c.get("raw_ms") or 0 for c in out["clips"]) / 1000, False), voice)
    listening = [c for c in out["clips"] if c["kind"] == "listening"]
    out["pass"] = bool(out["clips"]) and all(not c.get("silent") and not c.get("error") for c in out["clips"]) \
        and all(c.get("length_ok") for c in listening) and not any(c.get("spelled_out") for c in out["clips"])
    return out


# ---- part B: speech-to-text


def model_language(row: dict, base: str) -> str:
    langs = row.get("languages", [])
    if base in langs:
        return base
    return next((l for l in langs if l.split("-")[0].lower() == base), base)


async def stt_case(args, budget: Budget, data: Data, model: str, language: str, clip: dict,
                   interaction: bool = False, clip_language: Optional[str] = None) -> dict:
    """`clip_language`: the clip's language when the session's is not it (``multi``)."""
    pcm, rate = load_wav(data.wav(clip["wav"]))
    pcm = resample(pcm, rate, RATE)
    dur = len(pcm) / 2 / RATE
    out: Dict[str, Any] = {"model": model, "language": language, "clip_s": round(dur, 2),
                           "reference": clip.get("reference", ""), "load1": load1()}
    if clip_language:
        out["clip_language"] = clip_language
    est = budget.stt(model, dur + 3.5)
    if not budget.allow("B", est):
        out["skipped"] = budget.stopped
        return out
    config = {"type": "config", "stt_config": stt_cfg(model, language), "tts_config": tts_cfg("aura-2-thalia-en")}
    if interaction:
        config["interaction"] = True
        config["conversation_config"] = {"base_url": args.llm_url, "model": args.llm_model, "api_key": "local-dummy"}
    s = WsSession(ws_url(args.base_url, "/ws"), f"B-{model}-{language}")
    try:
        await s.open(config)
        out["ready"] = bool(s.ready)
        out["warnings"] = [{"code": w.get("code"), **({"reason": w["detail"].get("reason")} if isinstance(w.get("detail"), dict) else {})}
                           for w in s.warnings()]
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no ready"
            return out
        await s.silence(0.5, RATE)
        t0, t1 = await s.stream(pcm, RATE)
        speech_end = t0 + voiced_span(pcm, RATE)[1]
        await s.silence(2.0, RATE)
        # wait until no new result for 2.5 s (max 12 s)
        deadline, last = now() + 12, len(s.of("stt_result"))
        quiet_since = now()
        while now() < deadline:
            await asyncio.sleep(0.25)
            n = len(s.of("stt_result"))
            if n != last:
                last, quiet_since = n, now()
            elif now() - quiet_since > 2.5:
                break
        results = [(a, e) for a, e in s.of("stt_result") if e.get("transcript", "").strip()]
        finals = [(a, e) for a, e in results if e.get("is_final")]
        out["first_interim_ms"] = ms(results[0][0] - t0) if results else None
        out["final_latency_ms"] = ms(finals[-1][0] - speech_end) if finals else None
        hyp = " ".join(e["transcript"].strip() for _, e in finals)
        out["hypothesis"] = hyp
        out["metric"], out["error_rate"] = error_rate(out["reference"], hyp, clip_language or language)
        out["finals"] = len(finals)
        out["speech_finals"] = sum(1 for _, e in finals if e.get("is_speech_final"))
        out["first_final_ms"] = ms(finals[0][0] - t0) if finals else None
        confs = [e.get("confidence") for _, e in finals if e.get("confidence") is not None]
        out["confidence"] = {"min": min(confs) if confs else None, "max": max(confs) if confs else None,
                             "real": any(0.0 < c < 1.0 for c in confs)}
        named = [e.get("detected_language") for _, e in results if e.get("detected_language")]
        out["detected_language"] = named[-1] if named else None
        out["errors"] = error_text(s.errors())
    finally:
        await s.close()
        budget.charge("B", budget.stt(model, max(0.0, now() - s.opened) if s.opened else 0.0), f"{model}:{language}")
    return out


async def batch_case(args, budget: Budget, data: Data, model: str, language: str, clip: dict) -> dict:
    pcm, rate = load_wav(data.wav(clip["wav"]))
    dur = len(pcm) / 2 / rate
    out: Dict[str, Any] = {"model": model, "language": language, "route": "/transcribe/batch",
                           "reference": clip.get("reference", "")}
    if not budget.allow("B", budget.stt(model, dur, False)):
        out["skipped"] = budget.stopped
        return out
    # `api_key` is a required field of the flattened config; empty means the server key.
    body = {"provider": "deepgram", "api_key": "", "model": model, "language": language,
            "sample_rate": rate, "channels": 1, "punctuation": True, "encoding": "linear16",
            "audio_base64": base64.b64encode(wav_bytes(pcm, rate)).decode(), "content_type": "audio/wav"}
    t = now()
    status, raw, _ = await asyncio.to_thread(http, args.base_url.rstrip("/") + "/transcribe/batch", "POST", body, None, 300)
    out["status"], out["latency_ms"] = status, ms(now() - t)
    budget.charge("B", budget.stt(model, dur, False), f"batch {model}:{language}")
    try:
        job = json.loads(raw)
    except ValueError:
        out["error"] = raw[:200].decode("utf-8", "replace")
        return out
    for _ in range(30):  # a queued job is polled
        if job.get("status") in ("completed", "error", "failed") or status != 200:
            break
        await asyncio.sleep(1)
        status, raw, _ = await asyncio.to_thread(http, args.base_url.rstrip("/") + f"/transcribe/batch/{job.get('job_id')}")
        job = json.loads(raw) if raw[:1] == b"{" else {}
    text = ""
    result = job.get("result") or {}
    if isinstance(result, dict):
        # Deepgram's own prerecorded body is passed through.
        channel = ((result.get("results") or {}).get("channels") or [{}])[0]
        alt = (channel.get("alternatives") or [{}])[0]
        text = alt.get("transcript") or result.get("text") or result.get("transcript") or ""
        out["confidence"] = alt.get("confidence")
        out["detected_language"] = channel.get("detected_language")
    out["job_status"] = job.get("status")
    out["hypothesis"] = text
    out["metric"], out["error_rate"] = error_rate(out["reference"], text, language)
    if status != 200 or job.get("error"):
        out["error"] = str(job.get("error") or job)[:300]
    return out


# ---- part C: interaction smoke


async def smoke_case(args, budget: Budget, data: Data, model: str, language: str = "en",
                     stt_language: Optional[str] = None) -> dict:
    """`stt_language` (`multi`): the session's language when it is not the caller's."""
    wav = data.request(language, "q_capital")
    voice = PACK_VOICES.get(stt_language or language, "aura-2-thalia-en")
    out: Dict[str, Any] = {"model": model, "language": language, "voice": voice, "stt_language": stt_language}
    if not wav:
        out["error"] = f"no data/requests/{language}/q_capital.wav"
        return out
    pcm, rate = load_wav(wav)
    pcm = resample(pcm, rate, RATE)
    est = budget.stt(model, 50) + budget.tts(400)
    if not budget.allow("C", est):
        out["skipped"] = budget.stopped
        return out
    extras = json.loads(args.smoke_stt_extras) if args.smoke_stt_extras and model.startswith("flux") else None
    config = {"type": "config", "stt_config": stt_cfg(model, stt_language or language, extras),
              "tts_config": tts_cfg(voice, AGENT_RATE),
              "conversation_config": {"base_url": args.llm_url, "model": args.llm_model, "api_key": "local-dummy",
                                      "system_prompt": INSTRUCTIONS},
              "interaction": True}
    s = WsSession(ws_url(args.base_url, "/ws"), f"C-{model}")
    chars = 0
    try:
        await s.open(config)
        out["warnings"] = [w.get("code") for w in s.warnings()]
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no ready"
            return out
        out["interaction_on"] = bool((s.ready or {}).get("interaction"))
        out["load1"] = load1()
        await s.silence(2.0, RATE)

        async def keep_mic():
            while True:
                await s.silence(1.0, RATE)
        turns: List[dict] = []
        names = [t for t in (args.smoke_turns or "q_capital").split(",") if t]
        for n, name in enumerate(names):
            twav = data.request(language, name)
            if not twav:
                turns.append({"request": name, "error": f"no request clip {language}/{name}"})
                continue
            tpcm = pcm if name == "q_capital" else resample(*load_wav(twav), RATE)
            mark = len(s.events)
            t0, _ = await s.stream(tpcm, RATE)
            end = t0 + voiced_span(tpcm, RATE)[1]
            mic = asyncio.create_task(keep_mic())
            result = await s.wait(lambda e: e.get("type") == "task_result", args.smoke_timeout, mark)
            if result:
                await asyncio.sleep(4.0)  # let the relay be spoken
            # quiet: no agent audio for 1.5 s (at most 15 s more)
            deadline = now() + 15
            while now() < deadline and s.audio and now() - s.audio[-1][0] < 1.5:
                await asyncio.sleep(0.2)
            mic.cancel()
            await asyncio.sleep(0.05)
            ev = s.events[mark:]
            created = [(a, e) for a, e in ev if e.get("type") == "task_created"]
            lines = [(a, e) for a, e in ev if e.get("type") == "assistant_transcript" and (e.get("delta") or e.get("text"))]
            chars += sum(len(e.get("delta") or e.get("text") or "") for _, e in lines)
            turn: Dict[str, Any] = {"request": name, "text": data.request_text(language, name)}
            turn["task_created_ms"] = ms(created[0][0] - end) if created else None
            turn["task_result_ms"] = ms(result[0] - end) if result else None
            turn["task_result"] = (result[1].get("summary") or result[1].get("text") or result[1].get("state")) if result else None
            turn["lines"] = [{"t_ms": ms(a - end), "source": e.get("source"), "kind": e.get("kind"),
                              "text": (e.get("delta") or e.get("text") or "")[:120]} for a, e in lines]
            ack = [l for l in turn["lines"] if l["source"] in ("front", "gateway") and (l["t_ms"] or 0) >= 0]
            relay = [l for l in turn["lines"] if l["source"] == "background" or l["kind"] == "relay"]
            spoken = [l for l in relay if result and l["t_ms"] is not None and l["t_ms"] >= ms(result[0] - end) - 50]
            turn["result_spoken"] = bool(spoken) or (bool(result) and any(
                l["source"] == "gateway" and l["t_ms"] is not None and l["t_ms"] >= ms(result[0] - end) - 50
                for l in turn["lines"]))
            first_audio = next((a for a, d in s.audio if a > end and dbfs(d) > -50), None)
            turn["first_audio_ms"] = ms(first_audio - end) if first_audio else None
            segs = [(a, e) for a, e in ev if e.get("type") == "audio_segment" and e.get("phase") == "start"]
            turn["listening_sounds"] = sum(1 for _, e in segs if e.get("lane") == "backchannel")
            turn["segment_lanes"] = sorted({str(e.get("lane")) for _, e in segs})
            turn["caller_text"] = " ".join(e.get("transcript", "") for _, e in ev
                                           if e.get("type") == "stt_result" and e.get("is_final"))
            turn["pass"] = bool(created) and bool(result) and bool(ack or relay)
            turns.append(turn)
            await asyncio.sleep(0.5)
        out["turns"] = turns
        first = turns[0] if turns else {}
        for k in ("task_created_ms", "task_result_ms", "task_result", "lines", "first_audio_ms", "caller_text"):
            out[k] = first.get(k)
        out["errors"] = error_text(s.errors())
        stepped = any(w == "interaction_language_step_down" for w in out["warnings"]) or any(
            e.get("code") == "interaction_language_step_down" for _, e in s.events)
        out["stepped_down"] = stepped
        out["detected_language"] = [e.get("detected_language") for _, e in s.of("stt_result")
                                    if e.get("is_final") and e.get("detected_language")]
        out["pass"] = bool(turns) and all(t.get("pass") for t in turns) and out["interaction_on"] and not stepped
    finally:
        await s.close()
        budget.charge("C", budget.stt(model, max(0.0, now() - s.opened) if s.opened else 0) + budget.tts(chars), model)
    return out


# ---- part L: listening sounds


def log_lines(path: str, needle: str, stream_id: str) -> List[str]:
    try:
        text = Path(path).read_text(errors="replace")
    except OSError:
        return []
    return [line for line in text.splitlines() if needle in line and stream_id in line]


# Part L's caller: one sentence that goes on at its commas, and the Aura-2 voice that says it.
LONG_SENTENCE = {
    "en": ("aura-2-orion-en", "So I am planning a trip with my family next spring, and we want somewhere warm "
                              "by the sea, where the kids can swim and play."),
    "es": ("aura-2-agustina-es", "Estoy planeando un viaje con mi familia y... buscamos un lugar cálido "
                                 "cerca del mar porque... los niños quieren nadar y... bueno, también un "
                                 "hotel cerca de la playa."),
    "de": ("aura-2-julius-de", "Ich plane eine Reise mit meiner Familie und... wir suchen einen warmen Ort am Meer, "
                               "weil... die Kinder schwimmen wollen und... ja, auch ein Hotel nahe am Strand."),
    "fr": ("aura-2-hector-fr", "Je prépare un voyage avec ma famille et... nous cherchons un endroit chaud au bord "
                               "de la mer, parce que... les enfants veulent nager et... bon, aussi un hôtel près de la plage."),
    "it": ("aura-2-livia-it", "Sto organizzando un viaggio con la mia famiglia e... cerchiamo un posto caldo vicino "
                              "al mare, perché... i bambini vogliono nuotare e... insomma, anche un albergo vicino alla spiaggia."),
    "nl": ("aura-2-lars-nl", "Ik plan een reis met mijn familie en... we zoeken een warme plek aan zee, omdat... "
                             "de kinderen willen zwemmen en... nou ja, ook een hotel dicht bij het strand."),
    "ja": ("aura-2-fujin-ja", "家族と旅行を計画していて、 ... 海の近くの暖かい場所を探しているんですが、 ... "
                              "子どもたちが泳ぎたがっていて、 ... それから、ビーチに近いホテルもいいですね。"),
}


def long_sentence(budget: Budget, language: str, pause_s: float = 1.2) -> Optional[bytes]:
    """LONG_SENTENCE[language] at RATE, each inner pause (120 ms or more under -45 dBFS)
    stretched to `pause_s`: Smart Turn answers 260 to 390 ms after the caller stops on a debug
    build, after a short pause has ended."""
    voice, text = LONG_SENTENCE[language]
    status, pcm, _ = http(f"{DG_API}/v1/speak?model={voice}&encoding=linear16&sample_rate={RATE}&container=none",
                          "POST", json.dumps({"text": text}).encode(), dg_headers(), ctype="application/json")
    budget.charge("L", budget.tts(len(text), voice), f"caller {voice}")
    if status != 200:
        return None
    frame = RATE // 50 * 2
    quiet = [dbfs(pcm[i:i + frame]) <= -45 for i in range(0, len(pcm) - frame + 1, frame)]
    out, cursor, n = bytearray(), 0, 0
    while n < len(quiet):
        if not quiet[n]:
            n += 1
            continue
        run = n
        while run < len(quiet) and quiet[run]:
            run += 1
        inner = n > 0 and run < len(quiet) and (run - n) * 0.02 >= 0.12
        if inner:
            middle = (n + run) // 2 * frame
            out += pcm[cursor:middle] + bytes(int(RATE * max(0.0, pause_s - (run - n) * 0.02)) * 2)
            cursor = middle
        n = run
    return bytes(out + pcm[cursor:])


async def listening_case(args, budget: Budget, data: Data, voice: str, language: str) -> dict:
    out: Dict[str, Any] = {"voice": voice, "language": language}
    if language not in LONG_SENTENCE or not dg_key():
        out["error"] = f"no long sentence for {language}, or no DEEPGRAM_API_KEY to say it"
        return out
    pcm = long_sentence(budget, language)
    if pcm is None:
        out["error"] = "the caller's sentence could not be synthesised"
        return out
    est = budget.stt("nova-3", 40) + budget.tts(120, voice)
    if not budget.allow("L", est):
        out["skipped"] = budget.stopped
        return out
    stt = {**stt_cfg("nova-3", language), "features": {"endpointing_ms": 1400}}
    config = {"type": "config", "stt_config": stt, "tts_config": tts_cfg(voice, AGENT_RATE),
              "conversation_config": {"base_url": args.llm_url, "model": args.llm_model, "api_key": "local-dummy",
                                      "system_prompt": INSTRUCTIONS},
              "interaction": {"front": {"profile": "model"}}}
    s = WsSession(ws_url(args.base_url, "/ws"), f"L-{voice}")
    chars = 0
    try:
        await s.open(config)
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no ready"
            return out
        sid = s.ready.get("stream_id", "?")
        await s.silence(1.0, RATE)
        deadline = now() + 20
        while args.gateway_log and now() < deadline and not log_lines(args.gateway_log, "listening sounds ready", sid):
            await s.silence(0.5, RATE)
        ready = log_lines(args.gateway_log, "listening sounds ready", sid) if args.gateway_log else []
        found = re.search(r"clips=(\d+)", ready[0]) if ready else None
        out["bank_clips"] = int(found.group(1)) if found else None
        out["refused"] = [re.sub(r".*text=", "", line)[:120] for line in
                          (log_lines(args.gateway_log, "listening sound refused", sid) if args.gateway_log else [])]
        t0, _ = await s.stream(pcm, RATE)
        await s.silence(3.0, RATE)
        end = t0 + voiced_span(pcm, RATE)[1]
        starts = [(a, e) for a, e in s.of("audio_segment") if e.get("lane") == "backchannel" and e.get("phase") == "start"
                  and t0 <= a <= end + 0.5]
        out["starts_s"] = [round(a - t0, 2) for a, _ in starts]
        if starts:
            first = starts[0][1].get("utterance_id")
            ends = [a for a, e in s.of("audio_segment") if e.get("utterance_id") == first and e.get("phase") == "end"]
            played = s.pcm(starts[0][0] - 0.05, ends[0] if ends else starts[0][0] + 0.8)
            a, b = voiced_span(played, AGENT_RATE)
            out["played_ms"] = ms(b - a)
            out["played_dbfs"] = round(dbfs(played), 1)
            if dg_key() and played:
                status, heard, _ = dg_transcribe(played, AGENT_RATE, "nova-3", language)
                out["heard"] = heard if status == 200 else f"HTTP {status}"
                out["spelled_out"] = spelled_out(heard, "") if status == 200 else None
        lines = [e for _, e in s.of("assistant_transcript")]
        chars = sum(len(e.get("delta") or "") for e in lines)
        out["errors"] = error_text(s.errors())
        out["pass"] = bool(starts) and (out["bank_clips"] is None or out["bank_clips"] >= 1) \
            and 0 < (out.get("played_ms") or 0) <= 620 and (out.get("played_dbfs") or -100) > -50 \
            and not out.get("spelled_out")
    finally:
        await s.close()
        budget.charge("L", budget.stt("nova-3", max(0.0, now() - s.opened) if s.opened else 0)
                      + budget.tts(chars + 40, voice), voice)
    return out


# ---- part S: stop cue (the live script's S1)


class Mic:
    """A caller microphone: one 20 ms frame every 20 ms, silence unless a clip is queued.
    `start_at` is when the queued clip's first frame went out."""

    def __init__(self, s: WsSession, rate: int = RATE):
        self.s, self.rate = s, rate
        self.queue = bytearray()
        self.start_at: Optional[float] = None
        self.end_at: Optional[float] = None
        self.task: Optional[asyncio.Task] = None

    def start(self) -> "Mic":
        self.task = asyncio.create_task(self._run())
        return self

    async def _run(self):
        size = self.rate * FRAME_MS // 1000 * 2
        start, sent = now(), 0
        while True:
            if self.queue:
                if self.start_at is None:
                    self.start_at = now()
                frame = bytes(self.queue[:size]).ljust(size, b"\0")
                del self.queue[:size]
                if not self.queue:
                    self.end_at = now()
            else:
                frame = bytes(size)
            try:
                await self.s.ws.send(frame)
            except Exception:
                return
            sent += 1
            delay = start + sent * FRAME_MS / 1000 - now()
            if delay > 0:
                await asyncio.sleep(delay)

    def say(self, pcm: bytes):
        self.start_at, self.end_at = None, None
        self.queue += pcm

    async def started(self, timeout: float = 2.0) -> Optional[float]:
        deadline = now() + timeout
        while self.start_at is None and now() < deadline:
            await asyncio.sleep(0.005)
        return self.start_at

    def stop(self):
        if self.task:
            self.task.cancel()


LONG_LINES = {
    "en": ("While I look that up, here is a reminder. Our kitchen is open every day from noon until "
           "ten in the evening, the garden terrace is open when the weather is dry, and tables are "
           "held for fifteen minutes after the booked time."),
    "de": ("Während ich das nachsehe, noch ein Hinweis. Unsere Küche ist jeden Tag von zwölf Uhr mittags bis "
           "zehn Uhr abends geöffnet, die Gartenterrasse ist bei trockenem Wetter geöffnet, und Tische werden "
           "fünfzehn Minuten nach der gebuchten Zeit freigehalten."),
    "es": ("Mientras lo compruebo, un recordatorio. Nuestra cocina abre todos los días desde el mediodía hasta "
           "las diez de la noche, la terraza del jardín abre cuando hace buen tiempo, y las mesas se guardan "
           "durante quince minutos después de la hora reservada."),
    "fr": ("Pendant que je vérifie, un petit rappel. Notre cuisine est ouverte tous les jours de midi à "
           "vingt-deux heures, la terrasse du jardin est ouverte quand il fait beau, et les tables sont gardées "
           "quinze minutes après l'heure réservée."),
    "it": ("Mentre controllo, un promemoria. La nostra cucina è aperta tutti i giorni da mezzogiorno alle dieci "
           "di sera, la terrazza in giardino è aperta quando il tempo è asciutto, e i tavoli vengono tenuti per "
           "quindici minuti dopo l'orario prenotato."),
    "nl": ("Terwijl ik dat opzoek, nog even een herinnering. Onze keuken is elke dag open van twaalf uur 's middags "
           "tot tien uur 's avonds, het tuinterras is open als het droog is, en tafels worden een kwartier na de "
           "gereserveerde tijd vastgehouden."),
    "ja": ("確認している間に、ひとつお知らせです。当店のキッチンは毎日正午から夜十時まで営業しており、天気の良い日は"
           "ガーデンテラスもご利用いただけます。ご予約のお席は予約時間から十五分間お取りしております。"),
}
CUE_WORDS = {"en": ("stop", "wait"), "de": ("hören", "hör", "auf", "reden", "stopp"),
             "es": ("deja", "hablar", "para"), "fr": ("arrêtez", "arrête", "parler"),
             "it": ("smetti", "parlare", "basta"), "nl": ("stop", "praten"), "ja": ("やめ", "話す")}
EN_CUES = ["44_stop_talking"] * 5 + ["41_wait_a_second", "47_stop_stop"]


async def stop_case(args, budget: Budget, data: Data, model: str, language: str, runs: int,
                    extras: Optional[dict] = None, cueset: str = "") -> dict:
    """`cueset` ``req``: the language's spoken ``stop`` request (Aura-2) also for English."""
    voice = PACK_VOICES.get(language, "aura-2-thalia-en")
    out: Dict[str, Any] = {"model": model, "language": language, "voice": voice, "runs": [], "load1": load1(),
                           "cueset": cueset or ("s1" if language == "en" else "req"), "extras": extras}
    if language == "en" and cueset != "req":
        cues = [(c, Path(args.caller_dir) / f"{c}.wav") for c in (EN_CUES * 3)[:runs]]
    else:
        cues = [("stop", data.request(language, "stop"))] * runs
    if any(p is None or not Path(p).exists() for _, p in cues):
        out["error"] = f"cue clips missing for {language}"
        return out
    line = LONG_LINES[language]
    est = budget.stt(model, 25 * runs + 10) + budget.tts(len(line) * runs, voice)
    if not budget.allow("S", est):
        out["skipped"] = budget.stopped
        return out
    config = {"type": "config", "stt_config": stt_cfg(model, language, extras),
              "tts_config": tts_cfg(voice, AGENT_RATE),
              "conversation_config": {"base_url": args.llm_url, "model": args.llm_model, "api_key": "local-dummy",
                                      "system_prompt": "You answer a caller's question in one short sentence."},
              "interaction": {"front": {"profile": "model"}}}
    s = WsSession(ws_url(args.base_url, "/ws"), f"S-{model}-{language}")
    mic: Optional[Mic] = None
    chars = 0
    words = CUE_WORDS[language]
    try:
        await s.open(config)
        out["warnings"] = [w.get("code") for w in s.warnings()]
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no ready"
            return out
        out["interaction_on"] = bool((s.ready or {}).get("interaction"))
        mic = Mic(s).start()
        await asyncio.sleep(2.0)
        for n, (cue, path) in enumerate(cues):
            pcm, rate = load_wav(Path(path))
            pcm = resample(pcm, rate, RATE)
            first_voiced = voiced_span(pcm, RATE)[0]
            mark = len(s.events)
            t_speak = now()
            await s.send({"type": "speak", "text": line})
            chars += len(line)
            run: Dict[str, Any] = {"n": n, "cue": cue, "load1": load1()}
            playing = None
            deadline = now() + 15
            while now() < deadline and playing is None:
                playing = next((a for a, d in s.audio if a > t_speak and dbfs(d) > -50), None)
                await asyncio.sleep(0.02)
            if playing is None:
                run["error"] = "the agent's line did not play: " + error_text(s.errors()[-2:])
                out["runs"].append(run)
                continue
            run["line_first_audio_ms"] = ms(playing - t_speak)
            await asyncio.sleep(1.5)
            mic.say(pcm)
            started = await mic.started()
            voiced = started + first_voiced
            stopped = await s.wait(lambda e: e.get("type") == "agent_audio_stopped"
                                   and e.get("reason") != "client_clear", 6, mark)
            run["reason"] = stopped[1].get("reason") if stopped else None
            run["cut_ms"] = ms(stopped[0] - voiced) if stopped else None
            # Transcripts that arrive after the cut count too: the cue words may come later.
            await asyncio.sleep(max(0.0, (mic.end_at or now()) + 1.0 - now()))
            cue_at = [(a, e) for a, e in s.events[mark:] if e.get("type") == "stt_result" and a >= started]
            heard_any = next(((a, e) for a, e in cue_at if e.get("transcript", "").strip()), None)
            heard_cue = next(((a, e) for a, e in cue_at
                              if any(w in norm_text(e.get("transcript", "")) for w in words)), None)
            run["first_interim_ms"] = ms(heard_any[0] - voiced) if heard_any else None
            run["first_interim_text"] = heard_any[1].get("transcript") if heard_any else None
            run["interims"] = [(ms(a - voiced), e.get("transcript"), bool(e.get("is_final"))) for a, e in cue_at
                               if e.get("transcript", "").strip()][:10]
            run["events_before_cut"] = [(ms(a - voiced), e.get("type"), e.get("kind") or e.get("phase") or e.get("code") or "")
                                        for a, e in s.events[mark:] if a >= started and (not stopped or a <= stopped[0])
                                        and e.get("type") not in ("assistant_transcript",)][:12]
            run["words_ms"] = ms(heard_cue[0] - voiced) if heard_cue else None
            run["words_text"] = heard_cue[1].get("transcript") if heard_cue else None
            run["words_final"] = heard_cue[1].get("is_final") if heard_cue else None
            run["after_words_ms"] = ms(stopped[0] - heard_cue[0]) if stopped and heard_cue else None
            run["cut_before_words"] = bool(stopped and heard_cue and stopped[0] < heard_cue[0] - 0.02)
            run["share_before_words"] = (round(run["words_ms"] / run["cut_ms"], 3)
                                         if run["cut_ms"] and run["cut_ms"] > 0 and run["words_ms"] is not None else None)
            finals = [e.get("transcript") for a, e in s.events[mark:] if e.get("type") == "stt_result"
                      and e.get("is_final") and a >= started]
            run["heard"] = " ".join(finals)
            if stopped:
                nxt = next((a for a, e in s.events[mark:] if e.get("type") == "assistant_transcript"
                            and e.get("delta") and a > stopped[0]), stopped[0] + 1.0)
                run["late_frames"] = sum(1 for a, d in s.audio if stopped[0] + 0.1 < a < min(nxt - 0.06, stopped[0] + 1.0)
                                         and dbfs(d) > -50)
                run["said_after"] = " ".join(e.get("delta", "") for a, e in s.events[mark:]
                                             if e.get("type") == "assistant_transcript" and a > stopped[0])[:160]
            else:
                await s.send({"type": "clear"})
            deadline = now() + 30  # quiet: no audible agent audio for 1.5 s
            while now() < deadline:
                last = next((a for a, d in reversed(s.audio) if dbfs(d) > -50), 0.0)
                if now() - last > 1.5:
                    break
                await asyncio.sleep(0.2)
            await asyncio.sleep(0.5)
            run["pass"] = run["reason"] == "barge_in" and run["cut_ms"] is not None and run["cut_ms"] <= args.stop_limit_ms \
                and not run.get("late_frames")
            out["runs"].append(run)
        out["errors"] = error_text(s.errors())
    finally:
        if mic:
            mic.stop()
        await s.close()
        budget.charge("S", budget.stt(model, max(0.0, now() - s.opened) if s.opened else 0) + budget.tts(chars, voice),
                      f"stop {model} {language}")
    cuts = [r["cut_ms"] for r in out["runs"] if r.get("cut_ms") is not None]
    out["cut_ms"] = cuts
    out["cut_p50_ms"], out["cut_max_ms"] = pctl(cuts, 50), max(cuts, default=None)
    shares = [r["share_before_words"] for r in out["runs"] if r.get("share_before_words") is not None]
    out["share_before_words_p50"] = pctl(shares, 50)
    out["pass"] = bool(out["runs"]) and all(r.get("pass") for r in out["runs"])
    return out


# ---- part D: Voice Agent


async def realtime_case(args, budget: Budget, data: Data, think: str, voice: str = "aura-2-thalia-en",
                        language: str = "en", tools: Optional[list] = None, interaction: Any = None,
                        request: str = "q_capital", label: str = "", listen: Optional[str] = None,
                        region: Optional[str] = None, then: Optional[str] = None) -> dict:
    """`then`: a second request in the same session (a tool-calling one with `tools`)."""
    out: Dict[str, Any] = {"route": "/realtime", "think": think, "voice": voice, "language": language,
                           "request": request, "label": label, "listen": listen, "region": region,
                           "load1": load1()}
    wav = data.request(language, request)
    if not wav:
        out["error"] = f"no request clip {language}/{request}"
        return out
    pcm, rate = load_wav(wav)
    pcm = resample(pcm, rate, AGENT_RATE)
    if not budget.allow("D", budget.agent(30)):
        out["skipped"] = budget.stopped
        return out
    config: Dict[str, Any] = {"type": "config", "provider": "deepgram", "model": think, "voice": voice,
                              "instructions": INSTRUCTIONS, "language": language}
    if listen:
        config["transcription_model"] = listen
    if region:
        config["region"] = region
    if tools:
        config["tools"] = tools
    if interaction is not None:
        config["interaction"] = interaction
    s = WsSession(ws_url(args.base_url, "/realtime"), f"D-{think}")
    t_connect = now()
    try:
        await s.open(config, timeout=15)
        out["connect_ok"] = bool(s.ready)
        out["connect_ms"] = ms(now() - t_connect)
        out["warnings"] = [{"code": w.get("code"), "detail": w.get("detail")} for w in s.warnings()]
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no session_created"
            return out
        await s.silence(1.5, AGENT_RATE, 40)
        t0, _ = await s.stream(pcm, AGENT_RATE, 40)
        end = t0 + voiced_span(pcm, AGENT_RATE)[1]

        async def keep_mic():
            while True:
                await s.silence(0.4, AGENT_RATE, 40)
        mic = asyncio.create_task(keep_mic())
        done = await s.wait(lambda e: e.get("type") in ("response_done", "function_call", "task_result")
                            or (e.get("type") == "error" and e.get("code") in ("connection_lost", "provider_error")), 25)
        relay: List[str] = []
        if interaction is not None:
            # A delegated task: its result, and the gateway line that says it (it waits for the
            # agent's own reply and its audio to end). The session is held until it went quiet.
            result = await s.wait(lambda e: e.get("type") == "task_result", args.smoke_timeout)
            if result:
                done = result
                summary = norm_text(str(result[1].get("summary") or ""))
                out["task_summary"] = result[1].get("summary")
                deadline, last, quiet = now() + 30, len(s.events), now()
                while now() < deadline and now() - quiet < 6:
                    await asyncio.sleep(0.25)
                    if len(s.events) != last:
                        last, quiet = len(s.events), now()
                after = [(a, e.get("text", "")) for a, e in s.of("transcript")
                         if e.get("role") == "assistant" and a > result[0]]
                out["after_result"] = [t for _, t in after]
                words = set(summary.split())
                said = [(a, t) for a, t in after if summary and (norm_text(t) in summary or summary in norm_text(t)
                                                                 or len(words & set(norm_text(t).split())) >= 4)]
                if said:  # the last: the agent may say the same words in its own reply first
                    out["relay_ms_after_result"] = ms(said[-1][0] - result[0])
                    relay.append(said[-1][1])
        if done and done[1].get("type") == "function_call":
            call = done[1]
            out["function_call"] = {"name": call.get("name"), "arguments": call.get("arguments")}
            await s.send({"type": "function_result", "call_id": call.get("call_id"),
                          "result": json.dumps({"forecast": "sunny, 21 C"})})
            done = await s.wait(lambda e: e.get("type") == "response_done", 20)
        await asyncio.sleep(1.0)
        first = next((a for a, d in s.audio if a > end and dbfs(d) > -50), None)
        out["first_audio_ms"] = ms(first - end) if first else None
        if then and data.request(language, then):
            # Turn 2, after the agent went quiet: the tool-calling request.
            deadline = now() + 20
            while now() < deadline and s.audio and now() - s.audio[-1][0] < 1.2:
                await asyncio.sleep(0.2)
            mic.cancel()
            mark = len(s.events)
            pcm2 = resample(*load_wav(data.request(language, then)), AGENT_RATE)
            t2, _ = await s.stream(pcm2, AGENT_RATE, 40)
            end2 = t2 + voiced_span(pcm2, AGENT_RATE)[1]
            mic = asyncio.create_task(keep_mic())
            got = await s.wait(lambda e: e.get("type") in ("function_call", "response_done")
                               or (e.get("type") == "error" and e.get("code") in ("connection_lost", "provider_error")), 30, mark)
            turn2: Dict[str, Any] = {"request": then, "first_event": got[1].get("type") if got else None}
            if got and got[1].get("type") == "function_call":
                call = got[1]
                turn2["function_call"] = {"name": call.get("name"), "arguments": call.get("arguments"),
                                          "ms_after_speech": ms(got[0] - end2)}
                t_res = now()
                await s.send({"type": "function_result", "call_id": call.get("call_id"),
                              "result": json.dumps({"available": True, "table": "window, 19:00", "party": 2})})
                fin = await s.wait(lambda e: e.get("type") == "response_done", 25, len(s.events))
                after = next((a for a, d in s.audio if a > t_res and dbfs(d) > -50), None)
                turn2["audio_after_result_ms"] = ms(after - t_res) if after else None
                turn2["response_done"] = bool(fin)
            await asyncio.sleep(1.5)
            turn2["answer_text"] = " | ".join(e.get("text", "") for a, e in s.events[mark:]
                                             if e.get("type") == "transcript" and e.get("role") == "assistant")
            turn2["user_text"] = " | ".join(e.get("text", "") for a, e in s.events[mark:]
                                           if e.get("type") == "transcript" and e.get("role") == "user")
            turn2["errors"] = error_text([e for a, e in s.events[mark:] if e.get("type") == "error"])
            turn2["pass"] = "function_call" in turn2 and bool(turn2.get("answer_text")) if tools else bool(turn2.get("answer_text"))
            out["turn2"] = turn2
        mic.cancel()
        out["user_text"] = " | ".join(e.get("text", "") for _, e in s.of("transcript") if e.get("role") == "user")
        out["answer_text"] = " | ".join(e.get("text", "") for _, e in s.of("transcript") if e.get("role") == "assistant")
        out["tasks"] = [e.get("type") for _, e in s.events if str(e.get("type", "")).startswith("task_")]
        out["unverified_claims"] = [e for _, e in s.of("unverified_claim")]
        out["errors"] = error_text(s.errors())
        out["audio_s"] = round(len(s.pcm()) / 2 / AGENT_RATE, 2)
        out["pass"] = bool(out["answer_text"]) and out["first_audio_ms"] is not None
        if tools:
            out["function_round_trip"] = "function_call" in out or bool((out.get("turn2") or {}).get("pass"))
            out["pass"] = out["pass"] and out["function_round_trip"]
        if interaction is not None:
            out["relay_text"] = relay[0] if relay else None
            out["pass"] = out["pass"] and "task_created" in out["tasks"] and "task_result" in out["tasks"] \
                and bool(relay)
    finally:
        await s.close()
        budget.charge("D", budget.agent(max(0.0, now() - t_connect)), f"rt {think} {voice}")
    return out


async def inject_case(args, budget: Budget, base_url: str, think: str, label: str, steps: List[tuple]) -> dict:
    """`text` messages through the gateway's /realtime (InjectAgentMessage, or InjectUserMessage
    with the gateway's `deepgram_user_text` fix); every event after each step is recorded."""
    out: Dict[str, Any] = {"case": label, "think": think, "base_url": base_url, "steps": [], "load1": load1()}
    if not budget.allow("D", budget.agent(40)):
        out["skipped"] = budget.stopped
        return out
    config = {"type": "config", "provider": "deepgram", "model": think, "voice": "aura-2-thalia-en",
              "instructions": "You are a helpful phone assistant.", "language": "en"}
    s = WsSession(ws_url(base_url, "/realtime"), f"D-inject-{think}")
    t_connect = now()
    mic = None
    try:
        await s.open(config, timeout=15)
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no session_created"
            return out

        paused = [False]

        async def keep_mic():
            while True:
                if paused[0]:
                    await asyncio.sleep(0.04)
                else:
                    await s.silence(0.4, AGENT_RATE, 40)
        mic = asyncio.create_task(keep_mic())
        await asyncio.sleep(1.0)

        def playback_end() -> float:
            """When a client playing the audio in real time would finish it."""
            end = 0.0
            for a, d in s.audio:
                end = max(end, a) + len(d) / 2 / AGENT_RATE
            return end
        for kind, arg in steps:
            mark, t = len(s.events), now()
            step: Dict[str, Any] = {"step": kind, "arg": arg if not isinstance(arg, Path) else arg.name}
            if kind == "text":
                await s.send({"type": "text", "text": arg})
            elif kind == "audio":
                paused[0] = True
                await asyncio.sleep(0.45)
                pcm = resample(*load_wav(arg), AGENT_RATE)
                await s.stream(pcm, AGENT_RATE, 40)
                paused[0] = False
            elif kind == "wait_assistant":
                got = await s.wait(lambda e: e.get("type") == "transcript" and e.get("role") == "assistant", arg, mark)
                step["found"] = bool(got)
            elif kind == "sleep":
                await asyncio.sleep(arg)
            elif kind == "wait_audio":
                deadline = now() + arg
                while now() < deadline and not any(a > t and dbfs(d) > -50 for a, d in s.audio):
                    await asyncio.sleep(0.02)
            elif kind == "wait_quiet":
                # quiet: nothing new for 3 s and the audio so far played out
                deadline = now() + arg
                await asyncio.sleep(1.0)
                while now() < deadline and ((s.audio and now() - s.audio[-1][0] < 3.0) or now() < playback_end() + 1.0):
                    await asyncio.sleep(0.2)
            step["ms"] = ms(now() - t)
            out["steps"].append(step)
        await asyncio.sleep(1.0)
        out["events"] = [{"t_ms": ms(a - t_connect), **{k: (v[:200] if isinstance(v, str) else v) for k, v in e.items()
                                                         if k not in ("audio", "delta")}}
                         for a, e in s.events if e.get("type") not in ("speech_event",)][:80]
        out["assistant"] = [e.get("text") for _, e in s.of("transcript") if e.get("role") == "assistant"]
        out["user"] = [e.get("text") for _, e in s.of("transcript") if e.get("role") == "user"]
        out["errors_and_warnings"] = [{"type": e.get("type"), "code": e.get("code"), "message": str(e.get("message", ""))[:200]}
                                      for _, e in s.events if e.get("type") in ("error", "warning")]
        out["audio_s"] = round(len(s.pcm()) / 2 / AGENT_RATE, 2)
    finally:
        if mic:
            mic.cancel()
        await s.close()
        budget.charge("D", budget.agent(max(0.0, now() - t_connect)), f"inject {label} {think}")
    return out


THINK_TYPES = {"open_ai": "open_ai", "anthropic": "anthropic", "google": "google", "groq": "groq",
               "nvidia": "nvidia"}
TABLE_FN = {"name": "check_table", "description": "Check whether the restaurant has a free table.",
            "parameters": {"type": "object", "properties": {"party": {"type": "integer"}, "time": {"type": "string"}},
                           "required": ["party", "time"]}}
WEATHER_FN = {"name": "get_weather", "description": "Get the current weather for a city.",
              "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}


async def agent_direct(args, budget: Budget, label: str, settings_agent: dict, steps: List[dict],
                       pcm24: Optional[bytes] = None, settle: float = 12.0) -> dict:
    """One direct Voice Agent connection; every text frame both ways is recorded."""
    out: Dict[str, Any] = {"label": label, "frames": []}
    if not budget.allow("D", budget.agent(25)):
        out["skipped"] = budget.stopped
        return out
    frames = out["frames"]
    t0 = now()
    audio_bytes = 0
    first_audio: Optional[float] = None
    audible: List[float] = []
    speech_end: List[Optional[float]] = [None]
    settings = {"type": "Settings",
                "audio": {"input": {"encoding": "linear16", "sample_rate": AGENT_RATE},
                          "output": {"encoding": "linear16", "sample_rate": AGENT_RATE, "container": "none"}},
                "agent": settings_agent}
    try:
        async with websockets.connect(DG_AGENT, additional_headers=dg_headers(), max_size=None,
                                      open_timeout=15) as ws:
            async def reader():
                nonlocal audio_bytes, first_audio
                async for m in ws:
                    if isinstance(m, bytes):
                        audio_bytes += len(m)
                        first_audio = first_audio or now()
                        if dbfs(m) > -50:
                            audible.append(now())
                    else:
                        frames.append({"dir": "recv", "t_ms": ms(now() - t0), "frame": json.loads(m)})
            rt = asyncio.create_task(reader())

            async def send(msg: dict):
                frames.append({"dir": "sent", "t_ms": ms(now() - t0), "frame": msg})
                await ws.send(json.dumps(msg))

            async def wait_for(kind: str, timeout: float) -> Optional[dict]:
                deadline = now() + timeout
                seen = len(frames)  # only frames that arrive from now on
                while now() < deadline:
                    for f in frames[seen:]:
                        if f["dir"] == "recv" and f["frame"].get("type") in (kind, "Error"):
                            return f["frame"]
                    await asyncio.sleep(0.02)
                return None

            async def keepalive():
                while True:
                    await asyncio.sleep(5)
                    await ws.send(json.dumps({"type": "KeepAlive"}))
            ka = asyncio.create_task(keepalive())
            await send(settings)
            applied = await wait_for("SettingsApplied", 10)
            out["settings_applied"] = bool(applied and applied.get("type") == "SettingsApplied")
            if out["settings_applied"]:
                for step in steps:
                    if "audio" in step and pcm24:
                        start = now()
                        speech_end[0] = start + voiced_span(pcm24, AGENT_RATE)[1]
                        for i in range(0, len(pcm24), 1920):
                            await ws.send(pcm24[i:i + 1920])
                            await asyncio.sleep(max(0, start + (i + 1920) / 2 / AGENT_RATE - now()))
                        frames.append({"dir": "sent", "t_ms": ms(now() - t0), "frame": f"<audio {len(pcm24)} bytes>"})
                        sil = bytes(1920)
                        for _ in range(50):
                            await ws.send(sil)
                            await asyncio.sleep(0.04)
                    elif "wait" in step:
                        await wait_for(step["wait"], step.get("timeout", settle))
                    elif "sleep" in step:
                        await asyncio.sleep(step["sleep"])
                    elif "fn_reply" in step:
                        req = next((f["frame"] for f in frames if f["dir"] == "recv"
                                    and f["frame"].get("type") == "FunctionCallRequest"), None)
                        if req:
                            fn = req["functions"][0]
                            await send({"type": "FunctionCallResponse", "id": fn["id"], "name": fn["name"],
                                        "content": json.dumps({"forecast": "sunny, 21 C"})})
                    else:
                        await send(step["send"])
            await asyncio.sleep(2.0)  # what arrives after the last step is recorded too
            ka.cancel()
            rt.cancel()
    except websockets.exceptions.InvalidStatus as e:
        out["error"] = f"HTTP {e.response.status_code}: {(e.response.body or b'')[:200].decode('utf-8', 'replace')}"
    except Exception as e:
        out["error"] = f"{type(e).__name__}: {e}"[:300]
    wall = now() - t0
    budget.charge("D", budget.agent(wall), f"direct {label}")
    out["wall_s"] = round(wall, 1)
    out["audio_s"] = round(audio_bytes / 2 / AGENT_RATE, 2)
    out["first_audio_ms_from_connect"] = ms(first_audio - t0) if first_audio else None
    if speech_end[0]:
        after = next((a for a in audible if a > speech_end[0]), None)
        out["first_audio_ms"] = ms(after - speech_end[0]) if after else None
    recv = [f["frame"] for f in frames if f["dir"] == "recv"]
    out["conversation"] = [(f.get("role"), f.get("content")) for f in recv if f.get("type") == "ConversationText"]
    out["errors"] = [f for f in recv if f.get("type") in ("Error", "Warning", "InjectionRefused")]
    # Big frames (Settings echo, History) are kept, but the audio is only counted.
    return out


def agent_block(think_type: str, think: str, listen: str, voice: str, language: str,
                functions: Optional[list] = None) -> dict:
    think_block: Dict[str, Any] = {"provider": {"type": think_type, "model": think}, "prompt": INSTRUCTIONS}
    if functions:
        think_block["functions"] = functions
    return {"language": language, "listen": {"provider": {"type": "deepgram", "model": listen}},
            "think": think_block, "speak": {"provider": {"type": "deepgram", "model": voice}}}


# ---- part E: endpoint types


async def tts_ws_case(args, budget: Budget, label: str, voice: str, features: Optional[dict],
                      extras: Optional[dict], text: str = "This is an endpoint check.") -> dict:
    out: Dict[str, Any] = {"case": label, "voice": voice}
    if not budget.allow("E", budget.tts(len(text)) + budget.stt("nova-3", 10)):
        out["skipped"] = budget.stopped
        return out
    s = WsSession(ws_url(args.base_url, "/ws"), label)
    try:
        await s.open({"type": "config", "stt_config": stt_cfg("nova-3", "en"),
                      "tts_config": tts_cfg(voice, RATE, features, extras)})
        if not s.ready:
            out["error"] = error_text(s.errors()) or "no ready"
            return out
        t = now()
        await s.send({"type": "speak", "text": text, "flush": True})
        done = await s.wait(lambda e: e.get("type") in ("tts_playback_complete", "error"), 25)
        pcm = s.pcm(t)
        out["first_audio_ms"] = ms(next((a for a, _ in s.audio if a >= t), t) - t) if s.audio else None
        out["audio_ms"] = ms(len(pcm) / 2 / RATE)
        out["errors"] = error_text(s.errors())
        out["pass"] = bool(pcm) and done is not None and done[1].get("type") == "tts_playback_complete"
    finally:
        await s.close()
        budget.charge("E", budget.tts(len(text)) + budget.stt("nova-3", max(0.0, now() - s.opened)), label)
    return out


DG_WS = "wss://api.deepgram.com"


async def raw_stt_ws(budget: Budget, model: str, language: str, pcm: bytes) -> dict:
    """Deepgram's own streaming endpoint with the gateway's query (its log), timed as stt_case."""
    if model.startswith("flux"):
        url = f"{DG_WS}/v2/listen?model={model}&encoding=linear16&sample_rate={RATE}"
    else:
        url = (f"{DG_WS}/v1/listen?model={model}&language={language}&sample_rate={RATE}&channels=1&punctuate=true"
               f"&interim_results=true&smart_format=true&encoding=linear16&endpointing=200")
    out: Dict[str, Any] = {"route": "raw", "model": model, "load1": load1()}
    events: List[Tuple[float, dict]] = []
    t_open = now()
    try:
        async with websockets.connect(url, additional_headers=dg_headers(), max_size=None, open_timeout=20) as ws:
            out["connect_ms"] = ms(now() - t_open)

            async def reader():
                async for m in ws:
                    if isinstance(m, str):
                        events.append((now(), json.loads(m)))
            rt = asyncio.create_task(reader())
            size = RATE * FRAME_MS // 1000 * 2

            async def send_pcm(p: bytes) -> float:
                start, sent = now(), 0
                for i in range(0, len(p), size):
                    await ws.send(p[i:i + size].ljust(size, b"\0"))
                    sent += 1
                    delay = start + sent * FRAME_MS / 1000 - now()
                    if delay > 0:
                        await asyncio.sleep(delay)
                return start
            await send_pcm(bytes(int(RATE * 0.5) * 2))
            t0 = await send_pcm(pcm)
            speech_end = t0 + voiced_span(pcm, RATE)[1]
            await send_pcm(bytes(RATE * 2 * 2))
            deadline, last, quiet = now() + 12, len(events), now()
            while now() < deadline:
                await asyncio.sleep(0.25)
                if len(events) != last:
                    last, quiet = len(events), now()
                elif now() - quiet > 2.5:
                    break
            rt.cancel()
    except Exception as e:
        out["error"] = f"{type(e).__name__}: {e}"[:200]
        return out
    finally:
        budget.charge("E", budget.stt(model, now() - t_open), f"raw ws {model}")
    if model.startswith("flux"):
        texts = [(a, e) for a, e in events if e.get("type") == "TurnInfo" and e.get("transcript", "").strip()]
        finals = [(a, e) for a, e in texts if e.get("event") == "EndOfTurn"]
        hyp = " ".join(e["transcript"] for _, e in finals)
    else:
        texts = [(a, e) for a, e in events if e.get("type") == "Results"
                 and e["channel"]["alternatives"][0].get("transcript", "").strip()]
        finals = [(a, e) for a, e in texts if e.get("is_final")]
        hyp = " ".join(e["channel"]["alternatives"][0]["transcript"] for _, e in finals)
    out["first_interim_ms"] = ms(texts[0][0] - t0) if texts else None
    out["final_latency_ms"] = ms(finals[-1][0] - speech_end) if finals else None
    out["hypothesis"] = hyp
    return out


async def raw_tts_ws(budget: Budget, voice: str, text: str) -> dict:
    out: Dict[str, Any] = {"route": "raw", "voice": voice, "load1": load1()}
    url = f"{DG_WS}/v1/speak?model={voice}&encoding=linear16&sample_rate={RATE}"
    try:
        async with websockets.connect(url, additional_headers=dg_headers(), max_size=None, open_timeout=20) as ws:
            t = now()
            await ws.send(json.dumps({"type": "Speak", "text": text}))
            await ws.send(json.dumps({"type": "Flush"}))
            first, n = None, 0
            while True:
                m = await asyncio.wait_for(ws.recv(), 15)
                if isinstance(m, bytes):
                    first = first or now()
                    n += len(m)
                elif json.loads(m).get("type") == "Flushed":
                    break
            out["first_audio_ms"], out["complete_ms"], out["audio_ms"] = ms(first - t) if first else None, ms(now() - t), \
                ms(n / 2 / RATE)
            await ws.send(json.dumps({"type": "Close"}))
    except Exception as e:
        out["error"] = f"{type(e).__name__}: {e}"[:200]
    budget.charge("E", budget.tts(len(text)), f"raw tts ws {voice}")
    return out


E_TEXTS = ["This is an endpoint check number {n}.", "The gateway forwards sentence {n} to Deepgram.",
           "Comparing the overhead, round {n}."]
NUM = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"]


async def part_e_overhead(args, budget: Budget, data: Data) -> List[dict]:
    """The same work through the gateway and against Deepgram directly, `--e-repeats` times."""
    rows: List[dict] = []
    base = args.base_url.rstrip("/")
    en = data.fleurs.get("en")
    pcm_en = resample(*load_wav(data.wav(en["wav"])), RATE) if en else None
    voice = "aura-2-thalia-en"
    tag = time.strftime("%H%M%S")
    for i in range(args.e_repeats):
        word = NUM[i % len(NUM)] + " " + tag[-2:]  # distinct text per run: no gateway cache hit
        for model in ("nova-3", "flux-general-en"):
            if pcm_en is None:
                break
            g = await stt_case(args, budget, data, model, "en", en)
            r = await raw_stt_ws(budget, model, "en", pcm_en)
            rows.append({"case": f"overhead_stt_ws_{model}", "i": i, "gateway": {k: g.get(k) for k in (
                "first_interim_ms", "final_latency_ms", "error_rate", "errors", "error", "load1")},
                "raw": r, "first_interim_delta_ms": (g.get("first_interim_ms") or 0) - (r.get("first_interim_ms") or 0)
                if g.get("first_interim_ms") is not None and r.get("first_interim_ms") is not None else None,
                "final_delta_ms": (g.get("final_latency_ms") or 0) - (r.get("final_latency_ms") or 0)
                if g.get("final_latency_ms") is not None and r.get("final_latency_ms") is not None else None})
        text = E_TEXTS[0].format(n=word)
        g = await asyncio.to_thread(http_timed, base + "/speak", {"text": text, "tts_config": tts_cfg(voice)}, None, 60)
        r = await asyncio.to_thread(http_timed, f"{DG_API}/v1/speak?model={voice}&encoding=linear16&container=none"
                                                f"&sample_rate={RATE}", {"text": text}, dg_headers(), 60)
        budget.charge("E", 2 * budget.tts(len(text)), "overhead rest tts")
        rows.append({"case": "overhead_tts_rest", "i": i, "text": text,
                     "gateway": {k: g[k] for k in ("status", "ttfb_ms", "total_ms", "content_type")} | {"bytes": len(g["body"])},
                     "raw": {k: r[k] for k in ("status", "ttfb_ms", "total_ms", "content_type")} | {"bytes": len(r["body"])},
                     "ttfb_delta_ms": (g["ttfb_ms"] - r["ttfb_ms"]) if g["ttfb_ms"] is not None and r["ttfb_ms"] is not None else None,
                     "total_delta_ms": g["total_ms"] - r["total_ms"], "load1": load1()})
        text = E_TEXTS[1].format(n=word)
        g = await tts_ws_case(args, budget, "tts_websocket_v1_speak", voice, {"streaming": True}, None, text)
        r = await raw_tts_ws(budget, voice, text)
        rows.append({"case": "overhead_tts_ws", "i": i, "text": text, "gateway": g, "raw": r,
                     "first_audio_delta_ms": (g.get("first_audio_ms") - r.get("first_audio_ms"))
                     if g.get("first_audio_ms") is not None and r.get("first_audio_ms") is not None else None})
        text = E_TEXTS[2].format(n=word)
        g = await tts_ws_case(args, budget, "tts_rest_via_ws_session", voice, None, None, text)
        rows.append({"case": "gateway_tts_ws_session_rest_upstream", "i": i, "text": text, "gateway": g})
        if en:
            g = await batch_case(args, budget, data, "nova-3", "en", en)
            wav = wav_bytes(*load_wav(data.wav(en["wav"])))
            r = await asyncio.to_thread(http_timed, f"{DG_API}/v1/listen?model=nova-3&language=en&smart_format=true",
                                        wav, dg_headers(), 120, "audio/wav")
            budget.charge("E", budget.stt("nova-3", en.get("duration_s", 12), False), "overhead raw prerecorded")
            rows.append({"case": "overhead_stt_prerecorded", "i": i,
                         "gateway": {k: g.get(k) for k in ("status", "latency_ms", "error_rate", "job_status", "error")},
                         "raw": {"status": r["status"], "latency_ms": r["total_ms"]},
                         "delta_ms": (g.get("latency_ms") or 0) - r["total_ms"] if g.get("latency_ms") else None,
                         "load1": load1()})
        if not args.no_agent_e and dg_key():
            g = await realtime_case(args, budget, data, "gpt-4o-mini")
            q = data.request("en", "q_capital")
            pcm24 = resample(*load_wav(q), AGENT_RATE)
            r = await agent_direct(args, budget, "overhead agent",
                                   agent_block("open_ai", "gpt-4o-mini", "nova-3", "aura-2-thalia-en", "en"),
                                   [{"sleep": 1.5}, {"audio": True}, {"wait": "AgentAudioDone", "timeout": 15}], pcm24)
            rows.append({"case": "overhead_voice_agent", "i": i,
                         "gateway": {k: g.get(k) for k in ("first_audio_ms", "connect_ms", "answer_text", "errors", "error", "load1")},
                         "raw": {k: r.get(k) for k in ("first_audio_ms", "conversation", "error", "wall_s")},
                         "first_audio_delta_ms": (g.get("first_audio_ms") - r.get("first_audio_ms"))
                         if g.get("first_audio_ms") is not None and r.get("first_audio_ms") is not None else None})
    # EU host directly, for the EU rows' comparison
    if pcm_en is not None:
        global DG_WS
        orig, DG_WS = DG_WS, "wss://api.eu.deepgram.com"
        try:
            r = await raw_stt_ws(budget, "nova-3", "en", pcm_en)
        finally:
            DG_WS = orig
        rows.append({"case": "raw_stt_eu_host", **r})
    # /v1/realtime: the OpenAI-shaped facade serves Bud-mode deployments (voice_table in Redis)
    try:
        async with websockets.connect(ws_url(base, "/v1/realtime?model=deepgram"), open_timeout=10) as ws:
            m = await asyncio.wait_for(ws.recv(), 5)
            rows.append({"case": "v1_realtime_facade_probe", "status": 101, "first_message": str(m)[:300]})
    except websockets.exceptions.InvalidStatus as e:
        rows.append({"case": "v1_realtime_facade_probe", "status": e.response.status_code,
                     "body": (e.response.body or b"")[:300].decode("utf-8", "replace")})
    except Exception as e:
        rows.append({"case": "v1_realtime_facade_probe", "error": f"{type(e).__name__}: {e}"[:300]})
    return rows


async def part_e(args, budget: Budget, data: Data) -> List[dict]:
    rows: List[dict] = []
    en = data.fleurs.get("en")
    base = args.base_url.rstrip("/")
    if en:
        r = await stt_case(args, budget, data, "flux-general-en", "en", en)
        rows.append({"case": "stt_flux_v2_listen", **r,
                     "pass": r.get("error_rate") is not None and not r.get("error") and not r.get("errors")})
        # EU host for speech-to-text, through the session's endpoint override
        s_eu = dict(en)
        orig = stt_cfg
        globals()["stt_cfg"] = lambda m, l, extras=None: orig(m, l, {"endpoint_override": "wss://api.eu.deepgram.com"})
        try:
            r = await stt_case(args, budget, data, "nova-3", "en", s_eu)
        finally:
            globals()["stt_cfg"] = orig
        rows.append({"case": "stt_eu_endpoint_override", **r})
        rows.append({"case": "stt_prerecorded_transcribe_batch", **await batch_case(args, budget, data, "nova-3", "en", en)})
    text = "This is an endpoint check."
    if budget.allow("E", budget.tts(len(text))):
        t = now()
        status, body, headers = await asyncio.to_thread(
            http, base + "/speak", "POST", {"text": text, "tts_config": tts_cfg("aura-2-thalia-en")})
        budget.charge("E", budget.tts(len(text)), "speak REST")
        rows.append({"case": "tts_rest_speak", "status": status, "latency_ms": ms(now() - t),
                     "content_type": headers.get("Content-Type") or headers.get("content-type"),
                     "bytes": len(body), "pass": status == 200 and len(body) > 4000,
                     "error": None if status == 200 else body[:200].decode("utf-8", "replace")})
    rows.append(await tts_ws_case(args, budget, "tts_rest_via_ws_session", "aura-2-thalia-en", None, None))
    rows.append(await tts_ws_case(args, budget, "tts_websocket_v1_speak", "aura-2-thalia-en", {"streaming": True}, None))
    rows.append(await tts_ws_case(args, budget, "tts_websocket_eu_override", "aura-2-thalia-en", {"streaming": True},
                                  {"endpoint_override": "wss://api.eu.deepgram.com"}))
    if not args.no_agent_e:
        r = await realtime_case(args, budget, data, "gpt-4o-mini", region="eu")
        rows.append({"case": "voice_agent_eu", **r})
        r = await realtime_case(args, budget, data, "gpt-4o-mini", listen="flux-general-en")
        rows.append({"case": "voice_agent_flux_listen", **r})
    if args.e_repeats and dg_key():
        rows += await part_e_overhead(args, budget, data)
    return rows


# ---- selection and estimates


def streaming_models(data: Data) -> Dict[str, dict]:
    return {m: r for m, r in data.matrix["stt"].items() if r.get("streaming")}


def file_only_models(data: Data) -> Dict[str, dict]:
    return {m: r for m, r in data.matrix["stt"].items() if not r.get("streaming")}


def pick(items: List, limit: int, explicit: Optional[str], key=lambda x: x) -> List:
    if explicit:
        wanted = [w.strip() for w in explicit.split(",") if w.strip()]
        return [i for i in items if key(i) in wanted]
    return items[:limit] if limit else items


def b_pairs(args, data: Data) -> List[Tuple[str, str, dict]]:
    pairs = []
    for model, row in sorted(streaming_models(data).items()):
        for base in row["base_languages"]:
            if base in data.fleurs and (not args.languages or base in args.languages.split(",")):
                pairs.append((model, model_language(row, base), data.fleurs[base]))
    return pick(pairs, args.limit_pairs, args.pairs, key=lambda p: f"{p[0]}:{p[1]}")


# The languages a capped prerecorded model runs (``--file-cap``), in this order.
FILE_SPREAD = ["en", "es", "fr", "de", "hi", "ja", "zh", "ar", "ru", "pt", "it", "nl", "ko", "tr", "sw", "ta",
               "vi", "pl", "uk", "he"]


def file_pairs(args, data: Data) -> List[Tuple[str, str, dict]]:
    pairs = []
    full = [m for m in args.file_full_models.split(",") if m]
    for model, row in sorted(file_only_models(data).items()):
        bases = [b for b in row["base_languages"] if b in data.fleurs
                 and (not args.languages or b in args.languages.split(","))]
        if args.file_cap and model not in full:
            bases = [b for b in FILE_SPREAD if b in bases][: args.file_cap]
        for base in bases:
            pairs.append((model, model_language(row, base), data.fleurs[base]))
    if args.file_pairs:  # ``model:language`` as the pair names it (failures run again alone)
        wanted = set(x for x in args.file_pairs.split(",") if x)
        pairs = [p for p in pairs if f"{p[0]}:{p[1]}" in wanted]
    limit = args.limit_file_pairs if args.limit_file_pairs else 0
    return pairs[:limit] if limit else pairs


def flux_pairs(args, data: Data) -> List[Tuple[str, str, str]]:
    """(model, session language, clip language) of ``--flux-pairs`` (``model:clip[:session]``)."""
    out = []
    for item in [x for x in args.flux_pairs.split(",") if x]:
        parts = item.split(":")
        model, clip = parts[0], parts[1]
        if clip in data.fleurs:
            out.append((model, parts[2] if len(parts) > 2 else clip, clip))
    return out


def english_models(args, data: Data) -> List[str]:
    if args.smoke_models:  # named models run as named (Flux is not in the /v1 catalogue)
        return [m for m in args.smoke_models.split(",") if m and m != "none"]
    models = [m for m, r in sorted(streaming_models(data).items()) if "en" in r["base_languages"]]
    return pick(models, args.limit_smoke, None)


def voices(args, data: Data) -> List[Tuple[str, dict]]:
    items = sorted(data.matrix["tts_voices"].items())
    return pick(items, args.limit_voices, args.voices, key=lambda v: v[0])


def think_models(args, data: Data) -> List[dict]:
    return pick(data.matrix.get("think_models", []), args.limit_think, args.think, key=lambda m: m["id"])


def spread_voices(data: Data) -> List[Tuple[str, str]]:
    """One aura-2 voice per language of the spoken requests."""
    out = []
    for lang in ("en", "de", "es", "fr", "it", "nl", "ja"):
        if lang not in data.requests:
            continue
        v = next((n for n, v in sorted(data.matrix["tts_voices"].items())
                  if v["architecture"] == "aura-2" and v["base_language"] == lang and v.get("usable", True)), None)
        if v:
            out.append((lang, v))
    return out


def estimate(args, budget: Budget, data: Data, parts: List[str]) -> Dict[str, dict]:
    est: Dict[str, dict] = {}
    if "A" in parts:
        vs = voices(args, data)
        reps = max(1, args.voice_repeats) if args.voice_sentences else 0
        usd = sum(budget.tts(60 + 3 * reps * 65, v) + budget.stt("nova-3", 19 + 2 * 20 * reps)
                  + budget.stt("nova-3", 6 + 3 * reps * 5, False) for v, _ in vs)
        est["A"] = {"items": len(vs), "usd": usd,
                    "minutes": len(vs) * (20 + 3 * reps * 6) / 60 / max(1, args.voice_concurrency)}
    if "B" in parts:
        ps, fs = b_pairs(args, data), file_pairs(args, data)
        sec = lambda c: c.get("duration_s", 12)
        multi = [l for l in args.multi_languages.split(",") if l in data.fleurs]
        extra = [m for m, _, c in flux_pairs(args, data)] + [x.split(":")[0] for x in args.latency_pairs.split(",") if x
                                                             for _ in range(args.latency_repeats)]
        extra_usd = sum(budget.stt(m, 20) for m in extra)
        usd = sum(budget.stt(m, sec(c) + 6) for m, _, c in ps) + sum(budget.stt(m, sec(c), False) for m, _, c in fs) \
            + sum(budget.stt("nova-3-multilingual", sec(data.fleurs[l]) + 6) for l in multi) + extra_usd
        minutes = (sum(sec(c) + 8 for _, _, c in ps) / max(1, args.concurrency) + len(fs) * 4 / max(1, args.concurrency)
                   + len(file_only_models(data)) * 20) / 60
        est["B"] = {"items": len(ps) + len(fs) + len(extra), "streaming_pairs": len(ps), "file_pairs": len(fs),
                    "extra_sessions": len(extra), "usd": usd,
                    "minutes": minutes}
    if "C" in parts:
        ms_ = english_models(args, data) + ["nova-3" for l in args.smoke_languages.split(",") if l] \
            + ["nova-3" for l in args.multi_smoke.split(",") if l]
        turns = len([t for t in args.smoke_turns.split(",") if t])
        est["C"] = {"items": len(ms_), "usd": sum(budget.stt(m, 50 * turns) + budget.tts(400 * turns) for m in ms_),
                    "minutes": len(ms_) * 50 * turns / 60}
    if "S" in parts:
        sets = [x.split(":")[:3] for x in args.stop_sets.split(",") if x]
        est["S"] = {"items": sum(int(n) for _, _, n in sets),
                    "usd": sum(budget.stt(m, 25 * int(n) + 10) + budget.tts(260 * int(n)) for m, _, n in sets),
                    "minutes": sum(25 * int(n) + 10 for _, _, n in sets) / 60}
    if "D" in parts:
        tm = think_models(args, data)
        sv = spread_voices(data)
        sv = sv[: args.spread_voices]
        extra = [x for a in (args.delegate_think, args.gateway_listen, args.gateway_regions) for x in a.split(",") if x]
        inject = len([t for t in args.inject_think.split(",") if t]) * (5 if args.user_text_url else 3)
        spread = len(sv) * len([t for t in args.spread_think.split(",") if t])
        gw = len(tm) * (2 if args.think_tools else 1) + 1 + len(extra) + spread + inject
        direct = (len(tm) + len([l for l in args.listen_models.split(",") if l]) * len(sv) + 2) \
            if dg_key() and not args.no_direct and not args.no_direct_d else 0
        est["D"] = {"items": gw + direct, "gateway_sessions": gw, "direct_sessions": direct,
                    "usd": budget.agent(30) * gw + budget.agent(20) * direct, "minutes": (gw * 32 + direct * 20) / 60}
    if "L" in parts:
        pairs = [x for x in args.listen_voices.split(",") if x]
        est["L"] = {"items": len(pairs), "usd": sum(budget.stt("nova-3", 40) + budget.tts(120) for _ in pairs),
                    "minutes": len(pairs) * 45 / 60}
    if "E" in parts:
        est["E"] = {"items": 8 + 7 * args.e_repeats, "usd": budget.stt("nova-3", 60) + budget.tts(200)
                    + (0 if args.no_agent_e else 2 * budget.agent(30))
                    + args.e_repeats * (budget.stt("nova-3", 4 * 25) + budget.stt("nova-3", 15, False) * 2
                                        + budget.tts(150) * 2 + (0 if args.no_agent_e else budget.agent(30) + budget.agent(20))),
                    "minutes": 3 + args.e_repeats * 3.5}
    return est


# ---- runner


async def gather_limited(n: int, coros):
    sem = asyncio.Semaphore(n)

    async def run(c):
        async with sem:
            try:
                return await c
            except Exception as e:
                return {"error": f"{type(e).__name__}: {e}"[:300]}
    return await asyncio.gather(*(run(c) for c in coros))


async def run(args) -> int:
    data = Data(Path(args.data), Path(args.packs) if args.packs else Path(args.data) / "packs")
    pricing = json.loads((Path(args.data) / "budget/pricing.json").read_text())
    budget = Budget(args, pricing)
    parts = [p.strip().upper() for p in args.parts.split(",") if p.strip()]
    res_dir = Path(args.results)
    res_dir.mkdir(parents=True, exist_ok=True)
    est = estimate(args, budget, data, parts)
    print(f"ledger before this run: ${budget.before:.2f} of ${budget.limit:.2f}")
    for p, e in est.items():
        print(f"estimate part {p}: {e['items']} items, ~${e['usd']:.2f}, ~{e['minutes']:.0f} min")
    total = sum(e["usd"] for e in est.values())
    print(f"estimate total: ~${total:.2f}")
    report: Dict[str, Any] = {"base_url": args.base_url, "started": time.strftime("%Y-%m-%dT%H:%M:%S"),
                              "estimate": est, "parts": {}, "skipped": []}
    if args.estimate_only:
        (res_dir / "estimate.json").write_text(json.dumps(est, indent=1))
        return 0
    budget.check_balance("start")
    t_all = now()
    for p in parts:
        budget.check_balance(f"before part {p}")
        t = now()
        if budget.stopped:
            report["skipped"].append({"part": p, "why": budget.stopped})
            continue
        print(f"== part {p}")
        sampler = LoadSampler().start()
        if p == "A":
            rows = await gather_limited(args.voice_concurrency,
                                        [voice_case(args, budget, data, v, info) for v, info in voices(args, data)])
        elif p == "L":
            rows = []
            for pair in [x for x in args.listen_voices.split(",") if x]:
                voice, language = pair.split(":")
                rows.append(await listening_case(args, budget, data, voice, language))
        elif p == "B":
            pairs = b_pairs(args, data)
            rows = await gather_limited(args.concurrency, [stt_case(args, budget, data, m, l, c)
                                                           for m, l, c in pairs])
            # A connection that timed out (a slow network, or the gateway's deepgram circuit
            # breaker deferring dials after quick refusals) is tried once more, alone, after a pause.
            flaky = [i for i, r in enumerate(rows) if re.search(r"Connection timeout|circuit", str(r.get("errors") or r.get("error") or ""))]
            if flaky:
                print(f"retrying {len(flaky)} pairs that timed out")
                await asyncio.sleep(args.retry_pause)
                for i in flaky:
                    m, l, c = pairs[i]
                    rows[i] = {**await stt_case(args, budget, data, m, l, c), "retried": True,
                               "first_error": rows[i].get("errors") or rows[i].get("error")}
            # nova-3 multilingual: the language the recogniser heard reaches stt_result
            for lang in [l for l in args.multi_languages.split(",") if l]:
                clip = data.fleurs.get(lang)
                if clip:
                    r = await stt_case(args, budget, data, "nova-3", "multi", clip, clip_language=lang)
                    rows.append({**r, "case": "multi_detected", "expected_language": lang,
                                 "pass": (r.get("detected_language") or "").split("-")[0] == lang})
            # Flux on /v2/listen: one turn model's finals per clip
            for model, lang, clip_lang in flux_pairs(args, data):
                r = await stt_case(args, budget, data, model, lang, data.fleurs[clip_lang], clip_language=clip_lang)
                rows.append({**r, "case": "flux", "expected_language": clip_lang})
            # latency repeats, one at a time (``model:session language:clip language``)
            for item in [x for x in args.latency_pairs.split(",") if x]:
                model, lang, clip_lang = (item.split(":") + [""])[:3]
                clip_lang = clip_lang or lang
                for i in range(args.latency_repeats):
                    r = await stt_case(args, budget, data, model, lang, data.fleurs[clip_lang], clip_language=clip_lang)
                    rows.append({**r, "case": "latency_repeat", "i": i, "expected_language": clip_lang})
            # prerecorded-only models: interaction asked, mode off on /ws, and /transcribe/batch
            en = data.fleurs.get("en")
            for model in sorted(file_only_models(data)):
                if not en or (args.file_models and model not in args.file_models.split(",")):
                    continue
                r = await stt_case(args, budget, data, model, "en", en, interaction=True)
                rows.append({**r, "case": "file_only_interaction_asked",
                             "stt_file_only_warning": any(w.get("code") == "interaction_surface_unavailable"
                                                          and w.get("reason") == "stt_file_only" for w in r.get("warnings", []))})
                r = await stt_case(args, budget, data, model, "en", en)
                rows.append({**r, "case": "file_only_ws_mode_off"})
            rows += await gather_limited(args.file_concurrency, [batch_case(args, budget, data, m, l, c)
                                                                 for m, l, c in file_pairs(args, data)])
        elif p == "C":
            rows = []
            for m in english_models(args, data):
                rows.append(await smoke_case(args, budget, data, m))
            for lang in [l for l in args.smoke_languages.split(",") if l]:
                rows.append({"case": "pack_language", **await smoke_case(args, budget, data, "nova-3", lang)})
            # A caller in another language on a `multi` session: the recogniser names it, and the
            # turn starts no task (it is asked again instead).
            for lang in [l for l in args.multi_smoke.split(",") if l]:
                r = await smoke_case(args, budget, data, "nova-3", lang, "multi")
                foreign = lang != "en"
                r["pass"] = bool(r.get("interaction_on")) and lang in [d.split("-")[0] for d in r.get("detected_language", [])] \
                    and (not r.get("task_created_ms") if foreign else r.get("task_created_ms") is not None) and bool(r.get("lines"))
                rows.append({"case": "multi_language", **r})
        elif p == "S":
            rows = []
            for item in [x for x in args.stop_sets.split(",") if x]:
                model, lang, n, cueset = (item.split(":") + [""])[:4]  # ``model:language:runs[:req|eager]``
                extras = json.loads(args.smoke_stt_extras) if args.smoke_stt_extras and model.startswith("flux") else None
                if cueset == "eager":
                    extras, cueset = {"eager_eot_threshold": 0.5}, ""
                rows.append(await stop_case(args, budget, data, model, lang, int(n), extras, cueset))
        elif p == "D":
            rows = []
            # The gateway names each think model's vendor (core/realtime/deepgram/protocol.rs).
            for tm in ([] if args.no_gateway_d else think_models(args, data)):
                if args.think_tools:
                    r = await realtime_case(args, budget, data, tm["id"], tools=[{"type": "function", "function": TABLE_FN}],
                                            then="slow_lookup")
                else:
                    r = await realtime_case(args, budget, data, tm["id"])
                if not r.get("pass") and not r.get("skipped") and args.retry_failed:
                    first = {k: r.get(k) for k in ("errors", "error", "answer_text", "first_audio_ms", "turn2")}
                    await asyncio.sleep(args.d_retry_pause)  # > 60 s: past a FATAL deepgram-realtime breaker
                    r = {**await realtime_case(args, budget, data, tm["id"], tools=[{"type": "function", "function": TABLE_FN}]
                                               if args.think_tools else None, then="slow_lookup" if args.think_tools else None),
                         "retried": True, "first_attempt": first}
                rows.append({"case": "gateway_think", "provider": tm["provider"], **r})
            for think in [t for t in args.inject_think.split(",") if t and not args.no_gateway_d]:
                long_text = LONG_LINES["en"]
                rows.append({"case": "gateway_inject_agent_message", **await inject_case(
                    args, budget, args.base_url, think, "inject_agent_message",
                    [("text", "Hello, this is the inject probe."), ("wait_audio", 10), ("wait_quiet", 15)])})
                # Deepgram queues an agent line sent while an injected line plays (direct probe)
                rows.append({"case": "gateway_inject_queued", **await inject_case(
                    args, budget, args.base_url, think, "inject_during_injected_line",
                    [("text", long_text), ("wait_audio", 10), ("sleep", 1.0),
                     ("text", "Injected while you were speaking."), ("sleep", 4.0), ("wait_quiet", 30)])})
                # ... and refuses one sent while its think model replies (``already in the middle of a reply``)
                if args.story_clip:
                    rows.append({"case": "gateway_inject_refused", **await inject_case(
                        args, budget, args.base_url, think, "inject_during_reply",
                        [("audio", Path(args.story_clip)), ("wait_assistant", 15), ("sleep", 0.3),
                         ("text", "Injected while you were speaking."), ("sleep", 3.0), ("wait_quiet", 30)])})
                if args.user_text_url:
                    rows.append({"case": "gateway_inject_user_message", **await inject_case(
                        args, budget, args.user_text_url, think, "inject_user_message",
                        [("text", QUESTION["en"]), ("wait_audio", 15), ("wait_quiet", 15)])})
                    rows.append({"case": "gateway_inject_user_during_reply", **await inject_case(
                        args, budget, args.user_text_url, think, "inject_user_during_reply",
                        [("text", "Please tell me a long story about the sea, at least six sentences."), ("wait_audio", 15),
                         ("sleep", 1.5), ("text", QUESTION["en"]), ("wait_audio", 10), ("wait_quiet", 25)])})
            if not args.no_gateway_d:
                rows.append({"case": "gateway_function", **await realtime_case(
                    args, budget, data, "gpt-4o-mini", tools=[{"type": "function", "function": TABLE_FN}],
                    request="slow_lookup")})
            for think in [t for t in args.delegate_think.split(",") if t and not args.no_gateway_d]:
                rows.append({"case": "gateway_interaction", **await realtime_case(
                    args, budget, data, think, request="slow_lookup",
                    interaction={"tasks": {"llm": {"base_url": args.llm_url, "model": args.llm_model,
                                                   "api_key": "local-dummy"}}})})
            for think in [t for t in args.spread_think.split(",") if t and not args.no_gateway_d]:
                for lang, v in spread_voices(data)[: args.spread_voices]:
                    rows.append({"case": "gateway_voice_spread", **await realtime_case(
                        args, budget, data, think, voice=v, language=lang)})
            for item in [l for l in args.gateway_listen.split(",") if l and not args.no_gateway_d]:
                listen, lang = (item.split(":") + ["en"])[:2]  # ``listen[:language]``
                voice = next((v for l_, v in spread_voices(data) if l_ == lang), "aura-2-thalia-en")
                rows.append({"case": "gateway_listen", **await realtime_case(
                    args, budget, data, args.listen_think, listen=listen, language=lang, voice=voice)})
            for region in [r for r in args.gateway_regions.split(",") if r and not args.no_gateway_d]:
                rows.append({"case": "gateway_region", **await realtime_case(
                    args, budget, data, "gpt-4o-mini", region=region)})
            if dg_key() and not args.no_direct and not args.no_direct_d:
                q = data.request("en", "q_capital")
                pcm24 = resample(*load_wav(q), AGENT_RATE) if q else None
                for listen in [l for l in args.listen_models.split(",") if l]:
                    for lang, v in spread_voices(data)[: args.spread_voices]:
                        qq = data.request(lang, "q_capital")
                        pcm_l = resample(*load_wav(qq), AGENT_RATE) if qq else pcm24
                        lang_param = lang if not listen.startswith("flux") else "en"
                        r = await agent_direct(args, budget, f"listen {listen} x {v}",
                                               agent_block("open_ai", "gpt-4o-mini", listen, v, lang_param),
                                               [{"audio": True}, {"wait": "AgentAudioDone", "timeout": 15}], pcm_l)
                        rows.append({"case": "direct_listen_x_voice", "listen": listen, "voice": v, "language": lang_param,
                                     **{k: r.get(k) for k in ("settings_applied", "conversation", "errors", "error",
                                                              "audio_s", "wall_s", "first_audio_ms_from_connect")}})
                for tm in think_models(args, data):
                    r = await agent_direct(args, budget, f"think {tm['id']}",
                                           agent_block(THINK_TYPES.get(tm["provider"], tm["provider"]), tm["id"],
                                                       "nova-3", "aura-2-thalia-en", "en"),
                                           [{"send": {"type": "InjectUserMessage", "content": QUESTION["en"]}},
                                            {"wait": "AgentAudioDone", "timeout": 20}])
                    rows.append({"case": "direct_think", "think": tm["id"], "provider": tm["provider"],
                                 **{k: r.get(k) for k in ("settings_applied", "conversation", "errors", "error",
                                                          "audio_s", "wall_s")}})
                r = await agent_direct(args, budget, "function round trip",
                                       agent_block("open_ai", "gpt-4o-mini", "nova-3", "aura-2-thalia-en", "en",
                                                   [WEATHER_FN]),
                                       [{"send": {"type": "InjectUserMessage", "content": "What's the weather in Paris right now?"}},
                                        {"wait": "FunctionCallRequest", "timeout": 15}, {"fn_reply": True},
                                        {"wait": "AgentAudioDone", "timeout": 20}])
                rows.append({"case": "direct_function", **r})
                r = await agent_direct(args, budget, "inject probe",
                                       agent_block("open_ai", "gpt-4o-mini", "nova-3", "aura-2-thalia-en", "en"),
                                       [{"send": {"type": "InjectAgentMessage", "message": "Hello, this is the inject probe."}},
                                        {"wait": "AgentAudioDone", "timeout": 20},
                                        {"send": {"type": "InjectAgentMessage", "content": "Second line, sent with content."}},
                                        {"wait": "AgentAudioDone", "timeout": 20},
                                        {"sleep": 4},
                                        {"send": {"type": "InjectUserMessage", "content": "Please tell me a long story about the sea."}},
                                        {"wait": "ConversationText", "timeout": 15}, {"sleep": 1.0},
                                        {"send": {"type": "InjectAgentMessage", "message": "Injected while you were speaking."}},
                                        {"sleep": 3},
                                        {"send": {"type": "InjectUserMessage", "content": QUESTION["en"]}},
                                        {"wait": "AgentAudioDone", "timeout": 25}])
                rows.append({"case": "direct_inject_probe", **r})
        elif p == "E":
            rows = await part_e(args, budget, data)
        else:
            sampler.stop()
            continue
        report["parts"][p] = {"rows": rows, "seconds": round(now() - t, 1), "usd": round(budget.by_part.get(p, 0.0), 4),
                              "load": sampler.stop()}
        skipped = [r for r in rows if isinstance(r, dict) and r.get("skipped")]
        if skipped:
            report["skipped"].append({"part": p, "items": len(skipped), "why": skipped[0]["skipped"]})
        (res_dir / "results.json").write_text(json.dumps(report, indent=1, ensure_ascii=False, default=str))
    report["seconds"] = round(now() - t_all, 1)
    report["budget"] = budget.summary()
    (res_dir / "results.json").write_text(json.dumps(report, indent=1, ensure_ascii=False, default=str))
    table = render(report)
    (res_dir / "table.md").write_text(table)
    print(table)
    return 0


def conf_real(c) -> Optional[bool]:
    if isinstance(c, dict):
        return c.get("real")
    return None if c is None else 0.0 < float(c) < 1.0


def render(report: dict) -> str:
    out = [f"# Deepgram matrix: {report['base_url']} ({report['started']})", ""]
    p = report["parts"]
    if "A" in p:
        out += ["## A voices", "", "| voice | lang | pass | listening pass | sentences (route: first audio / complete ms, error rate) "
                "| clips (text: trimmed ms / round trip) |", "|---|---|---|---|---|---|"]
        for r in p["A"]["rows"]:
            clips = "; ".join(f"{c['text']}: {c.get('trimmed_ms')} / {(c.get('roundtrip') or {}).get('text', '')!r}"
                              + (" SPELLED" if c.get("spelled_out") else "") for c in r.get("clips", []))
            sent = "; ".join(f"{c.get('route')}: {c.get('first_audio_ms')} / {c.get('complete_ms')}, "
                             f"{(c.get('roundtrip') or {}).get('error_rate')}" + (f" ERR {c.get('error')}" if c.get("error") else "")
                             for c in r.get("sentences", []))
            out.append(f"| {r.get('voice')} | {r.get('language')} | {r.get('pass')} | {r.get('listening_pass')} | {sent} | "
                       f"{clips or r.get('error') or r.get('skipped')} |")
        out.append("")
    if "S" in p:
        out += ["## S stop cue", "", "| model | lang | pass | cut ms (each) | p50 / max | cue words ms (each) | share before words p50 |",
                "|---|---|---|---|---|---|---|"]
        for r in p["S"]["rows"]:
            out.append(f"| {r.get('model')} | {r.get('language')} | {r.get('pass')} | {r.get('cut_ms')} | {r.get('cut_p50_ms')} / "
                       f"{r.get('cut_max_ms')} | {[x.get('words_ms') for x in r.get('runs', [])]} | {r.get('share_before_words_p50')} "
                       f"{r.get('error') or r.get('skipped') or ''} |")
        out.append("")
    if "L" in p:
        out += ["## L listening sounds", "", "| voice | lang | pass | bank clips | refused | starts s | played ms / dBFS | heard |",
                "|---|---|---|---|---|---|---|---|"]
        for r in p["L"]["rows"]:
            out.append(f"| {r.get('voice')} | {r.get('language')} | {r.get('pass')} | {r.get('bank_clips')} | "
                       f"{'; '.join(r.get('refused') or [])} | {r.get('starts_s')} | {r.get('played_ms')} / "
                       f"{r.get('played_dbfs')} | {r.get('heard', '')!r} {r.get('error') or r.get('skipped') or ''} |")
        out.append("")
    if "B" in p:
        out += ["## B speech-to-text", "", "| case | model | lang | metric | rate | first interim ms | final ms | conf real | detected | warnings / errors |",
                "|---|---|---|---|---|---|---|---|---|---|"]
        for r in p["B"]["rows"]:
            case = r.get("case") or r.get("route") or "stream"
            warn = ",".join(f"{w.get('code')}:{w.get('reason', '')}" if isinstance(w, dict) else str(w) for w in r.get("warnings", []))
            out.append(f"| {case} | {r.get('model')} | {r.get('language')} | {r.get('metric', '')} | {r.get('error_rate')} | "
                       f"{r.get('first_interim_ms')} | {r.get('final_latency_ms')} | {conf_real(r.get('confidence'))} | {r.get('detected_language')} | "
                       f"{warn} {str(r.get('errors') or r.get('error') or r.get('skipped') or '')[:120]} |")
        out.append("")
    if "C" in p:
        out += ["## C interaction smoke", "", "| model | lang / voice | pass | task_created ms | task_result ms | caller | lines | errors |",
                "|---|---|---|---|---|---|---|---|"]
        for r in p["C"]["rows"]:
            lines = " / ".join(f"{l['source']}:{l['kind']}:{l['text'][:40]}" for l in r.get("lines", []))
            out.append(f"| {r.get('model')} | {r.get('language')} / {r.get('voice')} | {r.get('pass')} | {r.get('task_created_ms')} | "
                       f"{r.get('task_result_ms')} | {str(r.get('caller_text', ''))[:60]} | {lines} | {r.get('errors') or r.get('error') or ''} |")
        out.append("")
    if "D" in p:
        out += ["## D Voice Agent", "", "| case | think / listen | voice | ok | first audio ms | answer | errors |", "|---|---|---|---|---|---|---|"]
        for r in p["D"]["rows"]:
            ok = r.get("pass", r.get("settings_applied"))
            ans = r.get("answer_text") or " | ".join(f"{a}: {b}" for a, b in (r.get("conversation") or []))
            errs = r.get("errors") or r.get("error") or ""
            what = " ".join(str(x) for x in (r.get('think') or r.get('label', ''), r.get('listen') or '', r.get('region') or '',
                                              r.get('provider') or '') if x)
            out.append(f"| {r.get('case')} | {what} | {r.get('voice', '')} | {ok} | "
                       f"{r.get('first_audio_ms', r.get('first_audio_ms_from_connect'))} | {str(ans)[:100]} | {str(errs)[:140]} |")
        out.append("")
    if "E" in p:
        out += ["## E endpoint types", "", "| case | pass | detail |", "|---|---|---|"]
        for r in p["E"]["rows"]:
            ok = r.get("pass", r.get("error_rate") is not None and not r.get("error"))
            detail = r.get("note") or r.get("errors") or r.get("error") or r.get("hypothesis") or r.get("content_type") or ""
            out.append(f"| {r.get('case')} | {ok} | {str(detail)[:160]} |")
        out.append("")
    b = report.get("budget", {})
    out += [f"Budget: this run ${b.get('this_run_usd')}, ledger total ${b.get('total_usd')} of ${b.get('limit_usd')}; "
            f"measured: {b.get('measured_usd')}; stopped: {b.get('stopped')}", f"Skipped: {report.get('skipped')}"]
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base-url", default="http://127.0.0.1:3401")
    ap.add_argument("--results", required=True, help="directory for results.json and table.md")
    ap.add_argument("--data", default=os.environ.get("DG_MATRIX_DATA", ""), help="matrix work directory")
    ap.add_argument("--packs", default="", help="language packs directory (default <data>/packs)")
    ap.add_argument("--parts", default="A,B,C,E,D", help="D last: its gateway sessions can open the deepgram circuit breaker")
    ap.add_argument("--concurrency", type=int, default=8)
    ap.add_argument("--voice-concurrency", type=int, default=4)
    ap.add_argument("--languages", default="", help="restrict B to these base languages")
    ap.add_argument("--limit-voices", type=int, default=0)
    ap.add_argument("--voices", default="", help="comma-separated voice names")
    ap.add_argument("--limit-pairs", type=int, default=0)
    ap.add_argument("--pairs", default="", help="comma-separated model:language")
    ap.add_argument("--limit-file-pairs", type=int, default=0, help="batch pairs of prerecorded-only models")
    ap.add_argument("--file-models", default="", help="prerecorded-only models for the /ws checks")
    ap.add_argument("--limit-smoke", type=int, default=0)
    ap.add_argument("--smoke-models", default="")
    ap.add_argument("--smoke-timeout", type=float, default=40.0)
    ap.add_argument("--limit-think", type=int, default=0)
    ap.add_argument("--think", default="", help="comma-separated think model ids")
    ap.add_argument("--listen-models", default="nova-3,nova-2,flux-general-en")
    ap.add_argument("--spread-voices", type=int, default=7, help="voices of the spread (one per request language)")
    ap.add_argument("--no-direct", action="store_true", help="skip the direct agent.deepgram.com probes")
    ap.add_argument("--no-direct-d", action="store_true", help="skip only part D's direct probes")
    ap.add_argument("--no-gateway-d", action="store_true", help="part D's direct probes only")
    ap.add_argument("--delegate-think", default="gpt-4o-mini",
                    help="think models of the interaction-mode delegate sessions (part D)")
    ap.add_argument("--gateway-listen", default="flux-general-en", help="listen models through the gateway (D)")
    ap.add_argument("--gateway-regions", default="eu", help="agent regions through the gateway (D)")
    ap.add_argument("--no-agent-e", action="store_true", help="skip part E's EU Voice Agent session")
    ap.add_argument("--multi-languages", default="", help="FLEURS languages streamed with language=multi (B)")
    ap.add_argument("--smoke-languages", default="", help="pack languages of the interaction turn (C)")
    ap.add_argument("--multi-smoke", default="", help="caller languages of interaction turns on a language=multi session (C)")
    ap.add_argument("--smoke-stt-extras", default="",
                    help='stt_config.extras (JSON) of the Flux smoke sessions, e.g. {"eager_eot_threshold": 0.5}')
    ap.add_argument("--llm-url", default="http://127.0.0.1:18742/v1")
    ap.add_argument("--llm-model", default="qwen3b-q4_k_m")
    ap.add_argument("--keep-audio", action="store_true")
    ap.add_argument("--listen-voices", default="aura-2-thalia-en:en,aura-2-agustina-es:es",
                    help="part L: voice:language pairs (the gateway needs each language's pack)")
    ap.add_argument("--gateway-log", default="", help="part L: the gateway's log, for its clip-bank lines")
    ap.add_argument("--budget-usd", type=float, default=150.0)
    ap.add_argument("--ledger", default="", help="spend ledger (JSONL), default <data>/budget/spend_log.jsonl")
    ap.add_argument("--balance-every", type=int, default=50)
    ap.add_argument("--estimate-only", action="store_true")
    ap.add_argument("--retry-pause", type=float, default=60.0, help="pause before retrying timed-out STT pairs")
    ap.add_argument("--voice-repeats", type=int, default=1, help="A: sentences per route and voice")
    ap.add_argument("--no-voice-sentences", dest="voice_sentences", action="store_false",
                    help="A: only the listening sounds")
    ap.add_argument("--flux-pairs", default="", help="B: model:clip language[:session language] on /v2/listen")
    ap.add_argument("--latency-pairs", default="", help="B: model:session language[:clip language], repeated")
    ap.add_argument("--latency-repeats", type=int, default=3)
    ap.add_argument("--file-cap", type=int, default=0, help="B: languages per prerecorded model (0: all)")
    ap.add_argument("--file-full-models", default="", help="B: prerecorded models exempt from --file-cap")
    ap.add_argument("--file-concurrency", type=int, default=4)
    ap.add_argument("--file-pairs", default="", help="B: only these prerecorded pairs (model:language)")
    ap.add_argument("--smoke-turns", default="q_capital", help="C: the requests of each session, in order")
    ap.add_argument("--stop-sets", default="nova-3:en:7,flux-general-en:en:7", help="S: model:language:runs")
    ap.add_argument("--caller-dir", default="", help="S: the live script's caller clips (44_stop_talking.wav ...)")
    ap.add_argument("--stop-limit-ms", type=int, default=900)
    ap.add_argument("--think-tools", action="store_true", help="D: each think model also gets a tool-calling turn")
    ap.add_argument("--retry-failed", action="store_true", help="D: a failed think model runs once more")
    ap.add_argument("--d-retry-pause", type=float, default=5.0, help="D: pause before that second run")
    ap.add_argument("--inject-think", default="", help="D: think models of the text-injection probes")
    ap.add_argument("--user-text-url", default="", help="D: a gateway running with deepgram_user_text")
    ap.add_argument("--story-clip", default="", help="D: a spoken request for a long answer (the refused injection)")
    ap.add_argument("--spread-think", default="gpt-4o-mini", help="D: think models of the voice/language spread")
    ap.add_argument("--listen-think", default="gpt-4o-mini", help="D: think model of the gateway_listen sessions")
    ap.add_argument("--e-repeats", type=int, default=0, help="E: gateway vs direct Deepgram, this many times")
    args = ap.parse_args()
    if not args.data:
        ap.error("--data (or DG_MATRIX_DATA) is required")
    args.ledger = args.ledger or str(Path(args.data) / "budget/spend_log.jsonl")
    sys.exit(asyncio.run(run(args)))


if __name__ == "__main__":
    main()
