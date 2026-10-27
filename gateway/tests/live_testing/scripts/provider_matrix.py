#!/usr/bin/env python3
"""
Minimal live matrix of the gateway's ElevenLabs and Cartesia surfaces, against a running gateway
(or one this script starts with ``--start-stack``), plus one direct vendor call per endpoint for
the gateway's overhead.

Cells (``--vendors elevenlabs,cartesia``):

tts      every TTS model: one ``/ws`` sentence (English, voice A; the gateway calls the vendor's
         REST synthesis), one ``POST /speak`` sentence (Spanish with voice B for a multilingual
         model, a second English sentence otherwise) and, for a multilingual model, a German
         ``POST /speak`` sentence (the three-language spot check). Each clip is checked for
         silence and length and sent to the vendor's own prerecorded STT (ElevenLabs
         ``scribe_v2``, Cartesia ``ink-whisper``) for a word error rate. The English and Spanish
         sentences are also synthesised directly (same model, voice, text, format): overhead is
         gateway first audio minus direct time to first byte.
stt      every streaming STT model: a known clip streamed in real time through ``/ws``
         (``deepgram_matrix.stt_case``: word error rate, first partial, final latency), and the
         English clip once directly against the vendor's WebSocket with the same pacing.
batch    ElevenLabs ``POST /transcribe/batch`` per batch model, and the same upload directly.
probe    no-spend checks: refusals the gateway should give (wrong transport for a model, an
         unsupported batch vendor), ``GET /voices``, ``/ws`` TTS with ``features.streaming``
         (the gateway has no vendor WebSocket for either; it falls back to REST), Bud-only routes.
realtime ElevenLabs Conversational AI through ``/realtime`` (needs a pre-created agent id,
         ``--el-agent``), the provider aliases, and the same connect directly.
inter    one interaction-mode ``/ws`` session per vendor with that vendor's TTS and streaming STT:
         ready with the mode on, the listening-sound bank built in that voice (the gateway log's
         ``interaction listening sounds ready``), one spoken question answered.

Keys: read from ``--secrets`` (``elevenlabs.env``, ``cartesia.env``) into this process and the
gateway's environment only; every string written is redacted. A quota or credit refusal stops
that vendor's spending cells. Ledger: ``<results>/<vendor>/ledger.jsonl`` (characters
synthesised, audio seconds transcribed, gateway and direct). Results: ``results.jsonl`` and
``tables.md`` per vendor.
"""

import argparse
import asyncio
import base64
import json
import os
import re
import signal
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Dict, List, Optional

import websockets

sys.path.insert(0, str(Path(__file__).resolve().parent))
import deepgram_matrix as dm  # noqa: E402
from deepgram_matrix import (RATE, SENTENCES, WsSession, dbfs, error_rate, error_text, gather_limited,  # noqa: E402
                             http, http_timed, load1, load_wav, ms, now, pcm_of, resample, voiced_span, wav_bytes,
                             ws_url)

os.environ.pop("DEEPGRAM_API_KEY", None)  # Deepgram is not used here (dm's round trips stay off)

EL_API, CA_API, CA_VERSION = "https://api.elevenlabs.io", "https://api.cartesia.ai", "2025-04-16"
VENDORS: Dict[str, dict] = {
    "elevenlabs": {
        "env": "ELEVENLABS_API_KEY",
        # model: multilingual. The gateway's provider-info list plus eleven_v3_conversational, which
        # its context-continuity code names (core/tts/elevenlabs.rs:84). The key cannot read /v1/models.
        "tts": {"eleven_v3": True, "eleven_v3_conversational": True, "eleven_multilingual_v2": True,
                "eleven_flash_v2_5": True, "eleven_turbo_v2_5": True, "eleven_flash_v2": False, "eleven_turbo_v2": False},
        "voices": ("JBFqnCBsd6RMkjVDRZzb", "EXAVITQu4vr4xnSDxMaL"),  # George (the gateway default), Sarah
        "stt": {"scribe_v2_realtime": ["en", "es"]},
        "batch": ["scribe_v2", "scribe_v2_medical"],
        "inter": ("scribe_v2_realtime", "eleven_flash_v2_5"),
    },
    "cartesia": {
        "env": "CARTESIA_API_KEY",
        "tts": {"sonic-3.6": True, "sonic-3.5": True, "sonic-3": True, "sonic-preview": True},  # GET /models
        # The gateway's default voice (handlers/voices.rs:752) and a Spanish voice of GET /voices.
        "voices": ("a0e99841-438c-4a64-b679-ae501e7d6091", "35b2cfc1-e6fb-4d69-a598-c1780612be4a"),
        "stt": {"ink-whisper": ["en", "es"], "ink-2": ["en"], "ink-preview": ["en"]},
        "batch": [],
        "inter": ("ink-whisper", "sonic-3"),
    },
}
NOT_SUPPORTED = {
    "elevenlabs": [
        ("TTS streaming endpoint POST /v1/text-to-speech/{voice}/stream", "the gateway calls only the convert endpoint (core/tts/elevenlabs.rs:216)"),
        ("TTS WebSocket stream-input and multi-context", "features.streaming has a WebSocket variant for deepgram only (core/tts/standard.rs:207)"),
        ("speech-to-speech models eleven_multilingual_sts_v2, eleven_english_sts_v2", "no speech-to-speech route"),
        ("voice design eleven_ttv_v3, sound effects, music, dubbing, audio isolation, forced alignment", "no route"),
        ("scribe_v1", "removed by ElevenLabs on 2026-07-09 (core/stt/elevenlabs/batch.rs:45)"),
    ],
    "cartesia": [
        ("TTS WebSocket wss://api.cartesia.ai/tts/websocket and SSE /tts/sse", "the gateway calls only POST /tts/bytes (core/tts/cartesia/config.rs:52, core/tts/standard.rs:207)"),
        ("batch STT POST /stt (ink-whisper has a batch mode)", "batch_provider_supported has no cartesia (core/stt/batch.rs:839)"),
        ("voice listing", "GET /voices and fetch_catalog_with_key list ElevenLabs and Deepgram only (handlers/voices.rs:776)"),
        ("Cartesia agents (Line)", "no realtime provider for cartesia (core/realtime/mod.rs:131)"),
        ("voice changer, infill, voice cloning live test", "out of scope here (clone_voice_cartesia exists, not exercised)"),
    ],
}
QUOTA = re.compile(r"quota|credit|insufficient|payment.?required|limit.?reached|exceeds your|character limit", re.I)
EN_TEXT, EN2_TEXT, ES_TEXT, DE_TEXT = SENTENCES["en"][0], SENTENCES["en"][1], SENTENCES["es"][0], SENTENCES["de"][0]


class Ctx:
    def __init__(self, args):
        self.args, self.base = args, args.base_url.rstrip("/")
        self.keys = {v: read_key(Path(args.secrets) / f"{v}.env", VENDORS[v]["env"]) for v in VENDORS}
        self.secret_values = [k for k in self.keys.values() if k]
        self.rows: Dict[str, List[dict]] = {v: [] for v in VENDORS}
        self.stopped: Dict[str, Optional[str]] = {v: None for v in VENDORS}
        self.data = dm.Data(Path(args.data), Path(args.data) / "packs")
        for v in VENDORS:  # a vendor stopped by an earlier run's quota refusal stays stopped
            f = Path(args.results) / v / "ledger.jsonl"
            for line in (f.read_text().splitlines() if f.exists() else []):
                if '"case": "stopped"' in line:
                    self.stopped[v] = json.loads(line).get("note") or "stopped"

    def redact(self, text: str) -> str:
        for k in self.secret_values:
            text = text.replace(k, "[REDACTED]")
        return text

    def ledger(self, vendor: str, case: str, route: str, chars: int = 0, audio_s: float = 0.0, note: str = ""):
        d = Path(self.args.results) / vendor
        d.mkdir(parents=True, exist_ok=True)
        with (d / "ledger.jsonl").open("a") as f:
            f.write(self.redact(json.dumps({"at": time.strftime("%H:%M:%S"), "run": self.args.tag, "case": case, "route": route,
                                            "chars": chars, "audio_s": round(audio_s, 2), "note": note})) + "\n")

    def add(self, vendor: str, row: dict) -> dict:
        row = {"vendor": vendor, "run": self.args.tag, "load1": load1(), **row}
        text = json.dumps(row, ensure_ascii=False, default=str)
        if QUOTA.search(str(row.get("error") or "") + str(row.get("errors") or "")) and not self.stopped[vendor]:
            self.stopped[vendor] = f"quota/credit refusal in {row.get('case')}: {str(row.get('error') or row.get('errors'))[:200]}"
            self.ledger(vendor, "stopped", "", note=self.stopped[vendor])  # later runs stay stopped
        self.rows[vendor].append(json.loads(self.redact(text)))
        return row


def read_key(path: Path, var: str) -> str:
    for line in path.read_text().splitlines():
        line = line.strip()
        if line.startswith("export "):
            line = line[7:]
        if line.startswith(var + "="):
            return line.split("=", 1)[1].strip().strip("'\"")
    return ""


def multipart(fields: dict, data: bytes) -> tuple:
    b = "pm" + uuid.uuid4().hex
    out = [f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode() for k, v in fields.items()]
    out.append(f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="a.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode()
               + data + b"\r\n" + f"--{b}--\r\n".encode())
    return b"".join(out), f"multipart/form-data; boundary={b}"


# ---- configs and direct vendor calls

def stt_cfg(vendor: str, model: str, language: str) -> dict:
    return {"provider": vendor, "model": model, "language": language, "sample_rate": RATE, "channels": 1,
            "punctuation": True, "encoding": "linear16"}


def tts_cfg(vendor: str, model: str, voice: str, language: Optional[str] = None, streaming: bool = False) -> dict:
    cfg: Dict[str, Any] = {"provider": vendor, "model": model, "voice_id": voice, "audio_format": "linear16",
                           "sample_rate": RATE}
    features: Dict[str, Any] = {}
    if language and vendor == "cartesia":  # Cartesia's `language` (ElevenLabs takes language_code on v2.5 only)
        features["language"] = language
    if streaming:
        features["streaming"] = True
    if features:
        cfg["features"] = features
    return cfg


def headers(ctx: Ctx, vendor: str) -> dict:
    if vendor == "elevenlabs":
        return {"xi-api-key": ctx.keys["elevenlabs"]}
    return {"Authorization": "Bearer " + ctx.keys["cartesia"], "Cartesia-Version": CA_VERSION}


def direct_tts(ctx: Ctx, vendor: str, model: str, voice: str, text: str, language: str, path: str = "") -> dict:
    """The vendor's REST synthesis as the gateway sends it (`path`: `/stream` or `/tts/sse` for a reference)."""
    if vendor == "elevenlabs":
        url = f"{EL_API}/v1/text-to-speech/{voice}{path}?output_format=pcm_{RATE}"
        body = {"text": text, "model_id": model}
    else:
        url = CA_API + (path or "/tts/bytes")
        body = {"model_id": model, "transcript": text, "voice": {"mode": "id", "id": voice}, "language": language,
                "output_format": {"container": "raw", "encoding": "pcm_s16le", "sample_rate": RATE}}
    r = http_timed(url, body, headers(ctx, vendor), 60)
    ctx.ledger(vendor, f"tts {model}", "direct" + path, chars=len(text))
    return r


ISO = {"jw": "jv"}  # FLEURS codes that are not ISO 639-1


def vendor_transcribe(ctx: Ctx, vendor: str, pcm: bytes, rate: int, language: str, model: str = "") -> dict:
    """The vendor's own prerecorded STT (the TTS round trip, and the batch cells' direct call).
    A language the STT does not take is retried without one (the vendor detects it)."""
    language = ISO.get(language, language)
    out = _vendor_transcribe(ctx, vendor, pcm, rate, language, model)
    if language and out["status"] == 400 and "language" in out["text"].lower():
        out = _vendor_transcribe(ctx, vendor, pcm, rate, "", model) | {"language_refused": language}
    return out


def _vendor_transcribe(ctx: Ctx, vendor: str, pcm: bytes, rate: int, language: str, model: str = "") -> dict:
    fields = {"language_code" if vendor == "elevenlabs" else "language": language} if language else {}
    if vendor == "elevenlabs":
        body, ctype = multipart({"model_id": model or "scribe_v2", **fields}, wav_bytes(pcm, rate))
        url = f"{EL_API}/v1/speech-to-text"
    else:
        body, ctype = multipart({"model": model or "ink-whisper", **fields}, wav_bytes(pcm, rate))
        url = f"{CA_API}/stt"
    r = http_timed(url, body, headers(ctx, vendor), 120, ctype)
    ctx.ledger(vendor, "prerecorded stt", "direct", audio_s=len(pcm) / 2 / rate)
    out = {"status": r["status"], "latency_ms": r["total_ms"]}
    try:
        out["text"] = json.loads(r["body"]).get("text", "") if r["status"] == 200 else r["body"][:300].decode("utf-8", "replace")
    except ValueError:
        out["text"] = r["body"][:300].decode("utf-8", "replace")
    return out


def wer_ok(row: dict) -> bool:
    return row.get("wer") is not None and row["wer"] <= dm.WORDS_OK


def clip_checks(ctx: Ctx, vendor: str, pcm: bytes, text: str, language: str) -> dict:
    a, b = voiced_span(pcm, RATE)
    out = {"audio_ms": ms(len(pcm) / 2 / RATE), "trimmed_ms": ms(b - a), "dbfs": round(dbfs(pcm), 1)}
    out["silent"] = out["trimmed_ms"] == 0
    # ~13 characters a second of speech; outside 4..40 the clip is clipped, padded or at the wrong rate.
    cps = len(text) / max(0.001, (b - a))
    out["chars_per_s"] = round(cps, 1)
    out["length_ok"] = not out["silent"] and 4 <= cps <= 40
    if not out["silent"]:
        rt = vendor_transcribe(ctx, vendor, pcm, RATE, language)
        out["roundtrip_text"] = rt["text"]
        if rt.get("language_refused"):
            out["roundtrip_language_refused"] = rt["language_refused"]
        if rt["status"] == 200:
            out["metric"], out["wer"] = error_rate(text, rt["text"], language)
        else:
            out["roundtrip_error"] = f"HTTP {rt['status']}: {rt['text'][:200]}"
    return out


# ---- TTS cells

async def tts_ws(ctx: Ctx, vendor: str, model: str, voice: str, text: str, language: str, streaming: bool = False) -> dict:
    stt_model = VENDORS[vendor]["inter"][0]
    s = WsSession(ws_url(ctx.base, "/ws"), f"tts-{model}")
    row: Dict[str, Any] = {"case": "tts_ws_stream" if streaming else "tts_ws", "model": model, "voice": voice,
                           "language": language, "route": "gateway", "text": text}
    pcm = b""
    try:
        await s.open({"type": "config", "stt_config": stt_cfg(vendor, stt_model, language),
                      "tts_config": tts_cfg(vendor, model, voice, language, streaming)})
        row["warnings"] = [w.get("code") or w.get("message") for w in s.warnings()]
        if not s.ready:
            row["error"] = error_text(s.errors()) or f"no ready ({s.closed_reason})"
            return row
        clip = (await dm.speak_sentences(ctx.args, s, language, [text], "ws"))[0]
        row.update({k: clip.get(k) for k in ("first_audio_ms", "complete_ms", "error")})
        row["warnings"] = [w.get("code") or w.get("message") for w in s.warnings()]
        pcm = s.pcm()
        row["errors"] = error_text(s.errors())
    finally:
        await s.close()
        ctx.ledger(vendor, f"tts {model}", "gateway /ws", chars=len(text))
    if pcm:
        row.update(await asyncio.to_thread(clip_checks, ctx, vendor, pcm, text, language))
    row["pass"] = bool(pcm) and not row.get("error") and bool(row.get("length_ok")) and wer_ok(row)
    return row


async def tts_speak(ctx: Ctx, vendor: str, model: str, voice: str, text: str, language: str) -> dict:
    r = await asyncio.to_thread(http_timed, ctx.base + "/speak",
                                {"text": text, "tts_config": tts_cfg(vendor, model, voice, language)}, None, 90)
    ctx.ledger(vendor, f"tts {model}", "gateway /speak", chars=len(text))
    row: Dict[str, Any] = {"case": "tts_speak", "model": model, "voice": voice, "language": language, "route": "gateway",
                           "text": text, "status": r["status"], "first_audio_ms": r["ttfb_ms"], "complete_ms": r["total_ms"],
                           "content_type": r["content_type"]}
    if r["status"] == 200 and r["body"]:
        row.update(await asyncio.to_thread(clip_checks, ctx, vendor, pcm_of(r["body"]), text, language))
    else:
        row["error"] = f"HTTP {r['status']}: " + r["body"][:300].decode("utf-8", "replace")
    row["pass"] = r["status"] == 200 and bool(row.get("length_ok")) and wer_ok(row)
    return row


async def tts_direct(ctx: Ctx, vendor: str, model: str, voice: str, text: str, language: str, pair: dict,
                     path: str = "") -> dict:
    r = await asyncio.to_thread(direct_tts, ctx, vendor, model, voice, text, language, path)
    row: Dict[str, Any] = {"case": "tts_direct" + (path.replace("/", "_") if path else ""), "model": model,
                           "voice": voice, "language": language, "route": "direct", "status": r["status"],
                           "first_audio_ms": r["ttfb_ms"], "complete_ms": r["total_ms"], "pairs_with": pair.get("case")}
    if r["status"] == 200 and not path:
        pcm = r["body"]
        a, b = voiced_span(pcm, RATE)
        row.update(audio_ms=ms(len(pcm) / 2 / RATE), trimmed_ms=ms(b - a))
    elif r["status"] != 200:
        row["error"] = f"HTTP {r['status']}: " + r["body"][:300].decode("utf-8", "replace")
    if pair.get("first_audio_ms") is not None and r["ttfb_ms"] is not None and not path:
        row["overhead_ms"] = pair["first_audio_ms"] - r["ttfb_ms"]
        pair["overhead_ms"] = row["overhead_ms"]
    row["pass"] = r["status"] == 200
    return row


async def tts_model(ctx: Ctx, vendor: str, model: str, multilingual: bool) -> List[dict]:
    if ctx.stopped[vendor]:
        return [{"case": "tts", "model": model, "skipped": ctx.stopped[vendor]}]
    va, vb = VENDORS[vendor]["voices"]
    rows = [await tts_ws(ctx, vendor, model, va, EN_TEXT, "en")]
    rows.append(await tts_direct(ctx, vendor, model, va, EN_TEXT, "en", rows[0]))
    second = (ES_TEXT, "es") if multilingual else (EN2_TEXT, "en")
    rows.append(await tts_speak(ctx, vendor, model, vb, *second))
    rows.append(await tts_direct(ctx, vendor, model, vb, *second, rows[-1]))
    if multilingual:
        rows.append(await tts_speak(ctx, vendor, model, va, DE_TEXT, "de"))
    return rows


# ---- STT cells

def patch_dm(vendor: str):
    """deepgram_matrix.stt_case builds its configs from these two module functions."""
    dm.stt_cfg = lambda model, language, extras=None: stt_cfg(vendor, model, language)
    tts_model_ = VENDORS[vendor]["inter"][1]
    dm.tts_cfg = lambda voice, rate=RATE, features=None, extras=None: tts_cfg(vendor, tts_model_, VENDORS[vendor]["voices"][0])


class Spend:
    """The `budget` deepgram_matrix's cases expect: no price list, it records audio seconds."""

    def __init__(self, ctx: Ctx, vendor: str):
        self.ctx, self.vendor, self.stopped = ctx, vendor, None

    def allow(self, part, estimate):
        return not self.ctx.stopped[self.vendor]

    def stt(self, model, seconds, streaming=True):
        return seconds

    def tts(self, chars, model=""):
        return 0.0

    def charge(self, part, seconds, note=""):
        self.ctx.ledger(self.vendor, f"stt {note}", "gateway /ws", audio_s=seconds, note="session wall time")


def clip_of(ctx: Ctx, language: str) -> dict:
    name = "long_turn" if language == "en" else "slow_lookup"  # 9.2 s and 4.4 s: under the 10 s cap
    return {"wav": str(ctx.data.request(language, name)), "reference": ctx.data.request_text(language, name)}


async def direct_stt_ws(ctx: Ctx, vendor: str, model: str, language: str, pcm: bytes) -> dict:
    """The vendor's streaming endpoint, paced like stt_case: 0.5 s silence, the clip, 2 s silence."""
    if vendor == "elevenlabs":
        url = (f"wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id={model}&audio_format=pcm_{RATE}"
               f"&commit_strategy=vad&language_code={language}&vad_silence_threshold_secs=0.5")
    else:
        url = f"wss://api.cartesia.ai/stt/websocket?model={model}&language={language}&encoding=pcm_s16le&sample_rate={RATE}"
    events: List[tuple] = []
    row: Dict[str, Any] = {"case": "stt_direct", "model": model, "language": language, "route": "direct"}
    t_open = now()
    try:
        async with websockets.connect(url, additional_headers=headers(ctx, vendor), max_size=None, open_timeout=20) as ws:
            row["connect_ms"] = ms(now() - t_open)

            async def reader():
                async for m in ws:
                    if isinstance(m, str):
                        events.append((now(), json.loads(m)))
            rt = asyncio.create_task(reader())
            size = RATE * dm.FRAME_MS // 1000 * 2

            async def send(p: bytes) -> float:
                start, n = now(), 0
                for i in range(0, len(p), size):
                    f = p[i:i + size].ljust(size, b"\0")
                    if vendor == "elevenlabs":
                        await ws.send(json.dumps({"message_type": "input_audio_chunk", "audio_base_64": base64.b64encode(f).decode(),
                                                  "commit": False, "sample_rate": RATE}))
                    else:
                        await ws.send(f)
                    n += 1
                    delay = start + n * dm.FRAME_MS / 1000 - now()
                    if delay > 0:
                        await asyncio.sleep(delay)
                return start
            await send(bytes(RATE))
            t0 = await send(pcm)
            speech_end = t0 + voiced_span(pcm, RATE)[1]
            await send(bytes(RATE * 4))
            if vendor == "cartesia":
                await ws.send("finalize")
            deadline, last, quiet = now() + 12, len(events), now()
            while now() < deadline and now() - quiet < 2.5:
                await asyncio.sleep(0.25)
                if len(events) != last:
                    last, quiet = len(events), now()
            rt.cancel()
    except Exception as e:
        row["error"] = ctx.redact(f"{type(e).__name__}: {e}")[:300]
        return row
    finally:
        ctx.ledger(vendor, f"stt {model}", "direct ws", audio_s=0.5 + len(pcm) / 2 / RATE + 2)
    if vendor == "elevenlabs":
        texts = [(a, e) for a, e in events if e.get("message_type") in ("partial_transcript", "committed_transcript")
                 and e.get("text", "").strip()]
        finals = [(a, e) for a, e in texts if e["message_type"] == "committed_transcript"]
        errs = [e for _, e in events if "error" in e.get("message_type", "")]
    else:
        texts = [(a, e) for a, e in events if e.get("type") == "transcript" and e.get("text", "").strip()]
        finals = [(a, e) for a, e in texts if e.get("is_final")]
        errs = [e for _, e in events if e.get("type") == "error"]
    hyp = " ".join(e["text"].strip() for _, e in finals)
    row.update(first_partial_ms=ms(texts[0][0] - t0) if texts else None,
               final_latency_ms=ms(finals[-1][0] - speech_end) if finals else None, hypothesis=hyp,
               finals=len(finals), partials=len(texts) - len(finals),
               event_types=sorted({str(e.get("message_type") or e.get("type")) for _, e in events}),
               errors=ctx.redact(json.dumps(errs))[:300] if errs else "")
    return row


async def stt_model(ctx: Ctx, vendor: str, model: str, languages: List[str]) -> List[dict]:
    rows = []
    for lang in languages:
        if ctx.stopped[vendor]:
            rows.append({"case": "stt_ws", "model": model, "language": lang, "skipped": ctx.stopped[vendor]})
            continue
        clip = clip_of(ctx, lang)
        r = await dm.stt_case(ctx.args, Spend(ctx, vendor), ctx.data, model, lang, clip)
        r = {"case": "stt_ws", "route": "gateway", "first_partial_ms": r.pop("first_interim_ms", None), "wer": r.get("error_rate"), **r}
        r["pass"] = bool(r.get("ready")) and r.get("wer") is not None and r["wer"] <= dm.WORDS_OK and not r.get("errors")
        rows.append(r)
        if lang == "en":
            pcm = resample(*load_wav(Path(clip["wav"])), RATE)
            d = await direct_stt_ws(ctx, vendor, model, lang, pcm)
            d["metric"], d["wer"] = error_rate(clip["reference"], d.get("hypothesis", ""), lang)
            d["pass"] = d["wer"] is not None and d["wer"] <= dm.WORDS_OK and not d.get("error")
            for k, label in (("first_partial_ms", "first_partial_overhead_ms"), ("final_latency_ms", "final_overhead_ms")):
                if r.get(k) is not None and d.get(k) is not None:
                    r[label] = r[k] - d[k]
            rows.append(d)
    return rows


async def batch_cells(ctx: Ctx, vendor: str, model: str = "", language: str = "en", clip: Optional[dict] = None,
                      direct: bool = True) -> List[dict]:
    """`POST /transcribe/batch` per batch model (or `model` only), and the same upload directly."""
    if not model:
        rows = []
        for m in [m for m in VENDORS[vendor]["batch"] if wanted(ctx, m)]:
            if not ctx.stopped[vendor]:
                rows += await batch_cells(ctx, vendor, m)
        return rows
    clip = clip or clip_of(ctx, "en")
    pcm, rate = load_wav(Path(clip["wav"]))
    # `api_key` empty means the server key; --batch-client-key sends it in the body (local gateway only).
    body = {"provider": vendor, "api_key": ctx.keys[vendor] if ctx.args.batch_client_key else "", "model": model,
            "language": language, "sample_rate": rate, "channels": 1,
            "punctuation": True, "encoding": "linear16", "audio_base64": base64.b64encode(wav_bytes(pcm, rate)).decode(),
            "content_type": "audio/wav"}
    t = now()
    r = await asyncio.to_thread(http_timed, ctx.base + "/transcribe/batch", body, None, 180)
    ctx.ledger(vendor, f"batch {model}", "gateway", audio_s=len(pcm) / 2 / rate)
    row: Dict[str, Any] = {"case": "stt_batch", "model": model, "language": language, "route": "gateway", "status": r["status"],
                           "client_key": ctx.args.batch_client_key}
    try:
        job = json.loads(r["body"])
    except ValueError:
        job = {"error": r["body"][:300].decode("utf-8", "replace")}
    for _ in range(60):  # a queued job is polled
        if job.get("status") in ("completed", "error", "failed") or r["status"] != 200 or not job.get("job_id"):
            break
        await asyncio.sleep(0.25)
        _, raw, _ = await asyncio.to_thread(http, ctx.base + f"/transcribe/batch/{job.get('job_id')}")
        job = json.loads(raw) if raw[:1] == b"{" else {}
    row["latency_ms"] = ms(now() - t)
    res = job.get("result") or {}
    text = (res.get("text") or res.get("transcript") or "") if isinstance(res, dict) else ""
    row.update(job_status=job.get("status"), hypothesis=text, warnings=job.get("warnings") or job.get("config_warnings"))
    row["metric"], row["wer"] = error_rate(clip["reference"], text, language)
    if r["status"] != 200 or job.get("error"):
        row["error"] = str(job.get("error") or job)[:300]
    row["pass"] = r["status"] == 200 and row["wer"] is not None and row["wer"] <= (dm.WORDS_OK if language == "en" else ctx.args.fleurs_ok)
    if not direct:
        return [row]
    d = await asyncio.to_thread(vendor_transcribe, ctx, vendor, pcm, rate, language, model)
    drow = {"case": "stt_batch_direct", "model": model, "route": "direct", "status": d["status"], "latency_ms": d["latency_ms"],
            "hypothesis": d.get("text")}
    drow["metric"], drow["wer"] = error_rate(clip["reference"], d.get("text", "") if d["status"] == 200 else "", language)
    drow["pass"] = d["status"] == 200
    if d["status"] == 200 and row.get("latency_ms") is not None:
        row["overhead_ms"] = row["latency_ms"] - d["latency_ms"]
    return [row, drow]


# ---- probes, realtime, interaction

async def ws_probe(ctx: Ctx, path: str, config: dict, label: str, wait: float = 20, hold: float = 1.0) -> dict:
    s = WsSession(ws_url(ctx.base, path), label)
    row: Dict[str, Any] = {"case": label, "route": "gateway", "path": path}
    try:
        await s.open(config, wait)
        await asyncio.sleep(hold)
        row.update(ready=bool(s.ready), errors=error_text(s.errors()), closed=s.closed_reason,
                   warnings=[{"code": w.get("code"), "message": str(w.get("message", ""))[:200]} for w in s.warnings()],
                   events=[e.get("type") for _, e in s.events][:12])
    except Exception as e:
        row["error"] = f"{type(e).__name__}: {e}"[:300]
    finally:
        if s.ws:
            await s.close()
    return row


async def voices_list(ctx: Ctx, vendor: str) -> dict:
    """GET /voices: 200 with the vendor's voices, or the vendor named with its reason (a 502 body
    when no vendor answered, else the x-voices-failed header)."""
    st, body, hdrs = await asyncio.to_thread(http, ctx.base + "/voices")
    failed = {k.lower(): v for k, v in hdrs.items()}.get("x-voices-failed", "")
    try:
        doc = json.loads(body)
    except ValueError:
        doc = {}
    if st == 502:
        failed = json.dumps(doc.get("providers", {}))
        doc = {}
    listed = {k: len(v) for k, v in doc.items() if isinstance(v, list)}
    named = vendor in failed
    return {"case": "voices_list", "route": "gateway", "status": st, "listed": listed, "failed": failed[:300],
            "voices": [v.get("id") or v.get("voice_id") for v in doc.get(vendor, []) if isinstance(v, dict)],
            "pass": listed.get(vendor, 0) > 0 or named,
            "note": "the vendor refused the list and the gateway says so" if named else
                    ("" if listed.get(vendor) else "the gateway does not list this vendor's voices")}


async def probes(ctx: Ctx, vendor: str) -> List[dict]:
    rows = []
    va = VENDORS[vendor]["voices"][0]
    stt_m, tts_m = VENDORS[vendor]["inter"]
    rows.append(await voices_list(ctx, vendor))
    st, body, _ = await asyncio.to_thread(http, ctx.base + "/capabilities/features")
    rows.append({"case": "capabilities_features", "route": "gateway", "status": st,
                 "mentions_vendor": vendor in body.decode("utf-8", "replace"), "pass": st == 200 and vendor in body.decode()})
    if vendor == "elevenlabs":
        r = await ws_probe(ctx, "/ws", {"type": "config", "stt_config": stt_cfg(vendor, "scribe_v2", "en"),
                                        "tts_config": tts_cfg(vendor, tts_m, va)}, "stt_ws_batch_model_refused")
        r["expect"] = "a clear refusal naming the batch model and the realtime model"
        r["pass"] = not r.get("ready") and "not a realtime model" in (r.get("errors") or "")
        rows.append(r)
        en = clip_of(ctx, "en")
        pcm, rate = load_wav(Path(en["wav"]))
        st, raw, _ = await asyncio.to_thread(http, ctx.base + "/transcribe/batch", "POST", {
            "provider": vendor, "api_key": "", "model": "scribe_v2_realtime", "language": "en", "sample_rate": rate,
            "channels": 1, "punctuation": True, "encoding": "linear16", "audio_base64": base64.b64encode(wav_bytes(pcm[:rate * 2], rate)).decode(),
            "content_type": "audio/wav"})
        rows.append({"case": "batch_realtime_model_refused", "route": "gateway", "status": st,
                     "body": raw[:300].decode("utf-8", "replace"), "pass": st in (400, 422) and b"realtime" in raw})
    else:
        st, raw, _ = await asyncio.to_thread(http, ctx.base + "/transcribe/batch", "POST", {
            "provider": vendor, "api_key": "", "model": "ink-whisper", "language": "en", "sample_rate": RATE, "channels": 1, "punctuation": True,
            "encoding": "linear16", "audio_base64": base64.b64encode(wav_bytes(bytes(RATE), RATE)).decode(),
            "content_type": "audio/wav"})
        rows.append({"case": "batch_unsupported_vendor", "route": "gateway", "status": st,
                     "body": raw[:300].decode("utf-8", "replace"), "pass": st in (400, 422, 501)})
        r = await ws_probe(ctx, "/realtime", {"type": "config", "provider": "cartesia", "model": "sonic-3"}, "realtime_cartesia_refused")
        r["pass"] = not r.get("ready") and bool(r.get("errors") or r.get("closed"))
        rows.append(r)
    # features.streaming: no vendor WebSocket in the gateway; it must still speak (REST) and should say so.
    if not ctx.stopped[vendor]:
        r = await tts_ws(ctx, vendor, tts_m, va, "This is an endpoint check.", "en", streaming=True)
        r["fallback_warned_to_client"] = any("stream" in str(w).lower() for w in r.get("warnings") or [])
        rows.append(r)
    st, raw, _ = await asyncio.to_thread(http, ctx.base + "/v1/audio/speech", "POST",
                                         {"model": tts_m, "input": "probe", "voice": va})
    rows.append({"case": "openai_audio_speech_standalone", "route": "gateway", "status": st,
                 "body": raw[:200].decode("utf-8", "replace"), "note": "Bud control-plane route; standalone has no endpoint table"})
    return rows


async def realtime_cells(ctx: Ctx) -> List[dict]:
    agent = ctx.args.el_agent or "agent_provider_matrix_probe"
    rows = []
    for name in ("elevenlabs", "elevenlabs_convai", "elevenlabs-convai", "11labs"):
        r = await ws_probe(ctx, "/realtime", {"type": "config", "provider": name, "model": agent,
                                               "instructions": dm.INSTRUCTIONS}, f"realtime_{name}",
                           hold=ctx.args.realtime_hold if name == "elevenlabs" else 1.0)
        r.update(provider=name, agent_id=agent, real_agent=bool(ctx.args.el_agent))
        rows.append(r)
    row: Dict[str, Any] = {"case": "realtime_direct", "route": "direct", "agent_id": agent}
    t = now()
    try:
        async with websockets.connect(f"wss://api.elevenlabs.io/v1/convai/conversation?agent_id={agent}",
                                      additional_headers=headers(ctx, "elevenlabs"), open_timeout=15) as ws:
            row["connect_ms"] = ms(now() - t)
            try:
                m = await asyncio.wait_for(ws.recv(), 8)
                row["first_message"] = str(m)[:300]
            except Exception as e:
                row["close"] = f"{type(e).__name__}: {e}"[:300]
    except websockets.exceptions.InvalidStatus as e:
        row.update(status=e.response.status_code, body=(e.response.body or b"")[:300].decode("utf-8", "replace"))
    except Exception as e:
        row["error"] = ctx.redact(f"{type(e).__name__}: {e}")[:300]
    rows.append(row)
    st, raw, _ = await asyncio.to_thread(http, f"{EL_API}/v1/convai/agents", "GET", None, headers(ctx, "elevenlabs"), 20)
    rows.append({"case": "convai_agents_list_direct", "route": "direct", "status": st, "body": raw[:300].decode("utf-8", "replace")})
    return rows


def inter_stt(ctx: Ctx, vendor: str, language: str) -> dict:
    """The session's recogniser: the vendor's, or (`--inter-stt local`) the local streaming one."""
    if ctx.args.inter_stt == "deepgram":  # a vendor recogniser with no cap here (its key in the gateway's env)
        return stt_cfg("deepgram", "nova-3", language)
    if ctx.args.inter_stt == "local":
        return {**stt_cfg("deepgram", "nova-2", language), "api_key": "local-dummy",
                "extras": {"endpoint_override": f"ws://127.0.0.1:{ctx.args.local_stt_port}"}}
    return stt_cfg(vendor, VENDORS[vendor]["inter"][0], language)


async def interaction_case(ctx: Ctx, vendor: str, language: str = "en", label: str = "interaction") -> dict:
    """One interaction-mode session in `language` (a language with a pack): ready with the mode
    on, the listening sounds built in the voice, one question answered."""
    stt_m, tts_m = VENDORS[vendor]["inter"]
    voice = VENDORS[vendor]["voices"][0]
    wav = ctx.data.request(language, "q_capital") or ctx.data.request(language, "slow_lookup")
    pcm = resample(*load_wav(wav), RATE)
    row: Dict[str, Any] = {"case": label, "route": "gateway", "stt": stt_m if ctx.args.inter_stt == "vendor" else "local",
                           "model": tts_m, "voice": voice, "language": language,
                           "request": ctx.data.request_text(language, "q_capital") or ctx.data.request_text(language, "slow_lookup")}
    cfg = {"type": "config", "stt_config": inter_stt(ctx, vendor, language), "tts_config": tts_cfg(vendor, tts_m, voice, language),
           "conversation_config": {"base_url": ctx.args.llm_url, "model": ctx.args.llm_model, "api_key": "local-dummy",
                                   "system_prompt": dm.INSTRUCTIONS},
           "interaction": True}
    s = WsSession(ws_url(ctx.base, "/ws"), f"inter-{vendor}")
    log = ctx.args.gateway_log
    try:
        await s.open(cfg, 45)
        row["warnings"] = [{"code": w.get("code"), "message": str(w.get("message", ""))[:160]} for w in s.warnings()]
        if not s.ready:
            row["error"] = error_text(s.errors()) or f"no ready ({s.closed_reason})"
            return row
        sid = s.ready.get("stream_id", "?")
        row.update(stream_id=sid, interaction_on=bool(s.ready.get("interaction")))
        deadline = now() + 25
        while log and now() < deadline and not dm.log_lines(log, "listening sounds ready", sid):
            await s.silence(0.5, RATE)
        ready = dm.log_lines(log, "listening sounds ready", sid) if log else []
        found = re.search(r"clips=(\d+)", ready[0]) if ready else None
        row["bank_clips"] = int(found.group(1)) if found else None
        row["bank_refused"] = [re.sub(r".*?text=", "", line)[:160] for line in dm.log_lines(log, "listening sound refused", sid)]
        row["failure_lines"] = [line[-200:] for line in dm.log_lines(log, "failure line", sid)][:6]
        row["log_warnings"] = [line[-160:] for line in dm.log_lines(log, "WARN", sid)][:6]
        mark = len(s.events)
        t0, _ = await s.stream(pcm, RATE)
        end = t0 + voiced_span(pcm, RATE)[1]

        async def mic():
            while True:
                await s.silence(0.5, RATE)
        m = asyncio.create_task(mic())
        await s.wait(lambda e: e.get("type") in ("task_result", "assistant_transcript"), 40, mark)
        # then until nothing new (events or agent audio) for 4 s, at most 30 s
        quiet_deadline, last, quiet = now() + 30, len(s.events) + len(s.audio), now()
        while now() < quiet_deadline and now() - quiet < 4.0:
            await asyncio.sleep(0.2)
            if len(s.events) + len(s.audio) != last:
                last, quiet = len(s.events) + len(s.audio), now()
        m.cancel()
        ev = s.events[mark:]
        lines = [e for _, e in ev if e.get("type") == "assistant_transcript" and (e.get("delta") or e.get("text"))]
        row["lines"] = [f"{e.get('source')}:{e.get('kind')}:{(e.get('delta') or e.get('text'))[:100]}" for e in lines]
        row["caller_text"] = " ".join(e.get("transcript", "") for _, e in ev if e.get("type") == "stt_result" and e.get("is_final"))
        row["tasks"] = [e.get("type") for _, e in ev if str(e.get("type", "")).startswith("task_")]
        first = next((a for a, d in s.audio if a > end and dbfs(d) > -50), None)
        row["first_audio_ms"] = ms(first - end) if first else None
        row["listening_sounds_played"] = sum(1 for _, e in ev if e.get("type") == "audio_segment" and e.get("lane") == "backchannel"
                                             and e.get("phase") == "start")
        row["agent_audio_s"] = round(len(s.pcm(t0)) / 2 / RATE, 2)
        row["errors"] = error_text(s.errors())
        row["event_types"] = sorted({str(e.get("type")) for _, e in ev})
        row["interaction_updates"] = [e.get("state") or e.get("degraded") for _, e in ev if e.get("type") == "interaction_updated"]
        row["pass"] = row["interaction_on"] and (row["bank_clips"] or 0) >= 1 and row["first_audio_ms"] is not None \
            and bool(lines) and not row["errors"]
    finally:
        await s.close()
        spoken = sum(len(x.split(":", 2)[-1]) for x in row.get("lines", []))
        wall = max(0.0, now() - s.opened) if s.opened else 0
        ctx.ledger(vendor, "interaction", "gateway /ws", chars=spoken + 60, audio_s=wall if ctx.args.inter_stt == "vendor" else 0,
                   note="chars: spoken lines + about 60 for the listening-sound bank; audio: session wall time (vendor STT only)")
    return row


# ---- in depth (plan section 9.4): voices, languages, latency, failures

# ElevenLabs' premade voices (the key may not read /v2/voices, nor may the gateway's GET /voices).
EL_PREMADE = {"Rachel": "21m00Tcm4TlvDq8ikWAM", "Aria": "9BWtsMINqrJLrRacOk9x", "Roger": "CwhRBWXzGAHq8TQ4Fs17",
              "Sarah": "EXAVITQu4vr4xnSDxMaL", "Laura": "FGY2WhTYpPnrIDTdsKH5", "Charlie": "IKne3meq5aSn9XLyUdCD",
              "George": "JBFqnCBsd6RMkjVDRZzb", "Callum": "N2lVS1w4EtoT3dr4eOWO", "River": "SAz9YHcvj6GT2YYXdXww",
              "Liam": "TX3LPaxmHKxFdv7VOQHJ", "Charlotte": "XB0fDUnXU5powFXDhCwa", "Alice": "Xb7hH8MSUJpSbSDYk0k2",
              "Matilda": "XrExE9yKIg1WjnnlVkGX", "Will": "bIHbv24MWmeRgasZH58o", "Jessica": "cgSgspJ2msm6clMCkdW9",
              "Eric": "cjVigY5qzO86Huf0OWal", "Chris": "iP95p4xoKVk53GoZ742B", "Brian": "nPczCjzI2devNBz1zQrb",
              "Daniel": "onwK4e9ZLuTAKqWW03F9", "Lily": "pFZP5JQG7iQjIQuC4Bku", "Bill": "pqHfZKP75CvOlQylNhV4",
              "Adam": "pNInz6obpgDQGcFmaJgB"}
# The languages each model lists (vendor model pages; Cartesia: the gateway's provider info plus the
# sonic-3 page). FLEURS codes: fil -> tl, jv -> jw.
_ML2 = "en ja zh de hi fr ko pt it es id nl tr tl pl sv bg ro ar cs el fi hr ms sk da ta uk ru".split()
_V3 = ("af ar hy as az be bn bs bg ca ceb ny hr cs da nl en et tl fi fr gl ka de el gu ha he hi hu is id ga it ja jw "
       "kn kk ky ko lv ln lt lb mk ms ml zh mr ne no ps fa pl pt pa ro ru sr sd sk sl so es sw sv ta te th tr uk ur vi "
       "cy").split()
_SONIC = ("en fr de es pt zh ja hi it ko nl pl ru sv tr ar cs da el fi he hu id ms no ro sk th uk vi tl bg hr ta bn ka "
          "te gu kn ml mr pa").split()
LANGS = {
    "elevenlabs": {"eleven_v3": _V3, "eleven_v3_conversational": _V3, "eleven_multilingual_v2": _ML2,
                   "eleven_flash_v2_5": _ML2 + ["hu", "no", "vi"], "eleven_turbo_v2_5": _ML2 + ["hu", "no", "vi"],
                   "eleven_flash_v2": ["en"], "eleven_turbo_v2": ["en"],
                   # Scribe lists 90+ languages: every FLEURS one is tried.
                   "scribe_v2_realtime": "*", "scribe_v2": "*"},
    "cartesia": {**{m: _SONIC for m in ("sonic-3.6", "sonic-3.5", "sonic-3", "sonic-preview")},
                 "ink-whisper": "*", "ink-2": _SONIC, "ink-preview": _SONIC},
}
CAPS = {"chars": 40000, "audio_s": 3600}
VOICE_TEXT = {"en": "Thanks for calling. How can I help you today?"}


def short_text(text: str, language: str, limit: int = 110) -> str:
    """A FLEURS sentence cut at a word (unsegmented scripts: 45 characters)."""
    if language in dm.UNSEGMENTED:
        return text[:45]
    if len(text) <= limit:
        return text
    return text[:limit].rsplit(" ", 1)[0]


def spent(ctx: Ctx, vendor: str) -> Dict[str, float]:
    """What every run into these results spent (the caps hold for all of them together)."""
    f = Path(ctx.args.results) / vendor / "ledger.jsonl"
    out = {"chars": 0, "audio_s": 0.0}
    for line in (f.read_text().splitlines() if f.exists() else []):
        r = json.loads(line)
        out["chars"] += r.get("chars", 0)
        out["audio_s"] += r.get("audio_s", 0)
    return out


def within_cap(ctx: Ctx, vendor: str, chars: int = 0, audio_s: float = 0.0) -> Optional[str]:
    """Why a cell may not spend this (the per-vendor caps), or None."""
    if ctx.stopped[vendor]:
        return ctx.stopped[vendor]
    s = spent(ctx, vendor)
    if s["chars"] + chars > CAPS["chars"] or s["audio_s"] + audio_s > CAPS["audio_s"]:
        return f"spend cap: {s['chars']} characters and {s['audio_s'] / 60:.1f} audio minutes spent"
    return None


def list_voices(ctx: Ctx, vendor: str) -> List[dict]:
    """Every voice the account can use: the gateway's list, else the vendor's, else the premade set."""
    if vendor == "cartesia":
        voices, after = [], ""
        for _ in range(40):
            st, body, _ = http(f"{CA_API}/voices?limit=100" + (f"&starting_after={after}" if after else ""), "GET", None,
                               headers(ctx, vendor), 30)
            if st != 200:
                break
            doc = json.loads(body)
            voices += [{"id": v["id"], "name": v.get("name", ""), "language": v.get("language", "en"),
                        "source": "vendor GET /voices"} for v in doc.get("data", [])]
            if not doc.get("has_more"):
                break
            after = doc.get("next_page") or doc["data"][-1]["id"]
        return voices
    st, body, _ = http(f"{EL_API}/v2/voices?page_size=100", "GET", None, headers(ctx, vendor), 30)
    if st == 200:
        return [{"id": v["voice_id"], "name": v.get("name", ""), "language": "en", "source": "vendor /v2/voices"}
                for v in json.loads(body).get("voices", [])]
    return [{"id": i, "name": n, "language": "en", "source": "premade (the key cannot list voices)"}
            for n, i in EL_PREMADE.items()]


def text_for(ctx: Ctx, language: str) -> Optional[str]:
    if language in VOICE_TEXT:
        return VOICE_TEXT[language]
    if language in SENTENCES:
        return SENTENCES[language][0]
    fl = ctx.data.fleurs.get(language)
    return short_text(fl.get("raw_transcription") or fl["reference"], language) if fl else None


async def voice_cells(ctx: Ctx, vendor: str) -> List[dict]:
    """Every listed voice on every TTS model (`--voice-models` limits the models of the full
    voice list; the other models get `--voices-per-model` voices): POST /speak, audible, length,
    round-trip error rate, first audio."""
    voices = await asyncio.to_thread(list_voices, ctx, vendor)
    rows = [{"case": "voices_listed", "count": len(voices), "sources": sorted({v["source"] for v in voices}),
             "languages": sorted({v["language"] for v in voices}), "pass": bool(voices)}]
    if ctx.args.voice_limit and len(voices) > ctx.args.voice_limit:
        # Round robin over the voices' languages, so every language keeps a voice.
        by: Dict[str, List[dict]] = {}
        for v in voices:
            by.setdefault(v["language"], []).append(v)
        picked: List[dict] = []
        while len(picked) < ctx.args.voice_limit:
            for lang in sorted(by, key=lambda l: -len(by[l])):
                if by[lang] and len(picked) < ctx.args.voice_limit:
                    picked.append(by[lang].pop(0))
        rows[0]["tested"] = len(picked)
        rows[0]["untested_reason"] = f"{len(voices) - len(picked)} voices not synthesised: the per-vendor spend cap"
        voices = picked
    full = set(ctx.args.voice_models.split(",")) if ctx.args.voice_models else set(VENDORS[vendor]["tts"])
    jobs = []
    for model, multilingual in VENDORS[vendor]["tts"].items():
        if not wanted(ctx, model):
            continue
        for v in voices if model in full else voices[:ctx.args.voices_per_model]:
            lang = v["language"] if multilingual and text_for(ctx, v["language"]) else "en"
            jobs.append((model, v, lang))

    async def one(model, v, lang):
        text = text_for(ctx, lang)
        why = within_cap(ctx, vendor, len(text), 8)
        if why:
            return {"case": "voice", "model": model, "voice": v["id"], "name": v["name"], "language": lang, "skipped": why}
        r = await tts_speak(ctx, vendor, model, v["id"], text, lang)
        r.update(case="voice", name=v["name"])
        return r
    rows += await gather_limited(ctx.args.concurrency, [one(*j) for j in jobs])
    return rows


async def language_cells(ctx: Ctx, vendor: str) -> List[dict]:
    """Every language each model lists: a sentence for TTS (POST /speak, the vendor's own STT
    back), a FLEURS clip for streaming STT (/ws). Listed languages without a FLEURS clip are
    recorded as untested."""
    rows, jobs = [], []
    va = VENDORS[vendor]["voices"][0]
    for model, langs in LANGS[vendor].items():
        if not wanted(ctx, model):
            continue
        kind = "tts" if model in VENDORS[vendor]["tts"] else ("stt" if model in VENDORS[vendor]["stt"] else "batch")
        langs = sorted(ctx.data.fleurs) if langs == "*" else langs
        if ctx.args.languages:
            langs = [l for l in langs if l in ctx.args.languages.split(",")]
        for lang in langs:
            if lang not in ctx.data.fleurs and lang not in SENTENCES:
                rows.append({"case": f"lang_{kind}", "model": model, "language": lang, "skipped": "no FLEURS clip or sentence"})
            else:
                jobs.append((kind, model, lang))

    async def one(kind, model, lang):
        if kind == "tts":
            text = text_for(ctx, lang)
            why = within_cap(ctx, vendor, len(text), 12)
            if why:
                return {"case": "lang_tts", "model": model, "language": lang, "skipped": why}
            r = await tts_speak(ctx, vendor, model, va, text, ISO.get(lang, lang))
            r["case"] = "lang_tts"
            return r
        clip = ctx.data.fleurs[lang]
        why = within_cap(ctx, vendor, 0, clip.get("duration_s", 15) + 4)
        if why:
            return {"case": f"lang_{kind}", "model": model, "language": lang, "skipped": why}
        if kind == "batch":
            return (await batch_cells(ctx, vendor, model, lang, {"wav": clip["wav"], "reference": clip["reference"]},
                                      direct=False))[0] | {"case": "lang_batch"}
        r = await dm.stt_case(ctx.args, Spend(ctx, vendor), ctx.data, model, ISO.get(lang, lang), clip)
        r = {"case": "lang_stt", "route": "gateway", "first_partial_ms": r.pop("first_interim_ms", None),
             "wer": r.get("error_rate"), **r}
        r["pass"] = bool(r.get("ready")) and r.get("wer") is not None and r["wer"] <= ctx.args.fleurs_ok and not r.get("errors")
        return r
    rows += await gather_limited(ctx.args.concurrency, [one(*j) for j in jobs])
    return rows


class KeepAlive:
    """A direct vendor client on one kept-alive HTTPS connection (the gateway pools its own), so
    gateway minus direct is the gateway's overhead and not a TLS handshake."""

    def __init__(self, url: str):
        import http.client
        import urllib.parse
        self.u = urllib.parse.urlsplit(url)
        self.conn = http.client.HTTPSConnection(self.u.netloc, timeout=60)

    def post(self, path: str, body: bytes, hdrs: dict) -> dict:
        t, first, chunks = now(), None, []
        try:
            self.conn.request("POST", path, body=body, headers=hdrs)
            r = self.conn.getresponse()
            while True:
                c = r.read1(8192)
                if not c:
                    break
                first = first or now()
                chunks.append(c)
            return {"status": r.status, "ttfb_ms": ms(first - t) if first else None, "total_ms": ms(now() - t),
                    "body": b"".join(chunks)}
        except Exception as e:
            self.conn.close()
            return {"status": 0, "ttfb_ms": None, "total_ms": ms(now() - t), "body": str(e).encode()}


def direct_body(vendor: str, model: str, voice: str, text: str, path: str = "") -> tuple:
    if vendor == "elevenlabs":
        return (f"/v1/text-to-speech/{voice}{path}?output_format=pcm_{RATE}",
                json.dumps({"text": text, "model_id": model}).encode())
    return (path or "/tts/bytes", json.dumps({"model_id": model, "transcript": text, "voice": {"mode": "id", "id": voice},
                                               "language": "en", "output_format": {"container": "raw", "encoding": "pcm_s16le",
                                                                                   "sample_rate": RATE}}).encode())


async def direct_tts_ws(ctx: Ctx, vendor: str, model: str, voice: str, text: str) -> dict:
    """The vendor's TTS WebSocket (not used by the gateway): time to the first audio after the
    text is sent on an open socket, and the connect time."""
    if vendor == "elevenlabs":
        url = (f"wss://api.elevenlabs.io/v1/text-to-speech/{voice}/stream-input?model_id={model}"
               f"&output_format=pcm_{RATE}")
        first_msgs = [{"text": " ", "xi_api_key": ctx.keys[vendor]}, {"text": text + " ", "flush": True}, {"text": ""}]
    else:
        url = f"wss://api.cartesia.ai/tts/websocket?cartesia_version={CA_VERSION}"
        first_msgs = [{"model_id": model, "transcript": text, "voice": {"mode": "id", "id": voice}, "language": "en",
                       "context_id": uuid.uuid4().hex, "continue": False,
                       "output_format": {"container": "raw", "encoding": "pcm_s16le", "sample_rate": RATE}}]
    t = now()
    row: Dict[str, Any] = {"case": "tts_direct_ws", "model": model, "route": "direct"}
    try:
        hdrs = {"X-API-Key": ctx.keys[vendor]} if vendor == "cartesia" else {}
        async with websockets.connect(url, additional_headers=hdrs, max_size=None, open_timeout=15) as ws:
            row["connect_ms"] = ms(now() - t)
            sent = now()
            for m in first_msgs:
                await ws.send(json.dumps(m))
            first, n = None, 0
            while True:
                m = json.loads(await asyncio.wait_for(ws.recv(), 20))
                if m.get("audio") or (m.get("type") == "chunk" and m.get("data")):
                    first = first or now()
                    n += 1
                if m.get("isFinal") or m.get("done") or m.get("type") in ("done", "error") or m.get("error"):
                    if m.get("type") == "error" or m.get("error"):
                        row["error"] = ctx.redact(json.dumps(m))[:200]
                    break
            row.update(first_audio_ms=ms(first - sent) if first else None, chunks=n, complete_ms=ms(now() - sent))
    except Exception as e:
        row["error"] = ctx.redact(f"{type(e).__name__}: {e}")[:200]
    ctx.ledger(vendor, f"tts {model}", "direct ws", chars=len(text))
    row["pass"] = row.get("first_audio_ms") is not None
    return row


def summary(rows: List[dict], key: str) -> dict:
    v = [r.get(key) for r in rows if r.get(key) is not None]
    return {"n": len(v), "median": dm.pctl(v, 50), "p95": dm.pctl(v, 95)}


async def latency_cells(ctx: Ctx, vendor: str) -> List[dict]:
    """Every gateway route against the vendor directly, `--repeats` times (a fresh sentence each
    time: the gateway caches speech by text): median and p95, gateway minus direct."""
    stt_m, tts_m = VENDORS[vendor]["inter"]
    va = VENDORS[vendor]["voices"][0]
    n, tag = ctx.args.repeats, uuid.uuid4().hex[:4]
    texts = [f"Your table for {k + 2} is booked, reference {tag} {k}." for k in range(n)]
    if within_cap(ctx, vendor, 6 * sum(map(len, texts)), n * 2 * 6 + (n * 2 * 10 if vendor == "elevenlabs" else 0)):
        return [{"case": "latency", "skipped": within_cap(ctx, vendor, 6 * sum(map(len, texts)), 120)}]
    groups: Dict[str, List[dict]] = {k: [] for k in ("ws", "speak", "direct", "ws_streaming", "direct_stream", "direct_ws")}
    host = EL_API if vendor == "elevenlabs" else CA_API
    ka = KeepAlive(host)
    hdrs = {**headers(ctx, vendor), "Content-Type": "application/json"}
    await asyncio.to_thread(ka.post, *direct_body(vendor, tts_m, va, "Warm up."), hdrs)  # opens the connection
    stream_path = "/stream" if vendor == "elevenlabs" else "/tts/sse"
    for k, text in enumerate(texts):
        groups["ws"].append(await tts_ws(ctx, vendor, tts_m, va, text, "en"))
        groups["ws_streaming"].append(await tts_ws(ctx, vendor, tts_m, va, "Note. " + text, "en", streaming=True))
        r = await asyncio.to_thread(http_timed, ctx.base + "/speak", {"text": "Update. " + text,
                                                                       "tts_config": tts_cfg(vendor, tts_m, va, "en")})
        groups["speak"].append({"first_audio_ms": r["ttfb_ms"], "complete_ms": r["total_ms"], "status": r["status"]})
        for g, path in (("direct", ""), ("direct_stream", stream_path)):
            p, body = direct_body(vendor, tts_m, va, text, path)
            d = await asyncio.to_thread(ka.post, p, body, hdrs)
            groups[g].append({"first_audio_ms": d["ttfb_ms"], "complete_ms": d["total_ms"], "status": d["status"]})
        groups["direct_ws"].append(await direct_tts_ws(ctx, vendor, tts_m, va, text))
        for _ in range(4):
            ctx.ledger(vendor, f"tts {tts_m} latency", "gateway+direct", chars=len(text))
    rows = []
    base = summary(groups["direct"], "first_audio_ms")
    for g, rs in groups.items():
        sm = summary(rs, "first_audio_ms")
        rows.append({"case": f"latency_tts_{g}", "model": tts_m, "route": "direct" if g.startswith("direct") else "gateway",
                     "first_audio": sm, "complete": summary(rs, "complete_ms"),
                     "minus_direct_median": (sm["median"] - base["median"]) if sm["median"] is not None and base["median"] is not None else None,
                     "failures": sum(1 for r in rs if r.get("first_audio_ms") is None),
                     "pass": sm["n"] == len(rs)})
    # Streaming STT: the short question, n times through /ws and directly.
    clip = {"wav": str(ctx.data.request("en", "q_capital")), "reference": ctx.data.request_text("en", "q_capital")}
    pcm = resample(*load_wav(Path(clip["wav"])), RATE)
    gw, di = [], []
    for _ in range(n):
        r = await dm.stt_case(ctx.args, Spend(ctx, vendor), ctx.data, stt_m, "en", clip)
        gw.append({"first_partial_ms": r.get("first_interim_ms"), "final_latency_ms": r.get("final_latency_ms")})
        di.append(await direct_stt_ws(ctx, vendor, stt_m, "en", pcm))
    for label, rs, route in (("latency_stt_ws", gw, "gateway"), ("latency_stt_direct", di, "direct")):
        rows.append({"case": label, "model": stt_m, "route": route, "first_partial": summary(rs, "first_partial_ms"),
                     "final": summary(rs, "final_latency_ms"), "failures": sum(1 for r in rs if r.get("final_latency_ms") is None),
                     "pass": all(r.get("final_latency_ms") is not None for r in rs)})
    if VENDORS[vendor]["batch"]:
        en = clip_of(ctx, "en")
        nb = ctx.args.batch_repeats or n
        bg = [(await batch_cells(ctx, vendor, VENDORS[vendor]["batch"][0], "en", en, direct=True)) for _ in range(nb)]
        g, d = [b[0] for b in bg], [b[1] for b in bg]
        sg, sd = summary(g, "latency_ms"), summary(d, "latency_ms")
        rows += [{"case": "latency_batch", "model": VENDORS[vendor]["batch"][0], "route": "gateway", "whole": sg,
                  "minus_direct_median": (sg["median"] - sd["median"]) if sg["median"] and sd["median"] else None,
                  "pass": sg["n"] == nb},
                 {"case": "latency_batch_direct", "route": "direct", "whole": sd, "pass": sd["n"] == nb}]
    return rows


async def failure_cells(ctx: Ctx, vendor: str) -> List[dict]:
    """Client mistakes get the vendor's reason and do not trip the shared breaker (a good request
    right after still works); the account's concurrency limit is honoured with queueing."""
    stt_m, tts_m = VENDORS[vendor]["inter"]
    va = VENDORS[vendor]["voices"][0]
    rows = []

    async def speak(text, voice=va, key=None):
        cfg = tts_cfg(vendor, tts_m, voice, "en")
        if key:
            cfg["api_key"] = key
        st, body, hdrs = await asyncio.to_thread(http, ctx.base + "/speak", "POST", {"text": text, "tts_config": cfg})
        ctx.ledger(vendor, f"tts {tts_m} failure", "gateway /speak", chars=len(text))
        return st, ("" if st == 200 else ctx.redact(body[:300].decode("utf-8", "replace"))), hdrs

    tag = uuid.uuid4().hex[:4]
    for case, kw in (("bad_key", {"key": "pm-bad-key-" + tag}), ("unknown_voice", {"voice": "pm-no-such-voice-" + tag})):
        st, body, _ = await speak(f"Checking a {case.replace('_', ' ')} {tag}.", **kw)
        ok_st, _, _ = await speak(f"And a good request after it {tag} {case}.")
        rows.append({"case": f"fail_{case}", "route": "gateway /speak", "status": st, "body": body,
                     "good_after": ok_st, "pass": 400 <= st < 500 and ok_st == 200})
    s = WsSession(ws_url(ctx.base, "/ws"), "fail-stt-key")
    try:
        await s.open({"type": "config", "stt_config": {**stt_cfg(vendor, stt_m, "en"), "api_key": "pm-bad-key-" + tag},
                      "tts_config": tts_cfg(vendor, tts_m, va)}, 20)
        await s.silence(2.0, RATE)
        errs = error_text(s.errors())
    finally:
        await s.close()
    rows.append({"case": "fail_bad_key_stt", "route": "gateway /ws", "ready": bool(s.ready), "errors": ctx.redact(errs),
                 "pass": bool(errs) and any(w in errs.lower() for w in ("401", "auth", "key", "unauthor", "invalid"))})
    # The account's concurrency: a burst of `--burst` /speak at once.
    burst = await gather_limited(ctx.args.burst, [speak(f"Burst request {k} {tag}.") for k in range(ctx.args.burst)])
    sts = [b[0] for b in burst]
    rows.append({"case": "fail_concurrency_burst", "route": "gateway /speak", "statuses": sts,
                 "refusals": [b[1][:160] for b in burst if b[0] != 200],
                 "retry_after": [b[2].get("Retry-After") for b in burst if b[0] == 429],
                 "pass": all(x == 200 for x in sts)})
    return rows


async def sf3_case(ctx: Ctx, vendor: str) -> dict:
    """SF3 with the vendor's voice: the vendor becomes unreachable mid-call (the CONNECT proxy the
    gateway was started behind, `--sf3-proxy-pid`, is killed). Expect the stall line made at
    session start in that voice, then the exit line and `session_failed` (service voice)."""
    tts_m, voice = VENDORS[vendor]["inter"][1], VENDORS[vendor]["voices"][0]
    cfg = {"type": "config", "stt_config": inter_stt(ctx, vendor, "en"), "tts_config": tts_cfg(vendor, tts_m, voice, "en"),
           "conversation_config": {"base_url": ctx.args.llm_url, "model": ctx.args.llm_model, "api_key": "local-dummy",
                                   "system_prompt": dm.INSTRUCTIONS},
           "interaction": True}
    row: Dict[str, Any] = {"case": "sf3", "route": "gateway", "model": tts_m, "voice": voice}
    s = WsSession(ws_url(ctx.base, "/ws"), f"sf3-{vendor}")
    log = ctx.args.gateway_log
    try:
        await s.open(cfg, 45)
        if not s.ready:
            row["error"] = error_text(s.errors()) or "no ready"
            return row
        sid = s.ready.get("stream_id", "?")
        row["failure_settings"] = (s.ready.get("interaction") or {}).get("failure")
        deadline = now() + 40
        while now() < deadline and not dm.log_lines(log, "interaction failure lines ready", sid):
            await s.silence(0.5, RATE)
        row["lines_ready"] = [l[-120:] for l in dm.log_lines(log, "interaction failure lines ready", sid)]
        row["lines_refused"] = [l[-160:] for l in dm.log_lines(log, "interaction failure line refused", sid)]

        async def mic():
            while True:
                await s.silence(0.5, RATE)
        for name in ("q_capital",):
            pcm = resample(*load_wav(ctx.data.request("en", name)), RATE)
            mark = len(s.events)
            await s.stream(pcm, RATE)
            m = asyncio.create_task(mic())
            await s.wait(lambda e: e.get("type") == "assistant_transcript" and e.get("source") == "background", 30, mark)
            await asyncio.sleep(3)
            m.cancel()
        os.kill(ctx.args.sf3_proxy_pid, signal.SIGKILL)
        killed = now()
        mark = len(s.events)
        # A question whose answer is new text (a cached line would need no vendor).
        pcm = resample(*load_wav(ctx.data.request("en", "long_turn")), RATE)
        t0, _ = await s.stream(pcm, RATE)
        end = t0 + voiced_span(pcm, RATE)[1]
        m = asyncio.create_task(mic())
        failed = await s.wait(lambda e: e.get("type") == "session_failed", 90, mark)
        await asyncio.sleep(1.0)
        m.cancel()
        ev = s.events[mark:]
        segs = [(a, e) for a, e in ev if e.get("type") == "audio_segment" and e.get("kind") == "failure"]
        notes = [e.get("failure") for _, e in ev if e.get("type") == "interaction_updated" and e.get("failure")]
        starts = [(a, e) for a, e in segs if e.get("phase") == "start"]
        heard = []
        for a, e in starts:
            stop = next((b for b, x in segs if x.get("phase") == "end" and x.get("utterance_id") == e.get("utterance_id")), a + 6)
            pcm_out = s.pcm(a - 0.05, stop)
            heard.append({"at_s": round(a - end, 2), "ms": ms(len(pcm_out) / 2 / 24000), "dbfs": round(dbfs(pcm_out), 1)})
        row.update(killed_before_turn_end_s=round(end - killed, 1), failure_notes=notes, failure_audio=heard,
                   session_failed=failed[1] if failed else None, closed=s.closed_reason,
                   errors=error_text(s.errors()))
        kinds = [n.get("kind") for n in notes if isinstance(n, dict)]
        row["pass"] = bool(row["lines_ready"]) and "stall" in kinds and any(h["dbfs"] > -50 for h in heard) \
            and bool(failed) and (failed[1].get("service") == "voice")
    finally:
        await s.close()
        ctx.ledger(vendor, "sf3", "gateway /ws", chars=600, note="estimate: the failure lines rendered at session start + one answer")
    return row


# ---- stack

def port_free(port: int) -> bool:
    with socket.socket() as so:
        return so.connect_ex(("127.0.0.1", port)) != 0


class Stack:
    def __init__(self, args, ctx: Ctx):
        self.args, self.ctx, self.procs = args, ctx, {}

    def spawn(self, name, argv, env, port, wait=120.0):
        if not port_free(port):
            raise RuntimeError(f"port {port} for {name} is taken")
        log = open(Path(self.args.run_dir) / f"{name}.log", "ab")
        self.procs[name] = subprocess.Popen(argv, env=env, stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                            cwd=self.args.run_dir, start_new_session=True)
        deadline = time.time() + wait
        while time.time() < deadline:
            if self.procs[name].poll() is not None:
                raise RuntimeError(f"{name} exited {self.procs[name].returncode}; see {self.args.run_dir}/{name}.log")
            if not port_free(port) and (name == "gateway" or http(f"http://127.0.0.1:{port}/health", timeout=3)[0] == 200):
                print(f"{name} up on {port} (pid {self.procs[name].pid})")
                return
            time.sleep(0.5)
        raise RuntimeError(f"{name} not ready on {port}")

    def start(self):
        a = self.args
        base = {"PATH": os.environ.get("PATH", ""), "HOME": os.environ.get("HOME", "")}
        llama = {**base, "LD_LIBRARY_PATH": a.llama_dir, "GGML_CUDA_ENABLE_UNIFIED_MEMORY": "1"}
        common = ["-ngl", "99", "-fa", "on", "-np", "1", "--host", "127.0.0.1", "--no-webui"]
        self.spawn("front", [f"{a.llama_dir}/llama-server", "-m", a.front_model, "-c", "4096", "--port", str(a.front_port), *common],
                   llama, a.front_port)
        self.spawn("background", [f"{a.llama_dir}/llama-server", "-m", a.background_model, "-c", "8192", "--jinja", "--alias",
                                  a.llm_model, "--port", str(a.background_port), *common], llama, a.background_port)
        env = {**base, "ELEVENLABS_API_KEY": self.ctx.keys["elevenlabs"], "CARTESIA_API_KEY": self.ctx.keys["cartesia"],
               "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": a.ort, "CACHE_PATH": str(Path(a.run_dir) / "cache"),
               "HOST": "127.0.0.1", "PORT": str(a.port), "WAAV_INTERACTION_ENABLED": "1",
               "WAAV_INTERACTION_ALLOW_CLIENT_TOOLS": "1", "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{a.front_port}",
               "WAAV_INTERACTION_TOOL_SECRET": "provider-matrix-secret", "NO_COLOR": "1",
               "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug"}
        if a.account_concurrency:
            env["WAAV_TTS_ACCOUNT_CONCURRENCY"] = a.account_concurrency
        self.spawn("gateway", [a.gateway_bin], env, a.port, 120)
        time.sleep(3)

    def stop(self):
        for name, p in reversed(list(self.procs.items())):
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGTERM)
                try:
                    p.wait(15)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
            print(f"stopped {name} (pid {p.pid})")


# ---- run and tables

def wanted(ctx: Ctx, model: str) -> bool:
    return not ctx.args.models or model in ctx.args.models.split(",")


async def run_vendor(ctx: Ctx, vendor: str, parts: List[str]):
    patch_dm(vendor)
    v = VENDORS[vendor]
    print(f"== {vendor}")
    if not ctx.keys[vendor]:
        ctx.add(vendor, {"case": "setup", "error": f"no {v['env']} in the secrets file"})
        return
    if "tts" in parts:
        for rows in await gather_limited(ctx.args.concurrency, [tts_model(ctx, vendor, m, ml) for m, ml in v["tts"].items() if wanted(ctx, m)]):
            for r in (rows if isinstance(rows, list) else [rows]):
                ctx.add(vendor, r)
        # Reference only: the vendor's streaming endpoint the gateway does not use.
        m = v["inter"][1]
        ref = {"elevenlabs": "/stream", "cartesia": "/tts/sse"}[vendor]
        if not ctx.stopped[vendor] and wanted(ctx, m):
            ctx.add(vendor, await tts_direct(ctx, vendor, m, v["voices"][0], EN_TEXT, "en", {}, ref))
    if "stt" in parts:
        for rows in await gather_limited(min(2, ctx.args.concurrency), [stt_model(ctx, vendor, m, ls) for m, ls in v["stt"].items() if wanted(ctx, m)]):
            for r in (rows if isinstance(rows, list) else [rows]):
                ctx.add(vendor, r)
    if "batch" in parts:
        for r in await batch_cells(ctx, vendor):
            ctx.add(vendor, r)
    if "probe" in parts:
        for r in await probes(ctx, vendor):
            ctx.add(vendor, r)
    if "realtime" in parts and vendor == "elevenlabs":
        for r in await realtime_cells(ctx):
            ctx.add(vendor, r)
    if "inter" in parts and not ctx.stopped[vendor]:
        ctx.add(vendor, await interaction_case(ctx, vendor))
    if "packs" in parts and not ctx.stopped[vendor]:  # one session per language with a pack
        for lang in ctx.args.pack_languages.split(","):
            ctx.add(vendor, await interaction_case(ctx, vendor, lang, "inter_pack"))
    if "inter10" in parts and not ctx.stopped[vendor]:  # concurrent interaction sessions
        rs = await asyncio.gather(*(interaction_case(ctx, vendor, "en", f"inter_concurrent") for _ in range(ctx.args.sessions)))
        gaps = [r.get("first_audio_ms") for r in rs]
        for r in rs:
            ctx.add(vendor, r)
        ctx.add(vendor, {"case": "inter_concurrent_summary", "sessions": len(rs), "passed": sum(1 for r in rs if r.get("pass")),
                         "errors": [r.get("errors") or r.get("error") for r in rs if r.get("errors") or r.get("error")],
                         "turn_gap": {"p50": dm.pctl(gaps, 50), "p99": dm.pctl(gaps, 99), "max": max([g for g in gaps if g] or [None])},
                         "pass": all(r.get("pass") for r in rs)})
    if "sf3" in parts and not ctx.stopped[vendor]:
        ctx.add(vendor, await sf3_case(ctx, vendor))
    for part, cells in (("voices", voice_cells), ("langs", language_cells), ("latency", latency_cells),
                        ("fail", failure_cells)):
        if part in parts:
            for r in await cells(ctx, vendor):
                ctx.add(vendor, r)


def table(rows: List[dict], vendor: str, stopped: Optional[str]) -> str:
    out = [f"# {vendor}: provider matrix ({time.strftime('%Y-%m-%d %H:%M')})", ""]
    if stopped:
        out += [f"Stopped early: {stopped}", ""]
    out += ["| case | model | voice / language | route | pass | first audio or partial ms | final or complete ms | overhead ms "
            "| word error rate | audio / trimmed ms | detail |", "|---|---|---|---|---|---|---|---|---|---|---|"]
    for r in rows:
        first = r.get("first_audio_ms", r.get("first_partial_ms"))
        final = r.get("final_latency_ms", r.get("complete_ms", r.get("latency_ms")))
        over = r.get("overhead_ms", r.get("first_partial_overhead_ms"))
        if r.get("final_overhead_ms") is not None:
            over = f"{over} / {r['final_overhead_ms']}"
        detail = r.get("skipped") or r.get("error") or r.get("errors") or r.get("roundtrip_error") or r.get("hypothesis") \
            or r.get("roundtrip_text") or r.get("body") or r.get("listed") or r.get("lines") or ""
        vl = " / ".join(str(x) for x in (r.get("voice", "")[:8] if r.get("voice") else "", r.get("language") or "") if x)
        out.append(f"| {r.get('case')} | {r.get('model', r.get('provider', ''))} | {vl} | {r.get('route', '')} | {r.get('pass')} | "
                   f"{first} | {final} | {over if over is not None else ''} | {r.get('wer', '')} | "
                   f"{r.get('audio_ms', '')} / {r.get('trimmed_ms', '')} | {str(detail).replace('|', '/')[:160]} |")
    out += ["", "Not supported by the gateway:", ""] + [f"- {a}: {b}" for a, b in NOT_SUPPORTED[vendor]]
    return "\n".join(out) + "\n"


async def main_async(args) -> int:
    ctx = Ctx(args)
    stack = Stack(args, ctx) if args.start_stack else None
    vendors = [v.strip() for v in args.vendors.split(",") if v.strip()]
    parts = [p.strip() for p in args.parts.split(",") if p.strip()]
    try:
        if stack:
            stack.start()
        for vendor in vendors:
            await run_vendor(ctx, vendor, parts)
            d = Path(args.results) / vendor
            d.mkdir(parents=True, exist_ok=True)
            with (d / "results.jsonl").open("a") as f:
                for r in ctx.rows[vendor]:
                    f.write(json.dumps(r, ensure_ascii=False, default=str) + "\n")
            (d / "tables.md").write_text(ctx.redact(table(ctx.rows[vendor], vendor, ctx.stopped[vendor])))
            print(ctx.redact(table(ctx.rows[vendor], vendor, ctx.stopped[vendor])))
    finally:
        if stack:
            stack.stop()
    return 0


def main():
    scratch = os.environ.get("WAAV_LIVE_SCRATCH", "/nonexistent")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vendors", default="elevenlabs,cartesia")
    ap.add_argument("--parts", default="tts,stt,batch,probe,realtime,inter")
    ap.add_argument("--base-url", default="http://127.0.0.1:3461")
    ap.add_argument("--results", default=f"{scratch}/providers")
    ap.add_argument("--secrets", default=f"{scratch}/secrets")
    ap.add_argument("--data", default=f"{scratch}/deepgram", help="the Deepgram matrix work dir (request clips)")
    ap.add_argument("--concurrency", type=int, default=3)
    ap.add_argument("--tag", default="full", help="run label on every row (a rerun supersedes earlier rows of its cells)")
    ap.add_argument("--models", default="", help="only these TTS/STT/batch models")
    ap.add_argument("--batch-client-key", action="store_true", help="batch: send the vendor key in the request body")
    ap.add_argument("--realtime-hold", type=float, default=8.0, help="realtime: seconds to watch a session after it opens")
    ap.add_argument("--el-agent", default="", help="a pre-created ElevenLabs ConvAI agent id")
    ap.add_argument("--llm-url", default="http://127.0.0.1:18762/v1")
    ap.add_argument("--llm-model", default="qwen3b-q4_k_m")
    ap.add_argument("--gateway-log", default="", help="the gateway's log (default <run-dir>/gateway.log with --start-stack)")
    ap.add_argument("--start-stack", action="store_true", help="start the front and background llama-servers and the gateway")
    ap.add_argument("--run-dir", default=f"{scratch}/providers/run")
    ap.add_argument("--gateway-bin", default=f"{scratch}/providers/run/bin/waav-gateway")
    ap.add_argument("--port", type=int, default=3461)
    ap.add_argument("--front-port", type=int, default=18761)
    ap.add_argument("--background-port", type=int, default=18762)
    ap.add_argument("--llama-dir", default="/home/bud/ditto/llamacpp-build/b11200-cuda/llama-b11200")
    ap.add_argument("--front-model", default=f"{scratch}/models/Qwen3.5-2B-Q4_K_M.gguf")
    ap.add_argument("--background-model", default="/home/bud/models/gguf/qwen3b-q4_k_m.gguf")
    ap.add_argument("--ort", default=f"{scratch}/ort/onnxruntime-linux-aarch64-1.22.0/lib/libonnxruntime.so")
    ap.add_argument("--voice-models", default="", help="voices: the TTS models that get every voice (default all)")
    ap.add_argument("--voices-per-model", type=int, default=5, help="voices: voices on the other models")
    ap.add_argument("--languages", default="", help="langs: only these languages")
    ap.add_argument("--fleurs-ok", type=float, default=0.5, help="langs: the error rate a FLEURS clip passes at")
    ap.add_argument("--repeats", type=int, default=10, help="latency: requests per route")
    ap.add_argument("--batch-repeats", type=int, default=0, help="latency: batch uploads per route (default --repeats)")
    ap.add_argument("--inter-stt", choices=("vendor", "local", "deepgram"), default="vendor", help="inter*: the session's recogniser")
    ap.add_argument("--local-stt-port", type=int, default=18866, help="--inter-stt local: local_streaming_stt.py's port")
    ap.add_argument("--pack-languages", default="en,de,es,fr,it,ja,nl", help="packs: the languages with a pack")
    ap.add_argument("--sessions", type=int, default=10, help="inter10: sessions at once")
    ap.add_argument("--sf3-proxy-pid", type=int, default=0, help="sf3: the pid of the CONNECT proxy the gateway reaches the vendor through")
    ap.add_argument("--burst", type=int, default=6, help="fail: /speak requests sent at once")
    ap.add_argument("--voice-limit", type=int, default=0, help="voices: at most this many voices, spread over their languages")
    ap.add_argument("--account-concurrency", default="", help="WAAV_TTS_ACCOUNT_CONCURRENCY for the started gateway")
    args = ap.parse_args()
    if args.port == 8080:
        ap.error("port 8080 is someone else's daemon")
    if args.start_stack:
        args.base_url = f"http://127.0.0.1:{args.port}"
        args.gateway_log = args.gateway_log or str(Path(args.run_dir) / "gateway.log")
    sys.exit(asyncio.run(main_async(args)))


if __name__ == "__main__":
    main()
