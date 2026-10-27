#!/usr/bin/env python3
"""
Interaction mode, live end to end.

Runs the gateway's interaction mode against real local models: a caller's speech is streamed
to `/ws` in real time (20 ms frames, silence between utterances), and every event and audio
frame that comes back is recorded with its receive time. Scenarios L0 to L10 and their
thresholds are those of the design's test plan.

What it needs:
- the debug gateway (`cargo build --features dag-routing,turn-ensemble,noise-filter,openapi`)
  on port 3401 with WAAV_INTERACTION_ENABLED=1, WAAV_INTERACTION_ALLOW_CLIENT_TOOLS=1,
  WAAV_INTERACTION_FRONT_URL, WAAV_INTERACTION_TOOL_SECRET and debug logging for
  `waav_gateway::core::interaction` (L8 reads who decided a turn from the log),
- a front llama-server on 18741 and a background llama-server (`--jinja`) on 18742,
- an OpenAI-compatible speech-to-text and text-to-speech server on 8124,
- caller WAV files (16 kHz mono 16-bit) in `--audio-dir`.
`--start-stack` starts whichever of the first two groups is not running, and stops what it
started. WAAV_LIVE_SCRATCH names the directory that holds `audio/caller`, `logs` and `ort`.

`--stt streaming` runs the sessions on a streaming recogniser with interim results instead of
the request-response one: `local_streaming_stt.py` (started here on `--stt-port`) speaks the
Deepgram live wire over the same Whisper (or, with `--stt-stand-in parakeet`, over Parakeet TDT
0.6B v3 on the CPU in that process), and the sessions use provider `deepgram` with
`extras.endpoint_override`. Scenarios S1 to S3 and S5 (stop cue, early yes, eager end of turn,
short answer) need it; S4 (the agent's echo with each echo defence, and a caller over it) runs
on both. `--only BL` counts barge-ins by attenuated copies of the caller. Every
run ends with PUMP (each utterance made the pump report `Started` once) and LABEL (no relayed
sentence opens with a speaker label).

Beyond the design's table: W1 (what the caller hears through a 15 s tool), T1 (truth after a
late correction and after a tool fails), R1 (a hold cue and small talk start no task), C1
(the client's controls, `GET /v1/tasks` and a session whose only background is
`interaction.tasks.llm`), D1 (a request that nearly repeats a task is asked about; the presence
line), H1 (a choice and a value asked through `ask_caller`), P1 (sensitive values are declined),
G1 (a session language with no pack; the language step-down), O1 (the outbound speech policy),
K1 (a session in a LiveKit room: it starts a LiveKit server from the local image and needs the
Python packages `livekit` and `livekit-api`) and, by name with `--tts-delay-ms`, A1 (the acknowledgement when the synthesiser is slow to its
first audio) and UH1 (an answer still in synthesis when the caller goes on within 1.5 s of
their turn's end waits for it to end), X1 (interaction mode behind the OpenAI Realtime wire: an OpenAI Realtime client on
`/v1/realtime?model=prompt:<agent>` of the Bud-mode gateway B1 starts), X2 (that endpoint as a
local speech-to-speech model: audio both ways, a function tool of the client's, injected text)
and DG1 (a `/ws` DAG that ends in text as the background; one that speaks itself is refused).
GK1 runs xAI Grok on `/realtime` (its own gateway on `--grok-gateway-port`, the key GROK_API_KEY
from the environment or `--xai-env`); without a key it is reported SKIP, neither passed nor failed.
EA1 runs an ElevenLabs agent on `/realtime` (its own gateway on `--elevenlabs-gateway-port`; the key
and ELEVENLABS_AGENT_ID from `--elevenlabs-env`); without the agent id it is reported SKIP (BLOCKED).
SDK1 runs an acknowledgement and relay and a confirmation through the Python SDK
(`clients_sdk/python`, imported from the repository: it needs `pydantic` and `httpx`).
DT1 to DT3 are the tasks that outlive the socket: they start two more gateway processes on
`--task-gateway-ports` that share a task Redis container on `--task-redis-port` (it needs
`docker`), and DT3 kills one of them. TD1 counts the loads of the voice activity model with
`stt_config.turn_detection` on: once for an interaction session, once without the mode.
An unclear turn has no scenario: the local Whisper is as sure of speech played
backwards as of a clear "okay", so no real recogniser here reports a low confidence. The clips of D1 to P1 are
synthesised with the speech server when they are not in `--audio-dir`; VENDOR_CLIPS (56_story, CT7's Spanish caller)
once with a Deepgram Aura voice through the controls gateway's `/speak` (it needs the Deepgram key). A clip whose
shape a scenario relies on (CLIP_SHAPES: 56_story's three comma pauses of 320 to 380 ms, 16_book_and_flowers in one
breath) is put in that shape when it is loaded, a local one re-synthesised up to CLIP_TRIES times when it cannot
be, and refused (the scenario does not run to the end) after that: the local Piper differs on every synthesis.

E is the time the final `stt_result` of a turn is received. All times are receive times at
this client. Events, received levels and the agent's audio of every session are written to
`--out-dir`. The script exits non-zero when a threshold fails.
"""

import argparse
import asyncio
import base64
import hashlib
import hmac
import json
import math
import os
import queue
import random
import re
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import wave
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

try:
    import websockets
except ImportError:
    print("Please install websockets: pip install websockets")
    sys.exit(1)

try:
    import numpy as np
except ImportError:
    print("Please install numpy: pip install numpy")
    sys.exit(1)

from local_streaming_stt import transcribe

REPO_GATEWAY = Path(__file__).resolve().parents[3]

CALLER_RATE = 16000
AGENT_RATE = 24000
FRAME_MS = 20
CALLER_FRAME = CALLER_RATE * FRAME_MS // 1000 * 2  # bytes
AGENT_FRAME = AGENT_RATE * FRAME_MS // 1000 * 2  # bytes
# A received 20 ms frame above this level counts as agent audio.
AUDIBLE_DB = -50.0
# The development keys of a LiveKit server started with `--dev`.
LIVEKIT_KEY, LIVEKIT_SECRET = "devkey", "secret"
# The room data topic of a LiveKit session's interaction events.
ROOM_TOPIC = "waav.interaction"
PROTOCOL_1_1 = {
    "task_created", "task_progress", "task_question", "task_answer_received", "task_result",
    "task_cancelled", "floor_state", "agent_audio_stopped", "interaction_updated",
}


def now() -> float:
    return time.monotonic()


def ms(seconds: Optional[float]) -> Optional[int]:
    return None if seconds is None else round(seconds * 1000)


def port_open(port: int) -> bool:
    with socket.socket() as sock:
        sock.settimeout(0.5)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def http_ok(url: str, needle: str = "") -> bool:
    try:
        with urllib.request.urlopen(url, timeout=3) as response:
            return needle in response.read().decode("utf-8", "replace")
    except Exception:
        return False


def get_json(url: str, timeout: float = 10) -> dict:
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return json.loads(response.read())


def get_status(url: str, timeout: float = 10) -> Tuple[int, dict]:
    """The status and the JSON body of a GET, also when the status is an error."""
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        return error.code, {}


def post_json(url: str, body: dict, timeout: float = 120) -> dict:
    request = urllib.request.Request(
        url, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read())


def frame_db(pcm: bytes) -> float:
    samples = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype="<i2").astype(np.float64)
    if samples.size == 0:
        return -100.0
    rms = math.sqrt(float(np.mean(samples * samples)))
    return max(-100.0, 20 * math.log10(rms / 32768.0)) if rms > 0 else -100.0


def median_f0(pcm: bytes, rate: int = AGENT_RATE) -> float:
    """The median pitch (Hz) of the voiced 40 ms frames (autocorrelation): a male voice is
    about 85-155 Hz, a female one 165-255 Hz. 0 without voiced frames."""
    samples = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype="<i2").astype(np.float64)
    size, pitches = int(rate * 0.04), []
    for start in range(0, samples.size - size, size):
        frame = samples[start:start + size] - samples[start:start + size].mean()
        if math.sqrt(float(np.mean(frame * frame))) < 500:
            continue
        corr = np.correlate(frame, frame, "full")[size - 1:]
        low, high = rate // 400, rate // 60
        lag = low + int(np.argmax(corr[low:high]))
        if corr[lag] > 0.3 * corr[0]:
            pitches.append(rate / lag)
    return round(float(np.median(pitches)), 1) if pitches else 0.0


def percentile(values: List[float], q: float) -> float:
    return float(np.percentile(values, q)) if values else -100.0


def load_wav(path: Path) -> bytes:
    with wave.open(str(path), "rb") as wav:
        if (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) != (CALLER_RATE, 1, 2):
            raise ValueError(f"{path}: caller audio must be 16 kHz mono 16-bit")
        return wav.readframes(wav.getnframes())


def resample(pcm: bytes, src: int, dst: int) -> bytes:
    samples = np.frombuffer(pcm[: len(pcm) // 2 * 2], dtype="<i2").astype(np.float64)
    if samples.size == 0:
        return b""
    count = int(samples.size * dst / src)
    at = np.linspace(0, samples.size - 1, count)
    return np.interp(at, np.arange(samples.size), samples).astype("<i2").tobytes()


def to_caller_rate(frame: bytes) -> bytes:
    """One frame of agent audio at the caller's rate, each sample at its own time."""
    samples = np.frombuffer(frame, dtype="<i2").astype(np.float64)
    at = np.arange(samples.size * CALLER_RATE // AGENT_RATE) * (AGENT_RATE / CALLER_RATE)
    return np.interp(at, np.arange(samples.size), samples).astype("<i2").tobytes()


def last_voiced_ms(pcm: bytes) -> int:
    """The end of the last 20 ms frame of caller audio that holds speech."""
    last = 0
    for start in range(0, len(pcm), CALLER_FRAME):
        if frame_db(pcm[start:start + CALLER_FRAME]) > -45.0:
            last = start + CALLER_FRAME
    return last // 2 * 1000 // CALLER_RATE


# ---- the stack: model servers and the gateway


def env_file_value(path: str, name: str) -> Optional[str]:
    """`name`'s value in a KEY=VALUE file (`export ` and quotes allowed); None without one."""
    if not path or not Path(path).is_file():
        return None
    for line in Path(path).read_text().splitlines():
        key, _, value = line.strip().removeprefix("export ").partition("=")
        if key.strip() == name and value.strip():
            return value.strip().strip("'\"")
    return None


def end_process(process: subprocess.Popen, seconds: float = 20):
    """Asks a process to stop and kills it when it has not within `seconds` (r18-verify: at a
    load average of 120 the front server took over 20 s, and FS1's TimeoutExpired left the rest
    of the run without a front)."""
    process.terminate()
    try:
        process.wait(timeout=seconds)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


class Stack:
    """Starts what is not running and stops only what it started."""

    def __init__(self, args):
        self.args = args
        self.started: Dict[str, subprocess.Popen] = {}
        # CL1's voice router, in front of the speech server of the gateway this run starts.
        self.voices: Optional["VoiceRouter"] = None
        self.foreign_front = False
        self.log_dir = Path(args.log_dir)
        self.gateway_log = Path(args.gateway_log or self.log_dir / "live-gateway.log")
        # The Bud-mode gateway's stand-in Bud gateway and its Redis container (`ensure_bud`).
        self.bud: Optional["BudStandIn"] = None
        self.redis: Optional[str] = None
        # The LiveKit server's container (`ensure_livekit`).
        self.livekit: Optional[str] = None
        # The two gateways that share a task Redis, and its container (`ensure_durable`).
        self.durable: Optional[Tuple[int, int]] = None
        self.task_redis: Optional[str] = None
        # The stand-in backends and the gateway whose catalog lists them (`ensure_backends`).
        self.backends: Optional["Backends"] = None
        # PR8: the webhook of the task gateways' catalog that calls back to the other one.
        self.relay_hook: Optional["RelayHook"] = None
        # PR12, HC2 (r4-left).
        self.person_hook: Optional["Webhook"] = None
        self.bud_env: Dict[str, str] = {}
        # The booking tool of the relay's operator block (RG3), on the run's webhook.
        self.booking_tool: dict = {}

    def _spawn(self, name: str, argv: List[str], env: Dict[str, str], log: Path, cwd: Path):
        log.parent.mkdir(parents=True, exist_ok=True)
        cwd.mkdir(parents=True, exist_ok=True)
        handle = open(log, "ab")
        self.started[name] = subprocess.Popen(
            argv, env={**os.environ, **env}, stdout=handle, stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL, cwd=str(cwd), start_new_session=True,
        )

    def _wait(self, name: str, ready: Callable[[], bool], seconds: float):
        deadline = now() + seconds
        while now() < deadline:
            if ready():
                return
            process = self.started.get(name)
            if process is not None and process.poll() is not None:
                raise RuntimeError(f"{name} exited with {process.returncode}; see the logs")
            time.sleep(0.25)
        raise RuntimeError(f"{name} is not ready after {seconds:.0f} s")

    def llama(self, name: str, port: int, model: str, extra: List[str], slots: int = 1):
        args = self.args
        argv = [
            f"{args.llama_dir}/llama-server", "-m", model, "-ngl", "99", "-fa", "on",
            "-np", str(slots), "--port", str(port), "--host", "127.0.0.1", "--no-webui", *extra,
        ]
        # Managed memory, as for the orchestrator: on this unified-memory machine a device
        # allocation fails while the page cache holds the free memory (fin-followup: the
        # background's CUDA out of memory at 10 GB free, 53 GB of page cache).
        env = {"LD_LIBRARY_PATH": args.llama_dir, "GGML_CUDA_ENABLE_UNIFIED_MEMORY": "1"}
        self._spawn(name, argv, env, self.log_dir / f"live-{name}.log", self.log_dir)
        self._wait(name, lambda: http_ok(f"http://127.0.0.1:{port}/health", '"ok"'), 90)

    def start_front(self):
        # Two slots of 4096: the gateway keeps the second for its decisions about words over
        # the agent and their primed prompt (front_model::INTERIM_SLOT).
        self.llama("front", self.args.front_port, self.args.front_model, ["-c", "8192"], slots=2)
        if self.foreign_front:
            # It replaces a server that was running before this script: it stays up.
            del self.started["front"]

    def ensure_orchestrator(self) -> int:
        """MA1's orchestrator: a large model (gpt-oss-20b) on its own llama-server, on
        `--orchestrator-port`, with four slots so several requests plan at once. Started
        once, stopped with the rest."""
        args = self.args
        port = args.orchestrator_port
        if "orchestrator" in self.started:
            return port
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--orchestrator-port)")
        argv = [
            f"{args.llama_dir}/llama-server", "-m", args.orchestrator_model, "-ngl", "99",
            "-fa", "on", "--jinja", "-np", "4", "-c", "32768", "--port", str(port),
            "--host", "127.0.0.1", "--no-webui", "--alias", "gpt-oss-20b",
        ]
        # Managed memory: on this unified-memory machine plain device allocations fail
        # (CUDA out of memory) while page cache holds what is free; managed allocations
        # let the kernel reclaim it. The 12 GB model loaded this way.
        env = {"LD_LIBRARY_PATH": args.llama_dir, "GGML_CUDA_ENABLE_UNIFIED_MEMORY": "1"}
        self._spawn("orchestrator", argv, env, self.log_dir / "live-orchestrator.log", self.log_dir)
        # Up to 15 minutes: on this shared machine a 12 GB model read cold from disk under
        # load took 5.8 and 6.8 minutes to load (fin-orch-speak).
        self._wait("orchestrator", lambda: http_ok(f"http://127.0.0.1:{port}/health", '"ok"'), 900)
        return port

    def start_background(self):
        extra = ["-c", "8192", "--jinja", "--alias", self.args.background_alias]
        self.llama("background", self.args.background_port, self.args.background_model, extra)

    def start_gateway(self):
        args = self.args
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(args.gateway_port),
            "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_ALLOW_CLIENT_TOOLS": "1",
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            "WAAV_INTERACTION_TOOL_SECRET": args.tool_secret,
            # PS1 registers a persona of the tenant (authentication is off: one tenant).
            "WAAV_INTERACTION_TENANT_PERSONAS": "1",
            # MEM1 signs caller identity tokens with this key.
            "WAAV_INTERACTION_CALLER_MEMORY_KEY": MEMORY_KEY,
            # LP1's Spanish test pack `es` (`write_test_pack`; test data, never shipped).
            "WAAV_INTERACTION_PACKS_DIR": str(write_test_pack(self.log_dir / "packs")),
            # CL1's recorded listening sounds, and the failure lines rendered before (SF1-SF4).
            "WAAV_INTERACTION_CLIPS_DIR": str(self.log_dir / "clips"),
            # The server's default set of failure lines, in the speech server's voice.
            "WAAV_INTERACTION_FAILURE_VOICE": json.dumps({
                "provider": "openai", "model": "tts-1", "voice_id": "live-failure-default",
                "audio_format": "pcm", "sample_rate": AGENT_RATE}),
            # The LiveKit server of scenario K1 (it is started when K1 runs).
            # An http address: the gateway's room API client takes no ws scheme, and its room
            # client turns http into ws itself.
            "LIVEKIT_URL": f"http://127.0.0.1:{args.livekit_port}",
            "LIVEKIT_PUBLIC_URL": f"http://127.0.0.1:{args.livekit_port}",
            "LIVEKIT_API_KEY": LIVEKIT_KEY, "LIVEKIT_API_SECRET": LIVEKIT_SECRET,
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        if args.stt == "streaming" and args.stt_vendor == "deepgram":
            # Read here and given to this gateway only, as for the controls gateway.
            key = os.environ.get("DEEPGRAM_API_KEY") or env_file_value(args.deepgram_env, "DEEPGRAM_API_KEY")
            if not key:
                raise RuntimeError("--stt-vendor deepgram needs DEEPGRAM_API_KEY or --deepgram-env")
            env["DEEPGRAM_API_KEY"] = key
        self._spawn("gateway", [args.gateway_bin], env, self.gateway_log, self.log_dir / "gateway-cwd")
        self._wait("gateway", lambda: port_open(args.gateway_port), 60)

    def start_stt(self):
        args = self.args
        argv = [sys.executable, str(Path(__file__).with_name("local_streaming_stt.py")),
                "--port", str(args.stt_port), "--whisper", args.audio_url, "--engine", args.stt_stand_in,
                "--verbose"]
        self._spawn("stt", argv, {}, self.log_dir / "live-streaming-stt.log", self.log_dir)
        self._wait("stt", lambda: port_open(args.stt_port), 20)

    def ensure(self):
        args = self.args
        if not http_ok(f"{args.audio_url}/health", '"ok"'):
            raise RuntimeError(f"the speech server on {args.audio_url} is not healthy (ENV.md section 3.2)")
        if args.stt == "streaming" and not port_open(args.stt_port):
            # A test helper of this script: started whenever it is needed.
            print(f"starting the streaming recogniser on {args.stt_port}")
            self.start_stt()
        wanted = [
            ("front", args.front_port, self.start_front),
            ("background", args.background_port, self.start_background),
            ("gateway", args.gateway_port, self.start_gateway),
        ]
        for name, port, start in wanted:
            if port_open(port):
                continue
            if not args.start_stack:
                raise RuntimeError(f"{name} is not listening on {port}; start it or pass --start-stack")
            print(f"starting {name} on {port}")
            start()

    def ensure_bud(self) -> "BudStandIn":
        """A second gateway under the Bud control plane, for voice-agent sessions: a Redis
        that holds one caller key, two speech deployments (the local speech server) and two
        agents, and the stand-in Bud gateway the agents' runs go to. Started once."""
        if self.bud is not None:
            return self.bud
        args = self.args
        if port_open(args.bud_gateway_port) or port_open(args.bud_redis_port):
            raise RuntimeError(f"port {args.bud_gateway_port} or {args.bud_redis_port} is taken")
        work = self.log_dir / "bud"
        work.mkdir(parents=True, exist_ok=True)
        key = work / "rsa.pem"
        if not key.exists():
            subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt",
                            "rsa_keygen_bits:2048", "-out", str(key)], check=True, capture_output=True)
        # A vendor credential as budapp stores it: RSA-OAEP-SHA-256, hex.
        sealed = subprocess.run(
            ["openssl", "pkeyutl", "-encrypt", "-inkey", str(key), "-pkeyopt", "rsa_padding_mode:oaep",
             "-pkeyopt", "rsa_oaep_md:sha256", "-pkeyopt", "rsa_mgf1_md:sha256"],
            input=b"local-dummy", check=True, capture_output=True).stdout.hex()
        self.redis = f"waav-ix-redis-{os.getpid()}"
        subprocess.run(["docker", "run", "-d", "--rm", "--pull", "never", "--name", self.redis, "-p",
                        f"127.0.0.1:{args.bud_redis_port}:6379", args.bud_redis_image],
                       check=True, capture_output=True)
        self._wait("redis", lambda: port_open(args.bud_redis_port), 30)
        time.sleep(1.0)

        project = "7c0c7e1d-0000-4000-8000-00000000aa01"
        prompts = {"support": "c0de0000-0000-4000-8000-000000000001",
                   "streamer": "c0de0000-0000-4000-8000-000000000002",
                   "vendor": "c0de0000-0000-4000-8000-000000000003"}
        # Each agent under its name and under its pinned version, which `/v1/realtime` names.
        grant = {f"prompt:{name}{pin}": {"prompt_id": uuid, "endpoint_id": "ep-llm", "project_id": project,
                                         "kind": "agent", "version": 1}
                 for name, uuid in prompts.items() for pin in ("", ":v1")}
        grant["__metadata__"] = {"api_key_id": "live-key", "user_id": "live-user",
                                 "api_key_project_id": project}
        # RG3's realtime deployment relays to this gateway's own GA cascade (agent `vendor`)
        # with the caller key as its vendor credential.
        sealed_key = subprocess.run(
            ["openssl", "pkeyutl", "-encrypt", "-inkey", str(key), "-pkeyopt", "rsa_padding_mode:oaep",
             "-pkeyopt", "rsa_oaep_md:sha256", "-pkeyopt", "rsa_mgf1_md:sha256"],
            input=BUD_KEY.encode(), check=True, capture_output=True).stdout.hex()
        grant["rt-vendor"] = {"endpoint_id": "ep-rt", "project_id": project}
        # RL1 (r3-s2s): a deployment whose vendor is the in-script stand-in (below).
        grant["rt-silent"] = {"endpoint_id": "ep-rt-silent", "project_id": project}
        digest = hashlib.sha256(b"bud-" + BUD_KEY.encode()).hexdigest()
        leg = {"vendor": "openai", "credential": sealed}
        pairs = {
            f"api_key:{digest}": json.dumps(grant),
            "voice_table:ep-stt": json.dumps({"ep-stt": {
                **leg, "endpoints": ["audio_transcription"], "model": "whisper-1", "language": "en"}}),
            "voice_table:ep-tts": json.dumps({"ep-tts": {
                **leg, "endpoints": ["text_to_speech"], "model": "tts-1", "voice": "alloy"}}),
            "voice_table:ep-rt": json.dumps({"ep-rt": {
                "vendor": "openai", "credential": sealed_key, "endpoints": ["realtime_session"],
                "model": "prompt:vendor", "api_base": f"http://127.0.0.1:{args.bud_gateway_port}/v1"}}),
        }
        if _VENDOR is not None:
            # RL1: the gateway reads its deployments at start, so the stand-in's is written now
            # (`run` starts the stand-in first when RL1 is chosen).
            pairs["voice_table:ep-rt-silent"] = json.dumps({"ep-rt-silent": {
                "vendor": "openai", "credential": sealed_key, "endpoints": ["realtime_session"],
                "model": "gpt-realtime", "api_base": f"http://127.0.0.1:{_VENDOR.port}/relay/v1"}})
        for name, uuid in prompts.items():
            entry = {"v": 1, "prompt_id": uuid, "prompt_name": name, "version": 1, "project_id": project,
                     "stt": {"endpoint_id": "ep-stt", "language": "en"},
                     "tts": {"endpoint_id": "ep-tts", "voice": f"live-{os.getpid()}"},
                     "greeting": {"mode": "static", "text": BUD_GREETING, "interruptible": True},
                     "session_overrides": []}
            if name == "streamer":
                # The agent's own block: budapp's field. `support` has none and takes the
                # operator's `agent_default`.
                entry["interaction"] = {"background": {"answer_marker": "ANSWER:"}}
            if name == "support":
                # What a `/v1/realtime` session may change with `bud.interaction.update`.
                entry["session_overrides"] = ["interaction.policy"]
            if name == "vendor":
                # A speech-to-speech model as an OpenAI Realtime client expects one: no
                # interaction mode, no greeting, and the session brings its own instructions
                # and function tools.
                entry["interaction"] = False
                entry["greeting"] = {"mode": "none"}
                # The voice, speed and language an OpenAI client names are taken, not refused.
                entry["session_overrides"] = ["tools", "instructions", "tts.voice", "tts.speed",
                                              "stt.language"]
            pairs[f"voice_agent:{uuid}:v1"] = json.dumps(entry)
        redis_set(args.bud_redis_port, pairs)

        bud = BudStandIn(f"http://127.0.0.1:{args.background_port}/v1", args.background_alias)
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(args.bud_gateway_port),
            "AUTH_REQUIRED": "true", "WAAV_REDIS_URL": f"redis://127.0.0.1:{args.bud_redis_port}",
            "WAAV_RSA_PRIVATE_KEY_PATH": str(key), "WAAV_LLM_BASE_URL": bud.url,
            "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_AGENT_DEFAULT": "true",
            # The agents' recogniser (`ep-stt`) is the local Whisper, a model that transcribes
            # uploaded files. Since release 1.0.0 a voice agent with automatic turn detection on
            # such a model is refused (`stt_live_unsupported`) unless the operator covers the
            # deployment with segmented speech-to-text or withdraws the refusal; these scenarios
            # test interaction mode over today's buffering client (the gateway ends each turn and
            # flushes it), so the operator's withdrawal switch keeps that path, with its warning.
            "WAAV_STT_FILE_ONLY_REFUSAL": "off",
            # Every turn is the front model's (chapter 9): without it the mode is refused.
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            # RG3: interaction mode on the relay, over the background model and a booking tool.
            "WAAV_INTERACTION_TRUTH_ACK": "openai",
            "WAAV_INTERACTION_REALTIME_DEFAULT": json.dumps({"tasks": {
                "llm": {"base_url": f"http://127.0.0.1:{args.background_port}/v1",
                        "model": args.background_alias, "api_key": "local-dummy",
                        "system_prompt": TOOLS_PROMPT},
                "tools": [self.booking_tool]},
                # RL1 (r3-s2s): a vendor that stays silent ends the relay 10 s after its first
                # stall line (30 s by default).
                "failure": {"max_wait_s": 10}}),
            # RL1: the server's default set of failure lines, in the speech server's voice.
            "WAAV_INTERACTION_FAILURE_VOICE": json.dumps({
                "provider": "openai", "model": "tts-1", "voice_id": "live-failure-default",
                "audio_format": "pcm", "sample_rate": AGENT_RATE}),
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        self._spawn("bud-gateway", [args.gateway_bin], env, self.log_dir / "live-bud-gateway.log",
                    self.log_dir / "bud-gateway-cwd")
        self._wait("bud-gateway", lambda: port_open(args.bud_gateway_port), 60)
        self.bud = bud
        self.bud_env = env
        return bud

    def ensure_bud_half(self) -> int:
        """HC2 (r4-left): a second Bud-mode gateway on `--bud-half-port`, on the same Redis and
        deployments, whose relay block (`interaction.realtime_default`) asks for the
        half-cascade (`speech.voice_owner: gateway`). Started once."""
        self.ensure_bud()
        port = self.args.bud_half_port
        if "bud-half-gateway" in self.started:
            return port
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--bud-half-port)")
        env = dict(self.bud_env)
        block = json.loads(env["WAAV_INTERACTION_REALTIME_DEFAULT"])
        block["speech"] = {"voice_owner": "gateway"}
        env.update({"PORT": str(port), "WAAV_INTERACTION_REALTIME_DEFAULT": json.dumps(block)})
        self._spawn("bud-half-gateway", [self.args.gateway_bin], env, self.log_dir / "live-bud-half-gateway.log",
                    self.log_dir / "bud-half-gateway-cwd")
        self._wait("bud-half-gateway", lambda: port_open(port), 60)
        return port

    def ensure_durable(self, push_url: str) -> Tuple[int, int]:
        """Two gateway processes that share a task Redis (`--task-redis-port`, from the local
        image), on `--task-gateway-ports`. The operator's default block of both names where a
        result nobody was there for is pushed, and gives a caller 20 s to come back. Returns
        the two ports. Started once."""
        if self.durable is not None:
            return self.durable
        args = self.args
        ports = tuple(int(port) for port in args.task_gateway_ports.split(","))
        if len(ports) != 2 or any(port_open(port) for port in (*ports, args.task_redis_port)):
            raise RuntimeError(f"ports {ports} or {args.task_redis_port} are taken (two gateway ports are needed)")
        self.task_redis = f"waav-ix-task-redis-{os.getpid()}"
        subprocess.run(["docker", "run", "-d", "--rm", "--pull", "never", "--name", self.task_redis, "-p",
                        f"127.0.0.1:{args.task_redis_port}:6379", args.bud_redis_image],
                       check=True, capture_output=True)
        self._wait("task-redis", lambda: port_open(args.task_redis_port), 30)
        time.sleep(1.0)
        # PR12 (r4-left): a caller's hand-off to a person goes to a local operator stand-in.
        self.person_hook = Webhook(args.tool_secret)
        default = {"tasks": {"push_url": push_url, "reconnect_grace_s": 20},
                   "controls": {"human": {"url": self.person_hook.url, "wait_s": PR12_WAIT_S}}}
        # PR8: a webhook whose callbacks reach the other process, as through a load balancer.
        self.relay_hook = RelayHook(f"http://127.0.0.1:{ports[1]}")
        for name, port in zip("ab", ports):
            self.start_task_gateway(name, port, default)
        self.durable = ports
        return ports

    def start_task_gateway(self, name: str, port: int, default: dict):
        args = self.args
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_ALLOW_CLIENT_TOOLS": "1",
            "WAAV_INTERACTION_TOOL_SECRET": args.tool_secret,
            "WAAV_INTERACTION_DEFAULT": json.dumps(default),
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            "WAAV_TASK_REDIS_URL": f"redis://127.0.0.1:{args.task_redis_port}/",
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        if self.relay_hook is not None:
            env.update({
                "WAAV_INTERACTION_TASK_BACKENDS": json.dumps([{
                    "name": "relayhook", "type": "webhook", "url": self.relay_hook.url,
                    "secret_env": "LIVE_HOOK_SECRET", "side_effects": "none"}]),
                "WAAV_INTERACTION_ALLOW_CLIENT_BACKENDS": "1", "LIVE_HOOK_SECRET": HOOK_SECRET,
            })
        key = f"task-gateway-{name}"
        self._spawn(key, [args.gateway_bin], env, self.log_dir / f"live-{key}.log",
                    self.log_dir / f"{key}-cwd")
        self._wait(key, lambda: port_open(port), 60)

    def ensure_backends(self) -> "Backends":
        """The stand-in backends (`Backends`) and one more gateway, on `--backend-gateway-port`,
        whose catalog lists them and lets a session name one. Started once."""
        if self.backends is not None:
            return self.backends
        args = self.args
        port = args.backend_gateway_port
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--backend-gateway-port)")
        llama = f"http://127.0.0.1:{args.background_port}"
        backends = Backends(llama, args.background_alias, port)
        backends.tool_secret = args.tool_secret
        # PR7: the A2A agent on the gRPC binding, on an interpreter with grpcio.
        grpc_python = args.grpc_python or str(Path(os.environ.get("WAAV_LIVE_SCRATCH", "/nonexistent"))
                                              / "tools" / "grpc-venv" / "bin" / "python")
        if Path(grpc_python).exists() and not port_open(args.grpc_agent_port):
            backends.grpc_log = self.log_dir / "grpc-agent-hits.jsonl"
            self._spawn("grpc-agent", [grpc_python, str(Path(__file__).with_name("a2a_grpc_standin.py")),
                                       "--port", str(args.grpc_agent_port), "--token", BACKEND_TOKEN,
                                       "--log", str(backends.grpc_log), "--answer", GRPC_ANSWER],
                        {}, self.log_dir / "grpc-agent.log", self.log_dir / "grpc-agent-cwd")
            self._wait("grpc-agent", lambda: port_open(args.grpc_agent_port), 30)
            backends.grpc_port = args.grpc_agent_port
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_ALLOW_CLIENT_TOOLS": "1",
            "WAAV_INTERACTION_ALLOW_CLIENT_BACKENDS": "1",
            # MA6: a session brings its caller's resume key.
            "WAAV_INTERACTION_ALLOW_RESUME_KEY": "1",
            # PR11: a session pushes to its own address, signed with its own secret.
            "WAAV_INTERACTION_ALLOW_CLIENT_PUSH": "1",
            "WAAV_INTERACTION_TOOL_SECRET": args.tool_secret,
            "WAAV_INTERACTION_TASK_BACKENDS": json.dumps(backends.catalog(llama)),
            # MA1-updates' narrator: the run's front model (every other session here asks for
            # the scripted front).
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            "LIVE_HOOK_SECRET": HOOK_SECRET, "LIVE_BACKEND_TOKEN": BACKEND_TOKEN,
            "LIVE_OAUTH_SECRET": OAUTH_SECRET,
            # KD1's card question runs on the webhook stand-in in a LiveKit room.
            "LIVEKIT_URL": f"http://127.0.0.1:{args.livekit_port}",
            "LIVEKIT_PUBLIC_URL": f"http://127.0.0.1:{args.livekit_port}",
            "LIVEKIT_API_KEY": LIVEKIT_KEY, "LIVEKIT_API_SECRET": LIVEKIT_SECRET,
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        self._spawn("backend-gateway", [args.gateway_bin], env, self.log_dir / "live-backend-gateway.log",
                    self.log_dir / "backend-gateway-cwd")
        self._wait("backend-gateway", lambda: port_open(port), 60)
        self.backends = backends
        return backends

    def ensure_controls_gateway(self) -> Optional[int]:
        """CT2's gateway, on `--controls-gateway-port`: the operator allows a switch to es and zh
        (answers_only), and its Deepgram key is DEEPGRAM_API_KEY of this script's environment or
        of `--deepgram-env` (a KEY=VALUE file read here and given to this gateway only, so no
        other process of the run gets it). `None` without a key. Started once."""
        args = self.args
        port = args.controls_gateway_port
        if "controls-gateway" in self.started:
            return port
        key = os.environ.get("DEEPGRAM_API_KEY") or env_file_value(args.deepgram_env, "DEEPGRAM_API_KEY")
        if not key:
            return None
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--controls-gateway-port)")
        default = {"controls": {"languages": ["es", "zh"], "answers_only": True}}
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy", "DEEPGRAM_API_KEY": key,
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": str(self.log_dir / "controls-cache"),
            # CT5: the failure lines each voice rendered are kept per voice here.
            "WAAV_INTERACTION_CLIPS_DIR": str(self.log_dir / "controls-clips"),
            # CT8 (r3-misc): the caller's language wish is kept with the verified caller.
            "WAAV_INTERACTION_CALLER_MEMORY_KEY": MEMORY_KEY,
            "HOST": "127.0.0.1", "PORT": str(port), "WAAV_INTERACTION_ENABLED": "1",
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            "WAAV_INTERACTION_DEFAULT": json.dumps(default),
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        self._spawn("controls-gateway", [args.gateway_bin], env, self.log_dir / "live-controls-gateway.log",
                    self.log_dir / "controls-gateway-cwd")
        self._wait("controls-gateway", lambda: port_open(port), 60)
        return port

    def ensure_grok_gateway(self) -> Optional[int]:
        """GK1's gateway, on `--grok-gateway-port`: its xAI key is GROK_API_KEY of this script's
        environment or of `--xai-env` (a KEY=VALUE file read here and given to this gateway only,
        so no other process of the run gets it). No provider fix is listed: Grok's are on by
        default. xAI's frames are traced to its log. `None` without a key. Started once."""
        args = self.args
        port = args.grok_gateway_port
        if "grok-gateway" in self.started:
            return port
        key = os.environ.get("GROK_API_KEY") or env_file_value(args.xai_env, "GROK_API_KEY")
        if not key:
            return None
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--grok-gateway-port)")
        env = {
            "GROK_API_KEY": key, "ORT_DYLIB_PATH": args.ort, "CACHE_PATH": str(self.log_dir / "grok-cache"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "RUST_LOG": "info,ort=warn,waav_gateway::realtime::wire=trace", "NO_COLOR": "1",
        }
        self._spawn("grok-gateway", [args.gateway_bin], env, self.log_dir / "live-grok-gateway.log",
                    self.log_dir / "grok-gateway-cwd")
        self._wait("grok-gateway", lambda: port_open(port), 60)
        return port

    def ensure_elevenlabs_gateway(self) -> Optional[int]:
        """EA1's gateway, on `--elevenlabs-gateway-port`: its ElevenLabs key is ELEVENLABS_API_KEY of
        this script's environment or of `--elevenlabs-env` (given to this gateway only). It logs
        at warn: its connect URL holds the agent id. `None` without a key. Started once."""
        args = self.args
        port = args.elevenlabs_gateway_port
        if "elevenlabs-gateway" in self.started:
            return port
        key = os.environ.get("ELEVENLABS_API_KEY") or env_file_value(args.elevenlabs_env, "ELEVENLABS_API_KEY")
        if not key:
            return None
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--elevenlabs-gateway-port)")
        env = {
            "ELEVENLABS_API_KEY": key, "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": str(self.log_dir / "elevenlabs-cache"), "HOST": "127.0.0.1", "PORT": str(port),
            "RUST_LOG": "warn", "NO_COLOR": "1",
        }
        self._spawn("elevenlabs-gateway", [args.gateway_bin], env, self.log_dir / "live-elevenlabs-gateway.log",
                    self.log_dir / "elevenlabs-gateway-cwd")
        self._wait("elevenlabs-gateway", lambda: port_open(port), 60)
        return port

    def ensure_human_gateway(self, url: str) -> int:
        """CT4's gateway, on `--human-gateway-port`: its operator block puts a caller who asks
        for a person through to `url` (`controls.human.url`, signed with the run's tool
        secret). Local speech only. Started once (with the first URL)."""
        args = self.args
        port = args.human_gateway_port
        if "human-gateway" in self.started:
            return port
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--human-gateway-port)")
        # r3-misc: the operator answers human_connected / human_unavailable on the callback
        # route (CT4); nobody within CT4_WAIT_S is the honest line.
        default = {"controls": {"human": {"url": url, "wait_s": CT4_WAIT_S}}}
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": str(self.log_dir / "human-cache"),
            "HOST": "127.0.0.1", "PORT": str(port), "WAAV_INTERACTION_ENABLED": "1",
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            "WAAV_INTERACTION_TOOL_SECRET": args.tool_secret,
            "WAAV_INTERACTION_DEFAULT": json.dumps(default),
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        self._spawn("human-gateway", [args.gateway_bin], env, self.log_dir / "live-human-gateway.log",
                    self.log_dir / "human-gateway-cwd")
        self._wait("human-gateway", lambda: port_open(port), 60)
        return port

    def log_since_of(self, name: str, needle: str, offset: int) -> List[str]:
        """As `log_since`, in the log `name` of another gateway of the run."""
        try:
            with open(self.log_dir / name, "rb") as log:
                log.seek(offset)
                fresh = log.read().decode(errors="replace")
        except OSError:
            return []
        return [line for line in fresh.splitlines() if needle in line]

    def ensure_regime(self) -> int:
        """A gateway whose `/realtime` OpenAI provider is the Bud-mode gateway's GA cascade
        (`ensure_bud`, agent `vendor`): the local speech-to-speech vendor of RG1 and RG2. The
        operator accepts the provider's own speech (`WAAV_INTERACTION_TRUTH_ACK=openai`). On
        `--regime-gateway-port`. Started once."""
        port = self.args.regime_gateway_port
        if "regime-gateway" in self.started:
            return port
        self.ensure_bud()
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--regime-gateway-port)")
        args = self.args
        env = {
            "OPENAI_REALTIME_URL": f"ws://127.0.0.1:{args.bud_gateway_port}/v1/realtime?model=prompt:vendor",
            "OPENAI_API_KEY": BUD_KEY,
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_ALLOW_CLIENT_TOOLS": "1",
            "WAAV_INTERACTION_TRUTH_ACK": "openai",
            "WAAV_INTERACTION_TOOL_SECRET": args.tool_secret,
            # r14 (owner, 24 Oct): a speech-to-speech session reads the caller's answer, a
            # cancel and a continuer with the front model; without one it runs without the mode.
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{args.front_port}",
            # RV1's room (the LiveKit server is started when RV1 runs).
            "LIVEKIT_URL": f"http://127.0.0.1:{args.livekit_port}",
            "LIVEKIT_PUBLIC_URL": f"http://127.0.0.1:{args.livekit_port}",
            "LIVEKIT_API_KEY": LIVEKIT_KEY, "LIVEKIT_API_SECRET": LIVEKIT_SECRET,
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug,"
                        "waav_gateway::handlers::ws::realtime_voice=debug", "NO_COLOR": "1",
        }
        self._spawn("regime-gateway", [args.gateway_bin], env, self.log_dir / "live-regime-gateway.log",
                    self.log_dir / "regime-gateway-cwd")
        self._wait("regime-gateway", lambda: port_open(port), 60)
        return port

    def ensure_fix_gateways(self, vendor_port: int) -> Tuple[int, int]:
        """PF1's two gateways on `--fix-gateway-ports`: Gemini Live, Deepgram Voice Agent and
        OpenAI Realtime all point at the stand-in vendor on `vendor_port`; the second lists the
        provider fixes (`WAAV_PROVIDER_FIXES`), the first none. Started once."""
        off, on = (int(p) for p in self.args.fix_gateway_ports.split(","))
        if "fix-gateway-on" in self.started:
            return off, on
        for port in (off, on):
            if port_open(port):
                raise RuntimeError(f"port {port} is taken (--fix-gateway-ports)")
        base = f"ws://127.0.0.1:{vendor_port}"
        for name, port, fixes in (("fix-gateway-off", off, ""), ("fix-gateway-on", on, PF1_FIXES)):
            env = {
                "GEMINI_REALTIME_URL": f"{base}/gemini", "DEEPGRAM_REALTIME_URL": f"{base}/deepgram",
                "OPENAI_REALTIME_URL": f"{base}/openai",
                "GEMINI_API_KEY": "live-dummy", "DEEPGRAM_API_KEY": "live-dummy",
                "OPENAI_API_KEY": "live-dummy",
                "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": self.args.ort,
                "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
                "HOST": "127.0.0.1", "PORT": str(port),
                "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_TRUTH_ACK": "gemini,deepgram",
                # r14: the front model reads the caller's words on a speech-to-speech session.
                "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{self.args.front_port}",
                "WAAV_PROVIDER_FIXES": fixes,
                # PF3 (r3-misc): a session of the second gateway turns its own fixes on.
                "WAAV_PROVIDER_FIXES_PER_SESSION": "1" if fixes else "",
                # SS1: the server's default set of failure lines, in the speech server's voice.
                "OPENAI_BASE_URL": self.args.audio_url,
                "WAAV_INTERACTION_FAILURE_VOICE": json.dumps({
                    "provider": "openai", "model": "tts-1", "voice_id": "live-failure-default",
                    "audio_format": "pcm", "sample_rate": AGENT_RATE}),
                # EL1: ElevenLabs text-to-speech at the stand-in.
                "ELEVENLABS_TTS_URL": _VENDOR.elevenlabs.url if _VENDOR else "",
                "ELEVENLABS_API_KEY": "el1-key",
                "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
            }
            self._spawn(name, [self.args.gateway_bin], env, self.log_dir / f"live-{name}.log",
                        self.log_dir / f"{name}-cwd")
        for name, port in (("fix-gateway-off", off), ("fix-gateway-on", on)):
            self._wait(name, lambda port=port: port_open(port), 60)
        return off, on

    def ensure_front_gateway(self) -> Tuple[int, int]:
        """FS1's gateway on `--front-gateway-port`: it runs its own front model's llama-server
        (`WAAV_INTERACTION_FRONT_COMMAND`, the front model of this run on the GPU) on
        `--supervised-front-port`. Returns both ports. Started once."""
        args = self.args
        port, front = args.front_gateway_port, args.supervised_front_port
        if "front-gateway" in self.started:
            return port, front
        for taken in (port, front):
            if port_open(taken):
                raise RuntimeError(f"port {taken} is taken (--front-gateway-port, --supervised-front-port)")
        command = (f"{args.llama_dir}/llama-server -m {args.front_model} -ngl 99 -fa on -np 1 -c 4096 "
                   f"--port {front} --host 127.0.0.1 --no-webui")
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "WAAV_INTERACTION_ENABLED": "1",
            "WAAV_INTERACTION_FRONT_URL": f"http://127.0.0.1:{front}",
            "WAAV_INTERACTION_FRONT_COMMAND": command,
            # The supervisor passes only PATH, LD_LIBRARY_PATH, CUDA_VISIBLE_DEVICES and HOME.
            "LD_LIBRARY_PATH": args.llama_dir,
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        log = self.log_dir / "live-front-gateway.log"
        # The log is appended to across runs: FS1 reads what this process wrote.
        self.front_log_start = log.stat().st_size if log.exists() else 0
        self._spawn("front-gateway", [args.gateway_bin], env, log, self.log_dir / "front-gateway-cwd")
        self._wait("front-gateway", lambda: port_open(port), 60)
        return port, front

    def ensure_socket_front_gateway(self, run_dir: Path) -> int:
        """FS2's gateway on `--socket-front-gateway-port`: it runs the front model's llama-server
        on its private UNIX socket (`front_model.url` `unix:`), with its run directories under
        `run_dir` (`WAAV_RUN_DIR`). Returns the port."""
        args = self.args
        port = args.socket_front_gateway_port
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--socket-front-gateway-port)")
        command = f"{args.llama_dir}/llama-server -m {args.front_model} -ngl 99 -fa on -np 1 -c 4096 --no-webui"
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "WAAV_INTERACTION_ENABLED": "1",
            "WAAV_INTERACTION_FRONT_URL": "unix:",
            "WAAV_INTERACTION_FRONT_COMMAND": command,
            "WAAV_RUN_DIR": str(run_dir),
            "LD_LIBRARY_PATH": args.llama_dir,
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        log = self.log_dir / "live-socket-front-gateway.log"
        self.socket_log_start = log.stat().st_size if log.exists() else 0
        self._spawn("socket-front-gateway", [args.gateway_bin], env, log, self.log_dir / "socket-front-gateway-cwd")
        self._wait("socket-front-gateway", lambda: port_open(port), 60)
        return port

    def start_licence_gateway(self, front_url: str, licence: Optional[str]) -> int:
        """FS3's gateway on `--licence-front-gateway-port`, whose front is an operator-run server
        at `front_url`, with or without `WAAV_FRONT_LICENCE`. Returns the port."""
        args = self.args
        port = args.licence_front_gateway_port
        if port_open(port):
            raise RuntimeError(f"port {port} is taken (--licence-front-gateway-port)")
        env = {
            "OPENAI_BASE_URL": args.audio_url, "OPENAI_API_KEY": "local-dummy",
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS": "1", "ORT_DYLIB_PATH": args.ort,
            "CACHE_PATH": os.path.expanduser("~/.cache/waav-gateway"),
            "HOST": "127.0.0.1", "PORT": str(port),
            "WAAV_INTERACTION_ENABLED": "1", "WAAV_INTERACTION_FRONT_URL": front_url,
            "RUST_LOG": "info,ort=warn,waav_gateway::core::interaction=debug", "NO_COLOR": "1",
        }
        if licence:
            env["WAAV_FRONT_LICENCE"] = licence
        self._spawn("licence-gateway", [args.gateway_bin], env, self.log_dir / "live-licence-gateway.log",
                    self.log_dir / "licence-gateway-cwd")
        self._wait("licence-gateway", lambda: port_open(port), 60)
        return port

    def kill(self, name: str):
        """Kills a process this script started, as a crash would: no signal it can handle."""
        process = self.started.pop(name)
        process.kill()
        process.wait(timeout=20)

    def ensure_livekit(self):
        """A LiveKit server in development mode on loopback ports (`--livekit-port` and the
        two after it), from the local image. Started once."""
        if self.livekit is not None:
            return
        port = self.args.livekit_port
        if any(port_open(port + n) for n in range(2)):
            raise RuntimeError(f"port {port} or {port + 1} is taken")
        config = (f"port: {port}\nbind_addresses: [\"127.0.0.1\"]\nrtc:\n  tcp_port: {port + 1}\n"
                  f"  udp_port: {port + 2}\n  use_external_ip: false\n  node_ip: 127.0.0.1\n")
        self.livekit = f"waav-ix-livekit-{os.getpid()}"
        subprocess.run(["docker", "run", "-d", "--rm", "--pull", "never", "--network", "host", "--name",
                        self.livekit, "-e", f"LIVEKIT_CONFIG={config}", self.args.livekit_image, "--dev"],
                       check=True, capture_output=True)
        self._wait("livekit", lambda: http_ok(f"http://127.0.0.1:{port}/", "OK"), 30)

    def stop_front(self) -> bool:
        """Stops the front model server. Only one this script started, or with the flag."""
        process = self.started.pop("front", None)
        if process is not None:
            end_process(process)
        elif self.args.allow_stop_front:
            self.foreign_front = True
            listing = subprocess.run(["ss", "-ltnpH"], capture_output=True, text=True).stdout
            found = re.search(rf":{self.args.front_port} .*pid=(\d+)", listing)
            if not found:
                return not port_open(self.args.front_port)
            os.kill(int(found.group(1)), signal.SIGTERM)
        else:
            return False
        deadline = now() + 20
        while port_open(self.args.front_port) and now() < deadline:
            time.sleep(0.2)
        return not port_open(self.args.front_port)

    def stop(self, keep: Tuple[str, ...] = ()):
        for name, process in list(self.started.items()):
            if name in keep:
                continue
            end_process(process)
            del self.started[name]
        if self.redis and not keep:
            subprocess.run(["docker", "stop", self.redis], capture_output=True)
            self.redis = None
        if self.task_redis and not keep:
            subprocess.run(["docker", "stop", self.task_redis], capture_output=True)
            self.task_redis = None
        if self.livekit and not keep:
            with open(self.log_dir / "live-livekit.log", "wb") as log:
                subprocess.run(["docker", "logs", self.livekit], stdout=log, stderr=subprocess.STDOUT)
            subprocess.run(["docker", "stop", self.livekit], capture_output=True)
            self.livekit = None

    def summaries(self) -> Dict[str, Dict[str, str]]:
        """The gateway's `interaction_session_summary` lines, by stream id."""
        found: Dict[str, Dict[str, str]] = {}
        try:
            text = self.gateway_log.read_text(errors="replace")
        except OSError:
            return found
        for line in text.splitlines():
            if "interaction_session_summary" in line:
                fields = dict(re.findall(r"(\w+)=(\S+)", line))
                found[fields.get("stream_id", "?")] = {**fields, "line": line.strip()}
        return found

    def log_size_of(self, name: str) -> int:
        log = self.log_dir / name
        return log.stat().st_size if log.exists() else 0

    def log_size(self) -> int:
        try:
            return self.gateway_log.stat().st_size
        except OSError:
            return 0

    def log_count(self, needle: str, offset: int) -> Tuple[int, int]:
        """How many log lines written from `offset` on hold `needle`, and the new offset."""
        try:
            with open(self.gateway_log, "rb") as log:
                log.seek(offset)
                fresh = log.read()
        except OSError:
            return 0, offset
        whole = fresh[:fresh.rfind(b"\n") + 1]
        return whole.count(needle.encode()), offset + len(whole)

    def log_since(self, needle: str, offset: int) -> List[str]:
        """The log lines written from `offset` on that hold `needle`."""
        try:
            with open(self.gateway_log, "rb") as log:
                log.seek(offset)
                fresh = log.read().decode(errors="replace")
        except OSError:
            return []
        return [line for line in fresh.splitlines() if needle in line]

    def log_lines(self, needle: str, stream_id: str) -> List[str]:
        try:
            text = self.gateway_log.read_text(errors="replace")
        except OSError:
            return []
        return [line for line in text.splitlines() if needle in line and stream_id in line]

    def started_counts(self, stream_id: str) -> Dict[str, int]:
        """How often the pump reported `Started` for each utterance of a session."""
        counts: Dict[str, int] = {}
        for line in self.log_lines("interaction pump event", stream_id):
            found = re.search(r"event=Started \{ utt: UttId\((\d+)\)", line)
            if found:
                counts[found.group(1)] = counts.get(found.group(1), 0) + 1
        return counts


# ---- a synthesiser that is slow to its first audio


class SlowSpeech:
    """A proxy in front of the speech server that holds back every synthesis of
    `SENTENCE_CHARS` characters or more by `delay` seconds. An acknowledgement is shorter (a
    clip has at most 24 characters), so only answers and other sentences are slow."""

    SENTENCE_CHARS = 25

    def __init__(self, upstream: str, delay: float):
        outer = self
        self.upstream, self.delay = upstream, delay

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def forward(self, body: Optional[bytes]):
                if body and self.path.endswith("/audio/speech"):
                    try:
                        text = json.loads(body).get("input", "")
                    except ValueError:
                        text = ""
                    if len(text) >= outer.SENTENCE_CHARS:
                        time.sleep(outer.delay)
                headers = {"Content-Type": self.headers.get("Content-Type", "application/json")}
                request = urllib.request.Request(outer.upstream + self.path, data=body, headers=headers)
                try:
                    with urllib.request.urlopen(request, timeout=120) as response:
                        status, kind, data = response.status, response.headers.get("Content-Type"), response.read()
                except urllib.error.HTTPError as error:
                    status, kind, data = error.code, error.headers.get("Content-Type"), error.read()
                self.send_response(status)
                self.send_header("Content-Type", kind or "application/octet-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                self.forward(None)

            def do_POST(self):
                self.forward(self.rfile.read(int(self.headers.get("Content-Length", "0"))))

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


# ---- a second local voice (CL1)


class VoiceRouter:
    """A proxy in front of the speech server: a synthesis whose voice ends in `-<name>` of
    `voices` (name -> Piper voice model) is made by a Piper process of that model (22.05 kHz,
    returned as 24 kHz PCM); every other request goes to the speech server. It keeps the PCM of
    every synthesis by voice, so CL1 can tell which voice a clip the gateway played came from."""

    def __init__(self, upstream: str, voices: Dict[str, str], piper: str, workdir: Path):
        outer = self
        self.upstream, self.voices, self.piper, self.workdir = upstream, voices, piper, workdir
        self.rendered: Dict[str, List[Tuple[str, bytes]]] = {}
        self.processes: Dict[str, subprocess.Popen] = {}
        # Voices whose Piper was killed (SF3): they stay down until revived.
        self.down: set = set()
        self.lock = threading.Lock()
        workdir.mkdir(parents=True, exist_ok=True)

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def reply(self, status: int, kind: str, data: bytes):
                try:
                    self.send_response(status)
                    self.send_header("Content-Type", kind)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass  # the gateway dropped a synthesis it no longer wanted (a cut)

            def forward(self, body: Optional[bytes]):
                speech = body is not None and self.path.endswith("/audio/speech")
                request = json.loads(body) if speech else {}
                voice, text = request.get("voice", ""), request.get("input", "")
                name = next((n for n in outer.voices if voice.endswith(f"-{n}")), None)
                if name is not None and name in outer.down:
                    self.reply(503, "application/json", b'{"error": "the voice is down"}')
                    return
                if name is not None:
                    pcm, rate = outer.piper_pcm(name, text)
                    pcm = resample(pcm, rate, AGENT_RATE)
                    outer.rendered.setdefault(voice, []).append((text, pcm))
                    self.reply(200, "audio/pcm", pcm)
                    return
                headers = {"Content-Type": self.headers.get("Content-Type", "application/json")}
                upstream = urllib.request.Request(outer.upstream + self.path, data=body, headers=headers)
                try:
                    with urllib.request.urlopen(upstream, timeout=120) as response:
                        status, kind, data = response.status, response.headers.get("Content-Type"), response.read()
                except urllib.error.HTTPError as error:
                    status, kind, data = error.code, error.headers.get("Content-Type"), error.read()
                if speech and status == 200:
                    outer.rendered.setdefault(voice, []).append((text, data))
                self.reply(status, kind or "application/octet-stream", data)

            def do_GET(self):
                self.forward(None)

            def do_POST(self):
                self.forward(self.rfile.read(int(self.headers.get("Content-Length", "0"))))

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def piper_pcm(self, name: str, text: str) -> Tuple[bytes, int]:
        """`text` in voice `name`: mono PCM16 and its rate."""
        with self.lock:
            process = self.processes.get(name)
            if process is None or process.poll() is not None:
                env = dict(os.environ, LD_LIBRARY_PATH=str(Path(self.piper).parent))
                process = subprocess.Popen(
                    [self.piper, "--model", self.voices[name], "--json-input", "--output_dir",
                     str(self.workdir), "--quiet"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, env=env, text=True, bufsize=1)
                self.processes[name] = process
            path = self.workdir / f"{name}-{os.urandom(4).hex()}.wav"
            process.stdin.write(json.dumps({"text": " ".join(text.split()), "output_file": str(path)}) + "\n")
            process.stdin.flush()
            process.stdout.readline()
        with wave.open(str(path), "rb") as wav:
            pcm, rate = wav.readframes(wav.getnframes()), wav.getframerate()
        path.unlink()
        return pcm, rate

    def kill(self, name: str) -> Optional[int]:
        """SF3: kills the Piper of voice `name` (SIGKILL) and keeps it down; its pid."""
        with self.lock:
            self.down.add(name)
            process = self.processes.pop(name, None)
        if process is None:
            return None
        process.kill()
        process.wait(timeout=10)
        return process.pid

    def revive(self, name: str):
        """The voice's Piper is started again with its next synthesis."""
        with self.lock:
            self.down.discard(name)

    def close(self):
        self.server.shutdown()
        for process in self.processes.values():
            process.stdin.close()
            process.wait(timeout=10)


# ---- the tool webhook


class Webhook:
    """An HTTP tool endpoint in this process: checks a signature when there is one, records every call."""

    def __init__(self, secret: str, slow_seconds: float = 6.0):
        self.secret = secret
        # Seconds a tool takes before it answers.
        self.delay: Dict[str, float] = {"check_availability": slow_seconds}
        # Calls from this index on are answered with HTTP 500 (the work of an earlier one stands).
        self.fail_from: Optional[int] = None
        self.calls: List[dict] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                stamp = self.headers.get("X-WaaV-Timestamp", "")
                signed = self.headers.get("X-WaaV-Signature")
                mac = hmac.new(outer.secret.encode(), f"{stamp}.".encode() + body, hashlib.sha256)
                # The gateway signs only the tools of the server's own block. These tools are
                # the session's, so a call comes unsigned; a signature that is there must be right.
                signature_ok = signed is None or hmac.compare_digest(signed, "sha256=" + mac.hexdigest())
                try:
                    request = json.loads(body)
                except ValueError:
                    request = {}
                call = {
                    "t": now(), "tool": request.get("tool"), "arguments": request.get("arguments"),
                    "operation_id": request.get("operation_id"), "signature_ok": signature_ok,
                    "signed": signed is not None,
                    "idempotency_key": self.headers.get("Idempotency-Key"),
                    # The whole body: a pushed task result is not a tool call.
                    "body": request,
                }
                index = len(outer.calls)
                outer.calls.append(call)
                status, reply = (401, {"error": "bad signature"})
                if signature_ok:
                    status, reply = 200, outer.answer(call)
                    if outer.fail_from is not None and index >= outer.fail_from:
                        status, reply = 500, {"error": "the booking system is down"}
                call["status"] = status
                call["done"] = now()
                data = json.dumps(reply).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def answer(self, call: dict) -> dict:
        args = call["arguments"] if isinstance(call["arguments"], dict) else {}
        tool = call["tool"]
        time.sleep(self.delay.get(tool, 0.0))
        if tool == "check_availability":
            return {"available": True, "times": ["seven p m", "eight thirty p m"]}
        if tool == "book_table":
            party = args.get("party_size")
            return {"status": "booked", "effect": f"booked a table for {party}"}
        if tool == "order_flowers":
            return {"status": "ordered", "effect": "ordered the flowers"}
        return {"status": "ok"}

    def since(self, mark: int) -> List[dict]:
        return self.calls[mark:]

    def tool(self, name: str) -> dict:
        tools = {
            "check_availability": {
                "description": "Look up the free tables of a restaurant for a day. Read-only.",
                "parameters": {
                    "type": "object",
                    "properties": {"restaurant": {"type": "string"}, "day": {"type": "string"}},
                    "required": ["restaurant", "day"],
                },
                "progress_phrase": "Checking the tables now.",
            },
            "book_table": {
                "description": "Book a table at a restaurant. This makes a real booking.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "restaurant": {"type": "string"},
                        "party_size": {"type": "integer"},
                        "day": {"type": "string"},
                        "time": {"type": "string"},
                    },
                    "required": ["restaurant", "party_size", "day", "time"],
                },
                "side_effects": True,
                "irreversible": True,
                "confirm_phrase": "book the table",
            },
            "order_flowers": {
                "description": "Order flowers for a booked table. This places a real order.",
                "parameters": {
                    "type": "object",
                    "properties": {"restaurant": {"type": "string"}},
                    "required": ["restaurant"],
                },
                "side_effects": True,
                "irreversible": True,
                "confirm_phrase": "order the flowers",
            },
        }
        return {"name": name, "url": f"{self.url}/{name}", **tools[name]}


# ---- one /ws session


BUD_KEY = "live-e2e-bud-key"
# What each agent of the stand-in is told. `streamer` writes the answer marker its entry declares.
BUD_AGENTS = {
    "support": "You answer callers of the Larkspur shop on the phone. Answer every question in "
               "three short plain sentences.",
    "streamer": "You answer callers of the Larkspur shop on the phone. Begin every reply with the "
                "line ANSWER: and then answer in five separate short sentences. End every sentence "
                "with a full stop.",
    "vendor": "You are a voice assistant on a phone line. Answer in one or two short plain sentences.",
}
BUD_GREETING = "Hello, this is the Larkspur shop."


class BudStandIn:
    """The Bud gateway's `/v1/responses` and `/v1/responses/{id}/cancel` for the agents of
    BUD_AGENTS, over the background llama-server: one streamed chat completion per run, one
    history per conversation, `bud_truncate` applied to it. A request's own `tools`,
    `instructions` and input items are honoured as the Responses API defines them: a call to
    a declared function is a `function_call` item, its output comes with a later request.
    Every request is recorded."""

    def __init__(self, llm_url: str, model: str):
        self.requests: List[dict] = []
        self.cancels: List[str] = []
        # Seconds between two text deltas of an agent, to give it the pace of a slower one.
        self.pace: Dict[str, float] = {}
        # conversation -> [{"role", "content", "response_id"}]
        self.history: Dict[str, List[dict]] = {}
        self.lock = threading.Lock()
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def event(self, kind: str, payload: dict):
                data = json.dumps({"type": kind, **payload})
                self.wfile.write(f"event: {kind}\ndata: {data}\n\n".encode())
                self.wfile.flush()

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                if self.path.endswith("/cancel"):
                    outer.cancels.append(self.path.split("/")[-2])
                    self.send_response(200)
                    self.send_header("Content-Length", "2")
                    self.end_headers()
                    self.wfile.write(b"{}")
                    return
                request = json.loads(body)
                agent = request.get("prompt", {}).get("id", "")
                conversation = request.get("conversation", "")
                # The Responses API's own fields a session may add: input items (a function's
                # output, a system note) ahead of the caller's words, instructions, tools.
                given = request.get("input", "")
                items = given if isinstance(given, list) else [{"role": "user", "content": given}]
                fresh = []
                for item in items:
                    if item.get("type") == "function_call_output":
                        fresh.append({"role": "tool", "tool_call_id": item.get("call_id", ""),
                                      "content": str(item.get("output", ""))})
                    elif item.get("role") in ("user", "system"):
                        fresh.append({"role": item["role"], "content": item.get("content", "")})
                tools = [{"type": "function", "function": {
                    "name": tool.get("name", ""), "description": tool.get("description", ""),
                    "parameters": tool.get("parameters") or {"type": "object", "properties": {}}}}
                    for tool in request.get("tools") or []]
                with outer.lock:
                    response_id = f"resp_{len(outer.requests) + 1}"
                    record = {"t": now(), "auth": self.headers.get("Authorization", ""),
                              "body": request, "response_id": response_id, "output": "",
                              "calls": [], "ended": "open"}
                    outer.requests.append(record)
                    history = outer.history.setdefault(conversation, [])
                    cut = request.get("bud_truncate")
                    for message in history:
                        if cut and message.get("response_id") == cut.get("response_id"):
                            message["content"] = cut.get("output_text", "")
                    # Instructions stand with the agent's own; for a run with no input they are
                    # what the model is to do now, so they come last.
                    told = request.get("instructions") or ""
                    system = BUD_AGENTS.get(agent, "") + (f"\n\n{told}" if told and fresh else "")
                    messages = [{"role": "system", "content": system}]
                    messages += [{k: v for k, v in m.items() if k != "response_id"} for m in history
                                 if m.get("content") or m.get("tool_calls")]
                    messages += fresh
                    if told and not fresh:
                        messages.append({"role": "system", "content": told})
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                self.close_connection = True
                chat = {"model": model, "messages": messages, "stream": True,
                        "max_tokens": 200, "temperature": 0.2}
                if tools:
                    chat["tools"] = tools
                upstream = urllib.request.Request(f"{llm_url}/chat/completions", data=json.dumps(chat).encode(),
                                                  headers={"Content-Type": "application/json"})
                calls: Dict[int, dict] = {}
                try:
                    self.event("response.created", {"response": {
                        "id": response_id, "status": "in_progress", "conversation": {"id": conversation}}})
                    with urllib.request.urlopen(upstream, timeout=120) as stream:
                        for line in stream:
                            line = line.decode().strip()
                            if not line.startswith("data: ") or line == "data: [DONE]":
                                continue
                            choices = json.loads(line[6:]).get("choices") or [{}]
                            delta = choices[0].get("delta", {})
                            for part in delta.get("tool_calls") or []:
                                call = calls.setdefault(part.get("index", 0), {"id": "", "name": "", "arguments": ""})
                                call["id"] = part.get("id") or call["id"]
                                function = part.get("function") or {}
                                call["name"] += function.get("name") or ""
                                call["arguments"] += function.get("arguments") or ""
                            text = delta.get("content")
                            if text:
                                record["output"] += text
                                self.event("response.output_text.delta", {"item_id": "msg_1", "delta": text})
                                time.sleep(outer.pace.get(agent, 0.0))
                    for index, call in sorted(calls.items()):
                        call["id"] = call["id"] or f"call_{response_id}_{index}"
                        item = {"id": f"fc_{response_id}_{index}", "type": "function_call",
                                "call_id": call["id"], "name": call["name"]}
                        self.event("response.output_item.added", {"item": {**item, "arguments": "", "status": "in_progress"}})
                        self.event("response.output_item.done", {"item": {
                            **item, "arguments": call["arguments"], "status": "completed"}})
                    record["calls"] = list(calls.values())
                    record["ended"] = "completed"
                    self.event("response.completed", {"response": {
                        "id": response_id, "status": "completed", "output": [],
                        "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}}})
                except (BrokenPipeError, ConnectionResetError):
                    # The gateway dropped the stream: the run is cancelled.
                    record["ended"] = "dropped"
                finally:
                    with outer.lock:
                        history.extend(fresh)
                        answer = {"role": "assistant", "content": record["output"],
                                  "response_id": response_id}
                        if calls:
                            answer["tool_calls"] = [
                                {"id": call["id"], "type": "function",
                                 "function": {"name": call["name"], "arguments": call["arguments"]}}
                                for _, call in sorted(calls.items())]
                        history.append(answer)
                    record["done"] = now()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/v1"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


HOOK_SECRET = "live-e2e-hook-secret"
# The sign-in link of the A2A stand-in's statement job (AU1).
SIGN_IN_LINK = "https://id.example.com/authorize?state=live-au1"
BACKEND_TOKEN = "live-e2e-backend-token"
MCP_VERSION = "2026-07-28"
# PR11: the client's own secret for the pushes to its own address (allow_client_push).
CLIENT_PUSH_SECRET = "live-e2e-the-clients-own-push-secret"
MCP_TASKS = "io.modelcontextprotocol/tasks"
# MA7's MCP server: three tools the orchestrator knows only from what the server says of them.
NOTES_TOOLS = [
    {"name": "search_notes", "title": "Search notes",
     "description": "Searches the caller's notes for a word or phrase and says which notes mention it.",
     "inputSchema": {"type": "object", "required": ["query"], "properties": {
         "query": {"type": "string", "description": "The word or phrase to look for."}}},
     "annotations": {"readOnlyHint": True}},
    {"name": "delete_note", "title": "Delete a note",
     "description": "Deletes one of the caller's notes, by its title. A deleted note cannot be restored.",
     "inputSchema": {"type": "object", "required": ["title"], "properties": {
         "title": {"type": "string", "description": "The title of the note."}}},
     "annotations": {"readOnlyHint": False, "destructiveHint": True}},
    {"name": "create_reminder", "title": "Create a reminder",
     "description": "Sets a reminder for the caller: what to do, on which day, at what time, and how "
                    "important it is.",
     "inputSchema": {"type": "object", "required": ["what", "day", "time"], "properties": {
         "what": {"type": "string", "description": "What to remind the caller of."},
         "day": {"type": "string", "description": "The day, for example Friday."},
         "time": {"type": "string", "description": "The time of day, for example 6 pm."},
         "priority": {"type": "string", "enum": ["low", "normal", "high"]}}},
     "annotations": {"readOnlyHint": False, "destructiveHint": False}},
]


class Streamed:
    """A stand-in's reply written as server-sent events: one `data:` line per item, each as the
    iterator gives it, then the connection closes."""

    def __init__(self, events):
        self.events = events


class Headed:
    """A stand-in's JSON reply with headers of its own (an MCP server's `Mcp-Session-Id`)."""

    def __init__(self, body: Any, headers: Dict[str, str]):
        self.body, self.headers = body, headers


# PR1: the OAuth client of the `authed` MCP entry, and how long its tokens live (renewed half
# way through, so the second call fetches another).
OAUTH_CLIENT, OAUTH_SECRET, OAUTH_TOKEN_S = "waav-live", "live-e2e-oauth-secret", 4


# PR7: what the gRPC agent answers; PR8: what the relay webhook's callback says.
GRPC_ANSWER = "The museum opens at ten in the morning, according to the gRPC desk."
RELAY_ANSWER = "Your parcel is at the depot and goes out on Friday."


class RelayHook:
    """PR8: a webhook of the task gateways' catalog. It takes a task's message at once (it is
    working), then calls back to the *other* gateway process (as a load balancer in front of
    both may): a callback with a wrong signature, one for a task nobody runs, then the result.
    Every callback's HTTP status is kept."""

    def __init__(self, other: str):
        self.other = other
        self.tasks: List[dict] = []
        self.callbacks: List[Tuple[str, int, float]] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                message = json.loads(raw or b"{}")
                stamp, signed = self.headers.get("X-WaaV-Timestamp", ""), self.headers.get("X-WaaV-Signature", "")
                mac = hmac.new(HOOK_SECRET.encode(), f"{stamp}.".encode() + raw, hashlib.sha256)
                ok = hmac.compare_digest(signed, "sha256=" + mac.hexdigest())
                outer.tasks.append({"t": now(), "message": message, "ok": ok})
                task = message.get("type") == "task" and ok
                body = json.dumps([{"type": "progress", "text": "Asking the other office."}] if task else []).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                if task:
                    threading.Thread(target=outer.call_back, args=(message["task_id"],), daemon=True).start()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def post(self, task_id: str, event: dict, secret: str = HOOK_SECRET) -> int:
        body, stamp = json.dumps(event).encode(), str(int(time.time()))
        mac = hmac.new(secret.encode(), f"{stamp}.".encode() + body, hashlib.sha256)
        request = urllib.request.Request(f"{self.other}/v1/tasks/{task_id}/callback", data=body, headers={
            "Content-Type": "application/json", "X-WaaV-Timestamp": stamp,
            "X-WaaV-Signature": "sha256=" + mac.hexdigest()})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code
        except Exception:
            return 0

    def call_back(self, task_id: str):
        time.sleep(1.5)
        forged = {"type": "result", "text": "A forged answer."}
        self.callbacks.append(("forged", self.post(task_id, forged, secret="not-the-hook-secret"), now()))
        self.callbacks.append(("unknown", self.post("task_nobody_runs", {"type": "progress", "text": "?"}), now()))
        time.sleep(0.5)
        self.callbacks.append(("result", self.post(task_id, {"type": "result", "text": RELAY_ANSWER}), now()))


class Backends:
    """The smallest servers of the four protocols a catalog entry speaks, in this process:

    - `/hook`: the gateway's webhook wire. It checks the signature of every message and reports
      through the gateway's callback route, signed with the same secret.
    - `/a2a`: an Agent2Agent 1.0 agent over JSON-RPC (`SendMessage`, `GetTask`, `CancelTask`).
    - `/mcp`: an MCP 2026-07-28 server whose tool `assist` runs as a task of the Tasks extension
      (`tools/call`, `tasks/get`, `tasks/update`, `tasks/cancel`).
    - `/v1/responses`: background responses with polling and cancel, in front of the
      llama-server's own `/v1/responses` (which answers at once and keeps nothing).
    - `/agent` (MA7): an A2A agent with an Agent Card that says it streams and takes its key in
      `X-Agent-Key`; `SendStreamingMessage` streams the task's status and artifact updates.
    - `/notes` (MA7): an MCP server with three tools (`tools/list`); `create_reminder` sends a
      progress notification on its reply before its result.

    The first three run one kind of job: a request about flowers first asks the caller which
    ones, one about an address asks for a form of two fields (the webhook as a form question,
    the MCP server as an elicitation of several fields), one about a statement asks the caller
    to sign in through a link (A2A `AUTH_REQUIRED`), a request for a report works until it is
    cancelled, and every answer is written by the background llama-server. Every request is
    recorded in `seen`."""

    FINAL = ("completed", "failed", "cancelled")

    def __init__(self, llm_url: str, model: str, gateway_port: int):
        self.llm, self.model = llm_url, model
        self.gateway = f"http://127.0.0.1:{gateway_port}"
        self.port = gateway_port
        self.jobs: Dict[str, dict] = {}
        self.responses: Dict[str, dict] = {}
        self.seen: List[dict] = []
        # PR1: the elicitations a server wrote on a reply, by id, until they are answered.
        self.elicited: Dict[str, dict] = {}
        # PR9: what the MCP server sends on its GET stream, in order, and its tasks.
        self.get_stream: "queue.Queue[dict]" = queue.Queue()
        self.mcp_tasks: Dict[str, dict] = {}
        # PR7: the port of the gRPC agent (a2a_grpc_standin.py), once it runs.
        self.grpc_port: Optional[int] = None
        # PR11: the pushes to the client's own address.
        self.own_pushes: List[dict] = []
        # MA1-updates and MA4: set, the flight and test jobs end only once it is released
        # (shortly after), so their results come while the call is busy with something else.
        self.hold: Optional[threading.Event] = None
        # MA2: set, the flight job asks the caller for the seat (with a 5 s deadline) once it
        # is released, and books the flight with the answer.
        self.ask_seat: Optional[threading.Event] = None
        self.seat_deadline_ms = 5000
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def reply(self, status: int, body: Any, headers: Optional[Dict[str, str]] = None):
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                for name, value in (headers or {}).items():
                    self.send_header(name, value)
                self.end_headers()
                self.wfile.write(data)

            def stream(self, streamed: "Streamed"):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                self.close_connection = True
                for event in streamed.events:
                    self.wfile.write(f"data: {json.dumps(event)}\n\n".encode())
                    self.wfile.flush()

            def do_GET(self):
                if self.path == "/mcpget":
                    return outer.mcp_get_stream(self)
                if self.path == "/rest/.well-known/agent-card.json":
                    outer.note("rest", "card", {}, True)
                    return self.reply(200, outer.rest_card())
                if self.path == "/grpcagent/.well-known/agent-card.json":
                    outer.note("grpcagent", "card", {}, True)
                    return self.reply(200, outer.grpc_card())
                if self.path == "/grpcpoll/.well-known/agent-card.json":
                    outer.note("grpcpoll", "card", {}, True)
                    return self.reply(200, outer.grpc_card(streams=False))
                if self.path.startswith("/restapi/"):
                    return self.reply(*outer.rest_api("GET", self.path, self.headers, {}))
                if self.path == "/agent/.well-known/agent-card.json":
                    outer.note("agent", "card", {}, True)
                    return self.reply(200, outer.agent_card())
                if self.path == "/push/.well-known/agent-card.json":
                    outer.note("push", "card", {}, True)
                    return self.reply(200, outer.push_card(extended=False))
                found = outer.responses.get(self.path.rsplit("/", 1)[-1])
                outer.note("responses", "get", {"path": self.path}, found is not None)
                self.reply(200 if found else 404, outer.response_view(found) if found else {})

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                try:
                    message = json.loads(raw or b"{}")
                except ValueError:
                    message = {}
                wires = {"/hook": outer.hook, "/a2a": outer.a2a, "/mcp": outer.mcp,
                         "/v1/responses": outer.respond, "/agent": outer.agent,
                         "/notes": outer.notes, "/push": outer.push_agent,
                         "/mcpauth": outer.mcp_authed, "/token": outer.token,
                         "/mcpelicit": outer.mcp_elicit, "/ctlhook": outer.ctl_hook,
                         "/ctlagent": outer.ctl_agent, "/mcpget": outer.mcp_get,
                         "/ownpush": outer.own_push}
                if self.path.startswith("/restapi/"):
                    self.reply(*outer.rest_api("POST", self.path, self.headers, message))
                elif self.path in wires:
                    status, body = wires[self.path](self.headers, raw, message)
                    if isinstance(body, Streamed):
                        self.stream(body)
                    elif isinstance(body, Headed):
                        self.reply(status, body.body, body.headers)
                    else:
                        self.reply(status, body)
                elif self.path.startswith("/v1/responses/") and self.path.endswith("/cancel"):
                    self.reply(*outer.respond_cancel(self.path.split("/")[-2]))
                else:
                    self.reply(404, {})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def catalog(self, llama_url: str) -> List[dict]:
        """`interaction.task_backends` of the gateway that uses these servers. `llama` is the
        llama-server itself, streamed."""
        token = {"bearer_env": "LIVE_BACKEND_TOKEN"}
        return [
            {"name": "hook", "type": "webhook", "url": f"{self.url}/hook", "secret_env": "LIVE_HOOK_SECRET"},
            {"name": "agent", "type": "a2a", "url": f"{self.url}/a2a", **token},
            {"name": "tool", "type": "mcp", "url": f"{self.url}/mcp", "tool": "assist", **token,
             "side_effects": "none"},
            {"name": "store", "type": "openai_responses", "url": f"{self.url}/v1", "model": self.model,
             "mode": "poll", "api_key_env": "LIVE_BACKEND_TOKEN"},
            {"name": "llama", "type": "openai_responses", "url": f"{llama_url}/v1", "model": self.model},
            # MA1's two agents of an orchestrator, on the A2A and webhook wires. MA1 to MA6
            # measure orchestration: the gateway does not confirm their calls (MA7 does).
            {"name": "browser", "type": "a2a", "url": f"{self.url}/a2a", **token,
             "description": "A browser agent: operates airline and travel websites to book "
                            "flights, hotels and other travel.", "confirm": False},
            {"name": "coder", "type": "webhook", "url": f"{self.url}/hook",
             "secret_env": "LIVE_HOOK_SECRET",
             "description": "A coding agent: works in the caller's code repository, fixes "
                            "failing tests and bugs.", "confirm": False},
            # MA7: agents with no description, known by what they say of themselves.
            {"name": "planner", "type": "a2a", "url": f"{self.url}/agent", **token,
             "side_effects": "none"},
            {"name": "notes", "type": "mcp", "url": f"{self.url}/notes", **token, "tools": ["*"]},
            # PR1: an MCP server with a session, reached with an OAuth client's token; one
            # that asks for input on a reply no session waits on.
            {"name": "authed", "type": "mcp", "url": f"{self.url}/mcpauth", "tool": "assist",
             "side_effects": "none",
             "oauth": {"token_url": f"{self.url}/token", "client_id": OAUTH_CLIENT,
                       "client_secret_env": "LIVE_OAUTH_SECRET", "scope": "assist"}},
            {"name": "elicitor", "type": "mcp", "url": f"{self.url}/mcpelicit", **token, "tools": ["*"]},
            # PR2: an A2A agent with an extended card that pushes its tasks' updates to the
            # gateway's callback route.
            {"name": "pusher", "type": "a2a", "url": f"{self.url}/push", **token,
             "side_effects": "none", "push_base_url": self.gateway},
            # PR3: a webhook and an A2A agent that ask for conversation controls.
            {"name": "ctlhook", "type": "webhook", "url": f"{self.url}/ctlhook",
             "secret_env": "LIVE_HOOK_SECRET", "side_effects": "none"},
            {"name": "ctlagent", "type": "a2a", "url": f"{self.url}/ctlagent", **token,
             "side_effects": "none"},
            # PR6: an A2A agent on the HTTP+JSON binding whose card names its OAuth token
            # endpoint; the entry holds only the client.
            {"name": "restagent", "type": "a2a", "url": f"{self.url}/rest", "side_effects": "none",
             "oauth": {"client_id": OAUTH_CLIENT, "client_secret_env": "LIVE_OAUTH_SECRET", "scope": "assist"}},
            # PR7: an A2A agent on the gRPC binding (a2a_grpc_standin.py).
            {"name": "grpcagent", "type": "a2a", "url": f"{self.url}/grpcagent", **token,
             "side_effects": "none"},
            # PR7P (r4-left): the same agent, whose card says it neither streams nor has an
            # extended card: polled.
            {"name": "grpcpoll", "type": "a2a", "url": f"{self.url}/grpcpoll", **token,
             "side_effects": "none"},
            # PR9: an MCP server that asks on its GET stream and asks for a control in a result.
            {"name": "getstream", "type": "mcp", "url": f"{self.url}/mcpget", "tool": "assist", **token,
             "side_effects": "none"},
        ]

    def note(self, wire: str, method: Optional[str], body: dict, ok: bool, **more):
        self.seen.append({"t": now(), "wire": wire, "method": method, "body": body, "ok": ok, **more})

    def of(self, wire: str, method: Optional[str] = None, start: int = 0) -> List[dict]:
        return [hit for hit in self.seen[start:]
                if hit["wire"] == wire and (method is None or hit["method"] == method)]

    def last(self, wire: str) -> Optional[dict]:
        jobs = [job for job in self.jobs.values() if job["wire"] == wire]
        return jobs[-1] if jobs else None

    # ---- the job the first three protocols run

    def start(self, wire: str, request: str, key: Optional[str] = None,
              notify: Optional[Callable[[dict], None]] = None) -> dict:
        job = {"id": key or f"{wire}-{os.urandom(4).hex()}", "wire": wire, "request": request,
               "state": "working", "progress": "Looking into it.", "question": None,
               "options": [], "answer": None, "result": None, "error": None, "created": now(),
               "context": f"ctx-{os.urandom(4).hex()}", "callbacks": [], "forged": [],
               "answered": threading.Event(), "cancel": threading.Event(),
               "notify": notify or (lambda job: None)}
        self.jobs[job["id"]] = job
        threading.Thread(target=self.work, args=(job,), daemon=True).start()
        return job

    def move(self, job: dict, state: str, **fields):
        job.update(state=state, **fields)
        if state in self.FINAL:
            job.setdefault("ended", now())
        job["notify"](job)

    def work(self, job: dict):
        request = job["request"]
        if "address" in request.lower():
            # A form for the client's screen (F1): the answer is the filled-in fields.
            self.move(job, "input_required", question="Where should the parcel go?", options=[],
                      form={"type": "object", "required": ["street", "city"], "properties": {
                          "street": {"type": "string", "title": "Street"},
                          "city": {"type": "string", "title": "City"}}})
            while not job["answered"].wait(0.05):
                if job["cancel"].is_set():
                    return self.move(job, "cancelled")
            if not isinstance(job["answer"], dict):
                return self.move(job, "failed", error="the form was not filled in")
            fields = job["answer"]
            request = (f"{request} (The caller filled in the form: {fields.get('street')}, "
                       f"{fields.get('city')}.) In one sentence, say the parcel will go to that street "
                       "and name the street.")
            job.update(state="working", question=None, form=None)
        if "statement" in request.lower():
            # A sign-in through a link (AU1): the caller signs in on the client's screen.
            self.move(job, "auth_required", question=f"Please sign in at {SIGN_IN_LINK} to see your statement.")
            while not job["answered"].wait(0.05):
                if job["cancel"].is_set():
                    return self.move(job, "cancelled")
            if job["answer"] is None:
                return self.move(job, "failed", error="the caller did not sign in")
            request = (f"{request} (The caller signed in.) In one sentence, say the statement shows two "
                       "payments this month.")
            job.update(state="working", question=None)
        if "bill" in request.lower():
            # A sensitive value (KD1): typed on a phone's keypad; the job keeps what it got.
            self.move(job, "input_required", question="What is your card number?", options=[],
                      value=True)
            while not job["answered"].wait(0.05):
                if job["cancel"].is_set():
                    return self.move(job, "cancelled")
            if not job["answer"]:
                return self.move(job, "failed", error="no card number")
            return self.move(job, "completed", result="Your bill is paid.")
        if "flower" in request.lower():
            self.move(job, "input_required", question="Which flowers would you like?",
                      options=["roses", "tulips"])
            while not job["answered"].wait(0.05):
                if job["cancel"].is_set():
                    return self.move(job, "cancelled")
            if job["answer"] is None:
                return self.move(job, "failed", error="the caller gave no answer")
            request = (f"{request} (Asked which flowers, the caller said: {job['answer']}.) "
                       "Say that the order is placed and name the flowers.")
            job.update(state="working", question=None)
        if "report" in request.lower():
            self.move(job, "working", progress="Writing the report.")
            stopped = job["cancel"].wait(120)
            return self.move(job, "cancelled" if stopped else "failed", error="nobody cancelled")
        seat = self.ask_seat
        if seat is not None and "flight" in request.lower() and "window" not in request.lower():
            # MA2: the airline needs the seat within 5 s, asked once the script releases it.
            self.move(job, "working", progress="Opening the airline's site.")
            if not seat.wait(180) or job["cancel"].is_set():
                return self.move(job, "cancelled")
            job["asked_at"] = now()
            self.move(job, "input_required", question="Window or aisle?", options=[],
                      expires_in_ms=self.seat_deadline_ms)
            while not job["answered"].wait(0.02):
                if job["cancel"].is_set():
                    return self.move(job, "cancelled")
            job["answered_at"] = now()
            return self.move(job, "completed",
                             result=f"Booked flight AI 101 to Delhi on Friday, departing at 9:40 am; "
                                    f"the caller said: {job['answer']}")
        if "apartment" in request.lower():
            # MA5: a search that takes 5 s, long enough for the caller to go on.
            if job["cancel"].wait(5.0):
                return self.move(job, "cancelled")
            return self.move(job, "completed", result="I found three pet friendly apartments in "
                                                      "Miami within your budget.")
        if "hotel" in request.lower():
            # MA7: the planner looks, asks the caller's budget, and finds a room.
            self.move(job, "working", progress="Looking at hotels in Goa.")
            if job["cancel"].wait(2.0):
                return self.move(job, "cancelled")
            job["asked_at"] = now()
            self.move(job, "input_required", question="What is your budget per night?", options=[])
            while not job["answered"].wait(0.02):
                if job["cancel"].is_set():
                    return self.move(job, "cancelled")
            job["answered_at"] = now()
            self.move(job, "working", progress="Checking the rooms within your budget.")
            if job["cancel"].wait(2.0):
                return self.move(job, "cancelled")
            return self.move(job, "completed", result="The Sea View hotel in Goa has a room for two "
                                                      "nights within your budget.")
        if "parcel" in request.lower() and "address" not in job["request"].lower():
            # MA6: a check that ends after the caller hung up (not MF1's address form, whose
            # filled-in request names the parcel too).
            if job["cancel"].wait(8.0):
                return self.move(job, "cancelled")
            return self.move(job, "completed", result="Your parcel is on its way and arrives on Thursday.")
        if "flight" in request.lower() or "test" in request.lower():
            # MA1: the browser agent books a flight in 6 to 10 s, the coding agent fixes a test
            # in 8 to 12 s. `created` and `ended` are when each job ran.
            flight = "flight" in request.lower()
            low, high = (6.0, 10.0) if flight else (8.0, 12.0)
            self.move(job, "working", progress="Opening the airline's site." if flight else "Running the tests.")
            hold = self.hold
            if hold is not None:
                if not hold.wait(180) or job["cancel"].is_set():
                    return self.move(job, "cancelled")
                low, high = (0.3, 0.9) if flight else (0.6, 1.4)
            if job["cancel"].wait(random.uniform(low, high)):
                return self.move(job, "cancelled")
            result = ("Booked flight AI 101 to Delhi on Friday, departing at 9:40 am." if flight
                      else "Fixed the failing test test_parse_dates; all 42 tests pass now.")
            return self.move(job, "completed", result=result)
        try:
            body = {"model": self.model, "max_tokens": 80, "temperature": 0, "messages": [
                {"role": "system", "content": PLAIN_PROMPT}, {"role": "user", "content": request}]}
            reply = post_json(f"{self.llm}/v1/chat/completions", body)
            text = reply["choices"][0]["message"]["content"].strip()
        except Exception as error:  # the model server's failure is the job's
            return self.move(job, "failed", error=repr(error))
        if job["cancel"].is_set():
            return self.move(job, "cancelled")
        self.move(job, "completed", result=text)

    def answer(self, job: dict, value: Optional[str]):
        job.update(answer=value, state="working", question=None)
        job["answered"].set()

    def stop(self, job: dict, wait: float = 2.0):
        """Asks the job to stop and waits for it to say so."""
        job["cancel"].set()
        deadline = now() + wait
        while job["state"] not in self.FINAL and now() < deadline:
            time.sleep(0.02)

    # ---- the webhook wire

    def call_back(self, url: str, event: dict, secret: str = HOOK_SECRET) -> int:
        """One signed event to the gateway's callback route; the HTTP status."""
        body, stamp = json.dumps(event).encode(), str(int(time.time()))
        mac = hmac.new(secret.encode(), f"{stamp}.".encode() + body, hashlib.sha256)
        request = urllib.request.Request(url, data=body, headers={
            "Content-Type": "application/json", "X-WaaV-Timestamp": stamp,
            "X-WaaV-Signature": "sha256=" + mac.hexdigest()})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code

    def hook(self, headers, raw: bytes, message: dict) -> Tuple[int, Any]:
        stamp, signed = headers.get("X-WaaV-Timestamp", ""), headers.get("X-WaaV-Signature", "")
        mac = hmac.new(HOOK_SECRET.encode(), f"{stamp}.".encode() + raw, hashlib.sha256)
        ok = hmac.compare_digest(signed, "sha256=" + mac.hexdigest())
        kind = message.get("type")
        self.note("hook", kind, message, ok, idempotency_key=headers.get("Idempotency-Key"),
                  traceparent=headers.get("traceparent"))
        if not ok:
            return 401, {"error": "bad signature"}
        if kind == "task":
            url = self.gateway + message["callback"]

            def notify(job: dict):
                events = {
                    "working": {"type": "progress", "text": job["progress"]},
                    "input_required": {"type": "question", "request_id": "flowers", "kind": "choose",
                                       "prompt": job["question"], "options": job["options"]},
                }
                if job.get("form"):
                    events["input_required"] = {"type": "question", "request_id": "parcel",
                                                "kind": "form", "prompt": job["question"],
                                                "schema": job["form"]}
                if job.get("value"):
                    events["input_required"] = {"type": "question", "request_id": "card",
                                                "kind": "value", "prompt": job["question"]}
                events = {
                    **events,
                    "completed": {"type": "result", "text": job["result"]},
                    "failed": {"type": "failed", "error": job["error"]},
                }
                # A cancel is confirmed in the reply to the gateway's cancel message.
                if job["state"] in events:
                    event = events[job["state"]]
                    # The same report under another secret, while the task runs: not taken.
                    job["forged"].append(self.call_back(url, event, "not-the-secret"))
                    job["callbacks"].append((event["type"], self.call_back(url, event)))

            job = self.start("hook", message["request"], message["task_id"], notify)
            job["callback_url"] = url
            return 200, {"type": "progress", "text": "Looking into it."}
        job = self.jobs.get(message.get("task_id"))
        if job is None:
            return 404, {}
        if kind == "answer":
            self.answer(job, message.get("value") if message.get("outcome") == "accept" else None)
            return 200, []
        if kind == "cancel":
            self.stop(job)
            done = [{"type": "effect", "label": "started the report"}] if "report" in job["request"].lower() else []
            return 200, done + [{"type": "cancelled"}]
        return 400, {}

    # ---- Agent2Agent 1.0

    def a2a(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        ok = headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}" and headers.get("A2A-Version") == "1.0"
        method, params = message.get("method"), message.get("params") or {}
        self.note("a2a", method, message, ok, traceparent=headers.get("traceparent"))
        if not ok:
            return 401, {}
        states = {"working": "TASK_STATE_WORKING", "input_required": "TASK_STATE_INPUT_REQUIRED",
                  "auth_required": "TASK_STATE_AUTH_REQUIRED",
                  "completed": "TASK_STATE_COMPLETED", "failed": "TASK_STATE_FAILED",
                  "cancelled": "TASK_STATE_CANCELED"}

        def task(job: dict) -> dict:
            said = {"working": job["progress"], "input_required": job["question"],
                    "auth_required": job["question"], "failed": job["error"]}.get(job["state"])
            status: Dict[str, Any] = {"state": states[job["state"]]}
            if said:
                status["message"] = {"messageId": os.urandom(8).hex(), "role": "ROLE_AGENT",
                                     "parts": [{"text": said}]}
                # MA2: A2A has no deadline field; the gateway reads the message's metadata.
                if job["state"] == "input_required" and job.get("expires_in_ms"):
                    status["message"]["metadata"] = {"expires_in_ms": job["expires_in_ms"]}
            view = {"id": job["id"], "contextId": job["context"], "status": status}
            if job["state"] == "completed":
                view["artifacts"] = [{"artifactId": "answer", "parts": [{"text": job["result"]}]}]
            return view

        if method == "SendMessage":
            sent = params.get("message") or {}
            text = " ".join(part.get("text", "") for part in sent.get("parts", []))
            if sent.get("taskId") in self.jobs:
                job = self.jobs[sent["taskId"]]
                self.answer(job, text)
            else:
                job = self.start("a2a", text)
                job["context"] = sent.get("contextId") or job["context"]
            result: Any = {"task": task(job)}
        elif method in ("GetTask", "CancelTask") and params.get("id") in self.jobs:
            job = self.jobs[params["id"]]
            if method == "CancelTask":
                self.stop(job)
            result = task(job)
        else:
            error = {"code": -32001, "message": "Task not found"}
            return 200, {"jsonrpc": "2.0", "id": message.get("id"), "error": error}
        return 200, {"jsonrpc": "2.0", "id": message.get("id"), "result": result}

    # ---- MCP 2026-07-28 with the Tasks extension

    def mcp(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        method, params = message.get("method"), message.get("params") or {}
        meta = params.get("_meta") or {}
        capabilities = meta.get("io.modelcontextprotocol/clientCapabilities") or {}
        ok = (headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}"
              and headers.get("MCP-Protocol-Version") == MCP_VERSION
              and meta.get("io.modelcontextprotocol/protocolVersion") == MCP_VERSION
              and headers.get("Mcp-Method") == method
              and (method != "tools/call" or headers.get("Mcp-Name") == params.get("name"))
              and MCP_TASKS in (capabilities.get("extensions") or {}))
        self.note("mcp", method, message, ok)

        def reply(result: Optional[dict] = None, error: Optional[dict] = None):
            body = {"jsonrpc": "2.0", "id": message.get("id")}
            body.update({"error": error} if error else {"result": result})
            return (400 if error else 200), body

        if not ok:
            return reply(error={"code": -32020, "message": "Header mismatch or missing request metadata"})

        def task(job: dict) -> dict:
            stamp = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(job["created"]))
            view = {"taskId": job["id"], "status": job["state"], "createdAt": stamp,
                    "lastUpdatedAt": stamp, "ttlMs": 600_000, "pollIntervalMs": 300}
            if job["state"] == "working":
                view["statusMessage"] = job["progress"]
            if job["state"] == "input_required" and job.get("form"):
                # Several fields: the gateway asks them on the client's screen (MF1).
                view["inputRequests"] = {"parcel": {"method": "elicitation/create", "params": {
                    "mode": "form", "message": job["question"], "requestedSchema": job["form"]}}}
            elif job["state"] == "input_required":
                schema = {"type": "object", "required": ["flowers"],
                          "properties": {"flowers": {"type": "string", "enum": job["options"]}}}
                view["inputRequests"] = {"flowers": {"method": "elicitation/create", "params": {
                    "mode": "form", "message": job["question"], "requestedSchema": schema}}}
            if job["state"] == "completed":
                view["result"] = {"content": [{"type": "text", "text": job["result"]}], "isError": False}
            if job["state"] == "failed":
                view["error"] = {"code": -32603, "message": job["error"]}
            return view

        if method == "tools/call":
            if params.get("name") != "assist":
                return reply(error={"code": -32602, "message": "Unknown tool"})
            job = self.start("mcp", str((params.get("arguments") or {}).get("request", "")))
            return reply({**task(job), "resultType": "task"})
        job = self.jobs.get(params.get("taskId"))
        if job is None:
            return reply(error={"code": -32602, "message": "Unknown task"})
        if method == "tasks/get":
            return reply({**task(job), "resultType": "complete"})
        if method == "tasks/update":
            responses = params.get("inputResponses") or {}
            if "parcel" in responses:
                given = responses["parcel"] or {}
                accepted = given.get("action") == "accept"
                self.answer(job, given.get("content") if accepted else None)
                return reply({"resultType": "complete"})
            given = responses.get("flowers") or {}
            accepted = given.get("action") == "accept"
            self.answer(job, (given.get("content") or {}).get("flowers") if accepted else None)
            return reply({"resultType": "complete"})
        if method == "tasks/cancel":
            # Cooperative: the request is taken, and the task says later how it ended.
            job["cancel"].set()
            return reply({"resultType": "complete"})
        return 404, {"jsonrpc": "2.0", "id": message.get("id"),
                     "error": {"code": -32601, "message": "Method not found"}}

    # ---- MA7: an A2A agent that describes itself and streams

    def agent_card(self) -> dict:
        return {
            "name": "Trip planner", "version": "1.0",
            "description": "Plans the caller's trips and finds hotels.",
            "supportedInterfaces": [{"url": f"{self.url}/agent", "protocolBinding": "JSONRPC",
                                     "protocolVersion": "1.0"}],
            "capabilities": {"streaming": True},
            "securitySchemes": {"key": {"apiKeySecurityScheme": {"location": "header",
                                                                 "name": "X-Agent-Key"}}},
            "skills": [{"id": "hotels", "name": "Hotels",
                        "description": "Finds hotels in a city for the caller's dates and budget"}],
        }

    def agent(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        """`SendStreamingMessage`: the task, then each change of its state as it happens, until
        it needs the caller or has ended; a message to the task answers it and streams on."""
        ok = (headers.get("X-Agent-Key") == BACKEND_TOKEN and headers.get("A2A-Version") == "1.0"
              and headers.get("Authorization") is None)
        method, params = message.get("method"), message.get("params") or {}
        self.note("agent", method, message, ok, traceparent=headers.get("traceparent"))
        if not ok:
            return 401, {}
        rpc = lambda result: {"jsonrpc": "2.0", "id": message.get("id"), "result": result}
        states = {"working": "TASK_STATE_WORKING", "input_required": "TASK_STATE_INPUT_REQUIRED",
                  "completed": "TASK_STATE_COMPLETED", "failed": "TASK_STATE_FAILED",
                  "cancelled": "TASK_STATE_CANCELED"}
        if method in ("GetTask", "CancelTask") and params.get("id") in self.jobs:
            job = self.jobs[params["id"]]
            if method == "CancelTask":
                self.stop(job)
            return 200, rpc({"id": job["id"], "contextId": job["context"],
                             "status": {"state": states[job["state"]]}})
        if method != "SendStreamingMessage":
            return 200, {"jsonrpc": "2.0", "id": message.get("id"),
                         "error": {"code": -32601, "message": "Method not found"}}
        changes: "queue.Queue[dict]" = queue.Queue()
        sent = params.get("message") or {}
        text = " ".join(part.get("text", "") for part in sent.get("parts", []))
        notify = lambda job: changes.put(dict(job))
        first = None
        if sent.get("taskId") in self.jobs:
            job = self.jobs[sent["taskId"]]
            job["notify"] = notify
            self.answer(job, text)
        else:
            job = self.start("agent", text, notify=notify)
            first = {"id": job["id"], "contextId": job["context"],
                     "status": {"state": "TASK_STATE_SUBMITTED"}}

        def update(state: str, said: Optional[str], final: bool) -> dict:
            status: Dict[str, Any] = {"state": states[state]}
            if said:
                status["message"] = {"messageId": os.urandom(8).hex(), "role": "ROLE_AGENT",
                                     "parts": [{"text": said}]}
            return rpc({"statusUpdate": {"taskId": job["id"], "contextId": job["context"],
                                         "status": status, "final": final}})

        def events():
            if first:
                yield rpc({"task": first})
            while True:
                try:
                    seen = changes.get(timeout=120)
                except queue.Empty:
                    return
                state = seen["state"]
                seen_at = now()
                job.setdefault("streamed", []).append((seen_at, state, seen.get("progress")))
                if state == "completed":
                    yield rpc({"artifactUpdate": {"taskId": job["id"], "artifact": {
                        "artifactId": "answer", "parts": [{"text": seen["result"]}]}}})
                    yield update(state, None, True)
                    return
                said = {"working": seen.get("progress"), "input_required": seen.get("question"),
                        "failed": seen.get("error")}.get(state)
                yield update(state, said, state != "working")
                if state != "working":
                    return

        return 200, Streamed(events())

    # ---- MA7: an MCP server whose tools are the orchestrator's

    def notes(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        method, params = message.get("method"), message.get("params") or {}
        meta = params.get("_meta") or {}
        ok = (headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}"
              and headers.get("MCP-Protocol-Version") == MCP_VERSION
              and meta.get("io.modelcontextprotocol/protocolVersion") == MCP_VERSION
              and headers.get("Mcp-Method") == method
              and (method != "tools/call" or headers.get("Mcp-Name") == params.get("name")))
        self.note("notes", method, message, ok, traceparent=headers.get("traceparent"))
        reply = lambda result: (200, {"jsonrpc": "2.0", "id": message.get("id"), "result": result})
        text = lambda words: {"content": [{"type": "text", "text": words}], "isError": False}
        if not ok:
            return 400, {"jsonrpc": "2.0", "id": message.get("id"),
                         "error": {"code": -32020, "message": "Header mismatch or missing request metadata"}}
        if method == "tools/list":
            return reply({"tools": NOTES_TOOLS})
        if method != "tools/call":
            return 404, {"jsonrpc": "2.0", "id": message.get("id"),
                         "error": {"code": -32601, "message": "Method not found"}}
        name, arguments = params.get("name"), params.get("arguments") or {}
        if name == "search_notes":
            found = "dentist" in str(arguments.get("query", "")).lower()
            return reply(text("One note mentions it: the dentist on Tuesday afternoon." if found
                              else "No note mentions it."))
        if name == "delete_note":
            return reply(text(f"Deleted the note {arguments.get('title', '')}."))
        if name == "create_reminder":
            token = meta.get("progressToken")

            def events():
                yield {"jsonrpc": "2.0", "method": "notifications/progress",
                       "params": {"progressToken": token, "progress": 1, "total": 2,
                                  "message": "Saving the reminder."}}
                time.sleep(1.5)
                yield {"jsonrpc": "2.0", "id": message.get("id"), "result": text(
                    f"The reminder to {arguments.get('what', 'do it')} is set for "
                    f"{arguments.get('day', 'that day')} at {arguments.get('time', 'that time')}.")}

            return 200, Streamed(events())
        return reply({"content": [{"type": "text", "text": "Unknown tool"}], "isError": True})

    # ---- PR1: an MCP server with a session, behind an OAuth authorisation server

    def token(self, headers, raw: bytes, _message: dict) -> Tuple[int, Any]:
        """The client credentials grant: HTTP Basic client authentication, a token per request
        that lives OAUTH_TOKEN_S seconds."""
        basic = base64.b64encode(f"{OAUTH_CLIENT}:{OAUTH_SECRET}".encode()).decode()
        form = urllib.parse.parse_qs(raw.decode())
        ok = (headers.get("Authorization") == f"Basic {basic}"
              and form.get("grant_type") == ["client_credentials"] and form.get("scope") == ["assist"])
        issued = f"tok-{os.urandom(6).hex()}" if ok else None
        self.note("token", "token", {"form": form}, ok, token=issued)
        if not ok:
            return 401, {"error": "invalid_client"}
        return 200, {"access_token": issued, "token_type": "Bearer", "expires_in": OAUTH_TOKEN_S}

    def tokens(self) -> List[str]:
        return [hit["token"] for hit in self.of("token") if hit["ok"]]

    def mcp_authed(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        """`initialize` makes a session (`Mcp-Session-Id`); every later request must carry it and
        a token the authorisation server issued. `tools/call` of `assist` is answered on its
        reply: a request about flowers first asks the caller with an elicitation on that reply
        (no MCP task), and the reply waits for the answer, which comes as a JSON-RPC response."""
        method, params = message.get("method"), message.get("params") or {}
        bearer = headers.get("Authorization", "")
        token_ok = bearer.startswith("Bearer ") and bearer[7:] in self.tokens()
        session = headers.get("Mcp-Session-Id")
        sessions = [hit["session"] for hit in self.of("mcpauth", "initialize") if hit.get("session")]
        if method == "initialize":
            made = f"sess-{os.urandom(4).hex()}"
            ok = token_ok and session is None and params.get("protocolVersion") == MCP_VERSION
            self.note("mcpauth", method, message, ok, session=made if ok else None, token=bearer[7:])
            if not ok:
                return 401, {}
            result = {"protocolVersion": MCP_VERSION, "capabilities": {"tools": {}},
                      "serverInfo": {"name": "live-authed", "version": "1"}}
            return 200, Headed({"jsonrpc": "2.0", "id": message.get("id"), "result": result},
                               {"Mcp-Session-Id": made})
        ok = token_ok and session in sessions and headers.get("MCP-Protocol-Version") == MCP_VERSION
        kind = method or ("response" if "id" in message else None)
        self.note("mcpauth", kind, message, ok, session=session, token=bearer[7:])
        if not ok:
            return 401, {}
        if method == "notifications/initialized" or method is None:
            if method is None:
                waiting = self.elicited.get(str(message.get("id")))
                if waiting is not None:
                    waiting["answer"] = message.get("result") or message.get("error")
                    waiting["event"].set()
            return 202, {}
        if method != "tools/call" or params.get("name") != "assist":
            return 200, {"jsonrpc": "2.0", "id": message.get("id"),
                         "error": {"code": -32601, "message": "Method not found"}}
        request = str((params.get("arguments") or {}).get("request", ""))
        # The caller's newest words: a correction comes after the first request, and a
        # follow-up's line about the earlier task ("(Earlier, ...)") is not the caller's.
        said = [line for line in request.split("\n") if not line.startswith("(Earlier,")]
        newest = said[-1] if said else request
        call_id = message.get("id")
        text = lambda words: {"content": [{"type": "text", "text": words}], "isError": False}

        def events():
            if "flower" not in newest.lower():
                yield {"jsonrpc": "2.0", "id": call_id, "result": text("Paris is the capital of France.")}
                return
            ask_id = f"e-{os.urandom(3).hex()}"
            waiting = {"event": threading.Event(), "answer": None, "asked": now()}
            self.elicited[ask_id] = waiting
            schema = {"type": "object", "required": ["flowers"],
                      "properties": {"flowers": {"type": "string", "enum": ["roses", "tulips"]}}}
            yield {"jsonrpc": "2.0", "id": ask_id, "method": "elicitation/create", "params": {
                "mode": "form", "message": "Which flowers would you like?", "requestedSchema": schema}}
            if not waiting["event"].wait(90):
                yield {"jsonrpc": "2.0", "id": call_id, "result": {**text("Nobody answered."), "isError": True}}
                return
            given = waiting["answer"] or {}
            flowers = (given.get("content") or {}).get("flowers") if given.get("action") == "accept" else None
            yield {"jsonrpc": "2.0", "id": call_id, "result": text(
                f"Your {flowers} are ordered." if flowers else "Nothing was ordered.")}

        return 200, Streamed(events())

    def mcp_elicit(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        """An orchestrator's server whose tool list comes after an elicitation on its reply: no
        session's call waits on that reply, so the gateway must refuse it (a JSON-RPC error),
        which lets the list go on."""
        method = message.get("method")
        kind = method or ("response" if "id" in message else None)
        ok = headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}"
        self.note("elicitor", kind, message, ok)
        if method is None:
            waiting = self.elicited.get(str(message.get("id")))
            if waiting is not None:
                waiting["answer"] = message
                waiting["event"].set()
            return 202, {}
        if method != "tools/list":
            return 404, {"jsonrpc": "2.0", "id": message.get("id"),
                         "error": {"code": -32601, "message": "Method not found"}}
        list_id = message.get("id")

        def events():
            ask_id = f"e-{os.urandom(3).hex()}"
            waiting = {"event": threading.Event(), "answer": None, "asked": now()}
            self.elicited[ask_id] = waiting
            yield {"jsonrpc": "2.0", "id": ask_id, "method": "elicitation/create", "params": {
                "mode": "form", "message": "Who is asking?", "requestedSchema": {
                    "type": "object", "properties": {"name": {"type": "string"}}}}}
            waiting["event"].wait(2.5)
            waiting["answered_after"] = now() - waiting["asked"] if waiting["event"].is_set() else None
            yield {"jsonrpc": "2.0", "id": list_id, "result": {"tools": NOTES_TOOLS[:1]}}

        return 200, Streamed(events())

    # ---- PR2: an A2A agent that pushes its tasks' updates

    def push_card(self, extended: bool) -> dict:
        card = {"name": "Parcel desk", "version": "1.0",
                "description": "Tracks the caller's parcels.",
                "capabilities": {"pushNotifications": True, "extendedAgentCard": True},
                "securitySchemes": {"bearer": {"httpAuthSecurityScheme": {"scheme": "Bearer"}}}}
        if extended:
            card["description"] = "Tracks the caller's parcels, with the courier's own records."
            card["skills"] = [{"id": "track", "name": "Tracking", "description": "Where a parcel is"}]
        return card

    def push_agent(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        """`SendMessage` takes the task's push configuration; each change of the task is pushed
        there (a status update while it works, the task once it has ended), each with the token,
        and once with a wrong one. `GetTask` is answered too and counted."""
        ok = headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}" and headers.get("A2A-Version") == "1.0"
        method, params = message.get("method"), message.get("params") or {}
        self.note("push", method, message, ok)
        rpc = lambda result: (200, {"jsonrpc": "2.0", "id": message.get("id"), "result": result})
        if not ok:
            return 401, {}
        states = {"working": "TASK_STATE_WORKING", "completed": "TASK_STATE_COMPLETED",
                  "failed": "TASK_STATE_FAILED", "cancelled": "TASK_STATE_CANCELED"}

        def view(job: dict) -> dict:
            status: Dict[str, Any] = {"state": states.get(job["state"], "TASK_STATE_WORKING")}
            if job["state"] == "working":
                status["message"] = {"messageId": os.urandom(8).hex(), "role": "ROLE_AGENT",
                                     "parts": [{"text": job["progress"]}]}
            task = {"id": job["id"], "contextId": job["context"], "status": status}
            if job["state"] == "completed":
                task["artifacts"] = [{"artifactId": "answer", "parts": [{"text": job["result"]}]}]
            return task

        if method == "GetExtendedAgentCard":
            return rpc(self.push_card(extended=True))
        if method in ("GetTask", "CancelTask") and params.get("id") in self.jobs:
            job = self.jobs[params["id"]]
            if method == "CancelTask":
                self.stop(job)
            return rpc(view(job))
        if method != "SendMessage":
            return 200, {"jsonrpc": "2.0", "id": message.get("id"),
                         "error": {"code": -32601, "message": "Method not found"}}
        sent = params.get("message") or {}
        text = " ".join(part.get("text", "") for part in sent.get("parts", []))
        config = (params.get("configuration") or {}).get("pushNotificationConfig") or {}

        def push(job: dict, body: dict, token: str) -> int:
            request = urllib.request.Request(config["url"], data=json.dumps(body).encode(), headers={
                "Content-Type": "application/json", "X-A2A-Notification-Token": token})
            try:
                with urllib.request.urlopen(request, timeout=10) as response:
                    return response.status
            except urllib.error.HTTPError as error:
                return error.code
            except Exception:
                return 0

        def notify(job: dict):
            if not config.get("url"):
                return
            final = job["state"] in self.FINAL
            body = ({"task": view(job)} if final else
                    {"statusUpdate": {"taskId": job["id"], "contextId": job["context"], "status": view(job)["status"]}})
            if not job.get("forged"):
                job["forged"].append(push(job, body, "not-the-token"))
            job["pushes"].append((now(), job["state"], push(job, body, config.get("token", ""))))

        job = self.start("push", text, notify=lambda job: None)
        job.update(pushes=[], push_config=config)
        job["notify"] = notify
        job["progress"] = "Asking the courier."
        # The first change: it is working on it.
        threading.Thread(target=lambda: (time.sleep(0.5), notify(job)), daemon=True).start()
        return rpc({"task": view(job)})

    # ---- PR6: an A2A agent on the HTTP+JSON binding, with OAuth from its card

    def rest_card(self) -> dict:
        return {"name": "Opening hours", "version": "1.0", "description": "Answers questions.",
                "supportedInterfaces": [{"url": f"{self.url}/restapi", "protocolBinding": "HTTP+JSON",
                                         "protocolVersion": "1.0"}],
                "capabilities": {"streaming": False},
                "securitySchemes": {"oauth": {"oauth2SecurityScheme": {"flows": {"clientCredentials": {
                    "tokenUrl": f"{self.url}/token", "scopes": {"assist": "Ask the agent"}}}}}}}

    def a2a_task(self, job: dict) -> dict:
        """A job as an A2A 1.0 task (ProtoJSON)."""
        states = {"working": "TASK_STATE_WORKING", "completed": "TASK_STATE_COMPLETED",
                  "failed": "TASK_STATE_FAILED", "cancelled": "TASK_STATE_CANCELED"}
        status: Dict[str, Any] = {"state": states.get(job["state"], "TASK_STATE_WORKING")}
        if job["state"] == "working":
            status["message"] = {"messageId": os.urandom(8).hex(), "role": "ROLE_AGENT",
                                 "parts": [{"text": job["progress"]}]}
        task = {"id": job["id"], "contextId": job["context"], "status": status}
        if job["state"] == "completed":
            task["artifacts"] = [{"artifactId": "answer", "parts": [{"text": job["result"]}]}]
        return task

    def rest_api(self, verb: str, path: str, headers, message: dict) -> Tuple[int, Any]:
        """`POST /restapi/message:send`, `GET /restapi/tasks/{id}`, `POST /restapi/tasks/{id}:cancel`,
        each with a token the authorisation server issued and `A2A-Version: 1.0`."""
        bearer = headers.get("Authorization", "")
        ok = (bearer.startswith("Bearer ") and bearer[7:] in self.tokens()
              and headers.get("A2A-Version") == "1.0")
        self.note("rest", f"{verb} {path.split('?')[0]}", message, ok, token=bearer[7:])
        if not ok:
            return 401, {"error": "unauthenticated"}
        if verb == "POST" and path == "/restapi/message:send":
            sent = message.get("message") or {}
            text = " ".join(part.get("text", "") for part in sent.get("parts", []))
            return 200, {"task": self.a2a_task(self.start("rest", text))}
        found = re.match(r"^/restapi/tasks/([^/:?]+)(:cancel)?$", path)
        job = self.jobs.get(found.group(1)) if found else None
        if job is None:
            return 404, {"error": "no such task"}
        if found.group(2) and verb == "POST":
            self.stop(job)
        return 200, self.a2a_task(job)

    # ---- PR7: an A2A agent on the gRPC binding

    def grpc_card(self, streams: bool = True) -> dict:
        """PR7: streams and has an extended card (both read over gRPC); PR7P: neither, so its
        tasks are polled."""
        return {"name": "Museum desk", "version": "1.0", "description": "Answers over gRPC.",
                "supportedInterfaces": [{"url": f"http://127.0.0.1:{self.grpc_port}", "protocolBinding": "GRPC",
                                         "protocolVersion": "1.0"}],
                "capabilities": {"streaming": streams, "extendedAgentCard": streams},
                "securitySchemes": {"bearer": {"httpAuthSecurityScheme": {"scheme": "Bearer"}}}}

    # ---- PR9: an MCP server that asks on its GET stream

    def mcp_get_stream(self, handler):
        """The server's GET stream: what `get_stream` holds, as it comes, until the gateway
        goes; a comment every 5 s keeps it open."""
        bearer = handler.headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}"
        session = handler.headers.get("Mcp-Session-Id")
        ok = bearer and "text/event-stream" in (handler.headers.get("Accept") or "") and bool(session)
        self.note("mcpget", "GET", {}, ok, session=session)
        if not ok:
            return handler.reply(401, {})
        handler.send_response(200)
        handler.send_header("Content-Type", "text/event-stream")
        handler.send_header("Cache-Control", "no-cache")
        handler.end_headers()
        handler.close_connection = True
        try:
            while True:
                try:
                    event = self.get_stream.get(timeout=5)
                    handler.wfile.write(f"data: {json.dumps(event)}\n\n".encode())
                    self.note("mcpget", "streamed", event, True)
                except queue.Empty:
                    handler.wfile.write(b": keep-alive\n\n")
                handler.wfile.flush()
        except (BrokenPipeError, ConnectionResetError, OSError):
            return

    def mcp_get(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        """`initialize` (with a session), then `tools/call` of `assist` as a task of the Tasks
        extension. Half a second in, the task asks the caller which list on the GET stream (an
        elicitation whose `_meta` names the task), and says on the stream when it is done; its
        result asks for brief answers (`_meta["io.waav/control"]`)."""
        method, params = message.get("method"), message.get("params") or {}
        ok = headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}"
        kind = method or ("response" if "id" in message else None)
        self.note("mcpget", kind, message, ok, session=headers.get("Mcp-Session-Id"))
        if not ok:
            return 401, {}
        rpc = lambda result: (200, {"jsonrpc": "2.0", "id": message.get("id"), "result": result})
        if method == "initialize":
            result = {"protocolVersion": MCP_VERSION, "capabilities": {"tools": {}},
                      "serverInfo": {"name": "live-getstream", "version": "1"}}
            return 200, Headed({"jsonrpc": "2.0", "id": message.get("id"), "result": result},
                               {"Mcp-Session-Id": f"gs-{os.urandom(4).hex()}"})
        if method is None:
            for task in self.mcp_tasks.values():
                if task["ask_id"] == message.get("id"):
                    task["answer"] = message.get("result") or {}
                    task["answered"].set()
            return 202, {}
        if method and method.startswith("notifications/"):
            return 202, {}
        if method == "tools/call":
            task_id = f"mt-{os.urandom(4).hex()}"
            task = {"id": task_id, "state": "working", "ask_id": f"ge-{os.urandom(3).hex()}",
                    "answered": threading.Event(), "answer": None, "list": None}
            self.mcp_tasks[task_id] = task

            def run():
                time.sleep(0.5)
                schema = {"type": "object", "properties": {
                    "list": {"type": "string", "enum": ["personal", "work"]}}}
                self.get_stream.put({"jsonrpc": "2.0", "id": task["ask_id"], "method": "elicitation/create",
                                     "params": {"mode": "form", "message": "Which list should it go on?",
                                                "requestedSchema": schema,
                                                "_meta": {"io.modelcontextprotocol/related-task": {"taskId": task_id}}}})
                if not task["answered"].wait(90):
                    task["state"] = "failed"
                    return
                answer = task["answer"] or {}
                task["list"] = (answer.get("content") or {}).get("list") if answer.get("action") == "accept" else None
                task["state"] = "completed"
                self.get_stream.put({"jsonrpc": "2.0", "method": "notifications/tasks/status",
                                     "params": {"taskId": task_id, "status": "completed"}})

            threading.Thread(target=run, daemon=True).start()
            return rpc({"taskId": task_id, "status": "working", "pollIntervalMs": 5000,
                        "statusMessage": "Finding your lists."})
        if method == "tasks/get":
            task = self.mcp_tasks.get(params.get("taskId"))
            if task is None:
                return rpc({"taskId": params.get("taskId"), "status": "failed"})
            if task["state"] != "completed":
                return rpc({"taskId": task["id"], "status": task["state"]})
            words = f"Milk is on your {task['list']} list." if task["list"] else "Nothing was added."
            return rpc({"taskId": task["id"], "status": "completed", "result": {
                "content": [{"type": "text", "text": words}],
                "_meta": {"io.waav/control": {"kind": "verbosity", "value": "brief", "request_id": "mcp-brief-1"}}}})
        return 200, {"jsonrpc": "2.0", "id": message.get("id"),
                     "error": {"code": -32601, "message": "Method not found"}}

    # ---- PR11: the client's own push address

    def own_push(self, headers, raw: bytes, message: dict) -> Tuple[int, Any]:
        """The client's push receiver: it checks the signature with the client's own secret."""
        stamp, signed = headers.get("X-WaaV-Timestamp", ""), headers.get("X-WaaV-Signature", "")
        mine = hmac.new(CLIENT_PUSH_SECRET.encode(), f"{stamp}.".encode() + raw, hashlib.sha256)
        servers = hmac.new(self.tool_secret.encode(), f"{stamp}.".encode() + raw, hashlib.sha256) \
            if getattr(self, "tool_secret", None) else None
        self.own_pushes.append({"t": now(), "body": message, "own": hmac.compare_digest(signed, "sha256=" + mine.hexdigest()),
                                "servers": bool(servers) and hmac.compare_digest(signed, "sha256=" + servers.hexdigest()),
                                "key": headers.get("Idempotency-Key")})
        return 200, {}

    # ---- PR3: agents that ask for conversation controls

    def ctl_hook(self, headers, raw: bytes, message: dict) -> Tuple[int, Any]:
        """A webhook that asks for a slower voice with its task, and answers once it hears how
        that went (`control_applied`)."""
        stamp, signed = headers.get("X-WaaV-Timestamp", ""), headers.get("X-WaaV-Signature", "")
        mac = hmac.new(HOOK_SECRET.encode(), f"{stamp}.".encode() + raw, hashlib.sha256)
        ok = hmac.compare_digest(signed, "sha256=" + mac.hexdigest())
        kind = message.get("type")
        self.note("ctlhook", kind, message, ok, idempotency_key=headers.get("Idempotency-Key"))
        if not ok:
            return 401, {"error": "bad signature"}
        if kind == "task":
            return 200, [{"type": "control", "kind": "rate", "value": "slower", "request_id": "slow-1"},
                         {"type": "progress", "text": "Checking the store's hours."}]
        if kind == "control_applied":
            return 200, {"type": "result", "text": "The store opens at nine in the morning."}
        return 200, []

    def ctl_agent(self, headers, _raw: bytes, message: dict) -> Tuple[int, Any]:
        """An A2A agent whose working task asks for brief answers in a data part of its status
        message, and completes once a message to the task says how that went."""
        ok = headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}" and headers.get("A2A-Version") == "1.0"
        method, params = message.get("method"), message.get("params") or {}
        parts = (params.get("message") or {}).get("parts") or []
        applied = [part["data"] for part in parts if isinstance(part.get("data"), dict)
                   and part["data"].get("type") == "control_applied"]
        self.note("ctlagent", "control_applied" if applied else method, message, ok)
        if not ok:
            return 401, {}
        rpc = lambda result: (200, {"jsonrpc": "2.0", "id": message.get("id"), "result": result})
        control = {"data": {"type": "control", "kind": "verbosity", "value": "brief",
                            "request_id": "brief-1"}}
        working = {"id": "ctl-task", "contextId": "ctl-ctx", "status": {
            "state": "TASK_STATE_WORKING", "message": {"messageId": "ctl-m1", "role": "ROLE_AGENT",
                                                       "parts": [{"text": "Checking the hours."}, control]}}}
        if applied:
            done = {"id": "ctl-task", "contextId": "ctl-ctx", "status": {"state": "TASK_STATE_COMPLETED"},
                    "artifacts": [{"artifactId": "a", "parts": [{"text": "Nine in the morning."}]}]}
            return rpc({"task": done})
        if method in ("SendMessage", "GetTask"):
            return rpc({"task": working} if method == "SendMessage" else working)
        return 200, {"jsonrpc": "2.0", "id": message.get("id"),
                     "error": {"code": -32601, "message": "Method not found"}}

    # ---- background responses

    def response_view(self, entry: dict) -> dict:
        return {key: entry[key] for key in ("id", "object", "status", "model", "output", "error")}

    def respond(self, headers, _raw: bytes, body: dict) -> Tuple[int, Any]:
        ok = headers.get("Authorization") == f"Bearer {BACKEND_TOKEN}"
        self.note("responses", "create", body, ok)
        if not ok:
            return 401, {"error": {"message": "no key"}}
        if body.get("stream") or not body.get("background"):
            return 400, {"error": {"message": "this server only runs background responses that are polled"}}
        entry = {"id": f"resp_{os.urandom(8).hex()}", "object": "response", "status": "queued",
                 "model": body.get("model"), "output": [], "error": None, "cancel": threading.Event()}
        self.responses[entry["id"]] = entry
        words = [item.get("content") for item in body.get("input") or [] if item.get("role") == "user"]
        entry["request"] = str(words[-1]) if words else ""

        def run():
            entry["status"] = "in_progress"
            if PE1_LINE in (body.get("instructions") or ""):
                # PE1: the background says its own prompt's first line first, as qwen2.5-3b did
                # live in P1 (r15-verify).
                text = f"{PE1_LINE} {PE1_ANSWER}"
                entry.update(status="completed", output=[{"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": text}]}])
                return
            if "report" in entry["request"].lower():
                stopped = entry["cancel"].wait(120)
                entry["status"] = "cancelled" if stopped else "failed"
                return
            upstream = {key: value for key, value in body.items() if key not in ("background", "store")}
            try:
                done = post_json(f"{self.llm}/v1/responses", upstream)
            except Exception as error:  # the model server's failure is the response's
                entry.update(status="failed", error={"message": repr(error)})
                return
            if entry["cancel"].is_set():
                entry["status"] = "cancelled"
                return
            entry.update(output=done.get("output", []), status=done.get("status", "completed"))

        threading.Thread(target=run, daemon=True).start()
        return 200, self.response_view(entry)

    def respond_cancel(self, response_id: str) -> Tuple[int, Any]:
        entry = self.responses.get(response_id)
        self.note("responses", "cancel", {"id": response_id}, entry is not None)
        if entry is None:
            return 404, {}
        entry["cancel"].set()
        deadline = now() + 2.0
        while entry["status"] not in self.FINAL and now() < deadline:
            time.sleep(0.02)
        return 200, self.response_view(entry)


def redis_set(port: int, pairs: Dict[str, str]):
    """SET each pair on the Redis at `port` (the wire protocol itself: no client is installed)."""
    with socket.create_connection(("127.0.0.1", port), timeout=5) as conn:
        for key, value in pairs.items():
            parts = [b"SET", key.encode(), value.encode()]
            conn.sendall(b"*3\r\n" + b"".join(b"$%d\r\n%s\r\n" % (len(part), part) for part in parts))
            if not conn.recv(64).startswith(b"+OK"):
                raise RuntimeError(f"redis refused SET {key}")


@dataclass
class Said:
    """One caller utterance as it was sent."""
    wav: str
    start: float
    last_voiced: float
    end: float


@dataclass
class Turn:
    said: Said
    # E: the final `stt_result` (None when none came).
    e: Optional[float]
    text: str
    first_event: int


class Mic:
    """A live microphone and what it hears of the agent: 20 ms frames in real time, silence
    between utterances, and every audio frame of the agent with its receive time."""

    def __init__(self, ctx: "Ctx"):
        self.ctx = ctx
        self.audio: List[Tuple[float, bytes]] = []
        self.opened = now()
        self._pending = bytearray()
        # `(delay in seconds, level in dB)` while the microphone also picks up the agent: what
        # was received is played in order as it comes, and comes back on the microphone that
        # much later and lower. A stopped agent is silent at once.
        self.loopback: Optional[Tuple[float, float]] = None
        self._playout = bytearray()
        self._returning: List[bytes] = []
        self._mic_on = True
        # The size of the gateway's log when the session opened (`warm`).
        self.log_mark = 0

    async def _send_audio(self, frame: bytes):
        raise NotImplementedError

    async def warm(self, timeout: float = 60.0) -> float:
        """Wait for the warm-up syntheses to end; returns how long that took.

        A warm-up line sends nothing to the client, so its end is read from the gateway's debug
        log (`interaction warm-up line synthesised`): over when none came for 1.5 s.
        """
        start = now()
        last = start
        offset = self.log_mark
        while now() - start < timeout:
            fresh, offset = self.ctx.stack.log_count("interaction warm-up line synthesised", offset)
            if fresh:
                last = now()
            if now() - last > 1.5:
                break
            await asyncio.sleep(0.1)
        return last - start

    def _heard(self, data: bytes):
        self.audio.append((now(), data))
        if self.loopback is not None:
            self._playout.extend(data)

    async def _mic(self):
        silence = bytes(CALLER_FRAME)
        start, sent = now(), 0
        try:
            while self._mic_on:
                frame = silence
                if self._pending:
                    frame = bytes(self._pending[:CALLER_FRAME]).ljust(CALLER_FRAME, b"\0")
                    del self._pending[:CALLER_FRAME]
                if self.loopback is not None:
                    frame = self._with_echo(frame)
                await self._send_audio(frame)
                sent += 1
                delay = start + sent * FRAME_MS / 1000 - now()
                if delay > 0:
                    await asyncio.sleep(delay)
        except Exception:
            pass

    def _with_echo(self, frame: bytes) -> bytes:
        """The microphone frame with the agent audio that was played `delay` ago in it."""
        delay, level_db = self.loopback
        played = bytes(self._playout[:AGENT_FRAME])
        del self._playout[:AGENT_FRAME]
        self._returning.append(
            attenuate(to_caller_rate(played.ljust(AGENT_FRAME, b"\0")), level_db) if played else b"")
        if len(self._returning) <= round(delay * 1000 / FRAME_MS):
            return frame
        echo = self._returning.pop(0)
        if not echo:
            return frame
        both = np.frombuffer(frame, dtype="<i2").astype(np.int32) + np.frombuffer(echo, dtype="<i2")
        return np.clip(both, -32768, 32767).astype("<i2").tobytes()

    async def say(self, name: str, pcm: Optional[bytes] = None, block: bool = True) -> Said:
        """Queue a caller utterance on the microphone; returns when it has been sent."""
        pcm = pcm if pcm is not None else load_wav(self.ctx.caller(name))
        while self._pending:
            await asyncio.sleep(0.005)
        start = now()
        self._pending.extend(pcm)
        seconds = len(pcm) / 2 / CALLER_RATE
        said = Said(name, start, start + last_voiced_ms(pcm) / 1000, start + seconds)
        if block:
            await asyncio.sleep(max(0.0, said.end - now()))
        return said

    def levels(self, since: float = 0.0, until: float = float("inf")) -> List[Tuple[float, float]]:
        """(receive time, dBFS) of each 20 ms of agent audio received in the window."""
        out = []
        for at, data in self.audio:
            if at < since or at > until:
                continue
            for n, start in enumerate(range(0, len(data), AGENT_FRAME)):
                out.append((at + n * FRAME_MS / 1000, frame_db(data[start:start + AGENT_FRAME])))
        return out

    def first_audible(self, since: float, until: float = float("inf")) -> Optional[float]:
        for at, db in self.levels(since, until):
            if db > AUDIBLE_DB:
                return at
        return None

    def last_audio(self) -> float:
        return self.audio[-1][0] if self.audio else 0.0

    def last_audio_before(self, at: float) -> float:
        return max((when for when, _ in self.audio if when < at), default=at)


class RoomCaller(Mic):
    """A participant of a LiveKit room: its microphone is a published track, it hears the
    agent's track, and it keeps what arrives on the room's interaction data topic. `kind`
    "sip" makes the server report it as a phone participant."""

    def __init__(self, ctx: "Ctx", room: str, identity: str, kind: Optional[str] = None):
        super().__init__(ctx)
        self.room_name, self.identity, self.kind = room, identity, kind
        self.events: List[Tuple[float, dict]] = []
        self._tasks: List[asyncio.Task] = []
        self.room = None
        self.source = None

    async def join(self) -> "RoomCaller":
        from livekit import api, rtc
        token = (api.AccessToken(LIVEKIT_KEY, LIVEKIT_SECRET).with_identity(self.identity)
                 .with_grants(api.VideoGrants(room_join=True, room=self.room_name)))
        if self.kind:
            token = token.with_kind(self.kind)
        self.room = rtc.Room()

        def subscribed(track, publication, participant):
            if track.kind == rtc.TrackKind.KIND_AUDIO:
                stream = rtc.AudioStream(track, sample_rate=AGENT_RATE, num_channels=1)
                self._tasks.append(asyncio.create_task(self._listen(stream)))

        def received(packet):
            if packet.topic == ROOM_TOPIC:
                event = json.loads(packet.data)
                self.events.append((now(), event))
                if event.get("type") == "agent_audio_stopped":
                    self._playout.clear()

        self.room.on("track_subscribed", subscribed)
        self.room.on("data_received", received)
        await self.room.connect(f"ws://127.0.0.1:{self.ctx.args.livekit_port}", token.to_jwt())
        self.source = rtc.AudioSource(CALLER_RATE, 1, queue_size_ms=100)
        track = rtc.LocalAudioTrack.create_audio_track("microphone", self.source)
        options = rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_MICROPHONE)
        await self.room.local_participant.publish_track(track, options)
        self._frame = lambda data: rtc.AudioFrame(data, CALLER_RATE, 1, len(data) // 2)
        self._tasks.append(asyncio.create_task(self._mic()))
        self.ctx.rooms.append(self)
        return self

    async def _listen(self, stream):
        async for event in stream:
            self._heard(bytes(event.frame.data))

    async def _send_audio(self, frame: bytes):
        await self.source.capture_frame(self._frame(frame))

    async def leave(self):
        self._mic_on = False
        for task in self._tasks:
            task.cancel()
        if self in self.ctx.rooms:
            self.ctx.rooms.remove(self)
        try:
            await asyncio.wait_for(self.room.disconnect(), 5)
        except Exception:
            pass


class Session(Mic):
    """A `/ws` session with a live microphone. Every event and every audio frame is kept with
    its receive time."""

    def __init__(self, ctx: "Ctx", label: str):
        super().__init__(ctx)
        self.label = label
        self.events: List[Tuple[float, dict]] = []
        self.ready: dict = {}
        self.warnings: List[dict] = []
        self._tasks: List[asyncio.Task] = []
        self.ws = None
        self.closed: Optional[Tuple[float, Optional[int], Optional[str]]] = None

    @classmethod
    async def open(cls, ctx: "Ctx", label: str, interaction: Any = None,
                   system_prompt: Optional[str] = None, mic: bool = True,
                   voice: str = "", stt_features: Optional[dict] = None,
                   conversation: bool = True, language: str = "en",
                   agent: Optional[str] = None, livekit: Optional[dict] = None,
                   extra: Optional[dict] = None, port: Optional[int] = None,
                   turn_detection: bool = False) -> "Session":
        self = cls(ctx, label)
        # The gateway caches synthesised speech by voice and text for the whole process. A
        # voice name of this run keeps a line an earlier run or session spoke out of the
        # measurement (the local synthesiser has one voice and ignores the name).
        voice = f"live-{ctx.run_id}{voice}"
        stt = {"provider": "openai", "model": "whisper-1"}
        if ctx.args.stt == "streaming":
            stt = {"provider": "deepgram", "model": "nova-2", "api_key": "local-dummy",
                   "extras": {"endpoint_override": f"ws://127.0.0.1:{ctx.args.stt_port}"}}
        if ctx.args.stt == "streaming" and ctx.args.stt_vendor == "deepgram" and port is None:
            # The production recogniser (`--stt-vendor deepgram`): Deepgram nova-3 streaming with
            # the gateway's own DEEPGRAM_API_KEY (`--deepgram-env`), not the local Whisper stand-in.
            stt = {"provider": "deepgram", "model": "nova-3"}
        tts = {"provider": "openai", "model": "tts-1", "voice_id": voice, "audio_format": "pcm", "sample_rate": AGENT_RATE}
        if ctx.args.tts_vendor:
            # A vendor's voice (plan 9.4; its key in the gateway's environment): provider:model:voice.
            provider, model, vendor_voice = ctx.args.tts_vendor.split(":", 2)
            tts = {"provider": provider, "model": model, "voice_id": vendor_voice, "audio_format": "linear16",
                   "sample_rate": AGENT_RATE}
        config = {
            "type": "config",
            "stt_config": {**stt, "language": language, "sample_rate": CALLER_RATE, "channels": 1,
                           "punctuation": True, "encoding": "linear16"},
            "tts_config": tts,
            "conversation_config": {
                "base_url": f"http://127.0.0.1:{ctx.args.background_port}/v1",
                "model": ctx.args.background_alias, "api_key": "local-dummy",
            },
        }
        if stt_features:
            config["stt_config"]["features"] = stt_features
        if turn_detection:
            # The gateway's own Silero and Smart Turn on the speech-to-text's audio.
            config["stt_config"]["turn_detection"] = {"enabled": True}
        if system_prompt:
            config["conversation_config"]["system_prompt"] = system_prompt
        if not conversation:
            # The session's only background is `interaction.tasks.llm`.
            del config["conversation_config"]
        if interaction is not None:
            config["interaction"] = interaction
        if livekit is not None:
            # The caller is in a LiveKit room: the socket only drives the session.
            config["livekit"] = livekit
        config.update(extra or {})
        self.log_mark = ctx.stack.log_size()
        url, headers = ctx.args.gateway_ws, {}
        if port is not None:
            # Another gateway process of this run (`Stack.ensure_durable`).
            url = f"ws://127.0.0.1:{port}/ws"
        if agent:
            # A voice-agent session on the Bud-mode gateway: the agent names both speech
            # deployments, the session only its audio format, and the caller's key is the
            # credential of every run.
            config = {
                "type": "config", "agent": {"id": agent},
                "stt_config": {"sample_rate": CALLER_RATE, "channels": 1, "punctuation": True,
                               "encoding": "linear16", "language": ""},
                "tts_config": {"audio_format": "pcm", "sample_rate": AGENT_RATE},
            }
            if interaction is not None:
                config["interaction"] = interaction
            url = f"ws://127.0.0.1:{ctx.args.bud_gateway_port}/ws"
            headers = {"Authorization": f"Bearer {BUD_KEY}"}
        self.ws = await websockets.connect(url, max_size=None, ping_interval=None,
                                           additional_headers=headers)
        self.opened = now()
        await self.ws.send(json.dumps(config))
        self._tasks.append(asyncio.create_task(self._read()))
        found = await self.wait(lambda e: e["type"] in ("ready", "error"), 60)
        if found is None or found[2]["type"] != "ready":
            await self.close()
            raise RuntimeError(f"{label}: no ready: {found[2] if found else 'timeout'}")
        self.ready = found[2]
        ctx.live.append(self)
        self.warnings = [e for _, e in self.events if e["type"] == "config_warning"]
        if mic:
            self._tasks.append(asyncio.create_task(self._mic()))
        return self

    @property
    def stream_id(self) -> str:
        return self.ready.get("stream_id", "?")

    async def _read(self):
        try:
            async for frame in self.ws:
                if isinstance(frame, bytes):
                    self._heard(frame)
                else:
                    self.events.append((now(), json.loads(frame)))
                    if self.events[-1][1].get("type") == "agent_audio_stopped":
                        self._playout.clear()
        except Exception:
            pass
        # When the gateway closed the socket, with its code and reason (SF2-SF4).
        self.closed = (now(), getattr(self.ws, "close_code", None), getattr(self.ws, "close_reason", None))

    async def _send_audio(self, frame: bytes):
        await self.ws.send(frame)

    async def send(self, message: dict):
        await self.ws.send(json.dumps(message))

    async def wait(self, match: Callable[[dict], bool], timeout: float, start: int = 0,
                   after: float = 0.0) -> Optional[Tuple[int, float, dict]]:
        """The first event from index `start` on, received at or after `after`, that matches."""
        deadline = now() + timeout
        index = start
        while True:
            while index < len(self.events):
                at, event = self.events[index]
                if at >= after and match(event):
                    return index, at, event
                index += 1
            if now() >= deadline:
                return None
            await asyncio.sleep(0.005)

    def all(self, kind: str, start: int = 0) -> List[Tuple[float, dict]]:
        return [(at, e) for at, e in self.events[start:] if e["type"] == kind]

    def open_tasks(self, start: int = 0) -> List[str]:
        live: List[str] = []
        for _, event in self.events[start:]:
            if event["type"] == "task_created":
                live.append(event["task_id"])
            elif event["type"] == "task_cancelled" and event["task_id"] in live:
                live.remove(event["task_id"])
            elif event["type"] == "task_result" and event["task_id"] in live:
                live.remove(event["task_id"])
        return live

    def floor(self) -> Optional[dict]:
        states = self.all("floor_state")
        return states[-1][1] if states else None

    async def quiet(self, timeout: float, since_event: int = 0, hold: float = 0.7) -> bool:
        """Wait until the agent has nothing more to say: no open task, the floor idle and no
        audio for `hold` seconds."""
        deadline = now() + timeout
        while now() < deadline:
            floor = self.floor()
            idle = floor is None or floor.get("holder") == "none"
            silent = now() - max(self.last_audio(), self.opened) > hold
            if idle and silent and not self.open_tasks(since_event):
                return True
            await asyncio.sleep(0.05)
        return False

    async def turn(self, name: str, pcm: Optional[bytes] = None, final_timeout: float = 10.0,
                   settle: float = 60.0) -> Turn:
        """One caller turn: speak, wait for E, then for the agent to finish."""
        first_event = len(self.events)
        said = await self.say(name, pcm)
        is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
        final = await self.wait(is_final, final_timeout, first_event)
        text = final[2].get("transcript", "") if final else ""
        # A caller who pauses mid-sentence is transcribed in pieces: E is the last one.
        while final and (more := await self.wait(is_final, 0.6, final[0] + 1)):
            final, text = more, f"{text} {more[2].get('transcript', '')}"
        turn = Turn(said, final[1] if final else None, text, first_event)
        if final and settle:
            await asyncio.sleep(1.0)
            await self.quiet(settle, first_event)
        return turn

    async def close(self, lost: bool = False):
        """Ends the session. `lost`: as a connection that drops does, with no close frame."""
        self._mic_on = False
        try:
            if self.ws is not None and lost:
                self.ws.transport.abort()
            elif self.ws is not None:
                await self.ws.close()
        except Exception:
            pass
        for task in self._tasks:
            task.cancel()
        if self in self.ctx.live:
            self.ctx.live.remove(self)
            self.ctx.record(self)

    # -- what the session shows

    def errors(self) -> List[dict]:
        return [e for _, e in self.events if e["type"] == "error"]

    def transcripts(self, start: int = 0, kind: Optional[str] = None) -> List[Tuple[float, dict]]:
        """`assistant_transcript` events that carry words."""
        return [(at, e) for at, e in self.all("assistant_transcript", start)
                if e.get("delta") and (kind is None or e.get("kind") == kind)]

    def audio_at(self, at: float) -> Optional[float]:
        """The first audible agent frame of the utterance whose transcript arrived at `at`
        (the transcript and its first frame travel together)."""
        return self.first_audible(at - 0.06)


# The `/ws` name of each interaction event that keeps its body under a `bud.*` name.
BUD_EVENTS = {
    "bud.task.created": "task_created", "bud.task.progress": "task_progress",
    "bud.task.question": "task_question", "bud.task.answer_received": "task_answer_received",
    "bud.task.result": "task_result", "bud.task.cancelled": "task_cancelled",
    "bud.floor.state": "floor_state", "bud.audio.stopped": "agent_audio_stopped",
    "bud.interaction.updated": "interaction_updated", "bud.warning": "config_warning",
    "bud.claim.unverified": "unverified_claim",
}


class Realtime(Session):
    """An OpenAI Realtime client on `/v1/realtime?model=prompt:<agent>` of the Bud-mode gateway:
    the caller's audio goes up as `input_audio_buffer.append` (24 kHz), the agent's comes back as
    `response.output_audio.delta`.

    `wire` is every event as it came. `events` is what a `/ws` client of the same session would
    have got, rebuilt from the wire alone: a `bud.*` event under its `/ws` name, each response
    with `metadata.bud_utterance_id` as the two `assistant_transcript` events of its line, each
    final transcript as an `stt_result`. The checks of the `/ws` scenarios then read it."""

    @classmethod
    async def open(cls, ctx: "Ctx", label: str, agent: str, session: Optional[dict] = None,
                   interaction: bool = True, mic: bool = True, model: Optional[str] = None,
                   port: Optional[int] = None) -> "Realtime":
        self = cls(ctx, label)
        self.wire: List[Tuple[float, dict]] = []
        self.session_id = ""
        # response id -> its metadata; item id -> the response it belongs to
        self._responses: Dict[str, dict] = {}
        self.log_mark = ctx.stack.log_size()
        url = f"ws://127.0.0.1:{port or ctx.args.bud_gateway_port}/v1/realtime?model={model or 'prompt:' + agent}"
        self.ws = await websockets.connect(
            url, max_size=None, ping_interval=None, subprotocols=["realtime"],
            additional_headers={"Authorization": f"Bearer {BUD_KEY}"})
        self.opened = now()
        self._tasks.append(asyncio.create_task(self._read()))
        if await self.wired("session.created", 20) is None:
            raise RuntimeError(f"{label}: no session.created")
        await self.send({"type": "session.update", "session": {"type": "realtime", **(session or {})}})
        if await self.wired("session.updated", 20) is None:
            raise RuntimeError(f"{label}: no session.updated: {self.wire[-1][1] if self.wire else None}")
        if interaction:
            found = await self.wait(lambda e: e["type"] in ("ready", "error"), 60)
            if found is None or found[2]["type"] != "ready":
                await self.close()
                raise RuntimeError(f"{label}: no bud.interaction.state: {found[2] if found else 'timeout'}")
            self.ready = found[2]
        else:
            self.ready = {"stream_id": self.session_id}
        ctx.live.append(self)
        self.warnings = [e for _, e in self.events if e["type"] == "config_warning"]
        if mic:
            self._tasks.append(asyncio.create_task(self._mic()))
        return self

    async def _send_audio(self, frame: bytes):
        pcm = resample(frame, CALLER_RATE, AGENT_RATE)
        await self.send({"type": "input_audio_buffer.append", "audio": base64.b64encode(pcm).decode()})

    async def _read(self):
        try:
            async for frame in self.ws:
                at, event = now(), json.loads(frame)
                kind = event.get("type", "")
                if kind == "response.output_audio.delta":
                    self._heard(base64.b64decode(event["delta"]))
                    # Kept without its payload: the order and the response it belongs to.
                    event = {**event, "delta": len(event["delta"])}
                self.wire.append((at, event))
                mirror = self._as_ws(kind, event)
                if mirror is not None:
                    self.events.append((at, mirror))
        except Exception:
            pass

    def _as_ws(self, kind: str, event: dict) -> Optional[dict]:
        if kind in BUD_EVENTS:
            if kind == "bud.audio.stopped":
                self._playout.clear()
            return {**event, "type": BUD_EVENTS[kind]}
        if kind == "session.created":
            self.session_id = event["session"]["id"]
        elif kind == "bud.interaction.state":
            block = event["interaction"]
            return {"type": "ready", "protocol_version": block.get("protocol_version"),
                    "stream_id": self.session_id, "interaction": block}
        elif kind == "response.created":
            self._responses[event["response"]["id"]] = event["response"].get("metadata") or {}
        elif kind == "response.output_audio_transcript.delta":
            meta = self._responses.get(event.get("response_id"), {})
            return {"type": "assistant_transcript", "turn_index": meta.get("bud_turn"),
                    "delta": event["delta"], "source": meta.get("bud_source"),
                    "kind": meta.get("bud_kind"), "verbatim": meta.get("bud_verbatim"),
                    "task_id": meta.get("bud_task_id"), "utterance_id": meta.get("bud_utterance_id"),
                    "heard": "none"}
        elif kind == "response.done":
            response = event["response"]
            meta = response.get("metadata") or {}
            if meta.get("bud_utterance_id"):
                heard = "full" if response.get("status") == "completed" else "partial"
                return {"type": "assistant_transcript", "delta": "", "kind": meta.get("bud_kind"),
                        "utterance_id": meta["bud_utterance_id"], "heard": heard}
        elif kind == "conversation.item.input_audio_transcription.delta":
            return {"type": "stt_result", "transcript": event.get("delta", "").strip(), "is_final": True}
        elif kind == "error":
            error = event.get("error", {})
            return {"type": "error", "message": error.get("message"), "code": error.get("code")}
        return None

    async def wired(self, kind: str, timeout: float, start: int = 0,
                    match: Callable[[dict], bool] = lambda e: True) -> Optional[Tuple[int, float, dict]]:
        """The first wire event of this type from index `start` on that matches."""
        deadline = now() + timeout
        index = start
        while True:
            while index < len(self.wire):
                at, event = self.wire[index]
                if event.get("type") == kind and match(event):
                    return index, at, event
                index += 1
            if now() >= deadline:
                return None
            await asyncio.sleep(0.005)

    def of(self, kind: str, start: int = 0) -> List[Tuple[float, dict]]:
        return [(at, e) for at, e in self.wire[start:] if e.get("type") == kind]

    async def close(self):
        await super().close()
        out = Path(self.ctx.args.out_dir)
        out.mkdir(parents=True, exist_ok=True)
        with open(out / f"{self.label}.wire.jsonl", "w") as handle:
            for at, event in self.wire:
                handle.write(json.dumps({"rx_ms": ms(at - self.opened), **event}) + "\n")

    async def played(self, start: int = 0, margin: float = 0.6):
        """Wait until the audio received since wire index `start` has played out at a client: a
        session without interaction mode sends it faster than real time, and a run that
        starts while the last answer still plays cuts that answer."""
        first = next((at for at, e in self.wire[start:] if e.get("type") == "response.output_audio.delta"), None)
        if first is None:
            return
        seconds = sum(len(data) for at, data in self.audio if at >= first) / 2 / AGENT_RATE
        await asyncio.sleep(max(0.0, first + seconds + margin - now()))

    async def typed(self, text: str, response: Optional[dict] = None):
        """A typed user message and the `response.create` that starts its run."""
        await self.send({"type": "conversation.item.create", "item": {
            "type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}})
        await self.send({"type": "response.create", **({"response": response} if response else {})})

    def well_formed(self, start: int = 0) -> List[str]:
        """What is wrong with the responses on the wire from `start` on: one that never ended,
        audio or words outside an open response, an item that is not the response's."""
        wrong, open_id, items = [], None, set()
        for _, event in self.wire[start:]:
            kind = event.get("type", "")
            if kind == "response.created":
                if open_id:
                    wrong.append(f"{event['response']['id']} created while {open_id} is open")
                open_id, items = event["response"]["id"], set()
            elif kind == "response.output_item.added":
                items.add(event["item"]["id"])
            elif kind in ("response.output_audio.delta", "response.output_audio_transcript.delta"):
                if event.get("response_id") != open_id or event.get("item_id") not in items:
                    wrong.append(f"{kind} outside its response")
            elif kind == "response.done":
                if event["response"]["id"] != open_id:
                    wrong.append(f"{event['response']['id']} done while {open_id} is open")
                open_id = None
        return wrong


# ---- the run


@dataclass
class Result:
    id: str
    checks: str
    passed: bool
    measured: str
    threshold: str
    notes: str = ""
    # A scenario that needs what this run does not have (a vendor key): neither passed nor failed.
    skipped: bool = False


@dataclass
class Ctx:
    args: Any
    stack: Stack
    webhook: Webhook
    results: List[Result] = field(default_factory=list)
    sessions: List[Tuple[str, str]] = field(default_factory=list)
    # Sessions that are open: a scenario that stops half way still leaves its record.
    live: List["Session"] = field(default_factory=list)
    # Room participants that have not left yet.
    rooms: List["RoomCaller"] = field(default_factory=list)
    marker_noncompliant: bool = False
    run_id: str = field(default_factory=lambda: os.urandom(3).hex())
    # Every sentence a session relayed from the background (the closing check LABEL).
    relayed: List[str] = field(default_factory=list)
    # The size of the gateway's log when the run began.
    log_start: int = 0
    push_sink: Optional[Webhook] = None

    def sink(self) -> Webhook:
        """Where the task gateways push results nobody was there for (DT1 to DT3)."""
        if self.push_sink is None:
            self.push_sink = Webhook(self.args.tool_secret)
        return self.push_sink

    def caller(self, name: str) -> Path:
        """The caller clip `name`; one of NEW_CLIPS that is not on disk is synthesised first. A
        clip with a documented shape (CLIP_SHAPES) is put in that shape, re-synthesised when it
        cannot be (the synthesiser differs on every run), and refused after CLIP_TRIES."""
        path = Path(self.args.audio_dir) / f"{name}.wav"
        pcm = load_wav(path) if path.exists() else None
        if pcm is None and name in VENDOR_CLIPS:
            pcm = self.vendor_speech(*VENDOR_CLIPS[name])
        if pcm is None and name in NEW_CLIPS:
            pcm = self.synthesise(NEW_CLIPS[name])
        if pcm is not None and name in CLIP_SHAPES:
            shape, fit = CLIP_SHAPES[name]
            shaped, tries, seen = fit(pcm), 0, [pauses_ms(pcm)]
            while shaped is None and name in NEW_CLIPS and tries < CLIP_TRIES:
                pcm, tries = self.synthesise(NEW_CLIPS[name]), tries + 1
                shaped = fit(pcm)
                seen.append(pauses_ms(pcm))
            if shaped is None:
                raise RuntimeError(f"caller clip {name}: not {shape} after {tries} syntheses (pauses "
                                   f"of 200 ms or more, ms: {seen})")
            if shaped == pcm and path.exists() and tries == 0:
                return path
            print(f"  caller clip {name} written {shape}: pauses {pauses_ms(shaped)} (start, ms) after "
                  f"{tries} new syntheses; pauses as synthesised {seen[-1]}")
            pcm = shaped
        if pcm is not None and (name in CLIP_SHAPES or not path.exists()):
            path.parent.mkdir(parents=True, exist_ok=True)
            with wave.open(str(path), "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)
                wav.setframerate(CALLER_RATE)
                wav.writeframes(pcm)
        return path

    def synthesise(self, text: str) -> bytes:
        """`text` from the speech server, at the caller's rate."""
        body = json.dumps({"model": "tts-1", "input": text, "voice": "alloy", "response_format": "pcm"}).encode()
        request = urllib.request.Request(f"{self.args.audio_url}/v1/audio/speech", data=body,
                                         headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=120) as response:
            return resample(response.read(), AGENT_RATE, CALLER_RATE)

    def vendor_speech(self, text: str, voice: str) -> bytes:
        """`text` in the Deepgram Aura `voice`, through the controls gateway's `/speak` (the key
        stays in that gateway), at the caller's rate. It needs the Deepgram key."""
        port = self.stack.ensure_controls_gateway()
        if port is None:
            raise RuntimeError(f"a {voice} caller clip needs DEEPGRAM_API_KEY (or --deepgram-env)")
        body = json.dumps({"text": text, "tts_config": {"provider": "deepgram", "voice_id": voice,
                                                         "audio_format": "linear16", "sample_rate": CALLER_RATE}}).encode()
        request = urllib.request.Request(f"http://127.0.0.1:{port}/speak", data=body,
                                         headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=60) as response:
            pcm = response.read()
        print(f"  caller clip synthesised with Deepgram {voice}: {len(text)} characters")
        return pcm[44:] if pcm[:4] == b"RIFF" else pcm

    def add(self, *args, **kwargs):
        result = Result(*args, **kwargs)
        self.results.append(result)
        print(f"  {verdict(result)} {result.id}: {result.measured}"
              + (f" [{result.notes}]" if result.notes else ""))

    def record(self, session: Session):
        """Keep the session's events and the agent's audio for the report."""
        out = Path(self.args.out_dir)
        out.mkdir(parents=True, exist_ok=True)
        self.sessions.append((session.label, session.stream_id))
        self.relayed += [e["delta"] for _, e in session.transcripts(kind="relay")]
        with open(out / f"{session.label}.events.jsonl", "w") as handle:
            for at, event in session.events:
                handle.write(json.dumps({"rx_ms": ms(at - session.opened), **event}) + "\n")
        with open(out / f"{session.label}.levels.csv", "w") as handle:
            for at, db in session.levels():
                handle.write(f"{ms(at - session.opened)},{db:.1f}\n")
        if session.audio:
            pcm = bytearray()
            origin = session.audio[0][0]
            for at, data in session.audio:
                # Keep the gaps between utterances.
                gap = int((at - origin) * AGENT_RATE) * 2 - len(pcm)
                if gap > AGENT_FRAME * 3:
                    pcm.extend(bytes(gap))
                pcm.extend(data)
            with wave.open(str(out / f"{session.label}.agent.wav"), "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)
                wav.setframerate(AGENT_RATE)
                wav.writeframes(bytes(pcm))

    async def summary(self, session: Session) -> Dict[str, str]:
        """The gateway's summary line of a closed session."""
        for _ in range(40):
            found = self.stack.summaries().get(session.stream_id)
            if found:
                return found
            await asyncio.sleep(0.1)
        return {}


# Caller clips of the scenarios D1, H1, P1 and S1, by text: made with the speech server when missing.
NEW_CLIPS = {
    # One "Stop." is 260 ms of synthetic speech that Whisper hears as "TOP!" or "SOP!".
    "47_stop_stop": "Stop, stop.",
    "48_availability_sunday": "Can you check the availability at the Olive Garden for Sunday evening?",
    "49_second_one": "A second one, please.",
    "50_order_flowers": "I'd like to order some flowers.",
    "51_tulips": "The tulips, please.",
    "52_name_jordan": "It's for Jordan.",
    "53_card_number": "My card number is 4111 1111 1111 1111.",
    "54_reset_password": "I need to reset my password.",
    "55_write_report": "Can you write up the monthly report for me?",
    "59_pay_bill": "I'd like to pay my bill, please.",
    # MA1: two tasks for two agents and a general question, in quick succession.
    "60_book_flight": "Book me a flight to Delhi on Friday.",
    "61_fix_test": "Also, please fix the failing test in my code repository.",
    "62_capital_peru": "What's the capital of Peru?",
    # MA1 since its fourth run: the two tasks with the details a planner asks for first.
    "63_book_flight_from": "Book me the morning flight from Mumbai to Delhi this Friday.",
    "64_fix_named_test": "Also, fix the failing test test_parse_dates in my waav repository.",
    "65_no_preference": "No preference, just go ahead.",
    # MA1-updates: the caller takes the offer of the updates.
    "66_yes_please": "Yes, please.",
    # MA2: a long answer the airline's question breaks into, and the caller's answer.
    "67_lima_four_sentences": "Tell me about Lima, the capital of Peru, in four sentences.",
    "68_window_please": "Window, please.",
    # MA3: a question about the finished flight, and more work for its agent.
    "69_when_leave": "What time does the flight leave?",
    "70_window_too": "Can it be a window seat?",
    # MA5: one request said in two goes.
    "71_apartment_first": "I need a pet friendly apartment in Miami.",
    "72_apartment_more": "And my budget is about fifteen hundred dollars a month.",
    # MA6: a request still running when the caller hangs up and calls back.
    "73_parcel_status": "Where is my parcel? Can you check its delivery status?",
    # MA7: agents known by what they say of themselves.
    "74_hotel_goa": "Find me a hotel in Goa for two nights.",
    "75_budget": "About two hundred dollars a night.",
    "76_search_dentist": "Do I have a note about the dentist?",
    "77_delete_note": "Delete my note called old shopping list.",
    "78_yes_go_ahead": "Yes, go ahead.",
    # CT1 to CT3: conversation controls (chapter 9), decided by the front model in context.
    "80_slower_please": "Could you speak a bit slower, please?",
    "81_faster_please": "You can talk faster now.",
    "82_say_again": "Sorry, can you say that again?",
    "83_the_number": "What was the number again?",
    "84_in_spanish": "Can you talk to me in Spanish, please?",
    "85_in_chinese": "Can you talk to me in Chinese?",
    "86_short_answers": "Please keep your answers short. What is the capital of France?",
    "87_store_open": "What time does the store open?",
    # CT4 to CT7 (gap round): a person, another voice, a line again slower or louder.
    # CT6: one breath each (live, "Can you say that last part again, slower?" was recognised in
    # two pieces, "...again? slower.", and the front read it as a rate control).
    "94_last_part_slowly": "Please say that last part again more slowly.",
    "95_again_louder": "Can you say that again louder please?",
    "90_a_person": "Let me talk to a person, please.",
    "91_male_voice": "Could you switch to a male voice?",
    "79_reminder": "Remind me to call mom on Friday at six pm, it's high priority.",
    # T1 and L7: a table and flowers asked for in one turn (one breath: CLIP_SHAPES).
    "16_book_and_flowers": "Please book a table for two at the Olive Garden on Saturday at 7pm and order "
                           "flowers for the table.",
}


def pauses_ms(pcm: bytes, least_ms: int = 200) -> List[Tuple[int, int]]:
    """The caller clip's pauses of `least_ms` or more between voiced 20 ms frames (louder than
    -45 dBFS, long_story's measure): (start, length) in ms. 200 ms of a pause is when the
    gateway asks Smart Turn whether the caller has finished."""
    loud = [frame_db(pcm[at:at + CALLER_FRAME]) > -45 for at in range(0, len(pcm) - CALLER_FRAME + 1, CALLER_FRAME)]
    voiced = [n for n, on in enumerate(loud) if on]
    return [(a * FRAME_MS + FRAME_MS, (b - a - 1) * FRAME_MS) for a, b in zip(voiced, voiced[1:])
            if (b - a - 1) * FRAME_MS >= least_ms]


def one_breath(pcm: bytes) -> Optional[bytes]:
    """The clip as it is when it has no pause the gateway judges, else None."""
    return None if pauses_ms(pcm) else pcm


def comma_pauses(count: int, low_ms: int, high_ms: int) -> Callable[[bytes], Optional[bytes]]:
    """A clip with `count` pauses (one at each comma), each set to the middle of low_ms to
    high_ms: the silence in the middle of each pause is cut or lengthened, the voice is the
    synthesiser's (paused_story stretches the same pauses). None when the synthesiser made
    another number of pauses."""
    def fit(pcm: bytes) -> Optional[bytes]:
        found = pauses_ms(pcm)
        if len(found) != count:
            return None
        if all(low_ms <= length <= high_ms for _, length in found):
            return pcm
        target, out, cursor = (low_ms + high_ms) // 2, bytearray(), 0
        for start, length in found:
            middle = (start + length // 2) * CALLER_RATE // 1000 * 2
            keep = (length - target) * CALLER_RATE // 1000 * 2
            # A cut is centred on the middle, so a pause over twice the target keeps its voice.
            half = max(0, keep) // 4 * 2
            out += pcm[cursor:middle - half] + bytes(max(0, -keep))
            cursor = middle - half + max(0, keep)
        out += pcm[cursor:]
        shaped = bytes(out)
        return shaped if all(low_ms <= length <= high_ms for _, length in pauses_ms(shaped)) \
            and len(pauses_ms(shaped)) == count else None
    return fit


# Caller clips synthesised once with a Deepgram Aura voice (`Ctx.vendor_speech`): (text, voice).
VENDOR_CLIPS = {
    # BC1's long turn: pauses of 320 to 380 ms at each comma (CLIP_SHAPES), with the voice of a
    # sentence that goes on. The local Piper voice ends a sentence at its first comma (Smart
    # Turn p 0.96 to 0.99 in 28 syntheses, r16); Aura's comma prosody is a speaker's (owner, 26
    # Oct). (Three phrases synthesised apart and joined with silence end each one as a finished
    # sentence: Smart Turn took the first pause for the end, p 0.97.)
    "56_story": ("So I am planning a trip with my family next spring, and we want somewhere warm "
                 "by the sea, where the kids can swim and play, and we also need a hotel that is "
                 "close to the beach.", "aura-2-thalia-en"),
}
# Caller clips whose shape a scenario relies on: (the shape, a function that returns the clip in
# that shape or None). The local Piper differs on every synthesis (live after the 25 Oct reboot:
# 56_story paused 420 to 460 ms, 16_book_and_flowers paused 420 ms at a comma and split there).
CLIP_SHAPES = {
    # BC1's comment: 320 to 380 ms at each of the three commas (BC2 needs them shorter than
    # Smart Turn's answer; TP1 and CL1 stretch them).
    "56_story": ("three comma pauses of 320 to 380 ms", comma_pauses(3, 320, 380)),
    "16_book_and_flowers": ("one breath (no pause of 200 ms or more)", one_breath),
}
CLIP_TRIES = 8
# Every session runs on the model front (chapter 9: the scripted front and the rule router are
# gone; `scripted` is refused).
MODEL_FRONT = {"front": {"profile": "model"}}
# The background answers general questions here; one short sentence keeps the runs comparable.
PLAIN_PROMPT = "You answer a caller's question in one short sentence."
# Segmented speech-to-text's contract (release 1.0.0): a session on a model that transcribes
# uploaded files (the local Whisper of `--stt request`) is told so before `ready` with this
# config_warning, mode or no mode. It is about the recogniser, not interaction mode, so the
# checks that a session runs the mode "with no warnings" leave it out.
STT_MODEL_NOTICES = frozenset({"stt_buffered_until_commit"})


def mode_warnings(warnings: List[dict]) -> List[dict]:
    """The `config_warning`s before `ready` other than the recogniser's own notices."""
    return [w for w in warnings if w.get("code") not in STT_MODEL_NOTICES]
QUESTIONS = ["20_q_capital", "21_q_week", "22_q_sky", "23_q_planet", "24_q_ocean"]
# Five things a caller asks a help line to do (the model front's delegate turns).
REQUESTS = ["01_flight_request", "02_bag_question", "30_seat", "31_hotel", "32_invoice"]
TOOLS_PROMPT = """You are the booking desk of the Olive Garden restaurant.
- To look up free tables, call check_availability.
- To book, call book_table at once with what the caller said. Never ask the caller to confirm and never use ask_caller for a booking: the phone system reads the booking back and gets the caller's yes before anything is booked.
- When the caller wants a table and flowers, call both book_table and order_flowers in your first reply.
- After a tool result, tell the caller the outcome in one short sentence."""


def slow_prompt() -> str:
    """An operator prompt the background model has to read in full on every session: a
    session mark at its top defeats the model server's prompt cache, and the notes take it
    about 0.7 s to read. This is the slow background that interaction mode exists for; the
    local model alone answers before the acknowledgement is due."""
    notes = "\n".join(
        f"Fare note {n}: on route {n} the Standard fare includes the first checked bag, the "
        f"Basic fare includes no checked bag, and a second bag costs {30 + n} dollars."
        for n in range(40))
    return (f"Session {os.urandom(4).hex()}.\nYou are an airline help desk. Answer in two short "
            f"sentences from these notes.\n{notes}")


def gateway_system_prompt(operator: str, tools: bool) -> str:
    """The system prompt a background run gets: the default persona's preamble, the
    operator's prompt and, with tools, the answer-marker rule (all read from the source)."""
    source = REPO_GATEWAY / "src/core/interaction"
    block = (source / "assets/front_system_prompt.md").read_text()
    block = block.split("<!-- template:background_preamble -->")[1]
    block = block.split("```text\n")[1].split("\n```")[0]
    slots = {"name": "the assistant", "org": "this help line", "channel": "phone",
             "answer_words": "40", "tone": "warm", "formality": "informal",
             "language_name": "English", "never_give_line": "", "caller_context_block": ""}
    lines = []
    for line in block.splitlines():
        alone = re.fullmatch(r"\s*\{\{(\w+)\}\}\s*", line)
        if alone and slots.get(alone.group(1)) == "":
            continue
        for name, value in slots.items():
            line = line.replace("{{" + name + "}}", value)
        lines.append(line)
    rule = ""
    if tools:
        rust = (source / "background.rs").read_text()
        literal = re.search(r'const ANSWER_RULE: &str = "((?:[^"\\]|\\.)*)";', rust, re.S).group(1)
        rule = re.sub(r"\\\n\s*", "", literal).replace("\\n", "\n")
    return "\n".join(lines).strip() + "\n\n" + operator.strip() + rule


def preflight(ctx: "Ctx") -> int:
    """Five tool prompts straight to the background model with the gateway's system prompt:
    on how many does the final round carry the answer marker?"""
    names = ("check_availability", "book_table", "order_flowers")
    tools = [{"type": "function", "function": {
        "name": tool["name"], "description": tool["description"], "parameters": tool["parameters"]}}
        for tool in map(ctx.webhook.tool, names)]
    prompts = [
        "Can you check the availability at the Olive Garden for Saturday evening?",
        "Do you have a free table on Friday?",
        "Is there anything free at the Olive Garden on Sunday at noon?",
        "Check the tables for tomorrow evening, please.",
        "What do you have free on Monday?",
    ]
    url = f"http://127.0.0.1:{ctx.args.background_port}/v1/chat/completions"
    marked = 0
    for prompt in prompts:
        messages = [{"role": "system", "content": gateway_system_prompt(TOOLS_PROMPT, True)},
                    {"role": "user", "content": prompt}]
        content, called = "", False
        for _ in range(4):
            body = {"model": ctx.args.background_alias, "messages": messages, "tools": tools}
            message = post_json(url, body)["choices"][0]["message"]
            calls = message.get("tool_calls") or []
            if not calls:
                content = message.get("content") or ""
                break
            called = True
            messages.append(message)
            for call in calls:
                result = {"available": True, "times": ["seven p m", "eight thirty p m"]}
                messages.append({"role": "tool", "tool_call_id": call["id"], "content": json.dumps(result)})
        has_marker = "ANSWER:" in content
        marked += has_marker
        print(f"  preflight: tool called {called}, marker {has_marker}: {content[:70]!r}")
    return marked



def describe(event: dict) -> str:
    kind = event["type"]
    if kind == "assistant_transcript":
        return f"{event.get('kind')}:{event.get('delta')!r}"
    return kind


async def l0(ctx: Ctx):
    """Three turns in one session: the per-utterance flush."""
    session = await Session.open(ctx, "L0", MODEL_FRONT, PLAIN_PROMPT)
    warm = await session.warm()
    turns = [await session.turn(name) for name in ("20_q_capital", "08_weather_smalltalk", "21_q_week")]
    await session.close()
    finals = [turn for turn in turns if turn.e is not None]
    errors = session.errors()
    texts = " | ".join(turn.text for turn in turns)
    ctx.add("L0", "three turns in one session (per-utterance flush)",
            len(finals) == 3 and not errors,
            f"{len(finals)} final stt_result(s), {len(errors)} error frame(s); heard: {texts}",
            "3 finals, 0 error frames", f"warm-up took {ms(warm)} ms")
    return session


async def first_audible_run(ctx: Ctx, label: str):
    """(first audio from E, from the last voiced sample, what it was, the session)."""
    session = await Session.open(ctx, label, MODEL_FRONT, slow_prompt())
    await session.warm()
    turn = await session.turn("02_bag_question")
    await session.close()
    first = session.first_audible(turn.e) if turn.e is not None else None
    if first is None:
        return None, None, "nothing", session
    spoken = session.transcripts(turn.first_event)
    what = spoken[0][1].get("kind", "?") if spoken else "?"
    return first - turn.e, first - turn.said.last_voiced, what, session


async def l1(ctx: Ctx):
    """First audible response, 5 runs."""
    from_e: List[float] = []
    from_voice: List[float] = []
    chunk_avg: List[float] = []
    first_was: List[str] = []
    for run in range(5):
        e_gap, voice_gap, what, session = await first_audible_run(ctx, f"L1-{run + 1}")
        first_was.append(what)
        if e_gap is not None:
            from_e.append(e_gap)
            from_voice.append(voice_gap)
        summary = await ctx.summary(session)
        if "analyser_chunk_avg_us" in summary:
            chunk_avg.append(float(summary["analyser_chunk_avg_us"]))
    if len(from_e) < 5:
        ctx.add("L1", "first audible response (5 runs)", False,
                f"only {len(from_e)} of 5 runs produced audio", "median <= E + 700 ms")
        return
    median_e, median_voice = statistics.median(from_e), statistics.median(from_voice)
    analyser_ms = statistics.mean(chunk_avg) / 1000 if chunk_avg else 0.0
    # The design allows E + 1,000 ms when the analyser's own cost shows a debug build.
    limit = 1.0 if analyser_ms > 8.0 else 0.7
    passed = median_e <= limit and max(from_voice) <= 2.5
    note = (f"first audio was {first_was}; analyser chunk average {analyser_ms:.1f} ms; the background "
            "here takes about a second to its first sentence (with one that answers before the "
            "acknowledgement is due none is spoken and the first audio is the answer: see L3)")
    if limit == 1.0:
        note += " (over 8 ms: debug-build overhead, limit E + 1,000 ms)"
    ctx.add("L1", "first audible response (02_bag_question, 5 runs)", passed,
            f"from E: median {ms(median_e)} ms, runs {[ms(v) for v in from_e]}; from the last "
            f"voiced sample: median {ms(median_voice)} ms, runs {[ms(v) for v in from_voice]}",
            f"median <= E + {ms(limit)} ms; every run <= 2,500 ms from the last voiced sample", note)


async def l2(ctx: Ctx):
    """Acknowledgement, then the background's answer word for word (no tools)."""
    session = await Session.open(ctx, "L2", MODEL_FRONT, slow_prompt())
    await session.warm()
    turn = await session.turn("02_bag_question")
    await session.close()
    start = turn.first_event
    order = [(at, e) for at, e in session.events[start:]
             if e["type"] in ("task_created", "task_result")
             or (e["type"] == "assistant_transcript" and e.get("delta"))]
    names = [describe(e) for _, e in order]
    created = next((at for at, e in order if e["type"] == "task_created"), None)
    acks = session.transcripts(start, "ack")
    relays = [(at, e) for at, e in session.transcripts(start, "relay") if e.get("verbatim")]
    results = [(at, e) for at, e in session.all("task_result", start)]
    completed = [e for _, e in results if e.get("state") == "completed"]
    ack_audio = session.audio_at(acks[0][0]) if acks else None
    relay_text = " ".join(e["delta"] for _, e in relays)
    summary = completed[0]["summary"] if completed else ""
    first_matches = bool(relays) and summary.startswith(relays[0][1]["delta"])
    ordered = bool(created and acks and relays and created <= acks[0][0] < relays[0][0]
                   and ack_audio is not None and ack_audio < relays[0][0])
    passed = ordered and bool(completed) and first_matches and relay_text == summary
    result_first = bool(results and relays and results[0][0] < relays[0][0])
    ctx.add("L2", "acknowledgement, then a verbatim relay (no tools)", passed,
            f"order: {' -> '.join(names)}; relay sentences {len(relays)}; relay == task_result.summary: "
            f"{relay_text == summary}; first relay sentence opens the summary: {first_matches}",
            "task_created -> ack audio and transcript -> relay transcript verbatim -> task_result "
            "completed; relay text equals the answer's first sentence",
            "task_result arrived before the relay transcript (it is sent when the run ends: "
            "DEVIATIONS S7 b)" if result_first else "")
    return session


async def mode_off_turn(ctx: Ctx, label: str, name: str, voice: str = "") -> Tuple[Optional[float], Session]:
    """One turn on a session without interaction mode, ended with `audio_end`."""
    session = await Session.open(ctx, label, None, PLAIN_PROMPT, mic=False, voice=voice)
    pcm = load_wav(ctx.caller(name))
    start = now()
    for n, at in enumerate(range(0, len(pcm), CALLER_FRAME)):
        await session.ws.send(pcm[at:at + CALLER_FRAME])
        await asyncio.sleep(max(0.0, start + (n + 1) * FRAME_MS / 1000 - now()))
    await session.send({"type": "audio_end"})
    final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 15)
    gap = None
    if final:
        deadline = now() + 30
        while now() < deadline and session.first_audible(final[1]) is None:
            await asyncio.sleep(0.02)
        first = session.first_audible(final[1])
        gap = None if first is None else first - final[1]
        # Let the answer finish so the next session starts on a quiet synthesiser.
        while now() < deadline and now() - session.last_audio() < 0.7:
            await asyncio.sleep(0.05)
    await session.close()
    return gap, session


async def l3(ctx: Ctx):
    """Time to the answer: mode on (5 turns, one session) against mode off (5 sessions)."""
    # Each mode has its own voice name, so neither finds the other's answers in the cache.
    session = await Session.open(ctx, "L3-on", MODEL_FRONT, PLAIN_PROMPT, voice="-on")
    await session.warm()
    on: List[Optional[float]] = []
    acked: List[bool] = []
    any_audio: List[Optional[float]] = []
    for name in QUESTIONS:
        turn = await session.turn(name)
        relays = session.transcripts(turn.first_event, "relay")
        first = session.audio_at(relays[0][0]) if relays and turn.e else None
        on.append(None if first is None else first - turn.e)
        acked.append(bool(session.transcripts(turn.first_event, "ack")))
        heard_first = session.first_audible(turn.e) if turn.e else None
        any_audio.append(None if heard_first is None else heard_first - turn.e)
    await session.close()
    off: List[Optional[float]] = []
    for n, name in enumerate(QUESTIONS):
        gap, _ = await mode_off_turn(ctx, f"L3-off-{n + 1}", name, voice="-off")
        off.append(gap)
    if None in on or None in off:
        ctx.add("L3", "time to answer, mode on against mode off", False,
                f"mode on {[ms(v) for v in on]} ms, mode off {[ms(v) for v in off]} ms (a turn has no answer audio)",
                "mode on <= mode-off median + 500 ms (turns 1 to 3), + 300 ms (turns 4, 5)")
        return
    base = statistics.median(off)
    early, late = statistics.median(on[:3]), statistics.median(on[3:])
    passed = early <= base + 0.5 and late <= base + 0.3
    ctx.add("L3", "time to answer from E, mode on against mode off", passed,
            f"mode off median {ms(base)} ms {[ms(v) for v in off]}; mode on turns 1-3 median "
            f"{ms(early)} ms, turns 4-5 median {ms(late)} ms {[ms(v) for v in on]}; "
            f"acknowledged turns {[n + 1 for n, a in enumerate(acked) if a]}; first agent audio of any "
            f"kind {[ms(v) for v in any_audio]} ms",
            "turns 1-3 median <= mode off + 500 ms; turns 4-5 median <= mode off + 300 ms")


async def l9(ctx: Ctx):
    """Mode off: no 1.1 event, the conversation path answers."""
    gap, session = await mode_off_turn(ctx, "L9", "20_q_capital")
    kinds = {e["type"] for _, e in session.events}
    new = sorted(kinds & PROTOCOL_1_1)
    version = session.ready.get("protocol_version")
    # A plain session sends the answer as audio only: the speech-to-text server says what it was.
    pcm = b"".join(data for _, data in session.audio)
    seconds = len(pcm) / 2 / AGENT_RATE
    words = transcribe(ctx.args.audio_url, pcm, AGENT_RATE) if pcm else ""
    extra = sorted(kind for kind in kinds if kind not in ("ready", "stt_result", "tts_playback_complete"))
    passed = (not new and "interaction" not in session.ready and version == "1.0"
              and gap is not None and seconds >= 0.5 and "paris" in words.lower())
    ctx.add("L9", "mode off: no 1.1 event, the conversation path answers", passed,
            f"protocol {version}, 1.1 events {new or 'none'}, other event types {extra or 'none'}, first "
            f"audio E + {ms(gap)} ms, {seconds:.1f} s of audio heard as {words[:70]!r}",
            "no 1.1 event, ready without `interaction`, a spoken answer to the question")


def with_tools(ctx: Ctx, *names: str) -> dict:
    return {**MODEL_FRONT, "tasks": {"tools": [ctx.webhook.tool(name) for name in names]}}


def says_number(text: str, digit: str, word: str) -> bool:
    return bool(re.search(rf"\b({digit}|{word})\b", text.lower()))


async def question(session: Session, start: int, timeout: float = 40.0):
    """The next `task_question` from event `start` on, and the event index after it."""
    found = await session.wait(lambda e: e["type"] == "task_question", timeout, start)
    if found is None:
        raise RuntimeError(f"no task_question within {timeout:.0f} s")
    return found[2], found[0] + 1


async def spoken(session: Session, start: int, kind: str, timeout: float = 30.0, nth: int = 1):
    """The `nth` agent line of `kind` that starts playing from event `start` on."""
    index = start
    for _ in range(nth):
        found = await session.wait(
            lambda e: e["type"] == "assistant_transcript" and e.get("kind") == kind and e.get("delta"),
            timeout, index)
        if found is None:
            raise RuntimeError(f"no {kind} line within {timeout:.0f} s")
        index = found[0] + 1
    return found


async def heard(session: Session, line, timeout: float = 30.0) -> float:
    """When all of an agent line has been delivered."""
    utterance = line[2]["utterance_id"]
    found = await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("utterance_id") == utterance
        and not e.get("delta") and e.get("heard") in ("full", "partial"), timeout, line[0])
    if found is None:
        raise RuntimeError(f"line {utterance} did not end within {timeout:.0f} s")
    return found[1]


async def asked_and_heard(session: Session, start: int):
    """Waits for a confirmation to be asked and its question to be heard in full."""
    asked, after = await question(session, start)
    await heard(session, await spoken(session, after - 1, "question"))
    # The caller answers a moment after the question ends (the audio leads its playout).
    await asyncio.sleep(0.45)
    return asked, after


async def answer_received(session: Session, start: int, timeout: float = 20.0) -> Optional[dict]:
    found = await session.wait(lambda e: e["type"] == "task_answer_received", timeout, start)
    return found[2] if found else None


async def l4(ctx: Ctx):
    """Confirmation with a correction; an "okay" is asked again."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "L4", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("10_book_table")
    first, after = await question(session, start)
    # The read-back is the second line of the confirmation, after the lead-in.
    await spoken(session, after - 1, "confirm", nth=2)
    await asyncio.sleep(0.5)
    amend = await session.say("11_make_it_four")
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 10, after)
    second, after_second = await question(session, after)
    await heard(session, await spoken(session, after_second - 1, "question"))
    await asyncio.sleep(0.45)
    await session.say("12_yes")
    answer = None
    for _ in range(3):
        answer = await answer_received(session, after_second)
        if answer is None or answer.get("outcome") != "superseded":
            break
    # Long enough for a further question to time out: a small model sometimes asks for a second
    # booking, which this caller leaves unanswered and the gateway then refuses.
    result = await session.wait(lambda e: e["type"] == "task_result", 90, after_second)
    await session.quiet(20, start)
    await session.close()
    calls = ctx.webhook.since(mark)
    extra = [e.get("prompt") for _, e in session.all("task_question", after_second)]
    revisions = {e.get("revision") for _, e in session.events[start:] if "revision" in e}
    # After the cut nothing is audible until the next agent line starts.
    silent = False
    if stopped:
        next_line = next((at for at, e in session.transcripts(stopped[0])), now())
        silent = session.first_audible(stopped[1] + 0.3, next_line - 0.06) is None
    party = [call["arguments"].get("party_size") for call in calls if isinstance(call["arguments"], dict)]
    checks = {
        "first question reads 2": first.get("kind") == "confirm" and says_number(first.get("prompt", ""), "2", "two"),
        "audio stops on the correction": bool(stopped) and stopped[2].get("reason") == "barge_in" and silent,
        "revision 2": 2 in revisions,
        "new question reads 4": says_number(second.get("prompt", ""), "4", "four"),
        "accept by voice": bool(answer) and answer.get("outcome") == "accept" and answer.get("by") == "voice",
        # The tool is the session's own, so the server's secret must not have signed the call.
        "one webhook call, party 4, unsigned": len(calls) == 1 and party == [4] and not calls[0]["signed"],
        "task completed": bool(result) and result[2].get("state") == "completed",
    }
    stop_ms = ms(stopped[1] - amend.start) if stopped else None

    # A weak yes on an irreversible action is asked once more.
    mark = len(ctx.webhook.calls)
    weak = await Session.open(ctx, "L4-okay", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await weak.warm()
    start = len(weak.events)
    await weak.say("10_book_table")
    _, after = await asked_and_heard(weak, start)
    await weak.say("13_okay")
    reask = await weak.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("delta", "").startswith("Just to be sure"),
        15, after)
    early = [e for _, e in weak.all("task_answer_received", after)]
    early_calls = len(ctx.webhook.since(mark))
    checks["okay is asked again"] = bool(reask) and not early and early_calls == 0
    if reask:
        await heard(weak, reask)
        await asyncio.sleep(0.45)
        await weak.say("12_yes")
        accepted = await answer_received(weak, reask[0])
        await weak.wait(lambda e: e["type"] == "task_result", 40, reask[0])
        checks["yes after the re-ask books once"] = (
            bool(accepted) and accepted.get("outcome") == "accept" and len(ctx.webhook.since(mark)) == 1)
    await weak.quiet(20, start)
    await weak.close()

    failed = [name for name, ok in checks.items() if not ok]
    ctx.add("L4", "confirmation with a correction; okay is asked again", not failed,
            f"question 1 {first.get('prompt')!r}; audio stopped {stop_ms} ms after the correction began; "
            f"question 2 {second.get('prompt')!r}; answer {answer and (answer.get('outcome'), answer.get('by'))}; "
            f"webhook calls {[(c['tool'], c['arguments']) for c in calls]}; okay re-ask "
            f"{reask[2]['delta'] if reask else None!r}"
            + (f"; the model then asked for one more call, left unanswered and refused at its timeout: {extra}"
               if extra else "")
            + (f"; FAILED: {failed}" if failed else ""),
            "confirm with party 2; correction during the read-back stops the audio, revision 2, new question "
            "with 4; yes -> accept by voice; exactly one webhook call with party 4; okay is re-asked")


LONG_LINE = ("While I look that up, here is a reminder. Our kitchen is open every day from noon until "
             "ten in the evening, the garden terrace is open when the weather is dry, and tables are "
             "held for fifteen minutes after the booked time.")


def level_drop(session: Session, at: float) -> Tuple[float, float, Optional[float]]:
    """The agent's level before a caller onset sent at `at`, its level 400 to 900 ms after it
    (silence when nothing was received), and when it first went 10 dB down and stayed there
    for 200 ms."""
    before = percentile([db for _, db in session.levels(at - 0.6, at) if db > AUDIBLE_DB], 90)
    window = [db for _, db in session.levels(at + 0.4, at + 0.9)]
    after = percentile(window, 90) if window else -100.0
    down_at = None
    frames = session.levels(at, at + 2.0)
    for n, (when, db) in enumerate(frames):
        rest = [level for t, level in frames[n:] if t < when + 0.2]
        if max(rest) <= before - 10:
            down_at = when - at
            break
    return before, after, down_at


async def l5(ctx: Ctx):
    """Barge-in while a slow tool runs."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "L5", with_tools(ctx, "check_availability"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    # The tool must outlast the long line's synthesis and 1.5 s of it, which take over 4.5 s
    # on a loaded machine: 12 s rather than the usual 6 s.
    ctx.webhook.delay["check_availability"] = 12.0
    try:
        await session.say("15_availability")
        # The tool is running: the agent talks, and the caller cuts in.
        deadline = now() + 30
        while now() < deadline and not ctx.webhook.since(mark):
            await asyncio.sleep(0.02)
        if not ctx.webhook.since(mark):
            raise RuntimeError("the model did not call check_availability")
        line = await say_long_line(session)
        await asyncio.sleep(1.5)
        tool_running = "done" not in ctx.webhook.since(mark)[0]
        barge = await session.say("14_barge_in", block=False)
        stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 5, line[0])
        await asyncio.sleep(max(0.0, barge.end - now()))
        before, after, down_at = level_drop(session, barge.start)
        late = [at for at, db in session.levels(barge.start + 1.5, barge.end) if db > AUDIBLE_DB]
        result = await session.wait(
            lambda e: e["type"] == "task_result" and e.get("state") == "completed", 60, start)
        await session.quiet(40, start)
        await session.close()
    finally:
        ctx.webhook.delay["check_availability"] = 6.0
    drop = before - after
    passed = (down_at is not None and down_at <= 0.4 and drop >= 10 and not late and bool(stopped)
              and stopped[2].get("reason") == "barge_in" and bool(result) and tool_running)
    ctx.add("L5", "barge-in while a slow tool runs", passed,
            f"level {before:.1f} dBFS before the first barge-in frame; 10 dB down {ms(down_at)} ms after it "
            f"and {drop:.1f} dB down 400-900 ms after it; audible agent frames after 1.5 s: {len(late)}; "
            f"agent_audio_stopped {stopped[2].get('reason') if stopped else None} after "
            f"{ms(stopped[1] - barge.start) if stopped else None} ms; tool still running at the barge-in: "
            f"{tool_running}; task_result completed: {bool(result)}",
            "level falls >= 10 dB within 400 ms; no agent audio after 1.5 s; agent_audio_stopped barge_in; "
            "task_result completed",
            "the agent's audio during the wait is a client `speak` line (the acknowledgement is too short to cut)")


async def say_long_line(session: Session):
    """Has the agent say the long line; returns its transcript event once it plays."""
    start = len(session.events)
    await session.send({"type": "speak", "text": LONG_LINE})
    return await spoken(session, start, "notice", 20)


def gains(reference: bytes, heard: bytes) -> List[Optional[float]]:
    """The gain of each 20 ms of `heard` against the same 20 ms of `reference` (dB); `None`
    where the reference is silent."""
    out: List[Optional[float]] = []
    for at in range(0, min(len(reference), len(heard)) - AGENT_FRAME + 1, AGENT_FRAME):
        level = frame_db(reference[at:at + AGENT_FRAME])
        out.append(frame_db(heard[at:at + AGENT_FRAME]) - level if level > -45.0 else None)
    return out


async def l6(ctx: Ctx):
    """A short "uh-huh" over the agent does not stop it."""
    session = await Session.open(ctx, "L6", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    # The line once undisturbed: the gateway caches it, so the second time the same samples
    # come back and every 20 ms can be compared with its own full level.
    line = await say_long_line(session)
    await asyncio.sleep(8.0)
    reference = b"".join(data for at, data in session.audio if at >= line[1] - 0.06)
    await session.send({"type": "clear"})
    await session.quiet(20, line[0])
    await asyncio.sleep(0.5)

    line = await say_long_line(session)
    await asyncio.sleep(2.0)
    back = await session.say("06_backchannel")
    await asyncio.sleep(3.8)
    received = [(at, data) for at, data in session.audio if at >= line[1] - 0.06]
    await session.send({"type": "clear"})
    await session.quiet(20, line[0])
    await session.close()

    heard = b"".join(data for _, data in received)
    gain = gains(reference, heard)

    def frames(since: float, until: float) -> List[float]:
        """The gains of the audio received in a window after the uh-huh began."""
        first = sum(len(data) for at, data in received if at < back.start + since) // AGENT_FRAME
        last = sum(len(data) for at, data in received if at < back.start + until) // AGENT_FRAME
        return [value for value in gain[first:last] if value is not None]

    during, later = frames(0.0, 1.2), frames(2.0, 3.5)
    deepest = min(during, default=0.0)
    back_at = statistics.median(later) if later else -100.0
    stops = [e for _, e in session.all("agent_audio_stopped", line[0]) if e.get("reason") != "client_clear"]
    gaps = [b - a for (a, _), (b, _) in zip(received, received[1:]) if back.start <= a <= back.start + 2.0]
    passed = not stops and len(later) >= 50 and abs(back_at) <= 1.0
    ctx.add("L6", "a short uh-huh over the agent (06_backchannel)", passed,
            f"agent_audio_stopped events {len(stops)}; gain against the same line undisturbed: lowest "
            f"{deepest:.1f} dB in the 1.2 s after the uh-huh began, median {back_at:.1f} dB 2.0-3.5 s after it "
            f"({len(later)} frames); longest gap in the agent's audio {ms(max(gaps, default=0))} ms",
            "no agent_audio_stopped; the audio goes on and is back at full level (within 1 dB)",
            "the agent's audio is a client `speak` line said twice; the second time is compared sample "
            "for sample with the first")


async def l7(ctx: Ctx):
    """Cancellation truth before and after a commit."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "L7-before", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("10_book_table")
    _, after = await asked_and_heard(session, start)
    await session.say("04_cancel")
    cancelled = await session.wait(lambda e: e["type"] == "task_cancelled", 30, after)
    await session.quiet(20, start)
    await asyncio.sleep(1.0)
    await session.close()
    calls_before = len(ctx.webhook.since(mark))
    none = bool(cancelled) and cancelled[2].get("effects") == "none_committed" and calls_before == 0

    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "L7-after", with_tools(ctx, "book_table", "order_flowers"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("16_book_and_flowers")
    _, after = await asked_and_heard(session, start)
    await session.say("12_yes")
    # The table is booked; the second tool asks again, and the caller cancels.
    _, after = await asked_and_heard(session, after)
    committed = [call["tool"] for call in ctx.webhook.since(mark)]
    await session.say("04_cancel")
    cancelled_after = await session.wait(lambda e: e["type"] == "task_cancelled", 30, after)
    await session.quiet(30, start)
    await asyncio.sleep(1.0)
    await session.close()
    effects = cancelled_after[2].get("effects") if cancelled_after else None
    said = [e["delta"] for _, e in session.transcripts(after)]
    already = [line for line in said if "already" in line]
    calls_after = [call["tool"] for call in ctx.webhook.since(mark)]
    # Which of the two the model does first is its own choice: one call is committed when
    # the caller cancels at the second question, and that one is named.
    truth = (isinstance(effects, list) and len(effects) == 1 and bool(already)
             and len(committed) == 1 and calls_after == committed and effects[0] in already[0])
    ctx.add("L7", "cancellation truth before and after a commit", none and truth,
            f"before: task_cancelled effects {cancelled[2].get('effects') if cancelled else None!r}, webhook "
            f"calls {calls_before}; after the first of two tools: effects {effects!r}, webhook calls "
            f"{calls_after}, said {already[0] if already else said!r}",
            "cancel at the first confirmation: effects none_committed, no webhook call; cancel after the "
            "first tool committed: task_cancelled with the effect label and a line that says 'already'")


async def l8(ctx: Ctx):
    """The model front, and interaction mode refused when its server is gone."""
    session = await Session.open(ctx, "L8", {"front": {"profile": "model"}}, PLAIN_PROMPT)
    profile = session.ready.get("interaction", {}).get("front", {}).get("profile")
    await session.warm()
    await asyncio.sleep(2.0)
    for name in REQUESTS:
        await session.turn(name)
    await session.close()
    await ctx.summary(session)
    # The act the model itself named for each request (every one of them is a `delegate`);
    # the front's task logs it without the session's id.
    named = [dict(re.findall(r"(\w+)=(\S+)", line)).get("named", "?").strip('"')
             for line in ctx.stack.log_since("interaction front act", session.log_mark)]
    settled = [dict(re.findall(r"(\w+)=(\S+)", line))
               for line in ctx.stack.log_lines("interaction turn settled", session.stream_id)]
    delegates = [turn for turn in settled if turn.get("act", "").strip('"') == "delegate"]
    by_model = [turn for turn in delegates if turn.get("source") == "Model"]
    degraded_live = [e for _, e in session.all("interaction_updated") if e.get("degraded")]
    sources = [turn.get("source") for turn in settled]
    confusion = {act: named.count(act) for act in sorted(set(named))}

    # With the front server stopped, a new session is refused the mode (no rule router).
    refused, plain_answer, stopped = None, False, ctx.stack.stop_front()
    if stopped:
        try:
            down = await Session.open(ctx, "L8-front-down", {"front": {"profile": "model"}}, PLAIN_PROMPT)
            refused = next((w for w in down.warnings if w.get("code") == "interaction_refused"
                            and (w.get("detail") or {}).get("reason") == "front_unavailable"), None)
            # The session runs as without the mode: the plain conversation answers. Without the
            # mode a request-response recogniser hears a turn at the client's `audio_end`, as
            # in `mode_off_turn` (the mode's own turn ends are not there).
            await down.say("20_q_capital")
            await down.send({"type": "audio_end"})
            final = await down.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 15)
            deadline = now() + 30
            while final and now() < deadline and down.first_audible(final[1]) is None:
                await asyncio.sleep(0.05)
            plain_answer = ("interaction" not in down.ready and bool(final and final[2].get("transcript"))
                            and final is not None and down.first_audible(final[1]) is not None)
            await down.close()
        finally:
            ctx.stack.start_front()
    passed = (profile == "model" and len(delegates) >= 5 and len(by_model) >= 3
              and bool(refused) and plain_answer)
    ctx.add("L8", "model front; interaction mode refused when the front server is stopped", passed,
            f"echo profile {profile!r}; turn sources {sources} ({len(by_model)} of {len(delegates)} delegate "
            f"turns by the model); the model named {confusion} for the {len(REQUESTS)} requests "
            f"({len(REQUESTS) - len(named)} without an act in its budget); degraded while the server was up: "
            f"{[e['degraded'] for e in degraded_live] or 'no'}; front stopped: "
            + (f"config_warning interaction_refused front_unavailable {bool(refused)}, answered without "
               f"the mode {plain_answer}"
               if stopped else "NOT RUN (the front server is not this script's; pass --allow-stop-front)"),
            "profile model; source model on >= 3 of 5 delegate turns; with the front server stopped: "
            "interaction_refused (reason front_unavailable) and the session answers as without the mode",
            "the source of a turn is read from the gateway's debug log line `interaction turn settled`")


async def l10(ctx: Ctx):
    """The agent's own reply replayed as caller audio starts no task."""
    session = await Session.open(ctx, "L10", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    turn = await session.turn("23_q_planet")
    relays = session.transcripts(turn.first_event, "relay")
    if not relays:
        raise RuntimeError("the agent gave no answer to replay")
    reply = b"".join(data for at, data in session.audio if at >= relays[0][0] - 0.06)
    echo = resample(reply, AGENT_RATE, CALLER_RATE)
    replay = await session.turn("agent-reply", echo)
    tasks = session.all("task_created", replay.first_event)
    await asyncio.sleep(1.0)
    await session.close()
    said = " ".join(e["delta"] for _, e in relays)
    ctx.add("L10", "the agent's reply replayed as caller audio", replay.e is not None and not tasks,
            f"agent said {said!r}; replay heard as {replay.text!r}; task_created after the replay: {len(tasks)}",
            "the replay is transcribed and creates no task",
            f"replay started {ms(replay.said.start - session.last_audio_before(replay.said.start))} ms "
            "after the agent's last frame (the echo filter looks back 5 s)")


async def a1(ctx: Ctx):
    """A ready answer behind a slow synthesiser is acknowledged at once."""
    if not ctx.args.tts_delay_ms:
        raise RuntimeError("A1 needs a synthesiser that is slow to its first audio: pass --tts-delay-ms 1500")
    session = await Session.open(ctx, "A1", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    turns = []
    for name in QUESTIONS:
        turn = await session.turn(name)
        lines = session.transcripts(turn.first_event)
        relays = session.transcripts(turn.first_event, "relay")
        answer = session.audio_at(relays[0][0]) if relays and turn.e else None
        first = session.first_audible(turn.e) if turn.e else None
        ready = next((at for at, e in session.all("task_result", turn.first_event)), None)
        turns.append({
            "first_ms": ms(first - turn.e) if first else None,
            "order": [e.get("kind") for _, e in lines],
            "answer_ms": ms(answer - turn.e) if answer else None,
            "chars": len(relays[0][1]["delta"]) if relays else 0,
            "result_ms": ms(ready - turn.e) if ready and turn.e else None,
        })
    await session.close()
    # The first turn runs before any synthesis was measured. From the second on, an answer
    # whose own audio comes after E + 2,000 ms must have an acknowledgement before it.
    slow = [n for n, turn in enumerate(turns) if n > 0 and (turn["answer_ms"] or 0) > 2000]
    late = [n + 1 for n in slow if turns[n]["order"][:1] != ["ack"] or turns[n]["first_ms"] > 1000]
    silent = [n + 1 for n, turn in enumerate(turns)
              if n > 0 and (turn["first_ms"] is None or turn["first_ms"] > 2300)]
    passed = all(turn["answer_ms"] is not None for turn in turns) and bool(slow) and not late and not silent
    ctx.add("A1", "a ready answer behind a slow synthesiser is acknowledged at once", passed,
            "; ".join(f"turn {n + 1}: first audio E + {turn['first_ms']} ms, lines {turn['order']}, the "
                      f"answer's own audio E + {turn['answer_ms']} ms ({turn['chars']} characters, "
                      f"task_result at E + {turn['result_ms']} ms)" for n, turn in enumerate(turns))
            + (f"; FAILED: slow turns {[n + 1 for n in slow]}, of them without an early acknowledgement "
               f"{late}, silent past 2.3 s {silent}" if not passed else ""),
            "turns 2 to 5: when the answer's own first audio is later than E + 2,000 ms, an acknowledgement "
            "starts by E + 1,000 ms, and none of these turns is silent for more than 2,300 ms; at least one "
            "such turn occurs. Turn 1 is measured, not judged",
            f"the speech server sits behind a proxy that holds back every synthesis of {SlowSpeech.SENTENCE_CHARS} "
            f"characters or more by {ctx.args.tts_delay_ms} ms. Turn 1 runs on the old rule (nothing was "
            "measured yet) and shows what every turn was before: silence until the answer's own audio, "
            "and the 2,000 ms backstop line queued behind the answer. The first version of this check "
            "also held turn 1 to 2,300 ms; it measured 2,244 and 2,404 ms")



async def uh1(ctx: Ctx):
    """An answer still in synthesis when the caller goes on within 1.5 s of the end of their turn
    is not heard over them: it waits for the end of their turn (fin-experience; the FDB v1 Candor
    pauses where the turn-end model took a pause for the end)."""
    if not ctx.args.tts_delay_ms or ctx.args.tts_delay_ms < 2500:
        raise RuntimeError("UH1 needs a synthesiser that is slow to its first audio: pass --tts-delay-ms 3000")
    session = await Session.open(ctx, "UH1", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("67_lima_four_sentences")
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    final = await session.wait(is_final, 10, start)
    e_at = final[1] if final else now()
    # The answer goes to the slow synthesiser as soon as its first sentence is written. The
    # caller goes on within the 1.5 s of the turn's end in which a caller is still taken to be in
    # their turn (the Candor takeovers resumed 0.2-0.7 s after E), while nothing is audible: 0.15 s
    # after an acknowledgement clip ends, else 0.8 s after the final, at the latest 1.2 s after it
    # (the gateway's E can come before the final: by Smart Turn). Run 1 waited for an
    # acknowledgement that never came and went on at 1.98 s; run 2 went on at 0.8 s while the
    # acknowledgement played (a line heard: nothing taken back, rightly); run 3 at 1.40 s after
    # the final was 1.59 s after the gateway's E.
    first = await session.wait(lambda e: e["type"] == "task_created", 10, start)
    while now() < e_at + 1.2:
        heard = session.last_audio() > e_at
        if (heard and now() - session.last_audio() >= 0.15) or (not heard and now() >= e_at + 0.8):
            break
        await asyncio.sleep(0.02)
    went_on = await session.say("48_availability_sunday")
    await session.wait(is_final, 10, start, after=went_on.start)
    await asyncio.sleep(1.0)
    await session.quiet(60, start)
    task = first[2].get("task_id") if first else None
    relays = [(at, e) for at, e in session.all("audio_segment", start)
              if e.get("phase") == "start" and e.get("lane") == "main"]
    over = [at for at, _ in relays if went_on.start + 0.3 <= at <= went_on.last_voiced]
    answered = [at for at, e in relays if e.get("task_id") == task and at > went_on.last_voiced]
    taken_back = [at for at, e in session.all("agent_audio_stopped", start)
                  if e.get("reason") == "policy" and went_on.start <= at <= went_on.last_voiced + 0.5]
    loud = [at for at, db in session.levels(went_on.start + 0.3, went_on.last_voiced) if db > AUDIBLE_DB]
    lines = [(round(at - e_at, 2), e.get("kind")) for at, e in session.transcripts(start)]
    await session.close()
    checks = {
        "the caller went on within 1.5 s of the final": went_on.start - e_at <= 1.5,
        "the first answer was in synthesis when the caller went on (taken back, reason policy)": bool(taken_back),
        "no main-lane line starts while the caller speaks": not over,
        "no agent audio while the caller speaks": not loud,
        "the first answer is heard after the caller's turn": bool(answered),
    }
    report(ctx, "UH1", "an answer not yet heard waits when the caller goes on", checks,
           f"E at 0; the caller went on {went_on.start - e_at:.2f}-{went_on.last_voiced - e_at:.2f} s; "
           f"agent_audio_stopped (policy) at {[round(at - e_at, 2) for at in taken_back]} s; main-lane starts "
           f"{[(round(at - e_at, 2), e.get('kind'), e.get('task_id') == task) for at, e in relays]} "
           f"(kind, first task); {len(loud)} audible 20 ms frames while the caller spoke; lines {lines}",
           "the caller goes on within 1.5 s of the final (the window in which the gateway takes them to be in "
           "their turn) while the first answer is held by the slow synthesiser: it is taken back "
           "(`agent_audio_stopped` reason policy while they speak); no `audio_segment` on the main lane starts "
           "from 300 ms into the caller's second utterance to its last voiced moment and no agent frame is "
           "louder than -50 dBFS then; a main-lane `audio_segment` of the first task starts after it",
           "run with --tts-delay-ms 3000 (answers reach the client about 3 s after they are written)")
    return session

WAIT_TOOL_S = 15.0


async def slow_lookup(ctx: Ctx, label: str, port: Optional[int] = None):
    """A session (on the gateway at `port`, else the main one) that asks for a lookup whose
    tool takes WAIT_TOOL_S: the session, its first event, the `task_created` and the tool call."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, label, with_tools(ctx, "check_availability"), TOOLS_PROMPT,
                                 port=port)
    await session.warm()
    start = len(session.events)
    await session.say("15_availability")
    created = await session.wait(lambda e: e["type"] == "task_created", 15, start)
    deadline = now() + 30
    while now() < deadline and not ctx.webhook.since(mark):
        await asyncio.sleep(0.02)
    if created is None or not ctx.webhook.since(mark):
        raise RuntimeError("the model did not call check_availability")
    return session, start, created, ctx.webhook.since(mark)[0]


def longest_silence(session: Session, since: float, until: float) -> float:
    """The longest stretch without audible agent audio in a window."""
    marks = [since] + [at for at, db in session.levels(since, until) if db > AUDIBLE_DB] + [until]
    return max(b - a for a, b in zip(marks, marks[1:]))


async def w1(ctx: Ctx):
    """The wait: a progress phrase after 4 s, the holding line after 8 s of silence, presence and repeat."""
    ctx.webhook.delay["check_availability"] = WAIT_TOOL_S
    try:
        # Nobody speaks: the acknowledgement, the tool's phrase, then the holding line.
        session, start, created, call = await slow_lookup(ctx, "W1-wait")
        progress = await spoken(session, start, "progress", 15)
        progress_end = await heard(session, progress)
        hold = await spoken(session, progress[0], "hold", 20)
        running_at_hold = "done" not in call
        result = await session.wait(lambda e: e["type"] == "task_result", 60, start)
        await session.quiet(40, start)
        await session.close()
        final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 1, start)
        relays = session.transcripts(start, "relay")
        acks = session.transcripts(start, "ack")
        e_at = final[1] if final else created[1]
        quiet_for = longest_silence(session, e_at, relays[0][0] if relays else now())
        phrase_at = progress[1] - created[1]
        hold_after = hold[1] - progress_end
        waited = {
            "the tool's phrase is the progress line": progress[2]["delta"] == "Checking the tables now.",
            # Both times are client receive times, so a few ms of delivery jitter can show the
            # gateway's 4,000 ms gate as 3,99x ms; 50 ms of slack covers that and nothing more.
            "progress 4.0 to 6.5 s into the wait": 3.95 <= phrase_at <= 6.5,
            "holding line 8 to 10 s after the progress line ended": 8.0 <= hold_after <= 10.0,
            "the tool was still running at the holding line": running_at_hold,
            "task completed": bool(result) and result[2].get("state") == "completed",
        }

        # The caller speaks during the wait: "say that again", then "are you still there?".
        asking, start, created2, call = await slow_lookup(ctx, "W1-ask")
        phrase = await spoken(asking, start, "progress", 15)
        await heard(asking, phrase)
        await asyncio.sleep(0.2)
        again = await asking.turn("45_say_again", settle=0)
        repeat = await spoken(asking, again.first_event, "answer", 10)
        await heard(asking, repeat)
        await asyncio.sleep(0.2)
        there = await asking.turn("46_still_there", settle=0)
        running_at_presence = "done" not in call
        presence = await spoken(asking, there.first_event, "progress", 10)
        result = await asking.wait(lambda e: e["type"] == "task_result", 60, start)
        await asking.quiet(40, start)
        await asking.close()
        await ctx.summary(asking)
        acts = [dict(re.findall(r"(\w+)=(\S+)", line)).get("act", "").strip('"')
                for line in ctx.stack.log_lines("interaction turn settled", asking.stream_id)]
        tasks = asking.all("task_created", start)
        asked = {
            "the tool was still running when the caller asked": running_at_presence,
            "say that again repeats the last line": repeat[2]["delta"] == phrase[2]["delta"],
            # Chapter 9: the front words a status line itself and "say that again" is a
            # `repeat` control (the router's `echo` act and its presence template are gone).
            # ... about the running lookup: still working, and naming what it works on.
            "presence is answered with the task's status": "working on" in presence[2]["delta"].lower()
                and any(word in presence[2]["delta"].lower() for word in ("availability", "olive garden", "table")),
            "no new task": len(tasks) == 1,
            "the turns are a repeat control and progress": acts[-2:] == ["control", "progress"],
            "task completed": bool(result) and result[2].get("state") == "completed",
        }
    finally:
        ctx.webhook.delay["check_availability"] = 6.0
    failed = [name for name, ok in {**waited, **asked}.items() if not ok]
    ctx.add("W1", f"the wait through a {WAIT_TOOL_S:.0f} s tool", not failed,
            f"silent caller: acknowledgement {[e['delta'] for _, e in acks]} "
            f"{ms(acks[0][0] - e_at) if acks else None} ms after E; progress {progress[2]['delta']!r} "
            f"{ms(phrase_at)} ms after task_created; holding line {hold[2]['delta']!r} {ms(hold_after)} ms after "
            f"the progress line ended ({ms(hold[1] - created[1])} ms into the wait); longest stretch without "
            f"agent audio between E and the answer {ms(quiet_for)} ms; the tool took "
            f"{ms(call.get('done', now()) - call['t'])} ms. Asking caller: 'say that again' -> "
            f"{repeat[2]['delta']!r} {ms(repeat[1] - again.e) if again.e else None} ms after E; 'are you still "
            f"there?' -> {presence[2]['delta']!r} {ms(presence[1] - there.e) if there.e else None} ms after E; "
            f"task_created {len(tasks)}; turn acts {acts}"
            + (f"; FAILED: {failed}" if failed else ""),
            "the tool's progress phrase starts 4.0 (less 50 ms of delivery jitter) to 6.5 s after task_created (the policy's 4 s and one "
            "synthesis); the holding line starts 8 to 10 s after the progress line ended (8 s without agent "
            "speech and one synthesis); 'say that again' repeats the last line heard; 'are you still "
            "there?' is answered with the presence line that names the task, with no new task; both "
            "while the tool runs",
            "the holding line counts 8 s from the last agent speech, as the plan's chapter 4 has it; "
            "two sessions, because a caller who speaks moves that mark")


async def t1(ctx: Ctx):
    """Truth after a late correction, and after a tool fails once something was committed."""
    # A slow booking: the caller says yes, and corrects while the call is on the wire.
    ctx.webhook.delay["book_table"] = 6.0
    try:
        mark = len(ctx.webhook.calls)
        session = await Session.open(ctx, "T1-late", with_tools(ctx, "book_table"), TOOLS_PROMPT)
        await session.warm()
        start = len(session.events)
        await session.say("10_book_table")
        _, after = await asked_and_heard(session, start)
        await session.say("12_yes")
        deadline = now() + 20
        while now() < deadline and not ctx.webhook.since(mark):
            await asyncio.sleep(0.02)
        if not ctx.webhook.since(mark):
            raise RuntimeError("the yes did not start the booking")
        call = ctx.webhook.since(mark)[0]
        amend = await session.turn("11_make_it_four", settle=0)
        on_wire = "done" not in call
        stale = await session.wait(lambda e: e["type"] == "task_result" and e.get("superseded"), 30, after)
        line = await session.wait(
            lambda e: e["type"] == "assistant_transcript" and "had already gone through" in e.get("delta", ""),
            30, after)
        line_end = await heard(session, line) if line else None
        # The revised run asks for the booking of four. Whichever comes first, that question
        # or the old call's outcome, the caller hears the outcome before the question ends.
        second = await session.wait(lambda e: e["type"] == "task_question", 30, amend.first_event)
        asked_end = None
        if second and line_end:
            ask = await session.wait(
                lambda e: e["type"] == "assistant_transcript" and e.get("kind") == "question" and e.get("delta"),
                30, line[0])
            asked_end = await heard(session, ask) if ask else None
        calls = ctx.webhook.since(mark)
        revisions = sorted({e.get("revision") for _, e in session.events[start:] if "revision" in e})
        await session.close()
    finally:
        ctx.webhook.delay.pop("book_table", None)
    party = [c["arguments"].get("party_size") for c in calls if isinstance(c["arguments"], dict)]
    label = stale[2].get("effects") if stale else None
    late = {
        "the call was on the wire at the correction": on_wire,
        "task_result superseded, completed, revision 1": bool(stale) and stale[2].get("state") == "completed"
                                                         and stale[2].get("revision") == 1,
        "its effect is the booking for 2": label == ["booked a table for 2"],
        "the stale-outcome line names it": bool(line) and "booked a table for 2" in line[2]["delta"],
        "it is heard before the revised question ends": bool(line_end and asked_end) and line_end < asked_end,
        "one webhook call, party 2": party == [2],
        "revision 2 asks again with 4": bool(second) and says_number(second[2].get("prompt", ""), "4", "four")
                                        and 2 in revisions,
    }

    # Two tools: the first commits, the second tool's endpoint answers HTTP 500.
    mark = len(ctx.webhook.calls)
    ctx.webhook.fail_from = mark + 1
    try:
        session = await Session.open(ctx, "T1-fail", with_tools(ctx, "book_table", "order_flowers"), TOOLS_PROMPT)
        await session.warm()
        start = len(session.events)
        await session.say("16_book_and_flowers")
        _, after = await asked_and_heard(session, start)
        await session.say("12_yes")
        _, after = await asked_and_heard(session, after)
        await session.say("12_yes")
        ended = await session.wait(
            lambda e: e["type"] in ("task_result", "task_cancelled") and not e.get("superseded"), 60, after)
        await session.quiet(30, start)
        await asyncio.sleep(0.5)
        await session.close()
    finally:
        ctx.webhook.fail_from = None
    calls2 = ctx.webhook.since(mark)
    statuses = [(c["tool"], c.get("status")) for c in calls2]
    effects = ended[2].get("effects") if ended else None
    said = [(e.get("kind"), e.get("source"), e["delta"]) for _, e in session.transcripts(after)]
    done_label = {"book_table": "booked a table for 2", "order_flowers": "ordered the flowers"}.get(
        calls2[0]["tool"] if calls2 else "", "?")
    words = set(re.findall(r"[a-z]+", done_label)) - {"a", "the", "for"}
    names_it = [text for _, _, text in said if words <= set(re.findall(r"[a-z]+", text.lower()))]
    failed_call = {
        "the first call committed and the second got HTTP 500": [status for _, status in statuses] == [200, 500],
        "the task fails and the model is not asked to word it": bool(ended) and ended[2].get("state") == "failed"
                                                                and not [kind for kind, _, _ in said if kind == "relay"],
        "the effects carry the committed call": isinstance(effects, list) and done_label in effects,
        "the failed call is only ever unconfirmed": isinstance(effects, list) and len(effects) == 2
                                                    and effects[1].endswith("(unconfirmed)"),
        "a spoken line says what was already done": bool(names_it),
    }
    failed = [name for name, ok in {**late, **failed_call}.items() if not ok]
    ctx.add("T1", "truth after a late correction and after a tool fails", not failed,
            f"late correction: correction heard as {amend.text!r} with the 6 s call "
            f"{'still out' if on_wire else 'already back'}; task_result superseded "
            f"{stale and (stale[2].get('state'), stale[2].get('revision'), stale[2].get('effects'))}, "
            f"{ms(stale[1] - amend.e) if stale and amend.e else None} ms after the correction's E "
            f"({'after' if stale and second and second[1] < stale[1] else 'before'} the revised question opened); "
            f"the agent said {line[2]['delta'] if line else None!r}, heard "
            f"{ms(asked_end - line_end) if line_end and asked_end else None} ms before the revised question "
            f"ended; webhook calls {[(c['tool'], c['arguments']) for c in calls]}; "
            f"revisions {revisions}; next question {second[2].get('prompt') if second else None!r}. "
            f"Failed tool: calls {statuses}; {ended[2]['type'] if ended else None} state "
            f"{ended[2].get('state') if ended else None!r} effects {effects!r}; the agent then said {said}"
            + (f"; FAILED: {failed}" if failed else ""),
            "a correction while the confirmed call is on the wire: task_result with superseded true for "
            "revision 1 and the committed label, the stale-outcome line heard before the revised run's "
            "question ends, one call with party 2, and the revised run asks again with 4. A tool that answers HTTP 500 after an earlier commit: the "
            "task fails without another model round, its effects name the committed call and mark the "
            "failed one unconfirmed, and a spoken line says what was already done")


async def r1(ctx: Ctx):
    """The router: a hold cue over agent speech starts no second task, small talk no task at all."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "R1-hold", with_tools(ctx, "check_availability"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("15_availability")
    deadline = now() + 30
    while now() < deadline and not ctx.webhook.since(mark):
        await asyncio.sleep(0.02)
    if not ctx.webhook.since(mark):
        raise RuntimeError("the model did not call check_availability")
    line = await say_long_line(session)
    await asyncio.sleep(1.5)
    barge = await session.turn("14_barge_in", settle=0)
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 5, line[0])
    result = await session.wait(lambda e: e["type"] == "task_result", 60, start)
    await session.quiet(40, start)
    await asyncio.sleep(1.0)
    await session.close()
    await ctx.summary(session)
    settled = [dict(re.findall(r"(\w+)=(\S+)", entry))
               for entry in ctx.stack.log_lines("interaction turn settled", session.stream_id)]
    acts = [(turn.get("act", "").strip('"'), turn.get("source")) for turn in settled]
    tasks = session.all("task_created", start)
    results = session.all("task_result", start)
    relays = [e["delta"] for _, e in session.transcripts(start, "relay")]
    summary = result[2].get("summary", "") if result else ""
    hold = {
        "the cue cut the agent": bool(stopped) and stopped[2].get("reason") == "barge_in",
        # Chapter 9: the front decides every turn (the lexicon is gone).
        "it is settled as wait by the front": acts[-1:] == [("wait", "Model")],
        "one task": len(tasks) == 1,
        "one answer, relayed once": len(results) == 1 and " ".join(relays) == summary and bool(summary),
    }

    talk = await Session.open(ctx, "R1-talk", MODEL_FRONT, PLAIN_PROMPT)
    await talk.warm()
    turn = await talk.turn("08_weather_smalltalk")
    await talk.close()
    await ctx.summary(talk)
    talk_acts = [dict(re.findall(r"(\w+)=(\S+)", entry)).get("act", "").strip('"')
                 for entry in ctx.stack.log_lines("interaction turn settled", talk.stream_id)]
    lines = [(e.get("kind"), e.get("source"), e["delta"]) for _, e in talk.transcripts(turn.first_event)]
    first = talk.first_audible(turn.e) if turn.e else None
    small = {
        "no task": not talk.all("task_created", turn.first_event),
        # Chapter 9: the front words a social line itself; the persona's bank line is said
        # only when the front names an intent without words.
        "the front answers with one social line": len(lines) == 1 and lines[0][0] == "answer"
            and lines[0][1] in ("front", "gateway") and bool(lines[0][2].strip()),
        "the turn is social": talk_acts == ["social"],
    }
    failed = [name for name, ok in {**hold, **small}.items() if not ok]
    ctx.add("R1", "a hold cue and small talk start no task", not failed,
            f"hold cue heard as {barge.text!r}: agent_audio_stopped {stopped[2].get('reason') if stopped else None}, "
            f"turn acts {acts}, task_created {len(tasks)}, task_result {len(results)}, relay sentences "
            f"{len(relays)} ({' '.join(relays)!r}); small talk heard as {turn.text!r}: turn acts {talk_acts}, "
            f"task_created {len(talk.all('task_created', turn.first_event))}, the agent said {lines}, first audio "
            f"E + {ms(first - turn.e) if first and turn.e else None} ms"
            + (f"; FAILED: {failed}" if failed else ""),
            "'Hold on a second, I need to tell you something else first.' over agent speech while a tool "
            "runs: the audio is cut, the turn is a wait (the front's), no second task, the one answer is relayed "
            "once. 'How is your day going?': no task, a social turn, one answer line",
            "turn acts are the gateway's debug log line `interaction turn settled`")


TYPED_BOOKING = "Please book a table for two at the Olive Garden on Saturday at 7 p.m."


async def c1(ctx: Ctx):
    """Client controls, GET /v1/tasks, and a session whose only background is interaction.tasks.llm."""
    checks: Dict[str, bool] = {}
    http = f"http://127.0.0.1:{ctx.args.gateway_port}"
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "C1-controls", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()

    # A typed turn, the board over HTTP, and the answer as a button.
    start = len(session.events)
    await session.send({"type": "agent_input", "text": TYPED_BOOKING})
    asked, after = await question(session, start)
    created = session.all("task_created", start)
    listed = get_json(f"{http}/v1/tasks?stream_id={session.stream_id}").get("tasks", [])
    unnamed = get_json(f"{http}/v1/tasks").get("tasks", [])
    checks["a typed turn creates a task and its question"] = (
        len(created) == 1 and asked.get("kind") == "confirm" and says_number(asked.get("prompt", ""), "2", "two"))
    checks["GET /v1/tasks?stream_id= lists it as input_required"] = (
        [(t.get("id"), t.get("state"), t.get("open_question")) for t in listed]
        == [(created[0][1]["task_id"], "input_required", asked.get("request_id"))] if created else False)
    await session.send({"type": "task_answer", "request_id": asked.get("request_id"), "outcome": "accept"})
    answer = await answer_received(session, after)
    result = await session.wait(lambda e: e["type"] == "task_result", 60, after)
    await session.quiet(30, start)
    calls = ctx.webhook.since(mark)
    # What the control governs is the answer and the one call. How the task then ends is the
    # background model's doing: its round after the tool result sometimes fails (the model
    # server answers HTTP 500 to a tool call it cannot parse), and the task is then `failed`
    # with the booking in its effects.
    state = result[2].get("state") if result else None
    checks["task_answer accepts as the client and books once"] = (
        bool(answer) and (answer.get("outcome"), answer.get("by")) == ("accept", "client")
        and len(calls) == 1 and state in ("completed", "failed"))
    done = get_json(f"{http}/v1/tasks?stream_id={session.stream_id}").get("tasks", [])
    checks["the finished task is listed with its state and its effect"] = (
        [(t.get("state"), t.get("effects")) for t in done] == [(state, ["booked a table for 2"])])

    await session.close()

    # The same request in a new session, cancelled by the client at its question. (A second
    # booking in the first session is the model's to get right: with the cut acknowledgement
    # in its history it answered "Got it, I'll" and called no tool, twice in three runs.)
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "C1-cancel", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.send({"type": "agent_input", "text": TYPED_BOOKING})
    asked2, after = await question(session, start)
    second = session.all("task_created", start)
    await session.send({"type": "task_cancel", "task_id": second[0][1]["task_id"] if second else "?"})
    cancelled = await session.wait(lambda e: e["type"] == "task_cancelled", 20, after)
    await session.quiet(30, start)
    checks["task_cancel cancels with none_committed and no call"] = (
        bool(cancelled) and (cancelled[2].get("effects"), cancelled[2].get("by")) == ("none_committed", "client")
        and not ctx.webhook.since(mark))
    cancel_said = [e["delta"] for _, e in session.transcripts(after)]

    # interaction_update: one setting that may change, one that may not.
    start = len(session.events)
    patch = {"policy": {"hold_after_silence_ms": 5000}, "front": {"profile": "model"}}
    await session.send({"type": "interaction_update", "patch": patch})
    updated = await session.wait(lambda e: e["type"] == "interaction_updated", 10, start)
    applied = updated[2].get("applied") if updated else None
    refused = updated[2].get("refused") if updated else None
    checks["interaction_update applies the policy value and refuses the profile"] = (
        applied == ["policy.hold_after_silence_ms"]
        and [(r.get("path"), r.get("reason")) for r in refused or []] == [("front.profile", "not_mid_session")])
    await session.close()

    # audio_end ends the caller's turn at once, and the session goes on listening.
    plain = await Session.open(ctx, "C1-audio-end", MODEL_FRONT, PLAIN_PROMPT)
    await plain.warm()
    start = len(plain.events)
    said = await plain.say("20_q_capital")
    await plain.send({"type": "audio_end"})
    final = await plain.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 10, start)
    ended_ms = ms(final[1] - said.last_voiced) if final else None
    await asyncio.sleep(1.0)
    await plain.quiet(40, start)
    spoken_turn = await plain.turn("21_q_week")
    flushed_ms = ms(spoken_turn.e - spoken_turn.said.last_voiced) if spoken_turn.e else None
    results = [e.get("state") for _, e in plain.all("task_result", start)]
    await plain.close()
    checks["audio_end ends the turn and the next spoken turn is heard"] = (
        bool(final) and "capital" in final[2].get("transcript", "").lower()
        and spoken_turn.e is not None and results == ["completed", "completed"])

    # No conversation_config: the background is `interaction.tasks.llm`.
    llm = {"base_url": f"http://127.0.0.1:{ctx.args.background_port}/v1",
           "model": ctx.args.background_alias, "api_key": "local-dummy", "system_prompt": PLAIN_PROMPT}
    raw = await Session.open(ctx, "C1-raw", {**MODEL_FRONT, "tasks": {"llm": llm}}, conversation=False)
    await raw.warm()
    turn = await raw.turn("22_q_sky")
    relays = [e["delta"] for _, e in raw.transcripts(turn.first_event, "relay")]
    raw_result = [e for _, e in raw.all("task_result", turn.first_event)]
    await raw.close()
    checks["a session with only interaction.tasks.llm answers"] = (
        "interaction" in raw.ready and not mode_warnings(raw.warnings) and bool(relays)
        and [e.get("state") for e in raw_result] == ["completed"] and "blue" in " ".join(relays).lower())

    failed = [name for name, ok in checks.items() if not ok]
    ctx.add("C1", "client controls, GET /v1/tasks and the raw session", not failed,
            f"agent_input -> question {asked.get('prompt')!r}; GET /v1/tasks?stream_id= -> "
            f"{[(t.get('state'), t.get('open_question') is not None) for t in listed]}, without stream_id "
            f"{len(unnamed)} task(s); task_answer -> {answer and (answer.get('outcome'), answer.get('by'))}, "
            f"webhook calls {[(c['tool'], c['arguments']) for c in calls]}, task_result {state!r}, then listed as "
            f"{[(t.get('state'), t.get('effects')) for t in done]}; task_cancel at {asked2.get('prompt')!r} -> effects "
            f"{cancelled[2].get('effects') if cancelled else None!r}, the agent said {cancel_said}; "
            f"interaction_update -> applied {applied}, refused {refused}; audio_end -> final {ended_ms} ms "
            f"after the last voiced sample (a spoken turn in the same session: {flushed_ms} ms), heard "
            f"{final[2].get('transcript') if final else None!r}, task results {results}; raw session: "
            f"warnings {[w.get('code') for w in raw.warnings]}, answer {' '.join(relays)!r}"
            + (f"; FAILED: {failed}" if failed else ""),
            "a typed turn asks its confirmation; GET /v1/tasks?stream_id= shows the task input_required with "
            "the question's id; task_answer accept is recorded by client and books once, and the ended task "
            "is listed with its state and the booking as its effect; task_cancel "
            "gives none_committed; interaction_update applies policy.hold_after_silence_ms and refuses "
            "front.profile; audio_end ends a turn and the next one is still heard; a session with no "
            "conversation_config answers from interaction.tasks.llm",
            "the gateway runs without authentication here, so a list without stream_id is empty by design")



def log_time(line: str) -> float:
    """The time of day of a gateway log line, in seconds."""
    hours, minutes, seconds = re.match(r"\S+T(\d+):(\d+):([\d.]+)Z", line).groups()
    return int(hours) * 3600 + int(minutes) * 60 + float(seconds)


def first_voiced_ms(pcm: bytes) -> int:
    """The start of the first 20 ms frame of caller audio that holds speech."""
    for start in range(0, len(pcm), CALLER_FRAME):
        if frame_db(pcm[start:start + CALLER_FRAME]) > -45.0:
            return start // 2 * 1000 // CALLER_RATE
    return 0


def attenuate(pcm: bytes, db: float) -> bytes:
    samples = np.frombuffer(pcm, dtype="<i2").astype(np.float64) * 10 ** (db / 20)
    return samples.astype("<i2").tobytes()


CUE_WORDS = ("stop", "wait")


async def s1(ctx: Ctx):
    """A stop cue during agent speech cuts the audio."""
    streaming = ctx.args.stt == "streaming"
    session = await Session.open(ctx, "S1", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    runs = []
    for cue in ("44_stop_talking", "41_wait_a_second", "44_stop_talking", "41_wait_a_second", "47_stop_stop"):
        line = await say_long_line(session)
        await asyncio.sleep(1.5)
        pcm = load_wav(ctx.caller(cue))
        said = await session.say(cue, pcm, block=False)
        voiced = said.start + first_voiced_ms(pcm) / 1000
        stopped = await session.wait(
            lambda e: e["type"] == "agent_audio_stopped" and e.get("reason") != "client_clear", 5, line[0])
        # The first transcript that holds the cue: the gateway cannot cut on words before it.
        words = await session.wait(
            lambda e: e["type"] == "stt_result" and any(w in e.get("transcript", "").lower() for w in CUE_WORDS),
            5, line[0], said.start)
        final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 5, line[0], said.start)
        _, _, down_at = level_drop(session, voiced)
        await asyncio.sleep(max(0.0, said.end + 1.0 - now()))
        # Agent audio received after the cut, until its next line starts.
        late = 0
        if stopped:
            next_line = next((at for at, e in session.transcripts(stopped[0])), now())
            late = len([1 for at, db in session.levels(stopped[1] + 0.05, next_line - 0.06) if db > AUDIBLE_DB])
        runs.append({
            "cue": cue, "reason": stopped[2].get("reason") if stopped else None,
            "cut_ms": ms(stopped[1] - voiced) if stopped else None,
            "after_words_ms": ms(stopped[1] - words[1]) if stopped and words else None,
            "duck_ms": ms(down_at), "words_ms": ms(words[1] - voiced) if words else None,
            "heard": final[2].get("transcript") if final else None, "late_frames": late,
        })
        if not stopped:
            await session.send({"type": "clear"})
        await session.quiet(30, line[0])
        await asyncio.sleep(0.5)
    await session.close()

    def ok(run: dict) -> bool:
        if run["words_ms"] is None or run["after_words_ms"] is None:
            return False
        cut_ok = run["reason"] == "barge_in" and run["after_words_ms"] <= 100 and run["late_frames"] == 0
        in_time = run["cut_ms"] is not None and run["cut_ms"] <= 900
        return cut_ok and (in_time or not streaming)

    cuts = [run["cut_ms"] for run in runs]
    # The front's time from an interim's request to its act (`interaction front interim
    # verdict`, the first generated token), for every verdict of the session.
    verdict_ms = [int(m.group(1)) for line in ctx.stack.log_lines("front interim verdict", session.stream_id)
                  for m in [re.search(r"\bms=(\d+)", line)] if m]
    # How the interims went to the front server (`interaction front first bytes` since the
    # session began): client-rendered on `/completion` once the line's prompt is primed.
    asked = [line for line in ctx.stack.log_since("interaction front first bytes", session.log_mark)
             if "interim=true" in line]
    rendered = len([line for line in asked if "client_rendered=true" in line])
    detail = [(run["cue"], f"heard {run['heard']!r}", run["reason"], f"cut {run['cut_ms']} ms",
               f"cue words at {run['words_ms']} ms", f"cut {run['after_words_ms']} ms after them",
               f"10 dB down {run['duck_ms']} ms", f"audible frames after the cut {run['late_frames']}")
              for run in runs]
    limit = ("within 900 ms of the cue's first voiced frame (the design's verdict deadline) and "
             if streaming else "")
    ctx.add("S1", "a stop cue during agent speech cuts the audio", all(ok(run) for run in runs),
            f"agent_audio_stopped after the cue's first voiced frame: {cuts} ms; per cue {detail}; "
            f"front interim act (first token) {verdict_ms} ms; interims client-rendered {rendered} of {len(asked)}",
            f"'stop talking' and 'wait a second', twice each, and a bare 'Stop, stop.': "
            f"agent_audio_stopped barge_in {limit}within 100 ms of the first transcript that holds the "
            "cue word, no agent audio after the cut",
            "times count from the first voiced 20 ms frame of the cue. "
            + ("" if streaming else "This recogniser has no interim results: the words, and the cut, "
               "come with the final transcript.")
            + (" The recogniser is Deepgram nova-3 streaming (`--stt-vendor deepgram`)."
               if streaming and ctx.args.stt_vendor == "deepgram" else
               f" The recogniser is the local stand-in ({ctx.args.stt_stand_in})." if streaming else ""))
    return session


async def s2(ctx: Ctx):
    """A yes during the confirmation read-back does not bind; a yes after it does."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "S2", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("10_book_table")
    _, after = await question(session, start)
    # The read-back is the second line of the confirmation, after the lead-in.
    await spoken(session, after - 1, "confirm", nth=2)
    await asyncio.sleep(0.6)
    early = await session.say("12_yes")
    final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 10, after, early.start)
    # What the early yes did: nothing may be accepted or called before the question is asked again.
    again = await spoken(session, final[0] if final else after, "question", 40)
    early_answers = [e for _, e in session.all("task_answer_received", after) if e.get("outcome") == "accept"]
    early_calls = len(ctx.webhook.since(mark))
    await heard(session, again)
    await asyncio.sleep(0.45)
    late = await session.say("12_yes")
    answer = await answer_received(session, again[0])
    result = await session.wait(lambda e: e["type"] == "task_result", 60, again[0])
    await session.quiet(20, start)
    await session.close()
    calls = ctx.webhook.since(mark)
    said_between = [e["delta"] for _, e in session.transcripts(final[0] if final else after)][:4]
    checks = {
        "the early yes is heard": bool(final) and "yes" in final[2].get("transcript", "").lower(),
        "it accepts nothing": not early_answers and early_calls == 0,
        "the yes after the question binds": bool(answer) and answer.get("outcome") == "accept"
                                            and answer.get("by") == "voice",
        "one webhook call": len(calls) == 1,
        "task completed": bool(result) and result[2].get("state") == "completed",
    }
    failed = [name for name, ok in checks.items() if not ok]
    ctx.add("S2", "a yes during the read-back does not bind, a yes after the question does", not failed,
            f"early yes heard as {final[2].get('transcript') if final else None!r} "
            f"{ms(early.start - session.opened)} ms into the session; accepts before the question was asked "
            f"again: {len(early_answers)}, webhook calls: {early_calls}; then the agent said {said_between}; "
            f"second yes -> {answer and (answer.get('outcome'), answer.get('by'))} "
            f"{ms(answer_time(session, again[0]) - late.start) if answer else None} ms after it began; "
            f"webhook calls in all: {[(c['tool'], c['arguments']) for c in calls]}"
            + (f"; FAILED: {failed}" if failed else ""),
            "the early yes: 0 accepts and 0 webhook calls; the yes after the question is heard: accept by "
            "voice, exactly 1 webhook call, task completed")
    return session


def answer_time(session: Session, start: int) -> float:
    return next(at for at, e in session.events[start:] if e["type"] == "task_answer_received")


# The recogniser of the eager scenarios finalises 1,400 ms after the speech ends, so the turn
# model's verdict (600 to 1,000 ms after it in a debug build on a busy machine) comes first.
SLOW_FINAL = {"endpointing_ms": 1400}
EAGER_PAUSE_MS = 1200


async def s3(ctx: Ctx):
    """Eager end of turn: adopted when the caller stays silent, discarded when the caller goes on."""
    def eager_lines(session: Session):
        lines = ctx.stack.log_lines("interaction e", session.stream_id)
        pick = lambda needle: [line for line in lines if needle in line]
        return (pick("interaction eager end of turn"), pick("interaction eager end withdrawn"),
                pick("interaction end of turn"))

    same = lambda lines: [re.search(r"eager_same=(\S+)", line).group(1) for line in lines]

    # The caller stays silent: the run started at the eager end of turn is the turn's run.
    silent = await Session.open(ctx, "S3-silent", MODEL_FRONT, PLAIN_PROMPT, stt_features=SLOW_FINAL)
    await silent.warm()
    turn = await silent.turn("20_q_capital")
    await silent.close()
    await ctx.summary(silent)
    eager, withdrawn, ends = eager_lines(silent)
    tasks = silent.all("task_created", turn.first_event)
    relays = silent.transcripts(turn.first_event, "relay")
    head_start = ms(log_time(ends[0]) - log_time(eager[0])) if eager and ends else None
    adopted = (len(eager) == 1 and not withdrawn and same(ends) == ["Some(true)"]
               and len(tasks) == 1 and bool(relays))
    first = silent.audio_at(relays[0][0]) if relays and turn.e else None

    # The caller goes on 1,200 ms after a finished sentence: the eager run is withdrawn, and
    # the turn is one turn with one task.
    half, rest = load_wav(ctx.caller("42_fly_boston")), load_wav(ctx.caller("43_and_a_hotel"))
    pcm = (half[:last_voiced_ms(half) * CALLER_RATE // 1000 * 2] + bytes(CALLER_RATE * 2 * EAGER_PAUSE_MS // 1000)
           + rest[first_voiced_ms(rest) * CALLER_RATE // 1000 * 2:])
    going = await Session.open(ctx, "S3-goes-on", MODEL_FRONT, PLAIN_PROMPT, stt_features=SLOW_FINAL)
    await going.warm()
    turn2 = await going.turn("42+43", pcm)
    await going.close()
    await ctx.summary(going)
    eager2, withdrawn2, ends2 = eager_lines(going)
    finals = [e for _, e in going.all("stt_result", turn2.first_event) if e.get("is_final")]
    tasks2 = going.all("task_created", turn2.first_event)
    # Nothing is said to the first half: no agent line before E.
    before_e = [e["delta"] for at, e in going.transcripts(turn2.first_event) if turn2.e and at < turn2.e]
    heard_before_e = turn2.e is not None and going.first_audible(turn2.said.start, turn2.e) is not None
    title = tasks2[0][1].get("title", "") if tasks2 else ""
    discarded = (bool(eager2) and bool(withdrawn2) and len(finals) == 1 and len(tasks2) == 1
                 and "hotel" in turn2.text.lower() and "boston" in turn2.text.lower()
                 and not before_e and not heard_before_e)
    ctx.add("S3", "eager end of turn: adopted in silence, discarded when the caller goes on",
            adopted and discarded,
            f"silent: eager ends {len(eager)}, withdrawn {len(withdrawn)}, E {head_start} ms after the eager "
            f"end with eager_same {same(ends)}, task_created {len(tasks)}, first answer audio E + "
            f"{ms(first - turn.e) if first and turn.e else None} ms; "
            f"goes on: eager ends {len(eager2)}, withdrawn {len(withdrawn2)}, final stt_results {len(finals)} "
            f"({turn2.text!r}), task_created {len(tasks2)} ({title!r}), agent lines before E {before_e}, "
            f"agent audio before E: {heard_before_e}, eager_same at E {same(ends2)}",
            "silent: one eager end before E, not withdrawn, eager_same true at E, one task, an answer; goes on "
            f"({EAGER_PAUSE_MS} ms pause after a finished sentence): an eager end that is withdrawn, one final with both "
            "halves, one task, no agent line or audio before E",
            "the recogniser is asked to finalise after 1,400 ms of silence (stt features endpointing_ms); "
            "the eager lines are the gateway's debug log")
    return silent


# The echo defences S4 is run with: `duplex.echo` of the session, and whether the setting is
# judged. "gate" is what a `/ws` session gets by default and "gate+aec" what a phone line gets.
# "none" is the session without either (judged on the streaming recogniser only: with no
# interim words an undefended echo is cut at the verdict), and "aec" the canceller alone, which
# needs its first second of echo to find the path: both are measured.
ECHO_DEFENCES = [
    ("none", {"correlation_gate": False, "aec": "off"}),
    ("gate", {"correlation_gate": True, "aec": "off"}),
    ("aec", {"correlation_gate": False, "aec": "on"}),
    ("gate+aec", {"correlation_gate": True, "aec": "on"}),
]
# With the gate the agent is at most this long more than 6 dB down while only its echo returns.
ECHO_DUCK_LIMIT_MS = 100
# A caller who cuts in over the echo stops the agent within this of the first voiced frame
# (the 900 ms verdict, and 300 ms for the onset and the way to this client).
ECHO_CUT_LIMIT_MS = 1200
# How long after the line began the caller cuts in.
ECHO_BARGE_AT = 3.0
# The echo: what the client played, this much later and this much lower.
ECHO_DELAY = 0.2
ECHO_LEVEL_DB = -6.0


def overlap_classes(ctx: Ctx, session: Session) -> List[str]:
    return [re.search(r"class=(\w+)", entry).group(1)
            for entry in ctx.stack.log_lines("interaction overlap classified", session.stream_id)]


async def echo_run(ctx: Ctx, label: str, defence: dict) -> dict:
    """One session with this echo defence: the agent's line with its own echo, then the same
    with a caller who cuts in over the echo."""
    session = await Session.open(ctx, f"S4-{label}", {**MODEL_FRONT, "duplex": {"echo": defence}}, PLAIN_PROMPT)
    await session.warm()
    # The line once undisturbed (the gateway caches it: the second time the same samples come).
    line = await say_long_line(session)
    await heard(session, line, 40)
    await asyncio.sleep(0.5)
    reference = b"".join(data for at, data in session.audio if at >= line[1] - 0.06)
    seconds = len(reference) / 2 / AGENT_RATE
    await session.quiet(20, line[0])

    # From here on the microphone hears the agent too.
    session.loopback = (ECHO_DELAY, ECHO_LEVEL_DB)
    start = len(session.events)
    line = await say_long_line(session)
    end = await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("utterance_id") == line[2]["utterance_id"]
        and not e.get("delta"), seconds + 20, line[0])
    await asyncio.sleep(2.5)
    await session.quiet(20, start)
    received = b"".join(data for at, data in session.audio if at >= line[1] - 0.06)
    gain = [value for value in gains(reference, received) if value is not None]
    stops = [e for _, e in session.all("agent_audio_stopped", start)]
    tasks = session.all("task_created", start)
    classes = overlap_classes(ctx, session)
    run = {
        "label": label,
        "ducked_ms": len([value for value in gain if value < -6.0]) * FRAME_MS,
        "stops": len(stops), "tasks": len(tasks), "classes": classes,
        "heard": end[2].get("heard") if end else None,
        "whole": len(received) >= len(reference) - AGENT_FRAME,
        "received_s": len(received) / 2 / AGENT_RATE, "seconds": seconds,
        "took_ms": ms(end[1] - line[1]) if end else None,
        "finals": [e.get("transcript") for _, e in session.all("stt_result", start) if e.get("is_final")],
    }

    # The same line again; this time the caller cuts in over it and its echo.
    await asyncio.sleep(1.0)
    start = len(session.events)
    line = await say_long_line(session)
    await asyncio.sleep(max(0.0, line[1] + ECHO_BARGE_AT - now()))
    pcm = load_wav(ctx.caller("14_barge_in"))
    barge = await session.say("14_barge_in", pcm, block=False)
    voiced = barge.start + first_voiced_ms(pcm) / 1000
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 5, line[0])
    await asyncio.sleep(max(0.0, barge.end - now()))
    _, _, down_at = level_drop(session, voiced)
    late = len([1 for when, db in session.levels(voiced + 1.5, barge.end) if db > AUDIBLE_DB])
    if not stopped:
        await session.send({"type": "clear"})
    await asyncio.sleep(1.0)
    await session.quiet(40, start)
    session.loopback = None
    await session.close()
    await ctx.summary(session)
    run.update({
        "cut_reason": stopped[2].get("reason") if stopped else None,
        "cut_ms": ms(stopped[1] - voiced) if stopped else None,
        "duck_ms": ms(down_at), "late_frames": late,
        "cut_classes": overlap_classes(ctx, session)[len(classes):],
        "echo_events": len([1 for entry in ctx.stack.log_lines("interaction audio event", session.stream_id)
                            if "Echo {" in entry]),
    })
    return run


async def s4(ctx: Ctx):
    """The agent's own audio fed back as caller audio neither cuts it nor makes a task; a caller over it does cut."""
    streaming = ctx.args.stt == "streaming"
    runs = [await echo_run(ctx, label, defence) for label, defence in ECHO_DEFENCES]

    def echo_ok(run: dict) -> bool:
        return run["stops"] == 0 and run["tasks"] == 0 and run["heard"] == "full" and run["whole"]

    def cut_ok(run: dict) -> bool:
        return (run["cut_reason"] == "barge_in" and run["cut_ms"] is not None
                and run["cut_ms"] <= ECHO_CUT_LIMIT_MS and run["late_frames"] == 0)

    by = {run["label"]: run for run in runs}
    checks = {
        "gate: the echo neither cuts nor makes a task": echo_ok(by["gate"]),
        "gate+aec: the echo neither cuts nor makes a task": echo_ok(by["gate+aec"]),
        f"gate: at most {ECHO_DUCK_LIMIT_MS} ms ducked": by["gate"]["ducked_ms"] <= ECHO_DUCK_LIMIT_MS,
        f"gate+aec: at most {ECHO_DUCK_LIMIT_MS} ms ducked": by["gate+aec"]["ducked_ms"] <= ECHO_DUCK_LIMIT_MS,
        "gate: a caller over the echo cuts": cut_ok(by["gate"]),
        "gate+aec: a caller over the echo cuts": cut_ok(by["gate+aec"]),
    }
    if streaming:
        checks["none: the echo neither cuts nor makes a task"] = echo_ok(by["none"])
    failed = [name for name, ok in checks.items() if not ok]
    measured = "; ".join(
        f"[{run['label']}] echo only: agent_audio_stopped {run['stops']}, task_created {run['tasks']}, the line "
        f"heard {run['heard']!r} with {run['received_s']:.1f} of {run['seconds']:.1f} s received in "
        f"{run['took_ms']} ms, {run['ducked_ms']} ms played more than 6 dB down, overlap verdicts "
        f"{run['classes']}, echo heard as {run['finals']}; caller over the echo: agent_audio_stopped "
        f"{run['cut_reason']} {run['cut_ms']} ms after the first voiced frame, 10 dB down after "
        f"{run['duck_ms']} ms, audible agent frames from 1.5 s on {run['late_frames']}, verdicts "
        f"{run['cut_classes']}; analyser echo reports {run['echo_events']}"
        for run in runs)
    ctx.add("S4", "the agent's own audio fed back while it speaks, and a caller over it", not failed,
            measured + (f"; FAILED: {failed}" if failed else ""),
            "with the correlation gate (gate, the /ws default) and with gate and canceller (gate+aec, a phone "
            "line's default): no agent_audio_stopped and no task_created while only the echo returns, the whole "
            f"line delivered and heard in full, at most {ECHO_DUCK_LIMIT_MS} ms of it more than 6 dB down; a "
            f"caller who cuts in over the echo: agent_audio_stopped barge_in within {ECHO_CUT_LIMIT_MS} ms of "
            "the first voiced frame and no agent audio from 1.5 s after it"
            + ("; without a defence (none): no agent_audio_stopped, no task_created, heard in full"
               if streaming else ""),
            f"the echo is what this client received, played as it came, {ms(ECHO_DELAY)} ms late and "
            f"{-ECHO_LEVEL_DB:.0f} dB down, silent from an agent_audio_stopped on; the caller is 14_barge_in at "
            f"full level, {ECHO_BARGE_AT:.0f} s into the line. `none` (no defence"
            + ("" if streaming else "; not judged on this recogniser, which has no interim words")
            + ") and `aec` (the canceller alone, which needs its first second of echo) are measured")
    return None


# What a room adds to the way from the caller's microphone to the gateway (to be measured: the
# plan's figure for a room's lag is 80 ms).
ROOM_UPLINK_MS = 200
# RV1: the realtime voice is cut after `duplex.pause_after_ms` (400 ms) of the caller's speech,
# against the cascade's 1,200 ms limit for a cut over the echo: K1's room limit.
ROOM_VOICE_CUT_LIMIT_MS = ECHO_CUT_LIMIT_MS + ROOM_UPLINK_MS
# After a cut the room is silent within this of the `agent_audio_stopped` event: what was sent
# before the cut is still on its way to the listener.
ROOM_SILENT_AFTER_MS = 1000
# The room's audio reaches the speech-to-text through the noise filter, whose workers start
# with the first frame: the caller speaks once the path keeps up (a final within this of the
# end of a short clip).
ROOM_SETTLED_MS = 3000
# A phone participant's echo: what it heard of the agent, this much later.
PHONE_ECHO_DELAY = 0.1


def actor_events(session: Session, start: int = 0) -> List[dict]:
    """The events of a session that its room gets too: everything but the socket's own."""
    own = {"ready", "stt_result", "config_warning", "tts_playback_complete", "message",
           "participant_disconnected", "error"}
    return [event for _, event in session.events[start:] if event["type"] not in own]


async def cut_in(ctx: Ctx, session: Session, caller: Mic, after: float) -> dict:
    """The agent says the long line and the caller cuts in `after` seconds into it."""
    line = await say_long_line(session)
    await asyncio.sleep(max(0.0, line[1] + after - now()))
    pcm = load_wav(ctx.caller("14_barge_in"))
    barge = await caller.say("14_barge_in", pcm, block=False)
    voiced = barge.start + first_voiced_ms(pcm) / 1000
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 5, line[0])
    await asyncio.sleep(max(0.0, barge.end - now()))
    _, _, down_at = level_drop(caller, voiced)
    # The agent's audio that still arrived after the cut, until the caller ended.
    cut = stopped[1] if stopped else voiced
    last = max((when for when, db in caller.levels(cut, barge.end) if db > AUDIBLE_DB), default=cut)
    if not stopped:
        await session.send({"type": "clear"})
    return {"reason": stopped[2].get("reason") if stopped else None,
            "cut_ms": ms(stopped[1] - voiced) if stopped else None,
            "duck_ms": ms(down_at), "silent_ms": ms(last - cut)}


async def settle(session: Session, caller: Mic, tries: int = 12) -> List[int]:
    """Has the caller say a short thanks until its transcript comes promptly; returns how
    long after the end of each clip its final came."""
    took: List[int] = []
    for _ in range(tries):
        start = len(session.events)
        said = await caller.say("07_thanks")
        final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 30, start)
        took.append(ms(final[1] - said.end) if final else 30000)
        await asyncio.sleep(1.0)
        await session.quiet(30, start)
        if took[-1] <= ROOM_SETTLED_MS:
            break
    return took


async def k1(ctx: Ctx):
    """A LiveKit room: one caller is heard, the room gets the events, a phone participant gets the echo canceller."""
    try:
        import livekit.rtc  # noqa: F401
    except ImportError:
        raise RuntimeError("K1 needs the LiveKit client: pip install livekit livekit-api")
    ctx.stack.ensure_livekit()
    limit = ECHO_CUT_LIMIT_MS + ROOM_UPLINK_MS

    # A room with a caller on a browser and a second participant.
    session = await Session.open(ctx, "K1-room", MODEL_FRONT, PLAIN_PROMPT, mic=False,
                                 livekit={"room_name": f"k1-{ctx.run_id}"})
    refused = [w for w in session.warnings if w.get("code", "").startswith("interaction")]
    caller = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "caller").join()
    joined = len(session.events)
    await session.warm()
    await asyncio.sleep(1.0)
    settled = await settle(session, caller)
    bystander = await RoomCaller(ctx, caller.room_name, "bystander").join()
    await asyncio.sleep(1.0)
    start = len(session.events)
    asked = await caller.say("20_q_capital")
    final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 15, start)
    answer = await spoken(session, start, "relay", 60)
    await heard(session, answer, 40)
    in_room = caller.first_audible(asked.end)
    await session.quiet(30, start)
    # The second participant speaks: the session has its caller.
    mark = len(session.events)
    await bystander.say("21_q_week")
    await asyncio.sleep(4.0)
    stray = [e.get("transcript") for _, e in session.all("stt_result", mark) if e.get("transcript", "").strip()]
    stray_tasks = len(session.all("task_created", mark))
    barge = await cut_in(ctx, session, caller, 1.5)
    await asyncio.sleep(1.0)
    await session.quiet(40, start)
    await asyncio.sleep(1.0)
    ours, theirs = actor_events(session, joined), [event for _, event in caller.events]
    kinds = sorted({event["type"] for event in theirs})
    room_summary_wanted = session
    await bystander.leave()
    await asyncio.sleep(1.0)
    still_open = session.ws is not None and session.ws.close_code is None
    await caller.leave()
    await session.close()
    room_summary = await ctx.summary(room_summary_wanted)

    # A phone participant: the gateway turns the echo canceller on, and the line's own echo
    # comes back through the room.
    session = await Session.open(ctx, "K1-phone", MODEL_FRONT, PLAIN_PROMPT, mic=False,
                                 livekit={"room_name": f"k1p-{ctx.run_id}"})
    phone = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "sip_caller", kind="sip").join()
    await session.warm()
    await asyncio.sleep(1.0)
    await settle(session, phone)
    line = await say_long_line(session)
    await heard(session, line, 40)
    await session.quiet(20, line[0])
    phone.loopback = (PHONE_ECHO_DELAY, ECHO_LEVEL_DB)
    start = len(session.events)
    line = await say_long_line(session)
    end = await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("utterance_id") == line[2]["utterance_id"]
        and not e.get("delta"), 40, line[0])
    await asyncio.sleep(2.5)
    await session.quiet(20, start)
    echo_stops = len(session.all("agent_audio_stopped", start))
    echo_tasks = len(session.all("task_created", start))
    echo_classes = overlap_classes(ctx, session)
    ducks = len([1 for entry in ctx.stack.log_lines("interaction pump event", session.stream_id)
                 if "FirstDuckedFrame" in entry])
    await asyncio.sleep(1.0)
    over = await cut_in(ctx, session, phone, ECHO_BARGE_AT)
    await asyncio.sleep(1.0)
    await session.quiet(40, start)
    phone.loopback = None
    await phone.leave()
    await session.close()
    summary = await ctx.summary(session)

    checks = {
        "the room session runs the mode": bool(session.ready.get("interaction")) and not refused
                                          and room_summary.get("phone_leg") == "false",
        "the caller is heard through the room": bool(final) and "capital" in final[2].get("transcript", "").lower(),
        "the answer is audible in the room": in_room is not None,
        "the room's data topic carries the socket's events": bool(theirs) and ours == theirs,
        "the second participant is not heard": not stray and stray_tasks == 0,
        "the second participant's leaving does not end the session": still_open,
        "a barge-in in the room cuts": barge["reason"] == "barge_in" and barge["cut_ms"] is not None
                                       and barge["cut_ms"] <= limit
                                       and barge["silent_ms"] <= ROOM_SILENT_AFTER_MS,
        "a phone participant gets the echo canceller": summary.get("phone_leg") == "true"
                                                       and summary.get("echo_canceller") == "true",
        "the phone line's echo neither cuts nor makes a task": echo_stops == 0 and echo_tasks == 0
                                                               and bool(end) and end[2].get("heard") == "full",
        "the phone caller over the echo cuts": over["reason"] == "barge_in" and over["cut_ms"] is not None
                                               and over["cut_ms"] <= limit
                                               and over["silent_ms"] <= ROOM_SILENT_AFTER_MS,
    }
    failed = [name for name, ok in checks.items() if not ok]
    ctx.add("K1", "a LiveKit room: one caller, the events on the data topic, a phone participant", not failed,
            f"room: interaction warnings {[w.get('code') for w in refused]}, the speech-to-text's final came "
            f"{settled} ms after each short clip until the path kept up, question heard as "
            f"{final[2].get('transcript') if final else None!r}, answer {answer[2].get('delta')!r} audible in the "
            f"room {ms(in_room - asked.end) if in_room else None} ms after the question ended; data topic "
            f"{ROOM_TOPIC}: {len(theirs)} events {kinds} against {len(ours)} on the socket, equal: {ours == theirs}; "
            f"second participant: transcripts {stray}, tasks {stray_tasks}, socket open after it left: "
            f"{still_open}; barge-in: agent_audio_stopped {barge['reason']} {barge['cut_ms']} ms after the first "
            f"voiced frame, 10 dB down in the room after {barge['duck_ms']} ms, the room silent "
            f"{barge['silent_ms']} ms after the event; phone: phone_leg {summary.get('phone_leg')}, echo_canceller "
            f"{summary.get('echo_canceller')}; echo only: agent_audio_stopped {echo_stops}, task_created "
            f"{echo_tasks}, the line heard {end[2].get('heard') if end else None!r}, reflex ducks {ducks}, "
            f"overlap verdicts {echo_classes}; caller over the echo: agent_audio_stopped {over['reason']} "
            f"{over['cut_ms']} ms after the first voiced frame, 10 dB down after {over['duck_ms']} ms, the room "
            f"silent {over['silent_ms']} ms after the event" + (f"; FAILED: {failed}" if failed else ""),
            "the session with a `livekit` block runs the mode with no warning; the caller's question is "
            "transcribed and its answer is audible in the room; the data topic carries exactly the events the "
            "socket got from the caller's join on; a second participant's speech gives no transcript and no "
            "task, and its leaving leaves the session open; a barge-in: agent_audio_stopped barge_in within "
            f"{limit} ms of the first voiced frame and the room silent within {ROOM_SILENT_AFTER_MS} ms of that "
            "event. A participant of "
            "kind sip: phone_leg and echo_canceller true in the session summary; its echo: no "
            "agent_audio_stopped, no task_created, the line heard in full; a caller over the echo cuts "
            f"within {limit} ms",
            f"a LiveKit server in development mode from the local image; the participants are Python clients "
            f"on 16 kHz microphone tracks; the phone participant joins with a token of kind sip (no SIP trunk "
            f"is involved) and its microphone returns what it heard {ms(PHONE_ECHO_DELAY)} ms later, "
            f"{-ECHO_LEVEL_DB:.0f} dB down; the limit adds {ROOM_UPLINK_MS} ms for the way through the room")


async def s5(ctx: Ctx):
    """A short answer over the agent while a question is open is the answer, not an interruption."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "S5", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("10_book_table")
    _, after = await asked_and_heard(session, start)
    # The caller says nothing: the agent asks again, and the caller answers over that line.
    again = await spoken(session, len(session.events), "question", 30)
    await asyncio.sleep(0.4)
    pcm = load_wav(ctx.caller("12_yes"))
    said = await session.say("12_yes", pcm, block=False)
    voiced = said.start + first_voiced_ms(pcm) / 1000
    answer = await session.wait(lambda e: e["type"] == "task_answer_received", 15, again[0])
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 1, again[0])
    result = await session.wait(lambda e: e["type"] == "task_result", 60, again[0])
    await session.quiet(20, start)
    await session.close()
    calls = ctx.webhook.since(mark)
    tasks = session.all("task_created", start)
    classes = [re.search(r"class=(\w+)", entry).group(1)
               for entry in ctx.stack.log_lines("interaction overlap classified", session.stream_id)]
    passed = (bool(answer) and answer[2].get("outcome") == "accept" and answer[2].get("by") == "voice"
              and len(calls) == 1 and len(tasks) == 1 and bool(result) and result[2].get("state") == "completed")
    ctx.add("S5", "a short answer over the agent while a question is open", passed,
            f"the agent was saying {again[2].get('delta')!r}; the yes -> task_answer_received "
            f"{answer and (answer[2].get('outcome'), answer[2].get('by'))} "
            f"{ms(answer[1] - voiced) if answer else None} ms after its first voiced frame; agent audio stopped "
            f"{(stopped[2].get('reason'), ms(stopped[1] - voiced)) if stopped else None}; overlap verdicts "
            f"{classes}; task_created {len(tasks)}; webhook calls {[(c['tool'], c['arguments']) for c in calls]}; "
            f"task_result {result[2].get('state') if result else None}",
            "the yes is accepted by voice, exactly one webhook call, no second task, the task completes")
    return session


BARGE_LEVELS = (0, -6, -12, -18)
BARGE_RUNS = 12


async def bl(ctx: Ctx):
    """Barge-ins by attenuated copies of the caller recording: how many are caught at each level."""
    barge = load_wav(ctx.caller("14_barge_in"))
    report, counts = [], {}
    for level in BARGE_LEVELS:
        session = await Session.open(ctx, f"BL{level}", MODEL_FRONT, PLAIN_PROMPT)
        await session.warm()
        # One turn at full level gives the caller's running level.
        await session.turn("20_q_capital")
        caught, cut_ms = "", []
        for run in range(BARGE_RUNS):
            start = len(session.events)
            line = await say_long_line(session)
            await asyncio.sleep(1.5)
            # 3 ms more silence each time, so the onset falls differently on the 32 ms chunks.
            pcm = bytes(CALLER_RATE * 2 * 3 * run // 1000) + attenuate(barge, level)
            said = await session.say(f"barge{level}", pcm, block=False)
            stopped = await session.wait(
                lambda e: e["type"] == "agent_audio_stopped" and e.get("reason") == "barge_in", 4, line[0])
            caught += "1" if stopped else "0"
            if stopped:
                cut_ms.append(ms(stopped[1] - said.start))
            await asyncio.sleep(max(0.0, said.end - now()))
            if not stopped:
                await session.send({"type": "clear"})
            await asyncio.sleep(1.5)
            await session.quiet(40, start)
        await session.close()
        await ctx.summary(session)
        events = ctx.stack.log_lines("interaction audio event", session.stream_id)
        onsets = [float(v) for v in re.findall(r"Onset \{ at: \d+, level_db: (-?[\d.]+), reflex_ducked: true", "\n".join(events))]
        classes = [re.search(r"class=(\w+)", entry).group(1)
                   for entry in ctx.stack.log_lines("interaction overlap classified", session.stream_id)]
        counts[level] = caught.count("1")
        spread = f"{min(onsets):.1f} to {max(onsets):.1f} dBFS" if onsets else "none"
        report.append(f"{level} dB: {counts[level]} of {BARGE_RUNS} caught ({caught}), cut after "
                      f"{min(cut_ms, default=None)} to {max(cut_ms, default=None)} ms, onset level of the "
                      f"64 ms the old rule judged: {spread}, verdicts {classes}")
    passed = counts[0] == BARGE_RUNS and counts[-6] == BARGE_RUNS
    ctx.add("BL", "barge-ins at 0, -6, -12 and -18 dB (14_barge_in over a spoken line)", passed,
            "; ".join(report),
            f"0 dB and -6 dB: {BARGE_RUNS} of {BARGE_RUNS} caught. -12 dB is the margin itself and -18 dB is "
            "beyond it (a voice 12 dB below the caller's running level is background): counted, not judged",
            "each session opens with one full-level turn; a barge-in that was heard moves the running level "
            "20% towards its own, so later ones in a session are judged against a lower level")


WHICH = "Do you want me to change it, or is this a second one?"
REPEAT_LINE = "Sorry, I didn't catch that. Could you say it again?"
ENGLISH_ONLY = "Sorry, I can only help in English on this line."
SPANISH = ["Necesito una cita con el doctor para el lunes, por favor.",
           "Quiero una cita para el lunes con la doctora.",
           "¿Cuál es la capital de Francia?"]
ASK_PROMPT = """You are the order desk of a flower shop.
- To learn which flowers the caller wants, call ask_caller at once with kind "choose", question "Which flowers would you like:" and options ["roses", "tulips"]. Never ask in your own words.
- When you have the flowers, call ask_caller with kind "value" and question "What name is the order for?".
- After both tool results, tell the caller the order in one short sentence.
- Your reply to a caller who wants flowers is always a call of ask_caller, never text."""
PASSWORD_PROMPT = """You are the help desk for customer accounts.
- To reset a password you need the caller's current password. Get it by calling ask_caller at once with kind "value" and question "What is your current password?". Never ask in your own words.
- After a tool result, tell the caller the outcome in one short sentence.
- Your reply to a caller who wants a password reset is always a call of ask_caller, never text."""
LINK_PROMPT = ("You answer a caller's question in one short sentence. Then always add this sentence, word "
               "for word: For more, visit www.example.org or call 1 800 555 0100.")


async def said(session: Session, start: int, text: str, timeout: float = 15.0):
    """The agent line with exactly these words that starts playing from event `start` on."""
    return await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("delta") == text, timeout, start)


def report(ctx: Ctx, name: str, checks_text: str, checks: Dict[str, bool], measured: str, threshold: str,
           notes: str = ""):
    failed = [check for check, ok in checks.items() if not ok]
    ctx.add(name, checks_text, not failed, measured + (f"; FAILED: {failed}" if failed else ""), threshold, notes)


async def d1(ctx: Ctx):
    """A request like a lookup that just finished runs without a question; the presence line names the request."""
    mark = len(ctx.webhook.calls)
    # A session that can book (where a repeated action is asked about); a lookup that just
    # finished committed nothing, so a request like it simply runs (gap-livefix, 19 Oct).
    session = await Session.open(ctx, "D1", with_tools(ctx, "check_availability", "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("15_availability")
    deadline = now() + 30
    while now() < deadline and not ctx.webhook.since(mark):
        await asyncio.sleep(0.02)
    if not ctx.webhook.since(mark):
        raise RuntimeError("the model did not call check_availability")
    # While the tool runs: "Are you still there?" Chapter 9: the front words the status line
    # itself (no presence template), so the check reads the progress line it says (as W1).
    there = len(session.events)
    await session.say("46_still_there")
    # The tool's own progress phrase ("Checking the tables now.", 4 s into the wait) may come
    # first; the answer to the caller is the next progress line.
    presence = None
    try:
        for nth in (1, 2):
            presence = await spoken(session, there, "progress", 15, nth)
            if presence[2]["delta"] != "Checking the tables now.":
                break
    except RuntimeError:
        pass
    await session.wait(lambda e: e["type"] == "task_result", 40, start)
    await asyncio.sleep(1.0)
    await session.quiet(30, start)
    # The same lookup for another day, seconds after the first one finished. Not at once: the
    # answer names the restaurant and the evening too, and for 5 s after an agent line a turn
    # made mostly of its words is taken for its echo.
    await asyncio.sleep(6.0)
    again = len(session.events)
    await session.say("48_availability_sunday")
    second = await session.wait(lambda e: e["type"] == "task_created", 15, again)
    await session.wait(lambda e: e["type"] == "task_result", 60, again)
    await session.quiet(30, again)
    await session.close()
    calls = ctx.webhook.since(mark)
    asked = [e["delta"] for _, e in session.transcripts(again) if e.get("kind") == "question" and e.get("delta")]
    # What the second task's run was asked shows in its answer, or in the lookup it made.
    second_answer = " ".join([e["delta"] for _, e in session.transcripts(again, "relay")]
                             + [json.dumps(call["arguments"]) for call in calls[1:]])
    presence_line = presence[2]["delta"] if presence else None
    wanted = "a progress line that says it is working on the lookup and names it"
    status = (presence_line or "").lower()
    checks = {
        "the presence line words the request as a phrase": "working on" in status
            and any(word in status for word in ("availability", "olive garden", "table")),
        "a request like a lookup that just finished is not asked about": WHICH not in asked,
        "it runs as a task of its own for Sunday": bool(second) and "sunday" in second_answer.lower(),
    }
    report(ctx, "D1", "a request like a finished lookup runs; the presence line", checks,
           f"presence line {presence_line!r}; after the repeated lookup the agent asked {asked or None!r}, "
           f"task_created {bool(second)}, its answer and lookups {second_answer!r}",
           f"presence line: {wanted}; the request like the lookup that finished 6 s before gets no "
           f"{WHICH!r} and creates a task whose answer or lookup names Sunday",
           "whether the second task looks the tables up again is the background model's choice; a "
           "repeated action (committed effects) is still asked about (policy and actor tests)")
    return session


async def h1(ctx: Ctx):
    """A choice and a value asked through ask_caller bind only after the question was heard."""
    # The background model has to use the tool: a session in which it asks in its own words
    # instead shows nothing about the gateway, and is tried again (three sessions at most).
    for attempt in (1, 2, 3):
        session = await Session.open(ctx, "H1" if attempt == 1 else f"H1-{attempt}",
                                     with_tools(ctx, "check_availability"), ASK_PROMPT)
        await session.warm()
        start = len(session.events)
        await session.say("50_order_flowers")
        if await session.wait(lambda e: e["type"] == "task_question", 20, start):
            break
        await session.quiet(30, start)
        await session.close()
    choose, after = await question(session, start, 1.0)
    line = await spoken(session, after - 1, "question")
    # The caller answers 300 ms into the question: before the options were heard.
    await asyncio.sleep(max(0.0, line[1] + 0.3 - now()))
    early_from = len(session.events)
    await session.say("51_tulips")
    again = await spoken(session, line[0] + 1, "question", 25.0)
    early = [e for _, e in session.all("task_answer_received", early_from) if e.get("request_id") == choose.get("request_id")]
    early_bound = [e for _, e in session.events[early_from:again[0]] if e["type"] == "task_answer_received"]
    await heard(session, again)
    await asyncio.sleep(0.45)
    late_from = len(session.events)
    await session.say("51_tulips")
    chosen = await answer_received(session, late_from)
    value, after = await question(session, late_from)
    await heard(session, await spoken(session, after - 1, "question"))
    await asyncio.sleep(0.45)
    name_from = len(session.events)
    await session.say("52_name_jordan")
    named = await answer_received(session, name_from)
    result = await session.wait(lambda e: e["type"] == "task_result", 40, name_from)
    await session.quiet(30, name_from)
    await session.close()
    relays = " ".join(e["delta"] for _, e in session.transcripts(name_from, "relay"))
    checks = {
        "a choose question with its options": choose.get("kind") == "choose"
        and [o.lower() for o in choose.get("options") or []] == ["roses", "tulips"],
        "an answer said over the question does not bind": not early_bound,
        "the answer after the question binds": bool(chosen) and chosen.get("outcome") == "accept"
        and "tulip" in str(chosen.get("value")).lower(),
        "a value question": value.get("kind") == "value",
        "the value binds": bool(named) and named.get("outcome") == "accept"
        and "jordan" in str(named.get("value")).lower(),
        "the task completes": bool(result) and result[2].get("state") == "completed",
    }
    report(ctx, "H1", "ask_caller: a choice and a value are heard-checked", checks,
           f"task_question {choose.get('kind')!r} {choose.get('prompt')!r} options {choose.get('options')}; "
           f"'the tulips' said 300 ms into the question: answers before it was asked again {early_bound} "
           f"(all answers so far {early}); said after the question: {chosen}; then {value.get('kind')!r} "
           f"{value.get('prompt')!r} -> {named}; task_result {result[2].get('state') if result else None!r}, "
           f"the agent said {relays!r}",
           "task_question kind choose with options [roses, tulips]; an answer that begins 300 ms into the "
           "question gives no task_answer_received before the question is asked again; the same answer after "
           "the question was heard is accepted with value tulips; the value question accepts 'Jordan'; the "
           "task completes",
           f"the background model called ask_caller in session {attempt} of at most 3")
    return session


async def p1(ctx: Ctx):
    """A card number said out loud, and a question for a password, are declined."""
    # A client with no screen (`hitl.channels` voice only): a password question is declined.
    block = {**with_tools(ctx, "check_availability"), "hitl": {"channels": ["voice"]}}
    session = await Session.open(ctx, "P1", block, PASSWORD_PROMPT)
    await session.warm()
    card_from = len(session.events)
    card = await session.turn("53_card_number")
    card_line = await said(session, card_from, "I can't take your card number on this call for security reasons.", 5)
    card_tasks = len(session.all("task_created", card_from))
    settled = [line for line in ctx.stack.log_lines("interaction turn settled", session.stream_id)]
    refused = any('RuleOverride("sensitive_value")' in line for line in settled)

    ask_from = len(session.events)
    await session.say("54_reset_password")
    pass_line = await said(session, ask_from, "I can't take your password on this call for security reasons.", 30)
    await session.wait(lambda e: e["type"] in ("task_result", "task_cancelled"), 40, ask_from)
    await session.quiet(30, ask_from)
    await session.close()
    asked = await session.wait(lambda e: e["type"] == "task_question", 0, ask_from)
    closed = await answer_received(session, ask_from, 0)
    lines = [e["delta"] for _, e in session.transcripts(ask_from) if e.get("delta")]
    asks = ("tell", "provide", "give", "say", "enter", "confirm", "what is", "what's", "need your")
    asking = [line for line in lines if "password" in line.lower() and "can't take" not in line
              and any(word in line.lower() for word in asks)]
    checks = {
        "the card number turn is declined and starts no task": bool(card_line) and card_tasks == 0 and refused,
        "a password question is closed by the gateway": not asked or (
            bool(closed) and (closed.get("outcome"), closed.get("by")) == ("decline", "policy")
            and closed.get("value") is None),
        "the caller hears that a password is not taken": bool(pass_line),
        "no line asks for the password": not asking,
    }
    report(ctx, "P1", "sensitive values are declined", checks,
           f"heard {card.text!r} -> the agent said {card_line[2]['delta'] if card_line else None!r}, tasks "
           f"created {card_tasks}, settled by the sensitive rule {refused}; password: task_question "
           f"{asked[2].get('kind') if asked else None!r} {asked[2].get('prompt') if asked else None!r} -> "
           f"task_answer_received {closed}; the agent said {lines}",
           "a turn that names a card number and carries its digits gets the decline line, no task, and is "
           "settled by rule sensitive_value; the background's request for the password is never put to "
           "the caller: an ask_caller question is closed with decline by policy and no value, the decline "
           "line is spoken, and no other line that mentions the password holds a word of asking (tell, "
           "provide, give, say, enter, confirm, what is, need your)",
           "the background asks through ask_caller or in its answer; both are covered")
    return session


async def typed(session: Session, text: str, settle: float = 30.0, reply: float = 0.0) -> int:
    """A typed caller turn; the index of its first event. With `reply`, the agent's first line
    to it is waited for (at most that long) before the agent is waited on to finish: the turn's
    front decision and its language reading take 0.5-2 s on the loaded GPU (r14), and a turn
    typed before them supersedes it (live G1, r14-verify)."""
    start = len(session.events)
    await session.send({"type": "agent_input", "text": text})
    await asyncio.sleep(1.0)
    if reply:
        await session.wait(lambda e: e["type"] == "assistant_transcript" and bool(e.get("delta")),
                           reply, start)
    await session.quiet(settle, start)
    return start


async def g1(ctx: Ctx):
    """A session language with no pack runs without the mode; a caller who switches language is stepped down."""
    hindi = await Session.open(ctx, "G1-hindi", MODEL_FRONT, PLAIN_PROMPT, language="hi")
    off = [(w.get("code"), (w.get("detail") or {}).get("reason")) for w in hindi.warnings]
    version = hindi.ready.get("protocol_version")
    await hindi.close()

    session = await Session.open(ctx, "G1-switch", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    first = await typed(session, SPANISH[0], reply=15.0)
    first_line = [e["delta"] for _, e in session.transcripts(first) if e.get("delta")]
    second = await typed(session, SPANISH[1], reply=15.0)
    second_line = [e["delta"] for _, e in session.transcripts(second) if e.get("delta")]
    warned = [e.get("code") for _, e in session.all("config_warning", second)]
    before = len(session.all("task_created"))
    third = await typed(session, SPANISH[2], 60.0)
    lines = [(e.get("source"), e.get("kind")) for _, e in session.transcripts(third) if e.get("delta")]
    answer = " ".join(e["delta"] for _, e in session.transcripts(third, "relay"))
    created = len(session.all("task_created", third))
    await session.close()

    llm = {"base_url": f"http://127.0.0.1:{ctx.args.background_port}/v1",
           "model": ctx.args.background_alias, "api_key": "local-dummy", "system_prompt": PLAIN_PROMPT}
    raw = await Session.open(ctx, "G1-raw", {**MODEL_FRONT, "tasks": {"llm": llm}}, conversation=False)
    await raw.warm()
    raw_lines = []
    for text in SPANISH[:3]:
        start = await typed(raw, text, reply=15.0)
        raw_lines.append([e["delta"] for _, e in raw.transcripts(start) if e.get("delta")])
    raw_warned = [e.get("code") for _, e in raw.all("config_warning") if e.get("code") not in STT_MODEL_NOTICES]
    raw_tasks = len(raw.all("task_created"))
    await raw.close()
    checks = {
        "a Hindi session runs without the mode": version == "1.0"
        and ("interaction_surface_unavailable", "language_unsupported") in off,
        "the first turn in Spanish is asked again": first_line == [REPEAT_LINE],
        "the second steps the session down": second_line == [ENGLISH_ONLY]
        and warned == ["interaction_language_step_down"] and before == 0,
        "after it a turn goes straight to the background": created == 1 and bool(answer)
        and all(source == "background" for source, _ in lines),
        "a session with no conversation_config never steps down": raw_lines == [[REPEAT_LINE]] * 3
        and not raw_warned and raw_tasks == 0,
    }
    report(ctx, "G1", "session language without a pack; language step-down", checks,
           f"language hi: ready {version}, warnings {off}; typed Spanish turns: 1 -> {first_line}, 2 -> "
           f"{second_line} with config_warning {warned} (tasks so far {before}), 3 -> tasks {created}, lines "
           f"{lines}, answer {answer!r}; raw session: {raw_lines}, warnings {raw_warned}, tasks {raw_tasks}",
           f"stt language hi: ready 1.0 and interaction_surface_unavailable / language_unsupported; turn 1 "
           f"gets {REPEAT_LINE!r}; turn 2 gets {ENGLISH_ONLY!r} and config_warning "
           "interaction_language_step_down, with no task so far; turn 3 creates one task and only the "
           "background's sentences are spoken; a session whose background is interaction.tasks.llm gets the "
           "repeat line three times, no warning and no task",
           "the turns are typed: the local Whisper is English-only and names no other language, so the "
           "recogniser's language field is covered by the actor test")
    return session


async def o1(ctx: Ctx):
    """A link and a phone number the caller did not give are never spoken."""
    session = await Session.open(ctx, "O1", MODEL_FRONT, LINK_PROMPT)
    await session.warm()
    turn = await session.turn("20_q_capital")
    await session.close()
    relays = [e["delta"] for _, e in session.transcripts(turn.first_event, "relay")]
    withheld, _ = ctx.stack.log_count("sentence withheld by the speech policy", session.log_mark)
    spoken_text = " ".join(relays).lower()
    leaked = [word for word in ("www", "example", "http", "555", "0100") if word in spoken_text]
    checks = {
        "the answer is spoken": "paris" in spoken_text,
        "no link and no number is": not leaked,
        "the policy withheld a sentence": withheld >= 1,
    }
    report(ctx, "O1", "the outbound speech policy", checks,
           f"relayed {relays}; sentences withheld by the policy {withheld}; link or number words spoken {leaked}",
           "the answer names Paris; no relayed sentence holds www, example, http, 555 or 0100; the gateway "
           "log shows at least one sentence withheld",
           "the background is told to end every answer with a link and a phone number")
    return session


async def b1(ctx: Ctx):
    """A Bud voice agent as the background: its runs, the relay with and without an answer marker, a cut answer."""
    bud = ctx.stack.ensure_bud()
    # A hosted agent writes far slower than the local model: 25 text deltas a second, and 10
    # for the one whose answer must be seen to stream.
    bud.pace = {"support": 0.04, "streamer": 0.1}
    checks: Dict[str, bool] = {}

    # 1. `support`: no block in its entry, so the operator's `agent_default`; no answer marker.
    mark = len(bud.requests)
    session = await Session.open(ctx, "B1a", agent="support")
    backend = session.ready.get("interaction", {}).get("tasks", {})
    greeted = await said(session, 0, BUD_GREETING, 20.0)
    greeting_heard = greeted is not None and await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("heard") == "full"
        and e.get("utterance_id") == greeted[2]["utterance_id"], 20.0, greeted[0]) is not None
    await session.quiet(20.0)
    turn = await session.turn("20_q_capital")
    runs = bud.requests[mark:]
    created = [e for _, e in session.all("task_created", turn.first_event)]
    results = session.all("task_result", turn.first_event)
    relays = session.transcripts(turn.first_event, "relay")
    first = session.first_audible(turn.said.last_voiced)
    wait_ms = ms(first - turn.said.last_voiced) if first else None
    whole_gap = ms(relays[0][0] - results[0][0]) if relays and results else None
    body = runs[0]["body"] if runs else {}
    checks["the session runs the mode on the agent"] = (
        session.ready.get("protocol_version") == "1.1" and backend.get("backend") == "agent:support"
        and backend.get("speculative_start") == "off" and not mode_warnings(session.warnings))
    checks["the greeting is heard"] = greeting_heard
    checks["one run of the agent, as the caller"] = (
        len(runs) == 1 and runs[0]["auth"] == f"Bearer {BUD_KEY}" and body.get("bud_channel") == "voice"
        and body.get("prompt", {}).get("id") == "support"
        and body.get("conversation") == f"conv_voice_{session.stream_id}")
    checks["a task by the agent, not speculative"] = (
        len(created) == 1 and created[0].get("background") == "agent:support"
        and created[0].get("speculative") is False)
    checks["the answer is relayed and the task completes"] = (
        bool(relays) and bool(results) and results[0][1].get("state") == "completed"
        and "paris" in " ".join(e["delta"] for _, e in relays).lower())
    checks["the caller hears something within 3.0 s"] = wait_ms is not None and wait_ms <= 3000
    checks["without a marker the answer waits for the end of the run"] = (
        whole_gap is not None and whole_gap >= -50)

    # 2. The same session: a long answer is cut by the client; the next run says how much of
    #    it the caller heard.
    second = len(session.events)
    await session.send({"type": "agent_input", "text": "Tell me about the planets of the solar system."})
    relay = await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("kind") == "relay" and e.get("delta"),
        60.0, second)
    await asyncio.sleep(1.0)
    await session.send({"type": "clear"})
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 5.0, second)
    await session.quiet(30.0, second)
    await session.send({"type": "agent_input", "text": "And which one is the largest?"})
    await session.wait(lambda e: e["type"] == "task_result", 60.0, len(session.events))
    await session.quiet(30.0)
    runs = bud.requests[mark:]
    support_runs = len(runs)
    cut = runs[2]["body"].get("bud_truncate", {}) if len(runs) > 2 else {}
    written = runs[1]["output"] if len(runs) > 1 else ""
    kept = cut.get("output_text", "")
    checks["a cut answer is truncated for the agent's next run"] = (
        relay is not None and stopped is not None and len(runs) == 3
        and cut.get("response_id") == runs[1]["response_id"] and len(kept) < len(written)
        and not bud.cancels)
    await session.close()

    # 3. `streamer`: its entry carries the block and declares the marker its agent writes.
    mark = len(bud.requests)
    streamer = await Session.open(ctx, "B1b", agent="streamer")
    await said(streamer, 0, BUD_GREETING, 20.0)
    await streamer.quiet(20.0)
    asked = len(streamer.events)
    await streamer.send({"type": "agent_input", "text": "Describe the four seasons of the year."})
    await streamer.wait(lambda e: e["type"] == "task_result", 60.0, asked)
    await streamer.quiet(40.0, asked)
    results = streamer.all("task_result", asked)
    relays = streamer.transcripts(asked, "relay")
    lead = ms(results[0][0] - relays[0][0]) if relays and results else None
    spoken_text = " ".join(e["delta"] for _, e in relays)
    run = bud.requests[mark] if len(bud.requests) > mark else {"output": "", "t": 0.0}
    marked = run["output"].lstrip().startswith("ANSWER:")
    run_ms = ms(run.get("done", run["t"]) - run["t"])
    checks["the entry's block is taken"] = (
        streamer.ready.get("interaction", {}).get("tasks", {}).get("backend") == "agent:streamer")
    checks["with a marker the answer streams"] = lead is not None and lead >= 500
    checks["the marker is not spoken"] = bool(relays) and "ANSWER" not in spoken_text
    await streamer.close()

    report(ctx, "B1", "a Bud voice agent as the background", checks,
           f"support: backend {backend.get('backend')}, greeting heard {greeting_heard}, "
           f"{support_runs} run(s), first audio {wait_ms} ms after the caller's last "
           f"voiced sample, first answer sentence {whole_gap} ms after task_result; the cut answer: "
           f"agent wrote {len(written)} chars, bud_truncate kept {len(kept)} ({kept[:60]!r}), cancels "
           f"{len(bud.cancels)}; streamer: first answer sentence {lead} ms before task_result, "
           f"agent wrote the marker {marked}, its run took {run_ms} ms for {len(run['output'])} chars "
           f"({run['output'][:80]!r})",
           "ready 1.1 with tasks.backend agent:<name>, speculative_start off, no config_warning; the "
           "greeting is heard in full; one /v1/responses request per caller request with the caller's "
           "bearer, bud_channel voice and conversation conv_voice_<stream>; task_created background "
           "agent:support, speculative false; the answer names Paris and the task completes; first agent "
           "audio <= 3000 ms after the caller's last voiced sample (hard deadline 2000 ms after E, and "
           "E follows an upload); without a marker the first answer sentence starts no earlier than "
           "50 ms before task_result; after a client clear the third request carries bud_truncate for "
           "the second run with less text than the agent wrote, and no run is cancelled; with the "
           "declared marker the first answer sentence starts >= 500 ms before task_result and ANSWER "
           "is never spoken",
           "the agents are a stand-in Bud gateway over the background llama-server, paced at 25 text "
           "deltas a second (10 for the agent with the marker, told to answer in five sentences); the "
           "speech deployments are the local speech server; the answer is cut by a client `clear` (L5 "
           "measures a spoken barge-in)")
    return session


def norm(text: str) -> str:
    return re.sub(r"[^a-z0-9 ]", "", text.lower()).strip()


async def x1(ctx: Ctx):
    """Interaction mode behind the OpenAI Realtime wire: acknowledgement and relay, a barge-in, a request like a finished lookup, the client's messages."""
    bud = ctx.stack.ensure_bud()
    bud.pace = {"support": 0.04, "streamer": 0.1}
    checks: Dict[str, bool] = {}
    mark = len(bud.requests)
    session = await Realtime.open(ctx, "X1", "support")
    state = session.ready.get("interaction", {})
    order = [e["type"] for _, e in session.wire[:4]]
    greeted = await said(session, 0, BUD_GREETING, 20.0)
    greeting_heard = greeted is not None and await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("heard") == "full"
        and e.get("utterance_id") == greeted[2]["utterance_id"], 20.0, greeted[0]) is not None
    await session.quiet(20.0)
    checks["the session runs the mode behind the Realtime wire"] = (
        state.get("protocol_version") == "1.1" and state.get("regime") == "cascade"
        and state.get("tasks", {}).get("backend") == "agent:support"
        and "session.created" in order and not session.errors())
    checks["the greeting is a response that is heard"] = greeting_heard

    # 1. A spoken request: the front acknowledges, the agent's answer is relayed word for word.
    wire = len(session.wire)
    turn = await session.turn("20_q_capital")
    start = turn.first_event
    created = [e for _, e in session.all("task_created", start)]
    results = session.all("task_result", start)
    acks = session.transcripts(start, "ack")
    relays = session.transcripts(start, "relay")
    first = session.first_audible(turn.said.last_voiced)
    wait_ms = ms(first - turn.said.last_voiced) if first else None
    runs = bud.requests[mark:]
    written = norm(runs[0]["output"]) if runs else ""
    heard_text = [e["transcript"] for _, e in session.of("conversation.item.input_audio_transcription.completed", wire)]
    edges = (len(session.of("input_audio_buffer.speech_started", wire)),
             len(session.of("input_audio_buffer.speech_stopped", wire)))
    made = {e["response"]["id"]: e["response"].get("metadata") or {} for _, e in session.of("response.created", wire)}
    ended = {e["response"]["id"]: e["response"]["status"] for _, e in session.of("response.done", wire)}
    with_audio = {e.get("response_id") for _, e in session.of("response.output_audio.delta", wire)}
    relay_meta = [meta for meta in made.values() if meta.get("bud_kind") == "relay"]
    silent = [rid for rid, status in ended.items() if status == "completed" and rid not in with_audio]
    checks["the caller's speech is an item with speech edges and its transcript"] = (
        edges[0] >= 1 and edges[1] >= 1 and any("capital" in text.lower() for text in heard_text))
    checks["the acknowledgement is a response of the front, ahead of the answer"] = (
        bool(acks) and bool(relays) and acks[0][1].get("source") in ("front", "gateway")
        and acks[0][0] < relays[0][0])
    checks["a task by the agent, completed"] = (
        len(created) == 1 and created[0].get("background") == "agent:support"
        and bool(results) and results[0][1].get("state") == "completed")
    checks["the answer is relayed word for word, each sentence a response"] = (
        bool(relays) and "paris" in written
        and all(norm(e["delta"]) in written for _, e in relays)
        and len(relay_meta) == len(relays) and bool(created)
        and all(meta.get("bud_source") == "background" and meta.get("bud_verbatim") is True
                and meta.get("bud_task_id") == created[0]["task_id"] for meta in relay_meta))
    checks["the caller hears something within 3.0 s"] = wait_ms is not None and wait_ms <= 3000

    # 2. A long answer, and the caller cuts in by voice.
    second, wire2 = len(session.events), len(session.wire)
    await session.typed("Tell me about the planets of the solar system.")
    relay = await session.wait(
        lambda e: e["type"] == "assistant_transcript" and e.get("kind") == "relay" and e.get("delta"),
        60.0, second)
    await asyncio.sleep(1.0)
    barge = await session.say("14_barge_in", block=False)
    stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 5.0, relay[0] if relay else second)
    await asyncio.sleep(max(0.0, barge.end - now()))
    before, after, down_at = level_drop(session, barge.start)
    late = [at for at, db in session.levels(barge.start + 1.5, barge.end) if db > AUDIBLE_DB]
    drop = before - after
    cut_line = relay[2]["utterance_id"] if relay else None
    cut_response = next((rid for rid, meta in
                         ((e["response"]["id"], e["response"].get("metadata") or {})
                          for _, e in session.of("response.done", wire2))
                         if stopped and meta.get("bud_utterance_id") in stopped[2].get("utterance_ids", [])), None)
    statuses = {e["response"]["id"]: e["response"]["status"] for _, e in session.of("response.done", wire2)}
    truncated = [e for _, e in session.of("conversation.item.truncated", wire2)]
    delivered = stopped[2].get("delivered_ms", []) if stopped else []
    await session.quiet(40.0)
    checks["a spoken barge-in stops the answer"] = (
        relay is not None and stopped is not None and stopped[2].get("reason") == "barge_in"
        and down_at is not None and down_at <= 0.4 and drop >= 10 and not late)
    checks["the cut line's response ends cancelled and its item is truncated"] = (
        cut_response is not None and statuses.get(cut_response) == "cancelled"
        and bool(truncated) and truncated[0].get("audio_end_ms") in delivered)

    # 3. A request like a lookup just done: it committed nothing, so the request runs without
    #    "change it, or a second one?" (gap-livefix, 19 Oct; a repeated action is still asked,
    #    policy and actor tests).
    await asyncio.sleep(6.0)
    await session.turn("15_availability")
    # The caller's words over the answer started no task: this is the agent's next run.
    runs = bud.requests[mark:]
    cut = runs[2]["body"].get("bud_truncate", {}) if len(runs) > 2 else {}
    checks["the agent's next run is told what was heard"] = (
        len(runs) >= 3 and cut.get("response_id") == runs[1]["response_id"]
        and len(cut.get("output_text", "")) < len(runs[1]["output"]))
    await asyncio.sleep(6.0)
    again = len(session.events)
    runs_before = len(bud.requests)
    await session.say("48_availability_sunday")
    held = await session.wait(lambda e: e["type"] == "task_created", 15, again)
    await session.wait(lambda e: e["type"] == "task_result", 60, again)
    await session.quiet(40.0, again)
    asked = [e["delta"] for _, e in session.transcripts(again) if e.get("kind") == "question"]
    held_run = bud.requests[runs_before]["body"].get("input", "") if len(bud.requests) > runs_before else ""
    checks["a request like a finished lookup runs without the change-or-second question"] = (
        WHICH not in asked and bool(held) and "sunday" in str(held_run).lower())

    # 4. The client's own messages.
    third, wire3 = len(session.events), len(session.wire)
    await session.send({"type": "bud.interaction.update",
                        "interaction": {"policy": {"ack_due_ms": 250}, "events": {"floor": False}}})
    await session.wait(lambda e: e["type"] == "interaction_updated" and e.get("applied"), 5.0, third)
    updates = [e for _, e in session.all("interaction_updated", third)]
    applied = [path for e in updates for path in e.get("applied", [])]
    refused = [(r.get("path"), r.get("reason")) for e in updates for r in e.get("refused", [])]
    await session.send({"type": "bud.task.answer", "request_id": "q_none", "outcome": "accept"})
    unknown = await session.wait(lambda e: e["type"] == "error", 5.0, third)
    await session.typed("Describe the history of the Roman empire in detail.")
    task = await session.wait(lambda e: e["type"] == "task_created", 30.0, third)
    cancelled = None
    if task:
        await asyncio.sleep(0.5)
        await session.send({"type": "bud.task.cancel", "task_id": task[2]["task_id"]})
        cancelled = await session.wait(lambda e: e["type"] == "task_cancelled", 15.0, task[0])
    await session.quiet(30.0, third)
    last_run = bud.requests[-1]
    checks["bud.interaction.update is applied where the agent allows it"] = (
        applied == ["policy.ack_due_ms"] and refused == [("events.floor", "not_allowed")])
    checks["bud.task.answer for no open question is an error with its code"] = (
        unknown is not None and unknown[2].get("code") == "unknown_request_id")
    checks["bud.task.cancel cancels the task and the agent's run"] = (
        cancelled is not None and cancelled[2].get("by") == "client"
        and (last_run["ended"] == "dropped" or last_run["response_id"] in bud.cancels))
    wrong = session.well_formed()
    checks["every response on the wire is well formed and has its audio"] = not wrong and not silent
    await session.close()

    report(ctx, "X1", "interaction mode behind the OpenAI Realtime wire", checks,
           f"state: protocol {state.get('protocol_version')}, regime {state.get('regime')}, backend "
           f"{state.get('tasks', {}).get('backend')}; turn 1: speech edges started/stopped {edges}, "
           f"transcript {heard_text}, acknowledgement {acks[0][1]['delta'] if acks else None!r} "
           f"{ms(relays[0][0] - acks[0][0]) if acks and relays else None} ms ahead of the answer, "
           f"{len(relays)} relayed sentence(s) in {len(relay_meta)} relay response(s), first audio {wait_ms} ms "
           f"after the caller's last voiced sample; barge-in: level {before:.1f} dBFS before, 10 dB down "
           f"{ms(down_at)} ms after the first barge-in frame, {drop:.1f} dB down 400-900 ms after it, audible "
           f"frames after 1.5 s {len(late)}, bud.audio.stopped {stopped[2].get('reason') if stopped else None} "
           f"after {ms(stopped[1] - barge.start) if stopped else None} ms, cut response "
           f"{statuses.get(cut_response)}, truncated at {truncated[0].get('audio_end_ms') if truncated else None} "
           f"ms (delivered {delivered}), bud_truncate kept {len(cut.get('output_text', ''))} of "
           f"{len(runs[1]['output']) if len(runs) > 1 else 0} chars; request like the lookup: agent asked "
           f"{asked or None!r}, task_created {bool(held)}, its run input "
           f"{str(held_run)[:70]!r}; update applied {applied} refused {refused}; unknown answer -> "
           f"{unknown[2].get('code') if unknown else None}; cancel -> by {cancelled[2].get('by') if cancelled else None}, "
           f"run {last_run['ended']}; malformed responses {wrong or 'none'}, completed without audio {len(silent)}; "
           f"wire events {len(session.wire)}",
           "bud.interaction.state after session.created with protocol 1.1, regime cascade, backend "
           "agent:support, no error; the greeting's response is heard in full; turn 1 has speech_started, "
           "speech_stopped and a completed transcript that names the capital, an acknowledgement response "
           "(bud_kind ack, by the front or the gateway's phrase bank) before the first relay response, one task by agent:support that completes, "
           "every relayed sentence found word for word in what the agent wrote and carried by a response "
           "with bud_source background, bud_verbatim true and the task's id, first agent audio <= 3000 ms "
           "after the caller's last voiced sample; the barge-in: bud.audio.stopped reason barge_in, level "
           ">= 10 dB down within 400 ms of the first barge-in frame and no audible agent frame from 1.5 s "
           "to the end of the caller's words, the cut line's response ends cancelled with "
           "conversation.item.truncated at its delivered_ms, and the agent's next run carries bud_truncate "
           "for the cut one with less text than the agent wrote; a request like the lookup just done gets no "
           f"{WHICH!r} and starts a run that names Sunday; bud.interaction.update applies policy.ack_due_ms and refuses events.floor with "
           "not_allowed; bud.task.answer for an unknown request gives error code unknown_request_id; "
           "bud.task.cancel gives bud.task.cancelled by client and the agent's run is dropped or "
           "cancelled; no response is created while another is open, no audio or transcript delta "
           "lies outside its response, and every completed response has audio",
           "the client reads only the Realtime wire: what it checks is rebuilt from bud.* events and "
           "from responses with metadata.bud_*. The agent is the stand-in Bud gateway over the "
           "background llama-server. A tool confirmation cannot arise here: an agent's tools are "
           "budprompt's, and its spoken approvals are not in this build")
    return session


VENDOR_TOOL = {
    "type": "function", "name": "lookup_order",
    "description": "Look up the status of an order by its order number.",
    "parameters": {"type": "object", "properties": {"order_number": {"type": "string"}},
                   "required": ["order_number"]},
}
VENDOR_INSTRUCTIONS = ("You are the phone assistant of the Larkspur shop. When the caller asks about an "
                       "order, call lookup_order with its number and then tell the caller what it returned.")
INJECTED = "Your table is booked for seven o'clock."


async def x2(ctx: Ctx):
    """The cascade as a local speech-to-speech model for an OpenAI Realtime client: audio both ways, a function tool, injected text."""
    bud = ctx.stack.ensure_bud()
    checks: Dict[str, bool] = {}
    mark = len(bud.requests)
    plain = "You are the phone assistant of the Larkspur shop. Answer in one short plain sentence."
    session = await Realtime.open(ctx, "X2", "vendor", interaction=False, session={
        "instructions": plain,
        # The local recogniser returns a transcript per upload, so the client ends the turn.
        "audio": {"input": {"turn_detection": None}}})
    shown = session.of("session.updated")[-1][1]["session"]

    # 1. Audio in, a transcript and audio out; the client commits its turn.
    wire = len(session.wire)
    said_at = await session.say("20_q_capital")
    await asyncio.sleep(0.3)
    committed = now()
    await session.send({"type": "input_audio_buffer.commit"})
    done = await session.wired("response.done", 40.0, wire)
    heard_text = [e["transcript"] for _, e in session.of("conversation.item.input_audio_transcription.completed", wire)]
    answer = "".join(e["delta"] for _, e in session.of("response.output_audio_transcript.delta", wire))
    first = session.first_audible(said_at.last_voiced)
    wait_ms = ms(first - committed) if first else None
    edges = (len(session.of("input_audio_buffer.speech_started", wire)),
             len(session.of("input_audio_buffer.speech_stopped", wire)))
    checks["a spoken question is transcribed and answered in audio"] = (
        shown.get("instructions") == plain and shown["audio"]["input"]["turn_detection"] is None
        and done is not None and done[2]["response"]["status"] == "completed"
        and any("capital" in text.lower() for text in heard_text) and "paris" in answer.lower()
        and first is not None and edges[0] >= 1 and edges[1] >= 1)
    await session.played(wire)

    # 2. A note for the model: a system item says nothing until the next response.
    wire = len(session.wire)
    await session.send({"type": "conversation.item.create", "item": {
        "type": "message", "role": "system",
        "content": [{"type": "input_text", "text": "The shop closes at six today because of a staff meeting."}]}})
    await asyncio.sleep(1.5)
    quiet = not session.of("response.created", wire)
    await session.typed("When does the shop close today?")
    await session.wired("response.done", 40.0, wire)
    closing = "".join(e["delta"] for _, e in session.of("response.output_audio_transcript.delta", wire))
    checks["a system item is context for the next response"] = quiet and bool(re.search(r"\b(six|6)\b", closing.lower()))
    await session.played(wire)

    # 3. Text to be spoken: a response of its own with instructions, and its metadata back.
    wire = len(session.wire)
    await session.send({"type": "response.create", "response": {
        "instructions": f"Say exactly this sentence and nothing else: {INJECTED}",
        "metadata": {"bud_kind": "say", "probe": "x2"}}})
    made = await session.wired("response.created", 20.0, wire)
    await session.wired("response.done", 40.0, wire)
    injected = "".join(e["delta"] for _, e in session.of("response.output_audio_transcript.delta", wire))
    echoed = (made[2]["response"].get("metadata") or {}) if made else {}
    audio = [e for _, e in session.of("response.output_audio.delta", wire)]
    checks["injected text is spoken, and the response carries the client's metadata"] = (
        norm(INJECTED) in norm(injected) and echoed.get("probe") == "x2" and bool(audio))
    await session.played(wire)
    wrong = session.well_formed()
    cut = len(session.of("conversation.item.truncated"))
    # The session does not run interaction mode: its messages are refused by name.
    wire = len(session.wire)
    await session.send({"type": "bud.task.cancel", "task_id": "task_none"})
    not_on = await session.wired("error", 5.0, wire)
    checks["an interaction message on a session without the mode is refused"] = (
        not_on is not None and not_on[2]["error"].get("code") == "interaction_not_enabled")
    await session.close()

    # 4. A function of the client's own: the call, its output, the spoken result.
    tooled = await Realtime.open(ctx, "X2a", "vendor", interaction=False, mic=False, session={
        "instructions": VENDOR_INSTRUCTIONS, "tools": [VENDOR_TOOL], "tool_choice": "auto"})
    declared = tooled.of("session.updated")[-1][1]["session"]
    wire = len(tooled.wire)
    await tooled.typed("Where is my order number 4417?")
    call = await tooled.wired("response.function_call_arguments.done", 40.0, wire)
    call_done = await tooled.wired("response.done", 20.0, wire)
    arguments = call[2].get("arguments", "") if call else ""
    spoken_after = ""
    if call:
        wire = len(tooled.wire)
        await tooled.send({"type": "conversation.item.create", "item": {
            "type": "function_call_output", "call_id": call[2]["call_id"],
            "output": json.dumps({"status": "shipped", "arrives": "Tuesday"})}})
        await tooled.send({"type": "response.create"})
        await tooled.wired("response.done", 40.0, wire)
        spoken_after = "".join(e["delta"] for _, e in tooled.of("response.output_audio_transcript.delta", wire))
    runs = bud.requests[mark:]
    sent_back = [item for run in runs for item in (run["body"].get("input") or [])
                 if isinstance(item, dict) and item.get("type") == "function_call_output"]
    checks["the session declares its own function tool"] = (
        [tool.get("name") for tool in declared.get("tools", [])] == ["lookup_order"]
        and declared.get("instructions") == VENDOR_INSTRUCTIONS and not tooled.of("error"))
    checks["the agent calls the client's function"] = (
        call is not None and call[2].get("name") == "lookup_order" and "4417" in arguments
        and call_done is not None and any(
            item.get("type") == "function_call" for item in call_done[2]["response"].get("output", [])))
    checks["the function's output reaches the agent and the result is spoken"] = (
        bool(sent_back) and "tuesday" in spoken_after.lower())
    wrong += tooled.well_formed()
    checks["every response on the wire is well formed, and none was cut"] = not wrong and cut == 0
    await tooled.close()

    # 5. An agent that does not allow it keeps its own tools and instructions.
    closed = await Realtime.open(ctx, "X2b", "support", interaction=False, mic=False, session={
        "instructions": "Be rude.", "tools": [VENDOR_TOOL]})
    refusals = [e["error"].get("param") for _, e in closed.of("error")]
    kept = closed.of("session.updated")[-1][1]["session"]
    checks["an agent that does not allow it refuses both"] = (
        sorted(refusals) == ["session.instructions", "session.tools"]
        and kept.get("tools") == [] and kept.get("instructions") == "")
    await closed.close()

    report(ctx, "X2", "the cascade as a local speech-to-speech model", checks,
           f"session.updated tools {[tool.get('name') for tool in declared.get('tools', [])]}; spoken question: "
           f"transcript {heard_text}, speech edges {edges}, answer {answer[:80]!r}, first audio {wait_ms} ms after "
           f"the commit; function call {call[2].get('name') if call else None}"
           f"({arguments}), outputs sent to the agent {len(sent_back)}, then spoken {spoken_after[:80]!r}; "
           f"injected: asked {INJECTED!r}, spoken {injected!r}, metadata {echoed}, audio deltas {len(audio)}; "
           f"system item: silent before the next response {quiet}, then {closing[:80]!r}; malformed responses "
           f"{wrong or 'none'}, truncations {cut}; refusals on an agent without the overrides {refusals}; runs {len(runs)}",
           "session.updated shows the session's own instructions and turn_detection null, and on the "
           "session with a tool its one function tool, with no error; a spoken question followed by "
           "input_audio_buffer.commit gives speech_started, speech_stopped, a completed transcript "
           "that names the capital, a completed response whose transcript names Paris, and audible "
           "audio; a typed order question gives "
           "response.function_call_arguments.done for lookup_order with 4417 in its arguments and a "
           "function_call item in response.done; after function_call_output and response.create the "
           "agent's next request carries the output and the spoken answer names Tuesday; "
           "response.create with instructions speaks the sentence asked for, word for word, in a "
           "response whose metadata has the client's keys; a system item starts no response within "
           "1.5 s and the next answer uses it (six); no response is created while another is open "
           "and no conversation.item.truncated is sent; bud.task.cancel on this session gives error "
           "code interaction_not_enabled; "
           "an agent whose session_overrides list neither gets event_not_allowed for "
           "session.instructions and session.tools and shows none",
           "the agent is the stand-in Bud gateway over the background llama-server, which takes "
           "`tools`, `instructions` and input items as the Responses API defines them; whether "
           "budprompt does is not known here. The word-for-word check is on a small local model that "
           "is told to say one sentence. The local recogniser answers per upload, so the client ends "
           "its turn with input_audio_buffer.commit: the session's own turn detection needs a "
           "streaming speech-to-text deployment")
    return session


def dag_definition(ctx: Ctx, speaks: bool) -> dict:
    """An inline DAG over the background model: it ends in text, or it synthesises the answer
    itself."""
    nodes = [
        {"id": "audio_input", "type": "audio_input"},
        {"id": "stt", "type": "stt_provider", "provider": "openai"},
        {"id": "llm", "type": "llm_endpoint", "base_url": f"http://127.0.0.1:{ctx.args.background_port}/v1",
         "model": ctx.args.background_alias, "api_key": "local-dummy", "streaming": False,
         "system_prompt": "You answer callers of the Larkspur shop on the phone in two short plain sentences."},
    ]
    edges = [{"from": "audio_input", "to": "stt"}, {"from": "stt", "to": "llm"}]
    if speaks:
        nodes += [{"id": "tts", "type": "tts_provider", "provider": "openai"},
                  {"id": "out", "type": "audio_output", "destination": "web_socket"}]
        edges += [{"from": "llm", "to": "tts"}, {"from": "tts", "to": "out"}]
    else:
        nodes.append({"id": "out", "type": "text_output", "destination": "web_socket"})
        edges.append({"from": "llm", "to": "out"})
    return {"id": "live-dag", "name": "Live DAG", "version": "1.0.0", "nodes": nodes, "edges": edges,
            "entry_node": "audio_input", "exit_nodes": ["out"]}


async def dg1(ctx: Ctx):
    """A `/ws` DAG that ends in text is the background; one that speaks itself is refused."""
    checks: Dict[str, bool] = {}
    session = await Session.open(ctx, "DG1", MODEL_FRONT, conversation=False,
                                 extra={"dag_config": {"definition": dag_definition(ctx, False)}})
    await session.warm()
    backend = session.ready.get("interaction", {}).get("tasks", {})
    turn = await session.turn("20_q_capital")
    start = turn.first_event
    created = [e for _, e in session.all("task_created", start)]
    results = session.all("task_result", start)
    relays = session.transcripts(start, "relay")
    acks = session.transcripts(start, "ack")
    first = session.first_audible(turn.said.last_voiced)
    wait_ms = ms(first - turn.said.last_voiced) if first else None
    dag_messages = [e for _, e in session.events if e["type"] == "message"]
    # A second request: the graph's LLM node keeps the conversation.
    again = len(session.events)
    await session.send({"type": "agent_input", "text": "And how many people live there?"})
    await session.wait(lambda e: e["type"] == "task_result", 60.0, again)
    await session.quiet(40.0, again)
    follow = " ".join(e["delta"] for _, e in session.transcripts(again, "relay"))
    follow_result = session.all("task_result", again)
    checks["the session runs the mode on its DAG"] = (
        session.ready.get("protocol_version") == "1.1" and backend.get("backend") == "dag"
        and backend.get("speculative_start") == "off" and not mode_warnings(session.warnings))
    checks["a task by the DAG, not speculative, completed"] = (
        len(created) == 1 and created[0].get("background") == "dag"
        and created[0].get("speculative") is False
        and bool(results) and results[0][1].get("state") == "completed")
    checks["the graph's text is relayed, not sent as a message"] = (
        bool(relays) and "paris" in " ".join(e["delta"] for _, e in relays).lower() and not dag_messages)
    checks["the caller hears something within 3.0 s"] = wait_ms is not None and wait_ms <= 3000
    checks["the graph keeps the conversation across tasks"] = (
        bool(follow_result) and follow_result[0][1].get("state") == "completed"
        and bool(re.search(r"million|\d", follow.lower())))
    await session.close()

    # A graph that synthesises its own answer: refused with its code, and it runs as a plain DAG.
    plain = await Session.open(ctx, "DG1b", MODEL_FRONT, conversation=False, mic=False,
                               extra={"dag_config": {"definition": dag_definition(ctx, True)}})
    codes = [(w.get("code"), (w.get("detail") or {}).get("reason")) for w in plain.warnings]
    checks["a DAG with an audio output is refused with its code"] = (
        codes == [("interaction_dag_audio_output", "dag_audio_output")]
        and plain.ready.get("protocol_version") == "1.0" and "interaction" not in plain.ready)
    await plain.close()

    report(ctx, "DG1", "a text-output DAG as the background", checks,
           f"ready protocol {session.ready.get('protocol_version')}, backend {backend.get('backend')}, "
           f"speculative_start {backend.get('speculative_start')}, warnings {len(session.warnings)}; tasks "
           f"{len(created)} ({created[0].get('background') if created else None}), acknowledgement "
           f"{acks[0][1]['delta'] if acks else None!r}, relayed "
           f"{' '.join(e['delta'] for _, e in relays)[:90]!r}, dag_output messages {len(dag_messages)}, first "
           f"audio {wait_ms} ms after the caller's last voiced sample; follow-up answer {follow[:90]!r}; "
           f"audio-output DAG: warnings {codes}, ready protocol {plain.ready.get('protocol_version')}",
           "ready 1.1 with tasks.backend dag and speculative_start off, no config_warning; one task with "
           "background dag, speculative false, that completes; the answer names Paris and is spoken as "
           "relay lines, and no `message` with topic dag_output is sent; first agent audio <= 3000 ms "
           "after the caller's last voiced sample; a typed follow-up (\"how many people live there\") "
           "completes with an answer that gives a number, which needs the first turn; the same graph "
           "with a tts_provider and an audio_output gets exactly one config_warning, "
           "interaction_dag_audio_output, and ready 1.0 without an interaction block",
           "the graph is stt_provider -> llm_endpoint (the background llama-server, not streaming) -> "
           "text_output; the speech-to-text and the voice are the session's own")
    return session



# ---- tasks that outlive the socket (two gateway processes on one task Redis)

# Limits of DT1 to DT3, in seconds. The booking tool takes 6 s; the background model then words
# its answer in one more round.
DT_RESULT_S = 40.0  # from the end of the connection to the task's result, wherever it goes
DT_GRACE_S = 8  # the grace of the sessions that are lost on purpose
DT_SWEEP_S = (35.0, 70.0)  # a dead process's task is marked this long after the kill: the
#                            lease is 30 s from its last renewal (every 10 s), the beat every 10 s
#                            suspects it, and the sweep waits 15 s more (two beats) before acting
DT_LOOKUP_TOOL_S = 12.0  # the lookup that is still running when the caller goes


def task_results(session: Session, task_id: str) -> list:
    return [e for _, e in session.events if e["type"] == "task_result" and e.get("task_id") == task_id]


async def booking_on_the_wire(ctx: Ctx, label: str, port: int, grace_s: Optional[int] = None,
                              stream_id: Optional[str] = None):
    """A session on the gateway at `port` whose confirmed booking call is on the wire (the
    tool takes 6 s). Returns the session, the task's `task_created` event and the call."""
    block = with_tools(ctx, "book_table")
    if grace_s is not None:
        block["tasks"]["reconnect_grace_s"] = grace_s
    mark = len(ctx.webhook.calls)
    extra = {"stream_id": stream_id} if stream_id else None
    session = await Session.open(ctx, label, block, TOOLS_PROMPT, port=port, extra=extra)
    await asyncio.sleep(3.0)  # the warm-up lines
    start = len(session.events)
    await session.say("10_book_table")
    _, after = await asked_and_heard(session, start)
    await session.say("12_yes")
    deadline = now() + 20
    while now() < deadline and not ctx.webhook.since(mark):
        await asyncio.sleep(0.02)
    if not ctx.webhook.since(mark):
        raise RuntimeError("the yes did not start the booking")
    created = session.all("task_created", start)[-1][1]
    return session, created, ctx.webhook.since(mark)[0]


async def pushed(sink: Webhook, task_id: str, timeout: float) -> Optional[dict]:
    """The push of this task's result, once it arrives."""
    deadline = now() + timeout
    while now() < deadline:
        for call in sink.calls:
            if call["body"].get("task_id") == task_id:
                return call
        await asyncio.sleep(0.05)
    return None


def push_checks(push: Optional[dict], task_id: str, stream_id: str) -> Dict[str, bool]:
    body = push["body"] if push else {}
    return {
        "a push arrived": push is not None,
        "it is signed with the tool secret": bool(push) and push["signed"] and push["signature_ok"],
        "its Idempotency-Key is the task id": bool(push) and push["idempotency_key"] == task_id,
        "it is the task_result, completed, with the session's stream_id": body.get("type") == "task_result"
        and body.get("state") == "completed" and body.get("stream_id") == stream_id,
        "it carries the answer and the committed booking": bool(body.get("summary"))
        and body.get("effects") == ["booked a table for 2"],
    }


async def dt1(ctx: Ctx):
    """A booking on the wire when the caller hangs up finishes, and its result is pushed, signed."""
    sink = ctx.sink()
    first, _ = ctx.stack.ensure_durable(sink.url)
    http = f"http://127.0.0.1:{first}"
    ctx.webhook.delay["book_table"] = 6.0
    try:
        # 1. A clean close: nobody comes back, so the result is pushed when there is one.
        session, created, call = await booking_on_the_wire(ctx, "DT1-close", first)
        task_id, stream_id = created["task_id"], session.stream_id
        echo = session.ready.get("interaction", {}).get("tasks", {})
        await session.close()
        closed = now()
        on_wire = "done" not in call
        push = await pushed(sink, task_id, DT_RESULT_S + 5)
        status, kept = get_status(f"{http}/v1/tasks/{task_id}?stream_id={stream_id}")
        result = kept.get("result") or {}
        # 2. A lost connection and nobody back: the push waits for the grace to end.
        session, created2, call2 = await booking_on_the_wire(ctx, "DT1-lost", first, DT_GRACE_S)
        task2, stream2 = created2["task_id"], session.stream_id
        await session.close(lost=True)
        lost = now()
        on_wire2 = "done" not in call2
        push2 = await pushed(sink, task2, DT_GRACE_S + DT_RESULT_S + 5)
        # 3. A lookup (no tool call that needs the caller's yes) still running at the hang-up
        # runs on to its answer, which is pushed.
        ctx.webhook.delay["check_availability"] = DT_LOOKUP_TOOL_S
        session, _, created3, call3 = await slow_lookup(ctx, "DT1-lookup", first)
        task3, stream3 = created3[2]["task_id"], session.stream_id
        await session.close()
        closed3 = now()
        out3 = "done" not in call3
        unanswered3 = not task_results(session, task3)
        push3 = await pushed(sink, task3, DT_RESULT_S + 5)
    finally:
        ctx.webhook.delay.pop("book_table", None)
        ctx.webhook.delay["check_availability"] = 6.0
    took = push["t"] - closed if push else None
    took2 = push2["t"] - lost if push2 else None
    took3 = push3["t"] - closed3 if push3 else None
    body3 = push3["body"] if push3 else {}
    checks = {
        "ready shows the grace and that a push address is set, not the address": echo.get("push") is True
        and echo.get("reconnect_grace_s") == 20 and sink.url not in json.dumps(session.ready),
        "the call was on the wire at the close": on_wire,
        **{f"clean close: {name}": ok for name, ok in push_checks(push, task_id, stream_id).items()},
        f"clean close: pushed within {DT_RESULT_S:.0f} s of the close": took is not None and took <= DT_RESULT_S,
        "GET /v1/tasks/{id} keeps the result": status == 200 and kept.get("state") == "completed"
        and bool(push) and result.get("summary") == push["body"].get("summary")
        and result.get("effects") == ["booked a table for 2"] and kept.get("stream_id") == stream_id,
        "the call was on the wire when the connection was lost": on_wire2,
        **{f"lost: {name}": ok for name, ok in push_checks(push2, task2, stream2).items()},
        f"lost: pushed no sooner than the {DT_GRACE_S} s grace and within {DT_GRACE_S + DT_RESULT_S:.0f} s":
            took2 is not None and DT_GRACE_S <= took2 <= DT_GRACE_S + DT_RESULT_S,
        "each task is pushed once": [c["body"].get("task_id") for c in sink.calls].count(task_id) == 1
        and [c["body"].get("task_id") for c in sink.calls].count(task2) == 1,
        "lookup: its tool call was out and no result had come at the hang-up": out3 and unanswered3,
        f"lookup: its completed answer is pushed, signed, within {DT_RESULT_S:.0f} s of the hang-up":
            took3 is not None and took3 <= DT_RESULT_S and push3["signed"] and push3["signature_ok"]
            and body3.get("type") == "task_result" and body3.get("state") == "completed"
            and bool(body3.get("summary")) and body3.get("effects") == "none_committed"
            and body3.get("stream_id") == stream3,
    }
    report(ctx, "DT1", "a task outlives the hang-up and its result is pushed", checks,
           f"clean close with the 6 s booking {'still out' if on_wire else 'already back'}: push "
           f"{ms(took)} ms after the close, body {push['body'] if push else None}, signed "
           f"{push and push['signed'] and push['signature_ok']}, Idempotency-Key "
           f"{push and push['idempotency_key']!r}; GET /v1/tasks/{{id}} -> {status} state "
           f"{kept.get('state')!r} result {result}. Lost connection (grace {DT_GRACE_S} s) with the "
           f"call {'still out' if on_wire2 else 'already back'}: push {ms(took2)} ms after the loss, "
           f"state {push2['body'].get('state') if push2 else None!r}, summary "
           f"{push2['body'].get('summary') if push2 else None!r}. Lookup with its {DT_LOOKUP_TOOL_S:.0f} s "
           f"tool {'still out' if out3 else 'already back'} and {'no' if unanswered3 else 'a'} result at the "
           f"hang-up: push {ms(took3)} ms after it, state {body3.get('state')!r}, effects "
           f"{body3.get('effects')!r}, summary {body3.get('summary')!r}",
           f"the booking that was on the wire completes after the hang-up; a signed task_result with its "
           f"answer, effects and stream_id reaches the push address within {DT_RESULT_S:.0f} s of a clean "
           f"close, and between {DT_GRACE_S} and {DT_GRACE_S + DT_RESULT_S:.0f} s after a lost connection "
           f"with a {DT_GRACE_S} s grace; the result is kept for GET /v1/tasks/{{id}}; a lookup still "
           f"running at a clean close (tasks.session_end auto, a push address) completes and its answer, "
           f"effects none_committed, is pushed signed within {DT_RESULT_S:.0f} s",
           "two gateway processes on one task Redis; this scenario uses the first")


async def dt2(ctx: Ctx):
    """A caller who lost the connection and is back on another gateway process gets the result."""
    sink = ctx.sink()
    first, second = ctx.stack.ensure_durable(sink.url)
    ctx.webhook.delay["book_table"] = 6.0
    try:
        session, created, call = await booking_on_the_wire(ctx, "DT2-lost", first)
        task_id, stream_id = created["task_id"], session.stream_id
        await session.close(lost=True)
        lost = now()
        on_wire = "done" not in call
        # Back at once, on the other process, with the same stream id.
        back = await Session.open(ctx, "DT2-back", with_tools(ctx, "book_table"), TOOLS_PROMPT,
                                  port=second, extra={"stream_id": stream_id})
        result = await back.wait(lambda e: e["type"] == "task_result" and e.get("task_id") == task_id,
                                 DT_RESULT_S + 5)
        line = await back.wait(
            lambda e: e["type"] == "assistant_transcript" and e.get("task_id") == task_id and e.get("delta"),
            10) if result else None
        line_end = await heard(back, line) if line else None
        audible = back.first_audible(lost)
        listed = get_json(f"http://127.0.0.1:{second}/v1/tasks?stream_id={stream_id}").get("tasks", [])
        # A second request on the new connection takes the next task ref.
        start = len(back.events)
        await asyncio.sleep(1.0)
        await back.say("20_q_capital")
        again = await back.wait(lambda e: e["type"] == "task_created", 30, start)
        # The grace ends (20 s from the loss): the caller has the result, so nothing is pushed.
        await asyncio.sleep(max(0.0, lost + 20 + 4 - now()))
        push = await pushed(sink, task_id, 0.1)
        await back.close()
        # A lookup still running when the connection is lost runs on through the grace, and its
        # answer reaches the caller who is back on the other process.
        ctx.webhook.delay["check_availability"] = DT_LOOKUP_TOOL_S
        look, _, created_l, call_l = await slow_lookup(ctx, "DT2-lookup", first)
        task_l, stream_l = created_l[2]["task_id"], look.stream_id
        await look.close(lost=True)
        lost_l = now()
        out_l = "done" not in call_l and not task_results(look, task_l)
        back_l = await Session.open(ctx, "DT2-lookup-back", with_tools(ctx, "check_availability"),
                                    TOOLS_PROMPT, port=second, extra={"stream_id": stream_l})
        result_l = await back_l.wait(
            lambda e: e["type"] == "task_result" and e.get("task_id") == task_l, DT_RESULT_S + 5)
        line_l = await back_l.wait(
            lambda e: e["type"] == "assistant_transcript" and e.get("task_id") == task_l and e.get("delta"),
            10) if result_l else None
        line_l_end = await heard(back_l, line_l) if line_l else None
        await back_l.close()
    finally:
        ctx.webhook.delay.pop("book_table", None)
        ctx.webhook.delay["check_availability"] = 6.0
    event = result[2] if result else {}
    took = result[1] - lost if result else None
    event_l = result_l[2] if result_l else {}
    took_l = result_l[1] - lost_l if result_l else None
    checks = {
        "the call was on the wire when the connection was lost": on_wire,
        f"task_result of the first process's task on the second, within {DT_RESULT_S:.0f} s": took is not None
        and took <= DT_RESULT_S and event.get("state") == "completed",
        "it carries the committed booking": event.get("effects") == ["booked a table for 2"],
        "the answer is spoken as the background's words and heard in full": bool(line)
        and line[2].get("source") == "background" and line[2].get("verbatim") is True
        and line[2]["delta"] == event.get("summary") and line_end is not None and audible is not None,
        "the second process lists the task as completed": [t.get("state") for t in listed
                                                           if t.get("id") == task_id] == ["completed"],
        "a new task on the new connection is t2": bool(again) and again[2].get("task_ref") == "t2",
        "nothing is pushed for it after the grace": push is None,
        "lookup: its tool call was out and no result had come when the connection was lost": out_l,
        f"lookup: its completed answer reaches the caller back on the other process within "
        f"{DT_RESULT_S:.0f} s, and is heard in full": took_l is not None and took_l <= DT_RESULT_S
        and event_l.get("state") == "completed" and event_l.get("effects") == "none_committed"
        and bool(line_l) and line_l[2]["delta"] == event_l.get("summary") and line_l_end is not None,
    }
    report(ctx, "DT2", "reconnect within the grace on the other process", checks,
           f"lost on :{first} with the booking {'still out' if on_wire else 'already back'}, back on "
           f":{second} {ms(back.opened - lost)} ms later: task_result {ms(took)} ms after the loss, state "
           f"{event.get('state')!r}, effects {event.get('effects')!r}; the agent said "
           f"{line[2]['delta'] if line else None!r} (source {line[2].get('source') if line else None}, "
           f"verbatim {line[2].get('verbatim') if line else None}), heard in full "
           f"{ms(line_end - lost) if line_end else None} ms after the loss; GET /v1/tasks on :{second} -> "
           f"{[(t.get('task_ref'), t.get('state')) for t in listed]}; the next task there is "
           f"{again[2].get('task_ref') if again else None!r}; pushes for the task by "
           f"{ms(now() - lost)} ms: {0 if push is None else 1}. Lookup lost with its tool "
           f"{'still out' if out_l else 'already back'}: task_result on :{second} {ms(took_l)} ms after the "
           f"loss, state {event_l.get('state')!r}, effects {event_l.get('effects')!r}, said "
           f"{line_l[2]['delta'] if line_l else None!r}, heard in full "
           f"{ms(line_l_end - lost_l) if line_l_end else None} ms after the loss",
           f"the caller who is back on the other gateway process within the 20 s grace gets the task_result "
           f"of the task the first process ran within {DT_RESULT_S:.0f} s of the loss, hears its answer "
           f"word for word, sees the task listed there, and no push is sent; a lookup still running when "
           f"its connection is lost completes and its answer reaches the caller back on the other process "
           f"within {DT_RESULT_S:.0f} s, heard in full",
           "the result travels through the task Redis (publish and subscribe)")


async def dt3(ctx: Ctx):
    """A task is read from the other gateway process, and one whose process was killed is marked truthfully."""
    sink = ctx.sink()
    first, second = ctx.stack.ensure_durable(sink.url)
    other = f"http://127.0.0.1:{first}"
    ctx.webhook.delay["book_table"] = 30.0
    try:
        # The session runs on the second process; the first only shares the Redis.
        session, created, _ = await booking_on_the_wire(ctx, "DT3-run", second)
        task_id, stream_id = created["task_id"], session.stream_id
        await asyncio.sleep(0.5)
        status, read = get_status(f"{other}/v1/tasks/{task_id}?stream_id={stream_id}")
        listed = get_json(f"{other}/v1/tasks?stream_id={stream_id}").get("tasks", [])
        unnamed, _ = get_status(f"{other}/v1/tasks/{task_id}")
        wrong, _ = get_status(f"{other}/v1/tasks/{task_id}?stream_id=somebody-else")
        nothing = get_json(f"{other}/v1/tasks").get("tasks", [])
        # The process dies with the booking call on the wire.
        ctx.stack.kill("task-gateway-b")
        killed = now()
        await session.close(lost=True)
        # The caller is back on the process that is left, before anyone knows what happened.
        back = await Session.open(ctx, "DT3-back", with_tools(ctx, "book_table"), TOOLS_PROMPT,
                                  port=first, extra={"stream_id": stream_id})
        early_status, early = get_status(f"{other}/v1/tasks/{task_id}?stream_id={stream_id}")
        marked, after = None, {}
        while now() - killed < DT_SWEEP_S[1] + 10:
            _, after = get_status(f"{other}/v1/tasks/{task_id}?stream_id={stream_id}")
            if after.get("state") not in ("working", "input_required", "submitted"):
                marked = now() - killed
                break
            await asyncio.sleep(0.25)
        told = await back.wait(lambda e: e["type"] == "task_result" and e.get("task_id") == task_id, 10)
        line = await back.wait(
            lambda e: e["type"] == "assistant_transcript" and e.get("task_id") == task_id and e.get("delta"),
            10) if told else None
        if line:
            await heard(back, line)
        await asyncio.sleep(1.0)
        push = await pushed(sink, task_id, 0.1)
        await back.close()
    finally:
        ctx.webhook.delay.pop("book_table", None)
    result = after.get("result") or {}
    event = told[2] if told else {}
    checks = {
        "the other process reads the running task by id": status == 200 and read.get("state") == "working"
        and read.get("stream_id") == stream_id and read.get("result") is None,
        "and lists it for its session": [t.get("id") for t in listed] == [task_id],
        "not without the session's name, nor under another": unnamed == 404 and wrong == 404 and nothing == [],
        "right after the kill it is still working": early_status == 200 and early.get("state") == "working",
        f"marked {DT_SWEEP_S[0]:.0f} to {DT_SWEEP_S[1]:.0f} s after the kill": marked is not None
        and DT_SWEEP_S[0] <= marked <= DT_SWEEP_S[1],
        "as unknown with unknown effects (the booking call was out)": after.get("state") == "unknown"
        and after.get("effects") == "unknown" and result.get("state") == "unknown"
        and "stopped" in (result.get("summary") or ""),
        "the caller who is back is told so": event.get("state") == "unknown" and event.get("effects") == "unknown",
        "and hears that the agent is not sure it went through": bool(line)
        and line[2].get("source") == "gateway" and "not sure whether" in line[2]["delta"],
        "nothing is pushed while the caller has it": push is None,
    }
    report(ctx, "DT3", "a task read from the other process; a killed process's task", checks,
           f"task on :{second}; GET /v1/tasks/{{id}} on :{first} -> {status} state {read.get('state')!r}; "
           f"listed there {[t.get('task_ref') for t in listed]}; without stream_id {unnamed}, with "
           f"another's {wrong}, unnamed list {nothing}. :{second} killed with the booking call out: "
           f"still {early.get('state')!r} at once, {after.get('state')!r} with effects "
           f"{after.get('effects')!r} after {ms(marked)} ms, result {result}; the caller back on :{first} "
           f"got task_result state {event.get('state')!r} effects {event.get('effects')!r} and heard "
           f"{line[2]['delta'] if line else None!r}; pushes {0 if push is None else 1}",
           f"a task one process runs is read by id and listed on the other, only with its session's "
           f"stream_id; after kill -9 of its process it is marked unknown with unknown effects between "
           f"{DT_SWEEP_S[0]:.0f} and {DT_SWEEP_S[1]:.0f} s later (lease 30 s, then suspected for 15 s), and a "
           f"caller who is back on the other process is told that and not that it failed or was done",
           "this scenario kills the second task gateway: it runs last of DT1 to DT3")


# What the caller hears when a cancelled task is confirmed, by what is known of its effects.
STOPPED = {
    "none_committed": "It's stopped.",
    "unknown": "I've stopped it, but I can't confirm whether it had already gone through.",
}


async def backend_session(ctx: Ctx, label: str, name: str, tools: Tuple[str, ...] = (),
                          system_prompt: Optional[str] = None) -> Session:
    """A session whose tasks run on the catalog entry `name`. Without a prompt it is a raw
    session: speech-to-text and text-to-speech only."""
    stand_in = ctx.stack.ensure_backends()
    tasks: Dict[str, Any] = {"default_backend": name}
    if tools:
        tasks["tools"] = [ctx.webhook.tool(tool) for tool in tools]
    return await Session.open(ctx, label, {**MODEL_FRONT, "tasks": tasks}, system_prompt,
                              conversation=system_prompt is not None, port=stand_in.port)


async def cancel_by_voice(session: Session, start: int) -> Tuple[Optional[dict], Optional[float], List[str]]:
    """The caller says "cancel that": the `task_cancelled` event, how long after the end of
    that turn it came, and what the agent said from then on."""
    cancel_from = len(session.events)
    await session.say("04_cancel")
    final = await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 15, cancel_from)
    cancelled = await session.wait(lambda e: e["type"] == "task_cancelled", 20, cancel_from)
    await session.quiet(20, start)
    await asyncio.sleep(1.0)
    lines = [e["delta"] for _, e in session.transcripts(cancel_from) if e.get("delta")]
    after_e = cancelled[1] - final[1] if cancelled and final else None
    return (cancelled[2] if cancelled else None), after_e, lines


async def backend_calls(ctx: Ctx, sid: str, name: str, wire: str, cancel_effects: Any, cancel_line: str,
                        cancel_method: str, question_kind: str):
    """An answer, a question to the caller and a cancellation on a backend that runs the task
    itself (`Backends.work`)."""
    stand_in = ctx.stack.ensure_backends()
    label = f"{ {'hook': 'webhook', 'a2a': 'a2a', 'mcp': 'mcp'}[wire] }:{name}"
    checks: Dict[str, bool] = {}

    # An answer: the acknowledgement, then the backend's words.
    session = await backend_session(ctx, f"{sid}-answer", name)
    await session.warm()
    mark = len(stand_in.seen)
    turn = await session.turn("20_q_capital")
    created = session.all("task_created", turn.first_event)
    results = session.all("task_result", turn.first_event)
    first = session.first_audible(turn.e) if turn.e is not None else None
    lines = [(e.get("kind"), e["delta"]) for _, e in session.transcripts(turn.first_event) if e.get("delta")]
    await session.close()
    job = stand_in.last(wire) or {}
    summary = results[0][1].get("summary", "") if results else ""
    first_ms = ms(first - turn.e) if first is not None else None
    checks["the session runs on the backend, with no run before the turn is settled"] = (
        session.ready.get("interaction", {}).get("tasks", {}).get("backend") == label
        and len(created) == 1 and created[0][1].get("background") == label
        and created[0][1].get("speculative") is False)
    checks["something is audible within 2,000 ms of the end of the turn"] = (
        first_ms is not None and first_ms <= 2000)
    checks["the backend's answer is relayed word for word"] = (
        bool(results) and results[0][1].get("state") == "completed" and bool(job.get("result"))
        and norm(summary) == norm(job["result"]) and "paris" in norm(summary))
    checks["every request of the gateway passed the backend's checks"] = (
        bool(stand_in.of(wire, None, mark)) and all(hit["ok"] for hit in stand_in.of(wire, None, mark)))

    # A question: the backend asks, the caller answers by voice, the backend goes on.
    session = await backend_session(ctx, f"{sid}-question", name)
    await session.warm()
    start = len(session.events)
    await session.say("50_order_flowers")
    asked, after = await question(session, start)
    await heard(session, await spoken(session, after - 1, "question"))
    await asyncio.sleep(0.45)
    answer_from = len(session.events)
    await session.say("51_tulips")
    answered = await answer_received(session, answer_from)
    result = await session.wait(lambda e: e["type"] == "task_result", 60, answer_from)
    await session.quiet(30, answer_from)
    await session.close()
    flowers = stand_in.last(wire) or {}
    options = [str(option).lower() for option in asked.get("options") or []]
    checks["the backend's question is put to the caller"] = (
        asked.get("kind") == question_kind and "which flowers" in str(asked.get("prompt")).lower()
        and (question_kind != "choose" or options == ["roses", "tulips"]))
    checks["the caller's answer reaches the backend"] = (
        bool(answered) and answered.get("outcome") == "accept"
        and "tulip" in str(flowers.get("answer")).lower())
    checks["the task then completes with the backend's answer"] = (
        bool(result) and result[2].get("state") == "completed"
        and norm(result[2].get("summary", "")) == norm(flowers.get("result") or "-"))

    # A cancellation: the backend is told, and the caller hears what is known.
    session = await backend_session(ctx, f"{sid}-cancel", name)
    await session.warm()
    start = len(session.events)
    cancel_mark = len(stand_in.seen)
    await session.say("55_write_report")
    await session.wait(lambda e: e["type"] == "task_created", 20, start)
    told = await session.wait(
        lambda e: e["type"] == "task_progress" and e.get("text") == "Writing the report.", 20, start)
    await session.quiet(15, start)
    cancelled, after_e, said_after = await cancel_by_voice(session, start)
    await session.close()
    report_job = stand_in.last(wire) or {}
    stops = stand_in.of(wire, cancel_method, cancel_mark)
    checks["the backend's progress is passed on"] = told is not None
    checks["the backend is told to stop and stops"] = (
        len(stops) == 1 and report_job.get("state") == "cancelled")
    checks["task_cancelled within 3,000 ms of the end of the caller's turn, with what is known"] = (
        cancelled is not None and cancelled.get("effects") == cancel_effects
        and after_e is not None and after_e <= 3.0)
    checks["the caller hears the truth about it"] = cancel_line in said_after
    measured = (
        f"answer: task_created background {created[0][1].get('background') if created else None!r}, first "
        f"audio {first_ms} ms after E, the agent said {lines}, the backend wrote {job.get('result')!r}; "
        f"question: task_question {asked.get('kind')!r} {asked.get('prompt')!r} options {asked.get('options')}, "
        f"answer {answered}, the backend got {flowers.get('answer')!r} and wrote {flowers.get('result')!r}, "
        f"task_result {result[2].get('state') if result else None!r}; cancel: {len(stops)} {cancel_method} "
        f"request(s), the job is {report_job.get('state')!r}, task_cancelled "
        f"{ms(after_e) if after_e is not None else None} ms after E with effects "
        f"{cancelled.get('effects') if cancelled else None!r}, the agent said {said_after}")
    threshold = (
        f"task_created with background {label!r} and speculative false; first audio <= 2,000 ms after E; "
        "task_result completed with the backend's own text (it names Paris); the question as "
        f"task_question kind {question_kind!r}, the spoken answer accepted and received by the backend, the "
        f"task completed; one {cancel_method} request, the backend's job cancelled, task_cancelled <= 3,000 ms "
        f"after E of 'cancel that' with effects {cancel_effects!r} and the line {cancel_line!r}")
    return checks, measured, threshold


async def rw(ctx: Ctx):
    """A webhook backend on a raw session: an answer, a question to the caller, a cancellation."""
    checks, measured, threshold = await backend_calls(
        ctx, "RW", "hook", "hook", ["started the report"],
        "I've stopped it, but before it stopped it had already started the report.", "cancel", "choose")
    stand_in = ctx.stack.ensure_backends()
    job = stand_in.last("hook") or {}
    tasks = stand_in.of("hook", "task")
    hook_jobs = [hook_job for hook_job in stand_in.jobs.values() if hook_job["wire"] == "hook"]
    statuses = [status for hook_job in hook_jobs for _, status in hook_job["callbacks"]]
    # The gateway takes a report on a task only with the webhook's own signature.
    forged = [status for hook_job in hook_jobs for status in hook_job["forged"]]
    late = stand_in.call_back(job["callback_url"], {"type": "result", "text": "x"}) if job else None
    checks["the gateway's messages are signed and name the callback route"] = (
        bool(tasks) and all(hit["ok"] and hit["idempotency_key"] and
                            hit["body"].get("callback", "").startswith("/v1/tasks/task_") for hit in tasks))
    checks["signed callbacks are taken (202), forged ones refused (401), late ones unknown (404)"] = (
        bool(statuses) and set(statuses) == {202} and bool(forged) and set(forged) == {401} and late == 404)
    report(ctx, "RW", "webhook backend: answer, question, cancellation", checks,
           f"{measured}; callback statuses {statuses}, the same callbacks signed with another secret {forged}, "
           f"a signed callback after the task was over {late}",
           f"{threshold}; every callback of the webhook answered 202, each one signed with another secret "
           "while its task ran 401, and a signed one after the task was over 404")


async def ra(ctx: Ctx):
    """An A2A agent as the backend: an answer, a question to the caller, a cancellation."""
    checks, measured, threshold = await backend_calls(
        ctx, "RA", "agent", "a2a", "unknown", STOPPED["unknown"], "CancelTask", "value")
    report(ctx, "RA", "A2A backend: answer, question, cancellation", checks, measured, threshold,
           "the agent may change things and does not say what it did: a stopped task's effects are unknown")


async def rm(ctx: Ctx):
    """An MCP task tool as the backend: an answer, a question to the caller, a cancellation."""
    checks, measured, threshold = await backend_calls(
        ctx, "RM", "tool", "mcp", "none_committed", STOPPED["none_committed"], "tasks/cancel", "choose")
    report(ctx, "RM", "MCP Tasks backend: answer, question, cancellation", checks, measured, threshold,
           "the catalog entry declares side_effects none")


async def rr(ctx: Ctx):
    """An OpenAI Responses backend: a streamed answer, ask_caller over polled background responses, a cancellation."""
    stand_in = ctx.stack.ensure_backends()
    checks: Dict[str, bool] = {}

    # Streamed, from the llama-server's own /v1/responses.
    session = await backend_session(ctx, "RR-stream", "llama", system_prompt=PLAIN_PROMPT)
    await session.warm()
    turn = await session.turn("20_q_capital")
    created = session.all("task_created", turn.first_event)
    results = session.all("task_result", turn.first_event)
    first = session.first_audible(turn.e) if turn.e is not None else None
    relayed = [e["delta"] for _, e in session.transcripts(turn.first_event, "relay") if e.get("delta")]
    await session.close()
    streamed = results[0][1] if results else {}
    first_ms = ms(first - turn.e) if first is not None else None
    checks["the llama-server's own Responses stream is the answer"] = (
        len(created) == 1 and created[0][1].get("background") == "openai_responses:llama"
        and created[0][1].get("speculative") is False and streamed.get("state") == "completed"
        and "paris" in norm(streamed.get("summary", "")) and bool(relayed))
    checks["something is audible within 2,000 ms of the end of the turn"] = (
        first_ms is not None and first_ms <= 2000)

    # The model asks the caller through ask_caller; each round is a polled background response.
    # As in H1, a session in which the model asks in its own words shows nothing about the
    # gateway and is tried again (three sessions at most).
    for attempt in (1, 2, 3):
        mark = len(stand_in.seen)
        session = await backend_session(ctx, "RR-ask" if attempt == 1 else f"RR-ask-{attempt}", "store",
                                        ("check_availability",), ASK_PROMPT)
        await session.warm()
        start = len(session.events)
        await session.say("50_order_flowers")
        if await session.wait(lambda e: e["type"] == "task_question", 20, start):
            break
        await session.quiet(30, start)
        await session.close()
    asked, after = await question(session, start, 1.0)
    await heard(session, await spoken(session, after - 1, "question"))
    await asyncio.sleep(0.45)
    answer_from = len(session.events)
    await session.say("51_tulips")
    answered = await answer_received(session, answer_from)
    following = await session.wait(lambda e: e["type"] in ("task_question", "task_result"), 40, answer_from + 1)
    if following and following[2]["type"] == "task_question":
        # The prompt's second question: the name the order is for.
        await heard(session, await spoken(session, following[0], "question"))
        await asyncio.sleep(0.45)
        name_from = len(session.events)
        await session.say("52_name_jordan")
        following = await session.wait(lambda e: e["type"] == "task_result", 40, name_from)
    await session.quiet(30, answer_from)
    await session.close()
    creates = stand_in.of("responses", "create", mark)
    polls = stand_in.of("responses", "get", mark)
    outputs = [item for hit in creates for item in hit["body"].get("input") or []
               if item.get("type") == "function_call_output"]
    declared = [tool.get("name") for tool in (creates[0]["body"].get("tools") or [])] if creates else []
    final = following[2] if following else {}
    checks["each round is a background response that is polled"] = (
        len(creates) >= 2 and bool(polls) and all(hit["ok"] and hit["body"].get("background") is True
                                                  and hit["body"].get("stream") is False for hit in creates))
    checks["the model is given the session's tool and ask_caller"] = declared == ["check_availability", "ask_caller"]
    checks["the model's question is put to the caller and the answer goes back as the call's output"] = (
        asked.get("kind") == "choose" and bool(answered) and answered.get("outcome") == "accept"
        and any("tulip" in str(item.get("output")).lower() for item in outputs))
    checks["the task ends completed"] = final.get("type") == "task_result" and final.get("state") == "completed"

    # A cancellation stops the background response on the server.
    session = await backend_session(ctx, "RR-cancel", "store", system_prompt=PLAIN_PROMPT)
    await session.warm()
    start = len(session.events)
    cancel_mark = len(stand_in.seen)
    await session.say("55_write_report")
    await session.wait(lambda e: e["type"] == "task_created", 20, start)
    deadline = now() + 15
    while not stand_in.of("responses", "get", cancel_mark) and now() < deadline:
        await asyncio.sleep(0.1)
    await session.quiet(15, start)
    cancelled, after_e, said_after = await cancel_by_voice(session, start)
    await session.close()
    stops = stand_in.of("responses", "cancel", cancel_mark)
    held = [entry for entry in stand_in.responses.values() if "report" in entry["request"].lower()]
    checks["the response is cancelled on the server"] = (
        len(stops) == 1 and bool(held) and held[-1]["status"] == "cancelled")
    checks["task_cancelled within 3,000 ms of the end of the caller's turn"] = (
        cancelled is not None and cancelled.get("effects") == "none_committed"
        and after_e is not None and after_e <= 3.0 and STOPPED["none_committed"] in said_after)
    report(ctx, "RR", "OpenAI Responses backend: stream, ask_caller over polled responses, cancellation", checks,
           f"stream: background {created[0][1].get('background') if created else None!r}, first audio "
           f"{first_ms} ms after E, relayed {relayed}, task_result {streamed.get('state')!r} streamed "
           f"{streamed.get('streamed')!r}; ask: {len(creates)} create request(s) and {len(polls)} poll(s), "
           f"tools {declared}, task_question {asked.get('kind')!r} {asked.get('prompt')!r} options "
           f"{asked.get('options')}, answer {answered}, function_call_output items "
           f"{[item.get('output') for item in outputs]}, ended {final.get('type')!r} {final.get('state')!r} "
           f"{final.get('summary')!r}; cancel: {len(stops)} cancel request(s), the response is "
           f"{held[-1]['status'] if held else None!r}, task_cancelled "
           f"{ms(after_e) if after_e is not None else None} ms after E with effects "
           f"{cancelled.get('effects') if cancelled else None!r}, the agent said {said_after}",
           "stream: task_created background 'openai_responses:llama', first audio <= 2,000 ms after E, a "
           "completed task whose relayed answer names Paris; ask: at least two background create requests "
           "with stream false and at least one poll, the tools [check_availability, ask_caller], a choose "
           "question, the spoken answer accepted and sent back as a function_call_output that names tulips, "
           "the task completed; cancel: one cancel request, the server's response cancelled, task_cancelled "
           "<= 3,000 ms after E of 'cancel that' with effects none_committed and the line \"It's stopped.\"",
           f"the background model called ask_caller in session {attempt} of at most 3")


# ---- Release 2 experience: listening sounds, the client's playback, a form, a tenant persona

def long_story(ctx: Ctx) -> Tuple[bytes, List[Tuple[float, float]]]:
    """BC1's caller turn, and its voiced stretches (start, end) in seconds from the start of
    the audio: the pauses between them are silence of 300 ms or more (Smart Turn runs at
    200 ms of a pause)."""
    pcm = load_wav(ctx.caller("56_story"))
    samples = np.frombuffer(pcm, dtype="<i2")
    frame = CALLER_RATE // 50
    voiced = [frame_db(samples[i:i + frame].tobytes()) > -45 for i in range(0, len(samples) - frame, frame)]
    spans, start, quiet = [], None, 0
    for n, loud in enumerate(voiced + [False] * 20):
        if loud:
            if start is None:
                start = n
            quiet = 0
        elif start is not None:
            quiet += 1
            if quiet == 15:
                spans.append((start * 0.02, (n - quiet + 1) * 0.02))
                start, quiet = None, 0
    return pcm, spans


def backchannel_starts(session: Session, since: float, until: float = float("inf")) -> List[Tuple[float, dict]]:
    return [(at, e) for at, e in session.all("audio_segment")
            if e.get("lane") == "backchannel" and e.get("phase") == "start" and since <= at <= until]


async def bc1(ctx: Ctx):
    """A long caller turn gets a listening sound in a pause; a caller answering a confirmation gets none."""
    story, spans = long_story(ctx)
    session = await Session.open(ctx, "BC1", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    echo = (session.ready.get("interaction") or {}).get("duplex", {}).get("backchannel")
    start = len(session.events)
    turn = await session.turn("story", story)
    said = turn.said
    starts = backchannel_starts(session, said.start, turn.e or said.end + 3)
    pauses = [(said.start + end, said.start + spans[n + 1][0]) for n, (_, end) in enumerate(spans[:-1])]
    # A start inside a pause: after the phrase's last voiced moment and before the next phrase
    # has been heard for 400 ms (the gateway hears the caller 20 ms frames late, the lead is 150 ms).
    in_pause = [at for at, _ in starts if any(begin <= at <= until + 0.4 for begin, until in pauses)]
    loud = [db for at, _ in starts for _, db in session.levels(at, at + 0.6) if db > AUDIBLE_DB]
    after_e = backchannel_starts(session, (turn.e or said.end) + 0.05)
    words = [e.get("delta") for _, e in session.transcripts(start)]
    await session.close()

    booking = await Session.open(ctx, "BC1-confirm", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await booking.warm()
    ask_from = len(booking.events)
    await booking.say("10_book_table")
    asked, after = await question(booking, ask_from)
    await heard(booking, await spoken(booking, after - 1, "question"))
    answer = await booking.say("story", story)
    over_answer = backchannel_starts(booking, answer.start, answer.end + 1.5)
    asked_kind = asked.get("kind")
    await booking.close()
    checks = {
        "the session echoes backchannel auto": echo == "auto",
        "a listening sound starts in a pause of the long turn": bool(in_pause),
        "its audio reaches the client": bool(loud),
        "none after the end of the turn": not after_e,
        "none while the caller answers a confirmation": asked_kind == "confirm" and not over_answer,
    }
    report(ctx, "BC1", "listening sounds in a long turn, none over a confirmation", checks,
           f"long turn {said.end - said.start:.1f} s with pauses at "
           f"{[(round(b - said.start, 2), round(u - said.start, 2)) for b, u in pauses]} s: backchannel starts at "
           f"{[round(at - said.start, 2) for at, _ in starts]} s ({len(loud)} audible 20 ms frames), "
           f"E at {round((turn.e or 0) - said.start, 2)} s, starts after E {len(after_e)}; agent lines {words}; "
           f"answering a {asked_kind} question with the same turn: {len(over_answer)} starts",
           "at least one `audio_segment` with lane backchannel starts inside a pause (from the phrase's "
           "last voiced moment to 400 ms into the next phrase) and is louder than -50 dBFS; none starts "
           "after E; none starts while the caller answers an open confirmation (to 1.5 s after the "
           "answer's audio)")
    return session


class Player:
    """BC1's client playback, as `playback_position` reports it: each utterance of the front lane
    plays as its audio arrives, unless the player is stuck in one (`stop_at`), which it then
    keeps reporting, four times a second."""

    def __init__(self, session: Session):
        self.session = session
        self.stuck: Optional[Tuple[str, int]] = None
        self.reports: List[Tuple[float, str, int]] = []
        self._task = asyncio.create_task(self._run())

    def received_ms(self, utterance_id: str) -> int:
        """The audio of the utterance that has arrived: from its segment start to its end."""
        segments = [(at, e) for at, e in self.session.all("audio_segment")
                    if e.get("utterance_id") == utterance_id]
        if not segments:
            return 0
        start = segments[0][0]
        ends = [at for at, e in segments if e.get("phase") == "end"]
        end = ends[0] if ends else now()
        return int(sum(len(data) for at, data in self.session.audio if start - 0.05 <= at <= end)
                   / 2 / AGENT_RATE * 1000)

    def current(self) -> Optional[str]:
        starts = [e["utterance_id"] for _, e in self.session.all("audio_segment")
                  if e.get("phase") == "start" and e.get("lane") in ("front", "main")]
        return starts[-1] if starts else None

    async def _run(self):
        try:
            while True:
                await asyncio.sleep(0.25)
                if self.stuck is not None:
                    utterance, played = self.stuck
                else:
                    utterance = self.current()
                    if utterance is None:
                        continue
                    played = self.received_ms(utterance)
                self.reports.append((now(), utterance, played))
                await self.session.send({"type": "playback_position", "utterance_id": utterance,
                                         "played_ms": played})
        except Exception:
            pass

    def stop(self):
        self._task.cancel()


PB1_FIRST = "Your flight leaves at seven from gate twelve, and boarding starts at half past six."
PB1_SECOND = "Your seat is fourteen C, by the window."


async def pb1_run(ctx: Ctx, label: str, report_playback: bool) -> dict:
    session = await Session.open(ctx, label, MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    player = Player(session) if report_playback else None
    start = len(session.events)
    await session.send({"type": "speak", "text": PB1_FIRST})
    await session.send({"type": "speak", "text": PB1_SECOND})
    first = await said(session, start, PB1_FIRST, 15)
    first_id = first[2]["utterance_id"] if first else None
    if player and first_id:
        # The client plays one second of the first line and stops there.
        await asyncio.sleep(1.0)
        player.stuck = (first_id, 1000)
    second = await said(session, start, PB1_SECOND, 20)
    await session.quiet(15, start, hold=1.5)
    again_from = len(session.events)
    turn = await session.turn("45_say_again", settle=0)
    repeat = await session.wait(lambda e: e["type"] == "assistant_transcript" and e.get("delta")
                                and e.get("kind") == "answer", 15, again_from)
    if player:
        player.stuck = None
    await asyncio.sleep(1.0)
    await session.quiet(15, again_from, hold=1.5)
    await asyncio.sleep(1.0)
    if player:
        player.stop()

    def heard_of(found) -> Optional[str]:
        if not found:
            return None
        lines = [e for _, e in session.all("assistant_transcript")
                 if e.get("utterance_id") == found[2]["utterance_id"]]
        return lines[-1].get("heard")

    result = {
        "repeat": repeat[2].get("delta") if repeat else None,
        "repeat_after_e_ms": ms(repeat[1] - turn.e) if repeat and turn.e else None,
        "first": heard_of(first), "second": heard_of(second), "repeated": heard_of(repeat),
        "heard": turn.text, "reports": len(player.reports) if player else 0,
    }
    await session.close()
    return result


async def pb1(ctx: Ctx):
    """A client that reports a partial playback makes "say that again" repeat the line it stopped in."""
    stuck = await pb1_run(ctx, "PB1", True)
    control = await pb1_run(ctx, "PB1-estimate", False)
    checks = {
        "the repeat is the line the client stopped in": stuck["repeat"] == PB1_FIRST,
        "that line counts as heard in part, the next one not": stuck["first"] == "partial"
        and stuck["second"] in ("none", None),
        "the repeat is heard in full": stuck["repeated"] == "full",
        "the repeat starts within 3 s of E": stuck["repeat_after_e_ms"] is not None
        and stuck["repeat_after_e_ms"] <= 3000,
        "without reports the pump's estimate stands (the last line)": control["repeat"] == PB1_SECOND
        and control["first"] == "full" and control["second"] == "full",
    }
    report(ctx, "PB1", "playback_position decides what was heard", checks,
           f"reporting client ({stuck['reports']} reports, stopped 1,000 ms into the first line): heard "
           f"{stuck['heard']!r} -> repeat {stuck['repeat']!r} {stuck['repeat_after_e_ms']} ms after E; "
           f"heard first {stuck['first']}, second {stuck['second']}, repeat {stuck['repeated']}. "
           f"Without reports: repeat {control['repeat']!r}, heard first {control['first']}, second "
           f"{control['second']}",
           "with reports: the repeat is the first line, word for word, starting within 3,000 ms of E; the "
           "first line's last transcript says heard partial, the second never full, the repeat full; "
           "without reports: the repeat is the second line and both lines are heard full")


FORM_FIELDS = {"street": "Harbour Road", "city": "Leith"}


async def form_on_screen(ctx: Ctx, label: str, backend: str,
                         got: Callable[[int], Optional[Tuple[float, Any]]]) -> Tuple[Dict[str, bool], str]:
    """A backend asks for a two-field form; the client fills it in. `got(mark)` finds the
    fields at the backend: when they arrived and what they were."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    session = await backend_session(ctx, label, backend, system_prompt=None)
    await session.warm()
    start = len(session.events)
    await session.send({"type": "agent_input", "text": "Please send the parcel to my new address."})
    asked, after = await question(session, start, 30)
    line = await said(session, start, "I'm putting a secure form on your screen for the details.", 15)
    answered_at = now()
    await session.send({"type": "task_answer", "request_id": asked.get("request_id"),
                        "outcome": "accept", "value": FORM_FIELDS})
    arrived = None
    while now() - answered_at < 5 and arrived is None:
        arrived = got(mark)
        await asyncio.sleep(0.02)
    result = await session.wait(lambda e: e["type"] == "task_result", 40, after)
    await session.quiet(30, start)
    received = await answer_received(session, after, 0)
    relayed = [e.get("delta") for _, e in session.transcripts(after, kind="relay")]
    await session.close()
    checks = {
        "the question is a form for the screen": asked.get("kind") == "form"
        and asked.get("channels") == ["client"]
        and set((asked.get("schema") or {}).get("properties", {})) == {"street", "city"},
        "the caller is told it is on the screen": bool(line),
        "the client's answer reaches the backend within 2 s": arrived is not None
        and arrived[1] == FORM_FIELDS and arrived[0] - answered_at <= 2.0,
        "the answer is acknowledged as the client's": bool(received) and received.get("by") == "client"
        and received.get("outcome") == "accept",
        "the result names the street": bool(result) and result[2].get("state") == "completed"
        and "harbour" in " ".join(relayed).lower(),
    }
    measured = (f"task_question kind {asked.get('kind')!r} channels {asked.get('channels')} schema fields "
                f"{sorted((asked.get('schema') or {}).get('properties', {}))}; spoken {line[2]['delta'] if line else None!r}; "
                f"the backend got {arrived[1] if arrived else None} "
                f"{round((arrived[0] - answered_at) * 1000) if arrived else None} ms after task_answer; "
                f"task_answer_received {received}; relayed {relayed}")
    return checks, measured


FORM_THRESHOLD = ("kind form, channels [client], the backend's two fields; the screen line spoken; the "
                  "client's answer at the backend within 2,000 ms, as sent; answer received by client; the "
                  "task completes and its relayed answer names Harbour Road")


async def f1(ctx: Ctx):
    """A form question of a webhook backend is answered on the client's screen."""
    stand_in = ctx.stack.ensure_backends()

    def got(mark: int):
        answers = stand_in.of("hook", "answer", mark)
        return (answers[0]["t"], answers[0]["body"].get("value")) if answers else None

    checks, measured = await form_on_screen(ctx, "F1", "hook", got)
    report(ctx, "F1", "a form answered from the client's screen", checks, measured, FORM_THRESHOLD)


async def mf1(ctx: Ctx):
    """An MCP elicitation of several fields is a form answered on the client's screen."""
    stand_in = ctx.stack.ensure_backends()

    def got(mark: int):
        updates = [hit for hit in stand_in.of("mcp", "tasks/update", mark)
                   if "parcel" in (hit["body"].get("params") or {}).get("inputResponses", {})]
        if not updates:
            return None
        given = updates[0]["body"]["params"]["inputResponses"]["parcel"]
        return updates[0]["t"], given.get("content") if given.get("action") == "accept" else None

    checks, measured = await form_on_screen(ctx, "MF1", "tool", got)
    report(ctx, "MF1", "an MCP form of several fields answered from the client's screen", checks,
           measured, FORM_THRESHOLD.replace("the backend's two fields", "the MCP schema's two fields")
           + "; the MCP server gets action accept with the fields as content")


async def au1(ctx: Ctx):
    """An A2A agent's sign-in is a link on the client's screen; the agent goes on after it."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    session = await backend_session(ctx, "AU1", "agent", system_prompt=None)
    await session.warm()
    start = len(session.events)
    await session.send({"type": "agent_input", "text": "What does my latest statement say?"})
    asked, after = await question(session, start, 30)
    before = await said(session, start, AUTH_BEFORE, 15)
    sent = await said(session, start, "I've sent it.", 15)
    # The caller signs in, then the client says so.
    await asyncio.sleep(1.0)
    answered_at = now()
    await session.send({"type": "task_answer", "request_id": asked.get("request_id"),
                        "outcome": "accept", "value": True})
    resumed = None
    while now() - answered_at < 5 and resumed is None:
        told = [hit for hit in stand_in.of("a2a", "SendMessage", mark)
                if ((hit["body"].get("params") or {}).get("message") or {}).get("taskId")]
        resumed = told[0] if told else None
        await asyncio.sleep(0.02)
    result = await session.wait(lambda e: e["type"] == "task_result", 60, after)
    await session.quiet(30, start)
    received = await answer_received(session, after, 0)
    relayed = [e.get("delta") for _, e in session.transcripts(after, kind="relay")]
    await session.close()
    resumed_text = " ".join(part.get("text", "") for part in
                            resumed["body"]["params"]["message"].get("parts", [])) if resumed else None
    checks = {
        "the question is a sign-in link for the screen": asked.get("kind") == "auth_url"
        and asked.get("url") == SIGN_IN_LINK and asked.get("channels") == ["client"]
        and asked.get("sensitive") is True,
        "the caller is told a link is coming, then that it was sent": bool(before) and bool(sent)
        and before[0] < sent[0],
        "the agent is told within 2 s that the caller signed in": resumed is not None
        and resumed["t"] - answered_at <= 2.0 and resumed_text == "Signed in.",
        "the answer is the client's and redacted": bool(received) and received.get("by") == "client"
        and received.get("outcome") == "accept" and received.get("value") is None
        and received.get("value_redacted") is True,
        "the task completes with an answer": bool(result) and result[2].get("state") == "completed"
        and bool(" ".join(relayed).strip()),
    }
    report(ctx, "AU1", "an A2A sign-in link answered from the client's screen", checks,
           f"task_question kind {asked.get('kind')!r} url {asked.get('url')!r} channels {asked.get('channels')} "
           f"sensitive {asked.get('sensitive')}; spoken {before[2]['delta'] if before else None!r} then "
           f"{sent[2]['delta'] if sent else None!r} "
           f"({round((sent[1] - before[1]) * 1000) if before and sent else None} ms apart); the agent got "
           f"{resumed_text!r} {round((resumed['t'] - answered_at) * 1000) if resumed else None} ms after "
           f"task_answer; task_answer_received {received}; result "
           f"{result[2].get('state') if result else None!r}; relayed {relayed}",
           "kind auth_url with the agent's link, channels [client], sensitive; the before line, then "
           "\"I've sent it.\" after it; the agent's task gets the message \"Signed in.\" within 2,000 ms of "
           "task_answer; task_answer_received by client, accept, value null, redacted; the task completes "
           "with a relayed answer")
    return session


AUTH_BEFORE = "To do that I need you to sign in. I'm sending a secure link to your screen."


async def af1(ctx: Ctx):
    """The background model asks for a form of several fields with ask_caller; the client fills it in."""
    # The background model has to use the tool's form: a session in which it does not shows
    # nothing about the gateway, and is tried again (three sessions at most).
    attempts = []
    for attempt in (1, 2, 3):
        session = await Session.open(ctx, "AF1" if attempt == 1 else f"AF1-{attempt}",
                                     with_tools(ctx, "check_availability"), FORM_PROMPT)
        await session.warm()
        start = len(session.events)
        await session.send({"type": "agent_input", "text": "Please send my parcel to my new address."})
        found = await session.wait(lambda e: e["type"] == "task_question", 30, start)
        if found and found[2].get("kind") == "form":
            break
        await session.quiet(30, start)
        await session.close()
        said_lines = [e.get("delta") for _, e in session.transcripts(start) if e.get("delta")]
        attempts.append(f"session {attempt}: question {found[2].get('kind') if found else None!r}, said {said_lines}")
    else:
        report(ctx, "AF1", "ask_caller: a form of several fields answered from the client's screen",
               {"the background model asks a form in one of three sessions": False}, "; ".join(attempts),
               "a task_question of kind form in one of three sessions")
        return None
    asked, after = found[2], found[0] + 1
    line = await said(session, start, "I'm putting a secure form on your screen for the details.", 15)
    answered_at = now()
    await session.send({"type": "task_answer", "request_id": asked.get("request_id"),
                        "outcome": "accept", "value": FORM_FIELDS})
    result = await session.wait(lambda e: e["type"] == "task_result", 60, after)
    await session.quiet(30, start)
    received = await answer_received(session, after, 0)
    relayed = [e.get("delta") for _, e in session.transcripts(after, kind="relay")]
    await session.close()
    fields = sorted(((asked.get("schema") or {}).get("properties") or {}))
    checks = {
        "the model's form is a question for the screen": asked.get("kind") == "form"
        and asked.get("channels") == ["client"] and len(fields) >= 2,
        "the caller is told it is on the screen": bool(line),
        "the answer is the client's": bool(received) and received.get("by") == "client"
        and received.get("outcome") == "accept",
        "the result names the street": bool(result) and result[2].get("state") == "completed"
        and "harbour" in " ".join(relayed).lower(),
    }
    report(ctx, "AF1", "ask_caller: a form of several fields answered from the client's screen", checks,
           f"task_question kind {asked.get('kind')!r} channels {asked.get('channels')} fields {fields}; spoken "
           f"{line[2]['delta'] if line else None!r}; task_answer_received {received}; result "
           f"{result[2].get('state') if result else None!r} {round((result[1] - answered_at) * 1000) if result else None} "
           f"ms after task_answer; relayed {relayed}",
           "kind form with at least two fields, channels [client]; the screen line spoken; answered by the "
           "client; the task completes and its relayed answer names Harbour Road",
           f"the background model asked a form in session {attempt} of at most 3; before: {attempts}")
    return session


FORM_PROMPT = """You are the parcel desk of a delivery company. You never ask the caller anything in your own words.
- When the caller wants a parcel sent to a new address, your reply is always exactly this tool call: ask_caller with {"kind": "form", "question": "Where should the parcel go?", "fields": ["street", "city"]}. The caller fills the form in on their screen.
- After the tool result, tell the caller in one short sentence that the parcel goes to that street, and name the street."""


PS1_PERSONA = {
    "name": "Lumi", "org": "Lumen Air", "role": "booking assistant", "tone": "warm",
    "banks": {"ack": [{"text": "Lumi here, checking."}, {"text": "Lumi is on it."}]},
}


def put_status(url: str, body: Any, method: str = "PUT") -> Tuple[int, dict]:
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url, data=data, method=method,
                                     headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        return error.code, json.loads(error.read() or b"{}")


# The key the main gateway checks caller identity tokens with (MEM1).
MEMORY_KEY = "live-e2e-caller-memory-key"


def mint_identity(sub: str, tenant: str = "", ttl_s: int = 240) -> str:
    """A caller identity token as the operator mints it: `v1.<payload>.<mac>`. With
    authentication off the tenant is the empty id."""
    def b64(raw: bytes) -> str:
        return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()
    claims = {"sub": sub, "tenant": tenant, "exp": int(time.time()) + ttl_s}
    payload = b64(json.dumps(claims).encode())
    mac = hmac.new(MEMORY_KEY.encode(), f"v1.{payload}".encode(), hashlib.sha256).digest()
    return f"v1.{payload}.{b64(mac)}"


MEM1_KNOWN = "Travels with a small dog"
MEM1_LIMIT_S = 45.0


async def mem1(ctx: Ctx):
    """After a call with a known caller the background model adds what it learnt to the caller's memory."""
    base = f"http://127.0.0.1:{ctx.args.gateway_port}/v1/interaction/memory"
    sub = f"live-caller-{ctx.run_id}"
    put, _ = put_status(f"{base}/{sub}", {"facts": [MEM1_KNOWN]})
    block = {"front": {**MODEL_FRONT["front"], "caller": {"identity_token": mint_identity(sub)}}}
    session = await Session.open(ctx, "MEM1", block, PLAIN_PROMPT)
    await session.warm()
    refused = [w.get("code") for w in session.warnings
               if w.get("code") in ("caller_identity_rejected", "caller_memory_unavailable")]
    start = await typed(session, "I always want an aisle seat when I fly. Can you book my seats that way?")
    answered = [e.get("delta") for _, e in session.transcripts(start) if e.get("delta")]
    await session.close()
    closed_at = now()
    facts: List[str] = []
    written_after = None
    while now() - closed_at < MEM1_LIMIT_S:
        status, body = get_status(f"{base}/{sub}")
        facts = body.get("facts") or []
        if len(facts) > 1:
            written_after = now() - closed_at
            break
        await asyncio.sleep(0.5)
    added = [fact for fact in facts if fact != MEM1_KNOWN]
    keepable = all(0 < len(fact) <= 80 and not re.search(r"[0-9@$£€¥#]|://|www\.", fact) for fact in facts)
    # A fact with a colon is a label or a copy of the prompt ("Known facts: ..."): the first run
    # stored one, so it is checked from then on.
    copied = [fact for fact in added if ":" in fact or fact.lower() == MEM1_KNOWN.lower()]
    after, _ = put_status(f"{base}/{sub}", {"facts": []})
    gone, body = get_status(f"{base}/{sub}")
    checks = {
        "the operator's fact is kept": put == 204,
        "the caller's token is taken": not refused,
        "the background adds a fact within the limit, newest first": bool(added) and facts[0] in added
        and written_after is not None,
        "it is what the caller said about the aisle": any("aisle" in fact.lower() for fact in added),
        "the operator's fact stays, and every fact may be kept": MEM1_KNOWN in facts and keepable
        and len(facts) <= 10,
        "no added fact is a label or a copy of a known fact": not copied,
        "an empty list forgets the caller": after == 204 and gone == 200 and body.get("facts") == [],
    }
    report(ctx, "MEM1", "caller memory written by the background after the call", checks,
           f"PUT {put}; warnings {refused}; the agent said {answered}; facts {facts} "
           f"{round(written_after * 1000) if written_after is not None else None} ms after the close; "
           f"forget: PUT {after}, GET {gone} {body}",
           f"the PUT is 204; no identity warning; within {MEM1_LIMIT_S:.0f} s of the close GET lists a new fact "
           "first that names the aisle, and the operator's fact; no fact with a digit, @, currency sign or "
           "link, none over 80 characters, at most 10; no added fact with a colon or equal to the known one "
           "(added after the first run); an empty PUT forgets the caller")
    return session


# The line the gateway logs each time it loads the Silero voice activity model.
SILERO_LOADED = "Initializing Silero VAD with config"


async def silero_loads(ctx: Ctx, session: "Session", settle: float = 5.0) -> int:
    """The Silero loads the gateway logged from the session's start to `settle` s after ready."""
    await asyncio.sleep(settle)
    return len(ctx.stack.log_since(SILERO_LOADED, session.log_mark))


async def td1(ctx: Ctx):
    """With the speech-to-text's turn detection on, the turn models are loaded once a session."""
    mode = await Session.open(ctx, "TD1-mode", MODEL_FRONT, turn_detection=True)
    mode_loads = await silero_loads(ctx, mode)
    start = len(mode.events)
    await mode.say("20_q_capital")
    created = await mode.wait(lambda e: e["type"] == "task_created", 30, start)
    result = await mode.wait(lambda e: e["type"] == "task_result", 60, start)
    await mode.close()
    plain = await Session.open(ctx, "TD1-plain", None, turn_detection=True, mic=False)
    plain_loads = await silero_loads(ctx, plain)
    plain_mode = plain.ready.get("protocol_version")
    await plain.close()
    # A session that asks for the mode and is refused gets the speech-to-text's own models.
    refused = await Session.open(ctx, "TD1-refused", MODEL_FRONT, conversation=False, mic=False,
                                 turn_detection=True,
                                 extra={"dag_config": {"definition": dag_definition(ctx, True)}})
    refused_loads = await silero_loads(ctx, refused)
    codes = [w.get("code") for w in refused.warnings]
    refused_mode = refused.ready.get("protocol_version")
    await refused.close()
    checks = {
        "the mode's session runs the mode": mode.ready.get("protocol_version") == "1.1",
        "it loads Silero once (its analyser's), not twice": mode_loads == 1,
        "its turns still end: a task is created and answered": bool(created) and bool(result)
        and result[2].get("state") == "completed",
        "a session without the mode loads it once (the speech-to-text's)": plain_mode == "1.0"
        and plain_loads == 1,
        "a refused session loads it once (the speech-to-text's)": refused_mode == "1.0"
        and "interaction_dag_audio_output" in codes and refused_loads == 1,
    }
    report(ctx, "TD1", "turn models loaded once with turn_detection on", checks,
           f"Silero loads logged: interaction session {mode_loads} (task "
           f"{created[2].get('task_ref') if created else None}, result "
           f"{result[2].get('state') if result else None}); plain session {plain_loads} (protocol "
           f"{plain_mode}); refused session {refused_loads} (protocol {refused_mode}, warnings {codes})",
           "with stt_config.turn_detection.enabled, exactly one 'Initializing Silero VAD' log line from a "
           "session's start to 5 s after its ready: for an interaction session (its analyser; its turn "
           "still ends in a completed task), a session without interaction and a session whose "
           "interaction is refused (a DAG that speaks itself)",
           "before this, the interaction session loaded the voice manager's smart-turn processor too "
           "(two lines); the count reads the main gateway's log")


async def ps1(ctx: Ctx):
    """A persona the tenant registered is checked like a packaged one and speaks for the session that selects it."""
    base = f"http://127.0.0.1:{ctx.args.gateway_port}/v1/interaction/personas"
    persona_id = f"live-lumi-{ctx.run_id}"
    put, put_body = put_status(f"{base}/{persona_id}", PS1_PERSONA)
    liar = {**PS1_PERSONA, "banks": {"ack": [{"text": "Your booking is confirmed."}]}}
    refused, refused_body = put_status(f"{base}/{persona_id}-liar", liar)
    # The acknowledgement is due when the turn settles: the local model's answer comes before
    # E + 300 ms often enough that the default skips it (the first run of PS1 heard none).
    block = {"front": {**MODEL_FRONT["front"], "persona": persona_id}, "policy": {"ack_due_ms": 0}}
    session = await Session.open(ctx, "PS1", block, PLAIN_PROMPT)
    await session.warm()
    echoed = (session.ready.get("interaction") or {}).get("front", {}).get("persona")
    start = len(session.events)
    turn = await session.turn("21_q_week")
    acks = [e.get("delta") for _, e in session.transcripts(start, kind="ack")]
    persona_acks = {line["text"] for line in PS1_PERSONA["banks"]["ack"]}
    await session.close()
    read, read_body = get_status(f"{base}/{persona_id}")
    removed, _ = put_status(f"{base}/{persona_id}", None, "DELETE")
    gone, _ = get_status(f"{base}/{persona_id}")
    again, _ = put_status(f"{base}/{persona_id}", None, "DELETE")
    checks = {
        "the persona is registered": put == 200 and put_body.get("version") == 1,
        "a line that claims what has not happened is refused": refused == 422
        and "completion_claim" in refused_body.get("error", ""),
        "the session runs the persona": echoed == persona_id,
        "its acknowledgement is the persona's": bool(acks) and all(ack in persona_acks for ack in acks),
        "the tenant reads it back": read == 200 and read_body.get("version") == 1
        and read_body.get("versions") == [1] and (read_body.get("persona") or {}).get("name") == "Lumi",
        "the tenant removes it": removed == 204 and gone == 404 and again == 404,
    }
    report(ctx, "PS1", "a tenant persona selected by a session", checks,
           f"PUT {put} {put_body}; the persona with a false claim: {refused} {refused_body}; "
           f"ready.interaction.front.persona {echoed!r}; heard {turn.text!r} -> acknowledgements {acks}; "
           f"GET {read} version {read_body.get('version')} versions {read_body.get('versions')}; DELETE "
           f"{removed}, then GET {gone}, DELETE {again}",
           "200 with version 1; 422 naming completion_claim; the session echoes the persona's id; every "
           "acknowledgement of the turn is one of the persona's two lines; GET 200 with version 1, versions "
           "[1] and the persona; DELETE 204, then GET 404 and DELETE 404")
    return session


class Native(Session):
    """A client of the native `/realtime` endpoint: a JSON `config`, the caller's audio as
    binary 24 kHz frames, the turn ended by `commit_audio` (the vendor here transcribes per
    upload), the provider's audio back as binary frames and its words as `transcript`
    events. Interaction events arrive with their `/ws` bodies."""

    @classmethod
    async def open_native(cls, ctx: "Ctx", label: str, port: int, config: dict) -> "Native":
        self = cls(ctx, label)
        self.log_mark = ctx.stack.log_size()
        self.ws = await websockets.connect(f"ws://127.0.0.1:{port}/realtime", max_size=None,
                                           ping_interval=None)
        self.opened = now()
        await self.ws.send(json.dumps({"type": "config", **config}))
        self._tasks.append(asyncio.create_task(self._read()))
        found = await self.wait(lambda e: e["type"] in ("session_created", "error"), 60)
        if found is None or found[2]["type"] != "session_created":
            await self.close()
            raise RuntimeError(f"{label}: no session_created: {found[2] if found else 'timeout'}")
        self.ready = {"stream_id": found[2]["session_id"], **found[2]}
        self.warnings = [e for _, e in self.events if e["type"] == "warning"]
        ctx.live.append(self)
        self._tasks.append(asyncio.create_task(self._mic()))
        return self

    async def _send_audio(self, frame: bytes):
        await self.ws.send(resample(frame, CALLER_RATE, AGENT_RATE))

    async def turn_native(self, clip: str) -> float:
        """The caller says a clip and ends the turn; returns when the commit went out."""
        await self.say(clip)
        await asyncio.sleep(0.3)
        await self.send({"type": "commit_audio"})
        return now()

    def said(self, start: int = 0) -> List[Tuple[float, str]]:
        """The provider's final words, from event `start` on."""
        return [(at, e["text"]) for at, e in self.events[start:]
                if e["type"] == "transcript" and e.get("role") == "assistant" and e.get("is_final")]


VENDOR_FRONT = ("You are the phone assistant of the Olive Garden restaurant. Answer in one short "
                "plain sentence.")


def regime_config(ctx: Ctx, irreversible: bool, extra: Optional[dict] = None) -> dict:
    """A `/realtime` config on the OpenAI provider (the GA cascade here) with interaction mode:
    the background model and the booking tool as the cascade sessions use them."""
    tool = {**ctx.webhook.tool("book_table"), "irreversible": irreversible}
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{ctx.args.background_port}/v1",
                               "model": ctx.args.background_alias, "api_key": "local-dummy",
                               "system_prompt": TOOLS_PROMPT},
                       "tools": [tool]}}
    block.update(extra or {})
    return {"provider": "openai", "instructions": VENDOR_FRONT,
            "turn_detection": {"mode": "manual"}, "interaction": block}


def exact_word_checks(ctx: Ctx, since: int) -> List[dict]:
    """The regime gateway's `interaction exact-words check` lines from log offset `since`:
    what the gateway required and what the provider said."""
    log = ctx.stack.log_dir / "live-regime-gateway.log"
    out = []
    with open(log, "rb") as handle:
        handle.seek(since)
        for raw in handle.read().decode("utf-8", "replace").splitlines():
            if "interaction exact-words check" not in raw:
                continue
            found = re.search(r'required=(.*?) said=(.*?) passed=(true|false)', raw)
            if found:
                out.append({"required": found[1], "said": found[2], "passed": found[3] == "true"})
    return out


def script_verdict(required: str, said: str) -> bool:
    """The script's own reading of a guided line: the same numbers, and every content word of
    the read-back (after "To confirm:", before the question) heard."""
    def numbers(text: str) -> List[str]:
        words = {"one": "1", "two": "2", "three": "3", "four": "4", "five": "5", "six": "6",
                 "seven": "7", "eight": "8", "nine": "9", "ten": "10"}
        found = re.findall(r"\d+|[a-z]+", text.lower())
        return sorted({words.get(w, w) for w in found if w.isdigit() or w in words})
    readback = required.split(":", 1)[-1].split("Do you want me")[0]
    content = [w for w in re.findall(r"[a-z]+", readback.lower()) if len(w) > 3]
    heard = said.lower()
    return numbers(readback) == numbers(said) and all(w[:5] in heard for w in content)


def regime_log_size(ctx: Ctx) -> int:
    log = ctx.stack.log_dir / "live-regime-gateway.log"
    return log.stat().st_size if log.exists() else 0


async def rg1(ctx: Ctx):
    """Regime B on /realtime: the OpenAI provider (the GA cascade) delegates a booking; the gateway's confirmation is said and checked; a spoken yes binds."""
    port = ctx.stack.ensure_regime()
    mark, log_mark = len(ctx.webhook.calls), regime_log_size(ctx)
    session = await Native.open_native(ctx, "RG1", port, regime_config(ctx, irreversible=False))
    checks: Dict[str, bool] = {}
    echo = session.ready.get("interaction") or {}
    checks["the session runs regime B with the delegate function"] = (
        echo.get("regime") == "provider_speech" and echo.get("speech", {}).get("delegate_tool") == "waav_delegate")
    committed = await session.turn_native("10_book_table")
    created = await session.wait(lambda e: e["type"] == "task_created", 45, after=committed)
    checks["the provider's call becomes a task (origin provider_tool) within 45 s"] = (
        created is not None and created[2].get("origin") == "provider_tool")
    asked = await session.wait(lambda e: e["type"] == "task_question", 60, start=created[0] if created else 0)
    checks["the confirmation is asked by voice (task_question spoken) within 60 s of the task"] = (
        asked is not None and asked[2].get("spoken") is True)
    prompt = asked[2].get("prompt", "") if asked else ""
    # The provider says the line: wait for its words and their audio.
    # The question as the provider says it (it may be said again after a failed check).
    await session.wait(
        lambda e: e["type"] == "transcript" and e.get("role") == "assistant" and e.get("is_final")
        and "want me" in e.get("text", "").lower(), 40, start=asked[0] if asked else 0)
    # Its audio comes faster than real time: the caller answers once it would have played.
    await asyncio.sleep(3.0)
    lines = exact_word_checks(ctx, log_mark)
    question_line = next((l for l in lines if "Do you want me" in l["required"]), None)
    agree = question_line is not None and question_line["passed"] == script_verdict(
        question_line["required"], question_line["said"])
    checks["the gateway's exact-words verdict on the confirmation agrees with the script's reading"] = agree
    passed = bool(question_line and question_line["passed"])
    voice_start = len(session.events)
    yes_at = await session.turn_native("12_yes")
    # The stand-in vendor reports a caller's turn only when its agent answers it, and it does
    # not while the delegate call is open: the yes is given 8 s, then the client answers.
    by_voice = await session.wait(lambda e: e["type"] == "task_answer_received", 8, start=voice_start)
    heard_yes = [e.get("text") for _, e in session.all("transcript", voice_start) if e.get("role") == "user"]
    if by_voice is None:
        await session.send({"type": "task_answer", "request_id": asked[2]["request_id"], "outcome": "accept"})
        by_answer = await session.wait(lambda e: e["type"] == "task_answer_received", 10, start=voice_start)
    else:
        by_answer = by_voice
    if passed:
        checks["the confirmation is answered (by voice when the vendor reports the turn, else by the client)"] = (
            by_answer is not None and by_answer[2].get("outcome") == "accept"
            and (by_answer[2].get("by") == "voice" or not heard_yes))
    else:
        checks["a spoken yes to a confirmation the provider misspoke does not bind"] = (
            by_voice is None or by_voice[2].get("by") != "voice")
    result = await session.wait(lambda e: e["type"] == "task_result", 90, start=voice_start)
    calls = ctx.webhook.since(mark)
    effects = result[2].get("effects") if result else None
    checks["the booking is made once and the result is the gateway's line (spoken full)"] = (
        result is not None and isinstance(effects, list) and len([c for c in calls if c["tool"] == "book_table"]) == 1
        and result[2].get("spoken") == "full")
    await asyncio.sleep(6.0)
    lines = exact_word_checks(ctx, log_mark)
    claims = [e for _, e in session.all("unverified_claim")]
    leaked = [e for _, e in session.all("function_call") if e.get("name") == "waav_delegate"]
    checks["no call of the delegate function reaches the client"] = not leaked
    said = session.said()
    ctx.record(session)
    await session.close()
    report(ctx, "RG1", "regime B on /realtime over the local GA cascade", checks,
           f"regime {echo.get('regime')}; task {'+%d ms' % ms(created[1] - committed) if created else None} after the commit; "
           f"question {'+%d ms' % ms(asked[1] - created[1]) if asked and created else None} after the task, prompt {prompt!r}; "
           f"gateway lines (required -> said, verdict): {[(l['required'], l['said'], l['passed']) for l in lines]}; "
           f"caller's yes as the vendor reported it {heard_yes or 'not reported'}; answer "
           f"{by_answer[2].get('outcome') + ' by ' + by_answer[2].get('by') if by_answer else 'none'}"
           f"{' +%d ms after the yes' % ms(by_answer[1] - yes_at) if by_answer else ''}; "
           f"result {result[2].get('state') if result else None} effects {effects} spoken {result[2].get('spoken') if result else None}; "
           f"tool calls {[c['tool'] for c in calls]}; provider's own words {[t for _, t in said]}; "
           f"unverified_claim {[(c['kind'], c['rule'], c['text']) for c in claims]}",
           "regime provider_speech and the delegate function in the echo; task_created with origin "
           "provider_tool within 45 s of the commit; task_question spoken within 60 s; the gateway's "
           "exact-words verdict on the confirmation equals the script's (same numbers, every read-back "
           "content word heard); if it passed, the caller's spoken yes gives task_answer_received accept "
           "by voice within 8 s when the vendor reports the turn (else the client's task_answer accepts "
           "and the turn's absence is reported); if it failed, no voice answer binds; book_table called "
           "once, task_result with a list of effects and spoken full; no waav_delegate function_call on "
           "the client socket",
           "no real vendor was reached: the provider is the gateway's own GA cascade over the local "
           "Whisper, Piper and the background llama-server; say_exact has no OpenAI wire, so every "
           "gateway line is guided (response.create with instructions) and checked")


async def rg2(ctx: Ctx):
    """Regime C on /realtime: the provider is the front; an irreversible booking is asked on the client's screen; the caller cancels by voice; nothing is booked."""
    port = ctx.stack.ensure_regime()
    mark, log_mark = len(ctx.webhook.calls), regime_log_size(ctx)
    extra = {"regime": "delegating", "speech": {"claim_policy": "correct"}}
    session = await Native.open_native(ctx, "RG2", port, regime_config(ctx, irreversible=True, extra=extra))
    checks: Dict[str, bool] = {}
    echo = session.ready.get("interaction") or {}
    checks["the session runs regime C"] = echo.get("regime") == "delegating"
    committed = await session.turn_native("10_book_table")
    created = await session.wait(lambda e: e["type"] == "task_created", 45, after=committed)
    checks["the provider's call becomes a task within 45 s"] = (
        created is not None and created[2].get("origin") == "provider_tool")
    asked = await session.wait(lambda e: e["type"] == "task_question", 60, start=created[0] if created else 0)
    checks["the irreversible booking is asked on the client's screen, not by voice"] = (
        asked is not None and asked[2].get("spoken") is False and asked[2].get("irreversible") is True)
    await asyncio.sleep(4.0)
    cancel_start = len(session.events)
    cancel_at = await session.turn_native("04_cancel")
    # As in RG1: the spoken cancel is given 8 s, then the client cancels.
    cancelled = await session.wait(lambda e: e["type"] == "task_cancelled", 8, start=cancel_start)
    heard_cancel = [e.get("text") for _, e in session.all("transcript", cancel_start) if e.get("role") == "user"]
    if cancelled is None and created:
        await session.send({"type": "task_cancel", "task_id": created[2]["task_id"]})
        cancelled = await session.wait(lambda e: e["type"] == "task_cancelled", 12, start=cancel_start)
    checks["the cancel ends the task (by the user when the vendor reports the turn, else the client), nothing committed"] = (
        cancelled is not None and cancelled[2].get("effects") == "none_committed"
        and (cancelled[2].get("by") == "user" or not heard_cancel))
    await asyncio.sleep(10.0)
    calls = [c for c in ctx.webhook.since(mark) if c["tool"] == "book_table"]
    checks["nothing was booked"] = not calls
    said_after = [t for at, t in session.said() if cancelled and at >= cancelled[1]]
    claims = [e for _, e in session.all("unverified_claim")]
    lines = exact_word_checks(ctx, log_mark)
    leaked = [e for _, e in session.all("function_call") if e.get("name") == "waav_delegate"]
    checks["no call of the delegate function reaches the client"] = not leaked
    ctx.record(session)
    await session.close()
    report(ctx, "RG2", "regime C on /realtime over the local GA cascade", checks,
           f"regime {echo.get('regime')}; task {'+%d ms' % ms(created[1] - committed) if created else None} after the commit; "
           f"question spoken {asked[2].get('spoken') if asked else None}, prompt {asked[2].get('prompt') if asked else None!r}; "
           f"caller's cancel as the vendor reported it {heard_cancel or 'not reported'}; "
           f"cancelled {'+%d ms' % ms(cancelled[1] - cancel_at) if cancelled else None} after the cancel, by "
           f"{cancelled[2].get('by') if cancelled else None}, effects {cancelled[2].get('effects') if cancelled else None}; "
           f"book_table calls {len(calls)}; provider after the cancel {said_after}; all provider words "
           f"{[t for _, t in session.said()]}; gateway lines {[(l['required'], l['said'], l['passed']) for l in lines]}; "
           f"unverified_claim {[(c['kind'], c['rule'], c['text'], c['corrected']) for c in claims]}",
           "regime delegating in the echo; task_created (provider_tool) within 45 s; task_question with "
           "spoken false and irreversible true (client screen path); task_cancelled with effects "
           "none_committed, by the user within 8 s of the cancel's commit when the vendor reports the turn, "
           "else by the client's task_cancel; no book_table call; no waav_delegate "
           "function_call on the client socket",
           "no real vendor was reached. What the provider says on its own is reported, not judged here: "
           "each unverified_claim is listed with its sentence")


async def rg3(ctx: Ctx):
    """Regime B on the /v1/realtime relay: the relayed vendor (the GA cascade) delegates a booking; the confirmation is on the client's screen; the result is the gateway's line."""
    ctx.stack.ensure_bud()
    mark = len(ctx.webhook.calls)
    log_mark = ctx.stack.log_size_of("live-bud-gateway.log")
    session = await Realtime.open(ctx, "RG3", "vendor", model="rt-vendor", session={
        "instructions": VENDOR_FRONT, "audio": {"input": {"turn_detection": None}}})
    checks: Dict[str, bool] = {}
    echo = session.ready.get("interaction") or {}
    checks["bud.interaction.state says regime B on the relay"] = echo.get("regime") == "provider_speech"
    wire = len(session.wire)
    await session.say("10_book_table")
    await asyncio.sleep(0.3)
    committed = now()
    await session.send({"type": "input_audio_buffer.commit"})
    created = await session.wait(lambda e: e["type"] == "task_created", 45, after=committed)
    checks["the vendor's call becomes bud.task.created (origin provider_tool) within 45 s"] = (
        created is not None and created[2].get("origin") == "provider_tool")
    asked = await session.wait(lambda e: e["type"] == "task_question", 60, start=created[0] if created else 0)
    checks["the irreversible booking is asked on the client's screen"] = (
        asked is not None and asked[2].get("spoken") is False)
    if asked:
        await session.send({"type": "bud.task.answer", "request_id": asked[2]["request_id"], "outcome": "accept"})
    result = await session.wait(lambda e: e["type"] == "task_result", 90, start=asked[0] if asked else 0)
    calls = [c for c in ctx.webhook.since(mark) if c["tool"] == "book_table"]
    checks["the booking is made once and the result is the gateway's line"] = (
        result is not None and isinstance(result[2].get("effects"), list) and len(calls) == 1
        and result[2].get("spoken") == "full")
    await asyncio.sleep(8.0)
    leaked = [e for _, e in session.wire[wire:]
              if "waav_delegate" in json.dumps(e) and not e.get("type", "").startswith("session.")]
    checks["no frame of the delegate function reaches the client"] = not leaked
    lines = []
    with open(ctx.stack.log_dir / "live-bud-gateway.log", "rb") as handle:
        handle.seek(log_mark)
        for raw in handle.read().decode("utf-8", "replace").splitlines():
            found = re.search(r'interaction exact-words check .*required=(.*?) said=(.*?) passed=(true|false)', raw)
            if found:
                lines.append((found[1], found[2], found[3] == "true"))
    said = [e.get("transcript") for _, e in session.of("response.output_audio_transcript.done", wire)]
    claims = [e for _, e in session.all("unverified_claim")]
    await session.close()
    report(ctx, "RG3", "regime B on the /v1/realtime relay over the local GA cascade", checks,
           f"regime {echo.get('regime')}; task {'+%d ms' % ms(created[1] - committed) if created else None} after the commit; "
           f"question spoken {asked[2].get('spoken') if asked else None}; result {result[2].get('state') if result else None} "
           f"effects {result[2].get('effects') if result else None} spoken {result[2].get('spoken') if result else None}; "
           f"book_table calls {len(calls)}; gateway lines (required -> said, verdict) {lines}; vendor transcripts "
           f"{said}; unverified_claim {[(c['kind'], c['rule'], c['text']) for c in claims]}; delegate frames to the client {len(leaked)}",
           "bud.interaction.state regime provider_speech; bud.task.created with origin provider_tool within 45 s "
           "of the commit; bud.task.question spoken false for the irreversible booking; after bud.task.answer "
           "accept, one book_table call and bud.task.result with a list of effects, spoken full; no frame naming "
           "waav_delegate on the client socket",
           "no real vendor was reached: the relay's deployment points back at the same gateway's GA cascade")


# ---- Lifecycle track, DTMF on a phone line, language packs (slice lifecycle-phone)

# Thresholds of LC1, KD1 and LP1, written before the first run.
HUME_SHUTDOWN = "2026-11-13"
LIFECYCLE_CODES = {"provider_sunset", "provider_retired", "model_retiring", "model_retired"}
# How long after a key the gateway's `task_answer_received` may come (the room's data path and
# the actor's step; no model is involved).
KEY_ANSWER_LIMIT_S = 5.0
# r4-left: the keypad hint a phone caller hears with a confirmation (templates.en.json confirm.dtmf).
KEYPAD_HINT = "You can also press 1 for yes, or 2 for no."
KEYPAD_HINT = "You can also press 1 for yes, or 2 for no."
KEYPAD_CARD = "For your security, please type your card number on your keypad, then press the pound key."
# LP1's test pack (owner, 25 Oct): a small Spanish pack this script writes into its own log
# folder for the run (`write_test_pack`). Machine-written and unreviewed: TEST DATA ONLY, never
# copy it into the repository's packs or assets. Its repeat line is reworded, so the test tells
# the pack's line from a built-in one.
TEST_PACK = "es"
TEST_PACK_REPEAT = "Perdón, ¿me lo repite, por favor?"
TEST_PACK_NOTICE = "Lo siento, en esta línea solo puedo ayudar en español."
# The Spanish words of every template (`templates.en.json` ids; slots unchanged).
ES_TEMPLATES = {
    "confirm.question": "¿Quiere que lo haga?",
    "confirm.short_reask": "¿Quiere que haga esto: {verb_phrase}?",
    "confirm.weak_yes_reask": "Para estar seguro: ¿quiere que haga esto: {verb_phrase}?",
    "confirm.not_heard_reask": "Entonces: {params}. ¿Quiere que lo haga?",
    "confirm.dtmf": "También puede pulsar 1 para sí, o 2 para no.",
    "confirm.other_voice": "Estos son los datos de nuestro sistema:",
    "hitl.choose_dtmf": "También puede pulsar {options}.",
    "choose": "{question} ¿{options}?",
    "offer.result": "Tengo la respuesta sobre {task_title}. ¿La quiere ahora?",
    "offer.more": "Hay más. ¿Sigo?",
    "update.offer_many": "Por cierto, han llegado {count} novedades: {task_titles}. ¿Las quiere ahora?",
    "update.done": "Su {task_title} está listo: {summary_sentence}",
    "update.done_screen": "Su {task_title} está listo; los detalles están en su pantalla.",
    "update.screen": "Los detalles están en su pantalla.",
    "question.interrupt": "Una pregunta rápida.",
    "question.resume": "En fin,",
    "question.expired": "Una pregunta sobre {task_title} no podía esperar su respuesta, así que siguió sin ella.",
    "followup.undo_offer": "Su {task_title} ya está hecho. ¿Quiere que pida deshacerlo?",
    "choice_offer": "Sigue en marcha. ¿Quiere seguir esperando, o prefiere que {continuation_or_stop}?",
    "status.working": "Sigo trabajando en {task_title}.",
    "status.sentence_fragment": "Ahora mismo: {status_fragment}.",
    "status.queued": "Todavía estoy terminando {behind}; luego haré eso.",
    "status.queued_on_create": "Me pondré con eso justo después de {task_title}.",
    "status.queued_behind": "Eso está en cola detrás de otras {count} peticiones; me pondré con ello después.",
    "status.many": "Ahora mismo estoy trabajando en {task_titles}.",
    "status.many_more": "Ahora mismo estoy trabajando en {task_titles}, y {count} más.",
    "status.waiting_on_caller": "Estoy esperando su respuesta sobre {task_title}.",
    "status.result_ready": "Tengo una respuesta sobre {task_title}.",
    "presence": "Sí, aquí estoy. Sigo trabajando en {task_title}.",
    "holding.status": "Sigo con ello: {status_fragment}.",
    "bridge.about": "Sobre {task_title}:",
    "which_task.cancel": "¿Cuál paro: {task_a} o {task_b}?",
    "which_task.update": "¿Es para {task_a} o para {task_b}?",
    "duplicate.ask": "¿Quiere que lo cambie, o es otra petición?",
    "finish_or_stop": "Sigo trabajando en {task_title}. ¿Quiere que lo termine, o que lo pare?",
    "finish_or_stop.with_channel": "Sigo trabajando en {task_title}. ¿Quiere que lo termine y {continuation_phrase}, o que lo pare?",
    "goodbye.side_effect_running": "Su {task_title} todavía se está procesando. Por favor, espere un momento para que le cuente cómo ha ido.",
    "cancel.intent": "De acuerdo, lo paro.",
    "cancel.intent_operation_in_flight": "De acuerdo, lo estoy parando.",
    "cancel.confirmed_none_committed": "Está parado.",
    "cancel.confirmed_none_committed_named": "He cancelado {task_title}.",
    "cancel.confirmed_committed": "Lo he parado, pero antes de parar ya había {effect_labels}.",
    "cancel.confirmed_effects_unknown": "Lo he parado, pero no puedo confirmar si ya había {action_label}.",
    "cancel.unconfirmed_backend_cannot_confirm": "Le he dicho que pare, pero no puedo confirmar que lo haya hecho.",
    "cancel.unconfirmed_no_confirmation_in_time": "Le he pedido que pare; le aviso cuando esté confirmado.",
    "cancel.unconfirmed_operation_in_flight": "Lo he parado, pero puede que {action_label} ya se haya completado.",
    "cancel.already_done": "Eso ya había terminado:",
    "cancel.already_done_undo_offer": "¿Quiere que lo deshaga?",
    "stale_outcome": "Antes de su cambio, {effect_labels} ya se había completado.",
    "unknown": "He perdido el rastro de {task_title}; puede que ya se haya completado.",
    "failure.retryable": "Lo siento, no he podido hacerlo ahora mismo.",
    "failure.retry_offer": "¿Quiere que lo intente de nuevo?",
    "failure.second": "Lo siento, sigue sin funcionar{handoff_clause}.",
    "failure.timeout": "Está tardando mucho más de lo debido, así que lo he parado.",
    "failure.effects_unknown": "No sé si {verb_phrase} se ha completado. Lo comprobaré antes de hacer nada más.",
    "failure.with_effects": "No terminó, pero ya había {effect_labels}.",
    "rejected": "Eso no es algo que pueda hacer en esta llamada.",
    "approval_needed": "Eso necesita primero la aprobación de alguien, así que no lo he hecho.",
    "approval.not_recorded": "Lo siento, no he podido registrar su aprobación.",
    "approval.not_permitted": "No puedo aprobar eso desde esta llamada. Primero necesita la firma de otra persona.",
    "approval.more_needed": "Gracias. También necesita la firma de otra persona antes de seguir.",
    "hitl.accept": "De acuerdo.",
    "hitl.decline": "De acuerdo, no lo haré.",
    "hitl.decline_correction": "De acuerdo, lo cambio.",
    "hitl.cancel": "De acuerdo, lo dejo.",
    "hitl.timeout": "No pasa nada, lo dejo por ahora.",
    "hitl.unclear_confirm": "Perdone, ¿era un sí o un no?",
    "hitl.unclear_choose": "¿Cuál: {options}?",
    "sensitive.dtmf": "Por su seguridad, escriba {field_label} en el teclado y después pulse la tecla almohadilla.",
    "sensitive.screen": "Le pongo un formulario protegido en la pantalla para {field_label}.",
    "sensitive.decline": "No puedo tomar {field_label} en esta llamada por motivos de seguridad.",
    "sensitive.dont_say": "Por favor, no lo diga en voz alta. Use {channel}.",
    "auth_url.before": "Para eso necesito que inicie sesión. Le envío un enlace protegido a {channel}.",
    "auth_url.after": "Ya lo he enviado.",
    "control.unsupported": "Lo siento, en esta línea no puedo {what}.",
    "control.refused": "Lo siento, eso no lo puedo cambiar aquí.",
    "control.language_unsupported": "Lo siento, en esta línea solo puedo hablar {languages}.",
    "control.language_answers_only": "De acuerdo, responderé en {language}. Algunas de mis frases cortas pueden seguir en español.",
    "control.nothing_to_repeat": "Lo siento, no tengo eso para repetirlo.",
    "control.on_screen": "Ya está en su pantalla.",
    "control.voice_changed": "De acuerdo, ahora hablaré con esta voz.",
    "control.human_transfer": "Claro, le paso con una persona ahora.",
    "control.human_unavailable": "Lo siento, en esta línea no hay nadie con quien pueda ponerle.",
    "control.human_finding": "Claro, le busco una persona. Un momento.",
    "control.human_no_answer": "Lo siento, ahora mismo nadie puede atender su llamada, pero sigo aquí para ayudarle.",
    "repeat_request": TEST_PACK_REPEAT,
    "regime_c_correction": "Para que quede claro, eso todavía no está terminado. Le avisaré cuando lo esté.",
}
ES_TEMPLATE_SLOTS = {("confirm.lead_in", "lead_in", "values"): ["Para confirmar:", "Solo para comprobar:"],
                     ("choice_offer", "continuation_or_stop", "default"): "lo pare"}
ES_TEMPLATE_EXEMPT = {"confirm.not_heard_reask": [], "hitl.unclear_confirm": ["sí", "no"]}
# The Spanish lines of every bank group, in the English file's order (flags and clip ids kept).
ES_BANKS = {
    "openers": ["Vale", "Entendido", "De acuerdo", "Un momento", "Buena pregunta", "Claro", "Bien", "Por supuesto"],
    "inquiry": ["déjeme comprobar", "déjeme comprobar si", "voy a averiguar si", "voy a mirar", "déjeme mirar"],
    "restate_closers": ["mejor", "entonces"],
    "ack": ["Claro, lo compruebo.", "Vale, un momento.", "Déjeme mirarlo.", "Entendido, lo miro.",
            "Claro, lo averiguo.", "Déjeme comprobarlo.", "Un momento.", "Deme un momento.", "Un segundo."],
    "update_ack": ["Vale, lo cambio.", "Entendido, lo actualizo.", "Anotado, lo cambio.", "De acuerdo, lo actualizo."],
    "progress_openers": ["Sigo con ello", "Gracias por esperar", "Tenga paciencia", "Sigo trabajando en ello"],
    "status_working": ["Sigo trabajando en ello", "Sigo con ello", "Sigo comprobándolo", "Todavía está en curso"],
    "status_queued": ["Me pondré con eso justo después de", "Eso va justo después de",
                      "Empezaré con eso justo después de", "Eso viene justo después de"],
    "clarify": ["Perdone, ¿puede repetirlo?", "Perdone, no le he entendido. ¿Puede repetirlo?",
                "Perdone, se ha cortado la línea. ¿Puede repetirlo?", "¿Puede decirlo otra vez?"],
    "refuse": ["Eso no es algo con lo que pueda ayudarle en esta llamada.", "Lo siento, con eso no puedo ayudarle aquí.",
               "En esta línea no puedo ayudarle con eso.", "Eso está fuera de lo que puedo hacer en esta llamada."],
    "social": ["Hola, ¿en qué puedo ayudarle?", "De nada.", "Gracias por llamar a {{org}}. Adiós.", "Sí, aquí estoy.",
               "Ha llamado a {{org}}. Soy {{name}}, un asistente automático.", "Estoy bien, gracias por preguntar.",
               "Suena bien.", "Me parece justo.", "Le entiendo.", "Me alegra oírlo.",
               "Antes de que se vaya, todavía estoy trabajando en algo para usted.", TEST_PACK_NOTICE],
    "holding": ["Esto está tardando un poco más. Sigo con ello.", "Sigo trabajando en ello, gracias por esperar.",
                "Sigo con ello.", "Tenga paciencia, sigo trabajando en ello."],
    "check_in": ["Sigo trabajando en ello.", "Sigo aquí, sigo con ello.", "Sigo en ello, gracias por su paciencia.",
                 "Nada nuevo todavía, sigo con ello."],
    "wait_agreed": ["De acuerdo, sigo con ello.", "De acuerdo, no lo suelto.", "Muy bien, sigo trabajando en ello.",
                    "De acuerdo, continúo.", "Claro, tómese su tiempo.", "Sin prisa, aquí estoy.", "Cuando usted quiera.",
                    "De acuerdo, espero."],
    "bridge": ["Vale, esto es lo que he encontrado.", "Bien, ya lo tengo.", "Vale, ya lo tengo.", "De acuerdo, aquí está."],
    "withdrawal": ["Olvide eso.", "Ignore esa última parte.", "Déjeme rehacerlo.", "Un momento, eso ha cambiado.",
                   "Lo siento, no puedo darle el resto de esa respuesta.", "Un momento, déjeme comprobarlo bien.",
                   "El resto está en su pantalla."],
    "stalled": ["Un momento."],
    "backchannel": ["mhm", "claro", "vale", "ya veo", "sí"],
    "failure": ["Lo siento, mis sistemas están fallando. Tenga paciencia un momento.",
                "Tenga paciencia, algo va lento por mi lado.", "Gracias por esperar, mis sistemas siguen lentos.",
                "Lo siento, ahora mismo me cuesta oírle. Tenga paciencia un momento.",
                "Lo siento, ahora mismo no le oigo bien.", "Sigo sin oírle bien, lo siento. Tenga paciencia.",
                "Lo siento mucho, mis sistemas han caído y tengo que colgar. Por favor, vuelva a llamar un poco más tarde.",
                "Lo siento mucho, mis sistemas han caído y tengo que colgar. Me pondré en contacto con usted cuando "
                "vuelvan a funcionar.",
                "Gracias por esperar, ya estoy de vuelta."],
}
# The guard lexicon's Spanish tables (the rest stays the English file's: number words, codes,
# labels, the sanitiser).
ES_GUARD = {
    "rule_layer_cues": {
        "continuers": ["ajá", "mhm", "sí", "vale", "claro", "ya", "de acuerdo", "entiendo", "ya veo", "bueno", "sigue"],
        "social_intents": {
            "greeting": ["hola", "buenos días", "buenas tardes", "buenas noches", "buenas"],
            "thanks": ["gracias", "muchas gracias", "mil gracias", "se lo agradezco"],
            "goodbye": ["adiós", "hasta luego", "chao", "eso es todo", "nada más", "que tenga un buen día"],
            "presence": ["está ahí", "sigue ahí", "hay alguien", "me oye", "me escucha"],
            "identity": ["quién es", "con quién hablo", "es usted una persona", "es un robot", "cómo se llama"],
            "how_are_you": ["qué tal", "cómo está", "cómo le va", "cómo estás"],
        },
        "status_cues": ["todavía", "aún", "alguna novedad", "cuánto falta", "ya está", "cómo va", "qué tal va",
                        "ha encontrado", "encontró algo"],
        "cancel_cues": ["olvídelo", "déjelo", "cancele eso", "cancélelo", "no importa", "ya no", "he cambiado de idea",
                        "anule eso"],
        "repeat_cues": ["repita", "puede repetir", "qué ha dicho", "cómo dice", "perdón", "otra vez"],
        "amendment_cues": ["en realidad", "quería decir", "mejor", "en vez de", "cámbielo a", "no, dije", "dije",
                           "espere, no"],
        "request_cues": ["necesito", "quiero", "quisiera", "me gustaría", "puede", "podría", "por favor", "llamo por",
                         "ayúdeme", "hay", "qué", "cuándo", "dónde", "cuál", "por qué", "cómo", "puedo", "dígame"],
        "side_talk_cues": ["cariño", "un segundo, cariño", "estoy al teléfono", "no es contigo", "niños", "mamá", "papá"],
        "stop_talking_cues": ["pare", "basta", "calle", "espere", "espere un segundo", "un momento", "un segundo",
                              "deme un segundo", "déjeme pensar", "déjeme terminar"],
        "content_word_stop_list": ["el", "la", "los", "las", "un", "una", "y", "o", "pero", "a", "de", "del", "al",
                                   "en", "con", "por", "para", "es", "son", "era", "ser", "esto", "eso", "yo", "usted",
                                   "me", "mi", "su", "nos", "nuestro", "que", "qué", "cómo", "si", "sobre", "favor",
                                   "vale", "sí", "bueno", "pues", "también", "algo", "hay", "aquí", "ahora", "ya",
                                   "todavía", "necesito", "quiero", "puede", "podría", "cosa"],
    },
    "polar_affirmation": {
        "first_words": ["sí", "claro", "por supuesto", "desde luego", "exacto", "correcto", "no", "nunca",
                        "efectivamente", "cierto"],
        "polar_question_starters": ["es", "son", "está", "están", "puede", "puedo", "podría", "tiene", "hay", "quiere",
                                    "sabe", "ha", "han"],
    },
    "completion_claims": {
        "phrases": ["reservado", "confirmado", "cancelado", "reembolsado", "enviado", "hecho", "listo", "todo listo",
                    "ha sido", "han sido", "aprobado", "procesado", "completado", "terminado", "programado",
                    "entregado", "pagado", "cobrado", "está parado", "está cancelado", "se ha completado",
                    "solucionado", "añadido"],
        "patterns": [r"\bhe (reservado|enviado|cambiado|cancelado|movido|añadido|actualizado|reembolsado|pedido|pagado|"
                     r"cobrado|procesado|programado|arreglado)\b",
                     r"\b(voy a|le voy a) (reservar|cobrar|pagar|cancelar|reembolsar|enviar|pedir|comprar|transferir|"
                     r"mover)\b"],
    },
    "outcome_predictions": {
        "phrases": ["va a recibir", "estará bien", "por desgracia", "lamentablemente", "buenas noticias",
                    "malas noticias", "no podemos", "es posible", "no es posible", "está incluido", "es gratis",
                    "está disponible", "están disponibles", "no está disponible", "sin cargo", "cumple los requisitos",
                    "está cubierto", "llegará", "en camino", "agotado", "definitivamente", "seguro que sí"],
    },
    "eta_promises": {
        "phrases": ["en un minuto", "en un segundo", "en unos minutos", "en unos segundos", "en un momento",
                    "enseguida", "pronto", "casi está", "casi listo", "casi terminado", "no tardará", "en nada",
                    "de inmediato"],
        "patterns": [r"\ben \d+ (segundos?|minutos?|horas?)\b",
                     r"\ben (uno|una|dos|tres|cuatro|cinco|diez|quince|veinte|treinta) (segundos?|minutos?|horas?)\b"],
        "promise_verbs": ["voy a", "va a", "estará", "tendré", "le llamo", "vuelvo", "le aviso"],
        "strict_bank_phrases": ["casi listo", "casi terminado", "un minuto", "minutos", "segundos", "pronto",
                                "enseguida", "de inmediato", "no tardará", "en nada"],
    },
    "channel_promises": {
        "phrases": ["le mando un mensaje", "le envío un mensaje", "le llamo", "le escribo", "le enviaremos",
                    "recibirá un mensaje", "recibirá un correo"],
    },
    "action_narration": {
        "phrases": ["estoy comprobando", "lo compruebo", "voy a comprobar", "déjeme comprobar", "déjeme mirar",
                    "voy a mirar", "estoy mirando", "estoy buscando", "lo miro", "voy a averiguar", "lo averiguo",
                    "estoy en ello"],
    },
    "disfluency": {"phrases": ["eh", "em", "ehm", "mmm", "ejem", "déjeme pensar", "déjame pensar"]},
    "inquiry_frames": {
        "phrases": ["déjeme comprobar", "déjeme comprobar si", "voy a averiguar si", "voy a mirar", "déjeme mirar",
                    "déjeme ver si", "ha dicho"],
    },
    "sensitive_categories": {
        "categories": ["número de tarjeta", "tarjeta de crédito", "tarjeta de débito", "cvv", "cvc",
                       "código de seguridad", "pin", "contraseña", "código de un solo uso", "otp", "dni",
                       "seguridad social", "cuenta bancaria", "número de cuenta", "iban"],
    },
    "copy_spans": {
        "pronoun_turn": {"yo": "usted", "me": "le", "mi": "su", "mis": "sus", "nosotros": "ustedes",
                         "nuestro": "su", "nuestra": "su", "nuestros": "sus", "nuestras": "sus"},
        "pronoun_turn_pairs": {},
        "second_person_never_copied": ["tú", "te", "ti", "tu", "tus", "usted", "ustedes", "contigo"],
        "restate_stop_words": ["es", "son", "era", "fue", "ser", "estar", "está", "están", "puede", "podría", "debe",
                               "tiene", "tienen", "ha", "han", "no", "nunca", "aprobado", "incluido", "confirmado",
                               "reservado", "cancelado", "reembolsado", "disponible", "gratis", "hecho", "enviado",
                               "pagado"],
        "restate_stop_suffixes": [],
        "start_word_stop_list": ["y", "o", "pero", "así", "entonces", "también", "si", "porque", "eh", "bueno",
                                 "vale", "sí", "no", "hola", "por", "favor", "perdón", "perdone", "oiga", "mire",
                                 "puede", "podría", "quiero", "necesito", "me", "lo", "la", "el", "los", "las", "un",
                                 "una", "de", "que", "qué", "cómo", "cuándo", "dónde"],
        "dangling_end_words": ["a", "de", "el", "la", "los", "las", "un", "una", "y", "o", "pero", "para", "con",
                               "por", "en", "mi", "su", "que", "si", "porque", "como", "del", "al"],
        "allowed_word_pattern": "^[A-Za-z0-9áéíóúüñÁÉÍÓÚÜÑ¿¡$€£₹%'&/.,:?!-]+$",
    },
    "language_check": {
        "english_function_words": ["el", "la", "los", "las", "un", "una", "y", "o", "pero", "a", "de", "del", "al",
                                   "en", "con", "por", "para", "es", "son", "era", "yo", "usted", "me", "mi", "su",
                                   "que", "qué", "cómo", "cuándo", "dónde", "por qué", "si", "no", "sí", "hola",
                                   "gracias", "vale", "también", "ya", "todavía", "ahora", "aquí"],
    },
}
ES_ANSWERS = {
    "strong_accept": ["sí", "sí, por favor", "adelante", "hágalo", "confirmo", "confirmado", "correcto", "eso es",
                      "por favor, hágalo", "claro que sí", "resérvelo"],
    "weak_accept": ["vale", "bueno", "de acuerdo", "claro", "bien", "me parece bien", "ajá", "mhm", "supongo que sí"],
    "decline": ["no", "no, gracias", "no lo haga", "eso no", "eso está mal", "no es correcto"],
    "cancel": ["olvídelo", "déjelo", "cancele", "cancele eso", "no importa"],
}


# RV1's vendor: short answers, except a long one it can be cut in on.
ROOM_VOICE_FRONT = (VENDOR_FRONT + " When the caller asks why the sky is blue, explain it slowly in at "
                    "least six sentences.")


async def room_voice(ctx: Ctx, label: str, port: int) -> Tuple[Session, "RoomCaller"]:
    """A `/ws` session whose voice is the stand-in vendor (manual turns) in a fresh room, and
    its caller."""
    block = regime_config(ctx, irreversible=False)["interaction"]
    block["speech"] = {"realtime": {"provider": "openai", "instructions": ROOM_VOICE_FRONT,
                                    "turn_detection": {"mode": "manual"}}}
    # The room plays the vendor's question in real time before the caller answers (and the
    # client after 8 s more): 20 s, the default, ran out once.
    block["policy"] = {"hitl_answer_timeout_ms": 60000}
    session = await Session.open(ctx, label, block, mic=False, conversation=False, port=port,
                                 livekit={"room_name": f"{label.lower()}-{ctx.run_id}"},
                                 extra={"stt_config": None, "tts_config": None})
    caller = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "caller").join()
    await asyncio.sleep(2.0)
    return session, caller


def room_quiet(caller: "RoomCaller") -> bool:
    return not any(db > AUDIBLE_DB for _, db in caller.levels(now() - 1.0))


def room_topic_checks(session: Session, caller: "RoomCaller") -> Tuple[List[str], List[str]]:
    """The interaction events of the socket missing from the room's topic, and the kinds the
    room got."""
    socket_events = [json.dumps(e, sort_keys=True) for e in actor_events(session)]
    room_events = [json.dumps(e, sort_keys=True) for _, e in caller.events]
    missing = [e for e in socket_events if e not in room_events]
    return missing, sorted({json.loads(e)["type"] for e in room_events})


async def rv1(ctx: Ctx):
    """Regime B in a LiveKit room: the session's voice is the speech-to-speech vendor (the GA cascade); a booking is delegated, the caller cuts the vendor off, and the room gets the events."""
    try:
        import livekit.rtc  # noqa: F401
    except ImportError:
        raise RuntimeError("RV1 needs the LiveKit client: pip install livekit livekit-api")
    ctx.stack.ensure_livekit()
    port = ctx.stack.ensure_regime()
    mark, log_mark = len(ctx.webhook.calls), regime_log_size(ctx)
    checks: Dict[str, bool] = {}

    # 1. A booking: the gateway ends the caller's turn, the vendor delegates it.
    session, caller = await room_voice(ctx, "RV1-book", port)
    echo = session.ready.get("interaction") or {}
    checks["ready says regime B with the delegate function, no warnings"] = (
        echo.get("regime") == "provider_speech" and echo.get("speech", {}).get("delegate_tool") == "waav_delegate"
        and not session.warnings)
    said = await caller.say("10_book_table")
    created = await session.wait(lambda e: e["type"] == "task_created", 45, after=said.start)
    checks["the vendor's call becomes a task (origin provider_tool) within 45 s of the caller's turn"] = (
        created is not None and created[2].get("origin") == "provider_tool")
    asked = await session.wait(lambda e: e["type"] == "task_question", 60, start=created[0] if created else 0)
    checks["the confirmation is asked by voice (task_question spoken)"] = (
        asked is not None and asked[2].get("spoken") is True)
    # The vendor says the question in the room; the caller answers once it has played.
    deadline = now() + 40
    while now() < deadline and not any("Do you want me" in l["required"] for l in exact_word_checks(ctx, log_mark)):
        await asyncio.sleep(0.2)
    await asyncio.sleep(1.0)
    while now() < deadline and not room_quiet(caller):
        await asyncio.sleep(0.2)
    heard_question = caller.first_audible(asked[1]) if asked else None
    checks["the vendor's question is heard in the room"] = heard_question is not None
    voice_start = len(session.events)
    yes = await caller.say("12_yes")
    # As in RG1: the vendor does not report a caller's turn while the delegate call is open,
    # so the yes is given 8 s, then the client answers.
    by_voice = await session.wait(lambda e: e["type"] == "task_answer_received", 8, start=voice_start)
    if by_voice is None and asked:
        await session.send({"type": "task_answer", "request_id": asked[2]["request_id"], "outcome": "accept"})
    answered = by_voice or await session.wait(lambda e: e["type"] == "task_answer_received", 10, start=voice_start)
    result = await session.wait(lambda e: e["type"] == "task_result", 90, start=voice_start)
    calls = [c for c in ctx.webhook.since(mark) if c["tool"] == "book_table"]
    checks["the booking is made once and the result is the gateway's line (spoken full)"] = (
        result is not None and len(calls) == 1 and result[2].get("spoken") == "full")
    # The provider says the result line after the event: wait for its words, then its audio.
    summary = result[2].get("summary", "") if result else ""
    deadline = now() + 40
    while result and now() < deadline and not any(
            l["required"] == summary for l in exact_word_checks(ctx, log_mark)):
        await asyncio.sleep(0.2)
    await asyncio.sleep(1.5)
    heard_result = caller.first_audible(result[1]) if result else None
    checks["the result line is heard in the room"] = heard_result is not None
    while now() < deadline and not room_quiet(caller):
        await asyncio.sleep(0.2)
    await asyncio.sleep(2.0)
    book_missing, book_kinds = room_topic_checks(session, caller)
    book_count = len(caller.events)
    ctx.record(session)
    await caller.leave()
    await asyncio.sleep(1.0)
    await session.close()

    # 2. A barge-in on a fresh session: the vendor talks at length, the caller cuts in a
    # second into its audio.
    session, caller = await room_voice(ctx, "RV1-cut", port)
    asked_sky = await caller.say("22_q_sky")
    deadline = now() + 60
    began = None
    while now() < deadline and began is None:
        began = caller.first_audible(asked_sky.end)
        await asyncio.sleep(0.05)
    stopped = None
    cut: Dict[str, Any] = {"began": began is not None}
    if began is not None:
        await asyncio.sleep(max(0.0, began + 1.0 - now()))
        cut["talking"] = not room_quiet(caller)
        pcm = load_wav(ctx.caller("14_barge_in"))
        barge = await caller.say("14_barge_in", pcm, block=False)
        voiced = barge.start + first_voiced_ms(pcm) / 1000
        stopped = await session.wait(lambda e: e["type"] == "agent_audio_stopped", 6)
        await asyncio.sleep(max(0.0, barge.end - now()))
        before, after, down_at = level_drop(caller, voiced)
        cut.update({"stopped_ms": ms(stopped[1] - voiced) if stopped else None, "down_ms": ms(down_at),
                    "before_db": round(before, 1), "after_db": round(after, 1)})
    limit = ROOM_VOICE_CUT_LIMIT_MS
    checks["the vendor's long answer is heard, and talking when the caller cuts in"] = bool(cut.get("talking"))
    checks["agent_audio_stopped (barge_in) on the socket"] = (
        stopped is not None and stopped[2].get("reason") == "barge_in")
    checks[f"the vendor's audio in the room 10 dB down within {limit} ms of the caller's first word"] = (
        cut.get("down_ms") is not None and cut["down_ms"] <= limit)
    await asyncio.sleep(4.0)
    cut_missing, cut_kinds = room_topic_checks(session, caller)
    checks["every interaction event of the socket is on waav.interaction (the task's and the cut's among them)"] = (
        not book_missing and not cut_missing and book_count > 0
        and {"task_created", "task_question", "task_result"} <= set(book_kinds)
        and "agent_audio_stopped" in cut_kinds)
    lines = exact_word_checks(ctx, log_mark)
    claims = [e for _, e in session.all("unverified_claim")]
    ctx.record(session)
    await caller.leave()
    await asyncio.sleep(1.0)
    await session.close()
    report(ctx, "RV1", "regime B in a LiveKit room with a realtime voice", checks,
           f"regime {echo.get('regime')}; warnings {[w.get('code') for w in session.warnings]}; task "
           f"{'+%d ms' % ms(created[1] - said.end) if created else None} after the caller's turn; question "
           f"spoken {asked[2].get('spoken') if asked else None}, heard in the room "
           f"{'+%d ms' % ms(heard_question - asked[1]) if heard_question and asked else None}; answer "
           f"{answered[2].get('outcome') + ' by ' + answered[2].get('by') if answered else None}"
           f"{' +%d ms after the yes' % ms(answered[1] - yes.end) if answered else ''}; result "
           f"{result[2].get('state') if result else None} spoken {result[2].get('spoken') if result else None}, "
           f"heard in the room {'+%d ms' % ms(heard_result - result[1]) if heard_result and result else None}; "
           f"book_table calls {len(calls)}; barge-in {cut}; room events {book_count} ({book_kinds}) and "
           f"{len(caller.events)} ({cut_kinds}), socket events not in the room {len(book_missing) + len(cut_missing)}; "
           f"gateway lines {[(l['required'], l['said'], l['passed']) for l in lines]}; unverified_claim "
           f"{[(c['kind'], c['rule'], c['text']) for c in claims]}",
           "ready.interaction regime provider_speech with waav_delegate and no config_warning; task_created "
           "(provider_tool) within 45 s of the caller's turn; task_question spoken; the question and the result "
           "line audible in the room after their events; book_table called once and task_result spoken full; on a second session the "
           "caller cuts in 1 s into the vendor's long answer: agent_audio_stopped barge_in and the room's level "
           f"10 dB down within {limit} ms of the caller's first voiced frame; every interaction event of each "
           "socket also on its room's waav.interaction topic, task_created, task_question, task_result and "
           "agent_audio_stopped among them",
           "no real vendor was reached: the voice is the gateway's own GA cascade over the local Whisper, Piper "
           "and the background llama-server, with manual turns (the gateway commits each caller turn at 700 ms "
           "of silence and decides the barge-in after duplex.pause_after_ms, 400 ms); the yes goes through the "
           "client when the vendor does not report the turn (as RG1). The barge-in runs on a second session: "
           "that vendor answers no caller turn after a delegate call on the same session")


def write_test_pack(directory: Path) -> Path:
    """LP1's Spanish test pack in `directory` (the run's own log folder): the English pack files
    with their words in Spanish (`ES_TEMPLATES`, `ES_BANKS`, `ES_GUARD`, `ES_ANSWERS`). Test data
    only, machine-written and unreviewed: it must never ship."""
    assets = REPO_GATEWAY / "src/core/interaction/assets"
    directory.mkdir(parents=True, exist_ok=True)
    note = "Machine-written Spanish TEST pack of interaction_live_e2e.py (LP1); unreviewed, never shipped."
    english = {kind: json.loads((assets / f"{kind}.en.json").read_text())
               for kind in ("guard_lexicon", "phrase_banks", "templates")}
    templates = english["templates"]
    for template_id, text in ES_TEMPLATES.items():
        templates["templates"][template_id]["text"] = text
    for (template_id, slot, key), value in ES_TEMPLATE_SLOTS.items():
        templates["templates"][template_id]["slots"][slot][key] = value
    for template_id, exempt in ES_TEMPLATE_EXEMPT.items():
        templates["templates"][template_id]["answer_lexicon_exempt"] = exempt
    templates["answer_lexicon_for_validator"].update(ES_ANSWERS)
    banks = english["phrase_banks"]
    for group, lines in ES_BANKS.items():
        assert len(lines) == len(banks["groups"][group]["lines"]), group
        for line, text in zip(banks["groups"][group]["lines"], lines):
            line["text"] = text
    guard = english["guard_lexicon"]
    for key, tables in ES_GUARD.items():
        guard[key].update(tables)
    for kind, data in english.items():
        data["language"], data["description"] = TEST_PACK, note
        (directory / f"{kind}.{TEST_PACK}.json").write_text(json.dumps(data, ensure_ascii=False, indent=1))
    return directory


async def frames_of(url: str, config: dict, seconds: float) -> List[dict]:
    """The JSON frames a socket gets for `seconds` after its config message."""
    out: List[dict] = []
    async with websockets.connect(url, max_size=None, ping_interval=None) as ws:
        await ws.send(json.dumps(config))
        deadline = now() + seconds
        while now() < deadline:
            try:
                frame = await asyncio.wait_for(ws.recv(), max(0.05, deadline - now()))
            except asyncio.TimeoutError:
                break
            except Exception:
                break
            if isinstance(frame, str):
                out.append(json.loads(frame))
    return out


async def lc1(ctx: Ctx):
    """Lifecycle: Hume is named before its shutdown and refused after; a retired model is refused; nothing else changes."""
    today = time.strftime("%Y-%m-%d", time.gmtime())
    before_hume = today < HUME_SHUTDOWN
    base = f"ws://127.0.0.1:{ctx.args.gateway_port}"
    # /ws: Hume's text-to-speech (no Hume key on this machine: the key check fails after it).
    hume_ws = await frames_of(f"{base}/ws", {
        "type": "config",
        "stt_config": {"provider": "openai", "model": "whisper-1", "language": "en",
                       "sample_rate": CALLER_RATE, "channels": 1, "punctuation": True,
                       "encoding": "linear16"},
        "tts_config": {"provider": "hume", "voice_id": "live", "audio_format": "pcm",
                       "sample_rate": AGENT_RATE},
    }, 4.0)
    ws_notices = [e for e in hume_ws if e.get("code") in LIFECYCLE_CODES]
    # /realtime: a model Google retired on 2025-12-09, and Hume EVI.
    gemini = await frames_of(f"{base}/realtime", {"type": "config", "provider": "gemini",
                                                  "model": "gemini-2.0-flash-live-001"}, 3.0)
    hume_rt = await frames_of(f"{base}/realtime", {"type": "config", "provider": "hume"}, 3.0)
    rt_notices = [e for e in hume_rt if e.get("code") in LIFECYCLE_CODES]
    # A session whose OpenAI legs go to this machine's speech server is not affected.
    plain = await Session.open(ctx, "LC1-plain", mic=False)
    plain_codes = [w.get("code") for w in plain.warnings]
    await plain.close()
    if before_hume:
        hume_ok = (len(ws_notices) == 1 and ws_notices[0].get("type") == "config_warning"
                   and ws_notices[0].get("code") == "provider_sunset"
                   and (ws_notices[0].get("detail") or {}).get("side") == "tts"
                   and (ws_notices[0].get("detail") or {}).get("shutdown") == HUME_SHUTDOWN
                   and not any(e.get("type") == "ready" for e in hume_ws))
        rt_ok = (len(rt_notices) == 1 and rt_notices[0].get("type") == "warning"
                 and rt_notices[0].get("code") == "provider_sunset")
    else:
        hume_ok = [(e.get("type"), e.get("code")) for e in hume_ws] == [("error", "provider_retired")]
        rt_ok = [(e.get("type"), e.get("code")) for e in hume_rt] == [("error", "provider_retired")]
    checks = {
        "Hume text-to-speech on /ws": hume_ok,
        "Hume EVI on /realtime": rt_ok,
        "a retired Gemini model is refused before anything is dialled":
            [(e.get("type"), e.get("code")) for e in gemini] == [("error", "model_retired")],
        "a session on the operator's own OpenAI-compatible server hears nothing":
            not LIFECYCLE_CODES & set(plain_codes) and plain.ready.get("type") == "ready",
    }
    report(ctx, "LC1", "lifecycle warnings and refusals", checks,
           f"date {today}; /ws hume: {[(e.get('type'), e.get('code'), e.get('detail')) for e in hume_ws]}; "
           f"/realtime gemini-2.0-flash-live-001: {[(e.get('type'), e.get('code')) for e in gemini]}; "
           f"/realtime hume: {[(e.get('type'), e.get('code')) for e in hume_rt]}; openai tts-1 and whisper-1 "
           f"on {ctx.args.audio_url}: warnings {plain_codes}",
           f"before {HUME_SHUTDOWN}: /ws hume gets one config_warning provider_sunset (detail side tts, shutdown "
           f"{HUME_SHUTDOWN}) and no ready, /realtime hume one warning provider_sunset; from that date each gets "
           "only an error provider_retired. /realtime gemini-2.0-flash-live-001: exactly one frame, error "
           "model_retired. A session with OpenAI legs on the local server gets no lifecycle code",
           "no Hume or Gemini key exists here: the session after a warning fails on its key check, which "
           "shows the warning comes first; nothing reaches a vendor")


async def kd1_confirm(ctx: Ctx) -> Tuple[Dict[str, bool], str]:
    mark = len(ctx.webhook.calls)
    block = {**with_tools(ctx, "book_table"), "hitl": {"channels": ["voice", "dtmf"]}}
    session = await Session.open(ctx, "KD1-confirm", block, TOOLS_PROMPT, mic=False,
                                 livekit={"room_name": f"kd1-{ctx.run_id}"})
    phone = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "sip_caller", kind="sip").join()
    await session.warm()
    # The gateway knows the participant only after the room told it (keys before are dropped).
    await asyncio.sleep(3.0)
    await settle(session, phone)
    start = len(session.events)
    await phone.say("10_book_table")
    confirm, after = await question(session, start)
    hint = await said(session, after - 1, KEYPAD_HINT, 40)
    if hint:
        await heard(session, hint, 40)
    pressed = now()
    await phone.room.local_participant.publish_dtmf(code=1, digit="1")
    answer = await answer_received(session, after, 20)
    answered_s = (session.all("task_answer_received", after)[0][0] - pressed
                  if answer else None)
    result = await session.wait(lambda e: e["type"] == "task_result", 90, after)
    await session.quiet(20, start)
    channels = (session.ready.get("interaction") or {}).get("hitl", {}).get("channels")
    await phone.leave()
    await session.close()
    summary = await ctx.summary(session)
    calls = ctx.webhook.since(mark)
    checks = {
        "the session takes keys": channels == ["voice", "dtmf"] and summary.get("phone_leg") == "true",
        "the confirmation ends with the keypad hint": confirm.get("kind") == "confirm" and bool(hint),
        "key 1 accepts": bool(answer) and answer.get("outcome") == "accept" and answer.get("by") == "dtmf"
                         and answered_s is not None and answered_s <= KEY_ANSWER_LIMIT_S,
        "one booking, completed": len(calls) == 1 and bool(result) and result[2].get("state") == "completed",
    }
    measured = (f"confirm: hitl.channels {channels}, phone_leg {summary.get('phone_leg')}; task_question "
                f"{confirm.get('kind')!r} {confirm.get('prompt')!r}; hint said {bool(hint)}; key 1 -> "
                f"task_answer_received {answer} {ms(answered_s) if answered_s is not None else None} ms after "
                f"the key; booking calls {len(calls)}; task_result {result[2].get('state') if result else None!r}")
    return checks, measured


async def kd1_card(ctx: Ctx) -> Tuple[Dict[str, bool], str]:
    # The webhook stand-in asks for the card number (no model decides whether to ask).
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    block = {**MODEL_FRONT, "tasks": {"default_backend": "hook"}, "hitl": {"channels": ["voice", "dtmf"]}}
    session = await Session.open(ctx, "KD1-card", block, conversation=False, mic=False,
                                 port=stand_in.port, livekit={"room_name": f"kd1c-{ctx.run_id}"})
    phone = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "sip_caller", kind="sip").join()
    await session.warm()
    await asyncio.sleep(3.0)
    await settle(session, phone)
    start = len(session.events)
    await phone.say("59_pay_bill")
    asked = await session.wait(lambda e: e["type"] == "task_question", 40, start)
    prompt = await said(session, start, KEYPAD_CARD, 30)
    if prompt:
        await heard(session, prompt, 40)
    spoken_from = len(session.events)
    await phone.say("53_card_number")
    dont = await said(session, spoken_from, "Please don't say that out loud. Use your keypad instead.", 30)
    spoken_answer = session.all("task_answer_received", spoken_from)
    await session.quiet(20, spoken_from)
    keys_from = len(session.events)
    for digit in "9*4111#":
        code = {"#": 11, "*": 10}.get(digit) or int(digit)
        pressed = now()
        await phone.room.local_participant.publish_dtmf(code=code, digit=digit)
        await asyncio.sleep(0.2)
    typed_answer = await answer_received(session, keys_from, 20)
    answered_s = (session.all("task_answer_received", keys_from)[0][0] - pressed if typed_answer else None)
    result = await session.wait(lambda e: e["type"] in ("task_result", "task_cancelled"), 60, keys_from)
    await session.quiet(20, start)
    await phone.leave()
    await session.close()
    got = [hit["body"].get("value") for hit in stand_in.of("hook", "answer", mark)]
    checks = {
        "the card number is asked for on the keypad": bool(asked) and asked[2].get("kind") == "value"
                                                       and bool(prompt),
        "said aloud it is not taken": bool(dont) and not spoken_answer,
        "typed and ended with # it answers, redacted": bool(typed_answer)
            and (typed_answer.get("outcome"), typed_answer.get("by")) == ("accept", "dtmf")
            and typed_answer.get("value") is None and typed_answer.get("value_redacted") is True
            and answered_s is not None and answered_s <= KEY_ANSWER_LIMIT_S,
        "the backend gets the digits after *": got == ["4111"],
        "the task completes": bool(result) and result[2].get("state") == "completed",
    }
    measured = (f"card: task_question {asked[2].get('kind') if asked else None!r} "
                f"{asked[2].get('prompt') if asked else None!r}; keypad line said {bool(prompt)}; the number said "
                f"aloud -> {dont[2].get('delta') if dont else None!r}, answers {len(spoken_answer)}; keys "
                f"9 * 4 1 1 1 # -> task_answer_received {typed_answer} "
                f"{ms(answered_s) if answered_s is not None else None} ms after #; the webhook got {got}; then "
                f"{result[2].get('state') if result else None!r}")
    return checks, measured


async def kd1(ctx: Ctx):
    """DTMF on a phone line: key 1 confirms a booking; a card number is typed and ended with #."""
    try:
        import livekit.rtc  # noqa: F401
    except ImportError:
        raise RuntimeError("KD1 needs the LiveKit client: pip install livekit livekit-api")
    ctx.stack.ensure_livekit()
    parts = []
    for part in (kd1_confirm, kd1_card):
        try:
            parts.append(await part(ctx))
        except Exception as error:  # one part that cannot run fails, the other still reports
            parts.append(({f"{part.__name__} ran": False}, f"{part.__name__} did not run to the end: {error!r}"))
            for caller in list(ctx.rooms):
                await caller.leave()
    (confirm_checks, confirm_measured), (card_checks, card_measured) = parts
    report(ctx, "KD1", "a phone participant answers with keys", {**confirm_checks, **card_checks},
           f"{confirm_measured}; {card_measured}",
           "a room session with hitl.channels [voice, dtmf] echoes [voice, dtmf] and finds a phone "
           f"line; the booking's confirmation is followed by {KEYPAD_HINT!r}; key 1 gives "
           f"task_answer_received accept by dtmf within {KEY_ANSWER_LIMIT_S:.0f} s, one booking call and a "
           f"completed task; the card question is said as {KEYPAD_CARD!r}; keys 9 * 4 1 1 1 # give "
           f"task_answer_received accept by dtmf with value null and value_redacted true within "
           f"{KEY_ANSWER_LIMIT_S:.0f} s of #, the number said aloud first gets the keypad line and no answer, "
           "the webhook gets 4111, and the task completes",
           "the phone line is a participant with a token of kind sip that publishes SIP DTMF packets "
           "through the LiveKit server (no SIP trunk); the card question is the webhook stand-in's "
           "(backend gateway), so no model decides whether to ask")


async def lp1(ctx: Ctx):
    """A language pack is data: a session in its language runs the mode on its words; one without a pack does not."""
    packed = await Session.open(ctx, "LP1", MODEL_FRONT, PLAIN_PROMPT, language=TEST_PACK, mic=False)
    runs = bool(packed.ready.get("interaction"))
    codes = [(w.get("code"), (w.get("detail") or {}).get("reason")) for w in packed.warnings]
    await packed.warm()
    # A request in the pack's language (Spanish): the front reads it in Spanish, a task runs.
    request = await typed(packed, SPANISH[2], 60.0, reply=15.0)
    tasks = len(packed.all("task_created", request))
    request_lines = [e["delta"] for _, e in packed.transcripts(request) if e.get("delta")]
    request_warnings = [e.get("code") for _, e in packed.all("config_warning", request)]
    # English is another language than the pack's: the first such turn is asked again in the
    # pack's own words, the second steps down with the pack's notice (no word list reads the
    # language: the front does, owner 24 Oct).
    other = await typed(packed, "What time does the store open?", reply=15.0)
    other_lines = [e["delta"] for _, e in packed.transcripts(other) if e.get("delta")]
    asked = await typed(packed, "Can you book me a table for two tonight?", 30.0, reply=15.0)
    created = len(packed.all("task_created", other))
    asked_lines = [e["delta"] for _, e in packed.transcripts(asked) if e.get("delta")]
    stepped = [e.get("code") for _, e in packed.all("config_warning", asked)]
    await packed.close()
    without = await Session.open(ctx, "LP1-none", MODEL_FRONT, PLAIN_PROMPT, language="yy", mic=False)
    refused = [(w.get("code"), (w.get("detail") or {}).get("reason")) for w in without.warnings]
    await without.close()
    checks = {
        # The speech-to-text language mapper's own advisory about a code is not the mode's
        # (`language_unsupported_or_degraded`): only interaction codes count here.
        "a session in the pack's language runs the mode": runs
            and not [c for c, _ in codes if c and c.startswith("interaction")],
        "a request in the pack's language becomes a task": tasks >= 1 and not request_warnings,
        "the pack's own words are spoken": other_lines == [TEST_PACK_REPEAT],
        "a second turn in another language steps down with the pack's notice": created == 0
            and asked_lines == [TEST_PACK_NOTICE] and stepped == ["interaction_language_step_down"],
        "a language without a pack runs without the mode": not without.ready.get("interaction")
            and ("interaction_surface_unavailable", "language_unsupported") in refused,
    }
    report(ctx, "LP1", "language packs from the operator's directory", checks,
           f"language {TEST_PACK}: interaction {runs}, warnings {codes}; a Spanish request -> tasks {tasks}, lines "
           f"{request_lines}, warnings {request_warnings}; an English turn -> {other_lines}; a second English turn -> "
           f"tasks {created}, lines {asked_lines}, warnings {stepped}; language yy: interaction "
           f"{bool(without.ready.get('interaction'))}, warnings {refused}",
           f"stt language {TEST_PACK} (the Spanish test pack loaded from WAAV_INTERACTION_PACKS_DIR): ready.interaction "
           f"and no interaction_* warning; a typed Spanish request ({SPANISH[2]!r}) gets a task and no config_warning; "
           f"a typed English turn gets the pack's own repeat line {TEST_PACK_REPEAT!r}; a second typed English turn "
           f"gets the pack's notice {TEST_PACK_NOTICE!r}, config_warning interaction_language_step_down and no task "
           "from either English turn; stt language yy (no pack): no ready.interaction and "
           "interaction_surface_unavailable / language_unsupported",
           f"the {TEST_PACK} pack is a machine-written, unreviewed Spanish test pack this script writes into the run's "
           "log folder (owner, 25 Oct; never shipped); the turns are typed")


# ---- the Python SDK on a live session (SDK1)


SDK_PATH = REPO_GATEWAY.parent / "clients_sdk" / "python"


class SdkCall(Mic):
    """A `/ws` session opened with the Python SDK (`bud_waav`, from the repository): the
    microphone feeds `TalkSession.send_audio`, and every typed interaction event the SDK emits
    is kept with its receive time. Nothing here reads the socket itself."""

    def __init__(self, ctx: "Ctx", label: str, bud: Any, talk: Any):
        super().__init__(ctx)
        self.label, self.bud, self.talk = label, bud, talk
        self.events: List[Tuple[float, Any]] = []
        self.finals: List[Tuple[float, str]] = []
        # 1.1 frames the SDK did not type (they reach `server_message`): must stay empty.
        self.untyped: List[dict] = []
        self.errors: List[Any] = []
        self.warnings: List[Any] = []
        self.zombies: List[Any] = []
        self._task: Optional[asyncio.Task] = None
        talk.on("interaction", lambda event: self.events.append((now(), event)))
        talk.on("audio", lambda audio: self._heard(audio.audio))
        talk.on("transcript", lambda r: r.is_final and self.finals.append((now(), r.text)))
        talk.on("error", self.errors.append)
        talk.on("warning", self.warnings.append)
        talk._session.on("server_message", self._untyped)
        talk._session.on("zombie_detected", self.zombies.append)

    def _untyped(self, frame: dict):
        kind = frame.get("type")
        if kind in PROTOCOL_1_1 | {"audio_segment", "unverified_claim"} or (
                kind == "assistant_transcript" and "source" in frame):
            self.untyped.append(frame)

    @classmethod
    async def open(cls, ctx: "Ctx", label: str, interaction: Any, system_prompt: str) -> "SdkCall":
        if str(SDK_PATH) not in sys.path:
            sys.path.insert(0, str(SDK_PATH))
        from bud_waav import BudClient, ConversationConfig, STTConfig, TTSConfig

        bud = BudClient(base_url=f"http://127.0.0.1:{ctx.args.gateway_port}")
        talk = bud.talk.create(
            stt=STTConfig(provider="openai", model="whisper-1", language="en", sample_rate=CALLER_RATE),
            tts=TTSConfig(provider="openai", model="tts-1", voice_id=f"live-{ctx.run_id}-sdk",
                          audio_format="pcm", sample_rate=AGENT_RATE),
            conversation_config=ConversationConfig(
                base_url=f"http://127.0.0.1:{ctx.args.background_port}/v1",
                model=ctx.args.background_alias, api_key="local-dummy", system_prompt=system_prompt),
            interaction=interaction,
        )
        self = cls(ctx, label, bud, talk)
        self.log_mark = ctx.stack.log_size()
        await talk.connect(timeout=60)
        self.opened = now()
        self._task = asyncio.create_task(self._mic())
        return self

    async def _send_audio(self, frame: bytes):
        await self.talk.send_audio(frame)

    async def wait(self, kind: str, timeout: float, start: int = 0,
                   match: Callable[[Any], bool] = lambda _: True) -> Optional[Tuple[int, float, Any]]:
        deadline = now() + timeout
        while True:
            for index in range(start, len(self.events)):
                at, event = self.events[index]
                if event.type == kind and match(event):
                    return index, at, event
            if now() >= deadline:
                return None
            await asyncio.sleep(0.01)

    def of(self, kind: str, start: int = 0) -> List[Tuple[float, Any]]:
        return [(at, e) for at, e in self.events[start:] if e.type == kind]

    def lines(self, kind: str, start: int = 0) -> List[Tuple[float, Any]]:
        return [(at, e) for at, e in self.of("assistant_transcript", start) if e.delta and e.kind == kind]

    async def quiet(self, timeout: float, hold: float = 0.7) -> bool:
        """The floor is free and no agent audio came for `hold` seconds."""
        deadline = now() + timeout
        while now() < deadline:
            floors = self.of("floor_state")
            idle = not floors or floors[-1][1].holder == "none"
            if idle and now() - max(self.last_audio(), self.opened) > hold:
                return True
            await asyncio.sleep(0.05)
        return False

    async def close(self):
        self._mic_on = False
        if self._task:
            self._task.cancel()
        await self.talk.disconnect()
        await self.bud.close()
        out = Path(self.ctx.args.out_dir)
        out.mkdir(parents=True, exist_ok=True)
        with open(out / f"{self.label}.events.jsonl", "w") as handle:
            for at, event in self.events:
                handle.write(json.dumps({"rx_ms": ms(at - self.opened), "class": type(event).__name__,
                                         **event.model_dump(mode="json", by_alias=True)}) + "\n")


async def sdk1(ctx: Ctx):
    """The Python SDK on a live session: acknowledgement and relay, a confirmation answered with answer_task."""
    checks: Dict[str, bool] = {}
    # Thresholds, set before measuring. Relay: as L2 (task_created -> ack line and its audio ->
    # verbatim relay -> task_result completed, relay text == summary). Confirmation: a confirm
    # question that reads 2, `answer_task` -> accept by client within 20 s, exactly one booking
    # with party 2, the task listed by `list_tasks` and `get_task` with the booking as its effect,
    # `update_interaction` -> the policy value applied. Every 1.1 frame typed by the SDK; no
    # error frame; no SDK reconnect. Added after the first full run: 13 s of caller silence (over
    # the SDK's 12 s stale-inbound window) gives no `zombie_detected`.
    relay = await SdkCall.open(ctx, "SDK1-relay", MODEL_FRONT, slow_prompt())
    from bud_waav import (AssistantTranscript, InteractionUpdated, TaskAnswerReceived, TaskCreated,
                          TaskQuestion, TaskResult)
    await relay.warm()
    silent_from = len(relay.events)
    await asyncio.sleep(13.0)
    silent_frames = len(relay.events) - silent_from
    zombies_in_silence = len(relay.zombies)
    start = len(relay.events)
    said = await relay.say("02_bag_question")
    await relay.wait("task_result", 60, start)
    await asyncio.sleep(1.0)
    await relay.quiet(40)
    await relay.close()
    settings = relay.talk.interaction_settings or {}
    checks["ready: protocol 1.1 and the settings echo"] = (
        relay.talk._session.protocol_version == "1.1" and settings.get("enabled") is True
        and settings.get("tasks", {}).get("backend") == "llm")
    created = relay.of("task_created", start)
    acks = relay.lines("ack", start)
    relays = [(at, e) for at, e in relay.lines("relay", start) if e.verbatim]
    completed = [e for _, e in relay.of("task_result", start) if e.state == "completed"]
    relay_text = " ".join(e.delta for _, e in relays)
    summary = completed[0].summary if completed else ""
    ack_audio = relay.first_audible(acks[0][0] - 0.06) if acks else None
    checks["relay: task_created -> ack (audible) -> verbatim relay -> completed, relay == summary"] = bool(
        created and acks and relays and created[0][0] <= acks[0][0] < relays[0][0]
        and ack_audio is not None and ack_audio < relays[0][0] and completed and relay_text == summary)
    checks["relay: typed classes"] = (
        isinstance(created[0][1], TaskCreated) if created else False) and all(
        isinstance(e, AssistantTranscript) for _, e in relay.of("assistant_transcript"))
    e_at = next((at for at, _ in relay.finals if at >= said.start), None)

    mark = len(ctx.webhook.calls)
    confirm = await SdkCall.open(ctx, "SDK1-confirm", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await confirm.warm()
    start = len(confirm.events)
    await confirm.say("10_book_table")
    asked = await confirm.wait("task_question", 40, start)
    answered = done = None
    listed: list = []
    task = None
    if asked:
        question: Any = asked[2]
        await confirm.talk.answer_task(question.request_id, "accept")
        answered = await confirm.wait("task_answer_received", 20, asked[0],
                                      lambda e: e.request_id == question.request_id)
        done = await confirm.wait("task_result", 90, asked[0])
        await asyncio.sleep(1.0)
        await confirm.quiet(30)
        listed = await confirm.bud.list_tasks(stream_id=confirm.talk.stream_id)
        task = await confirm.bud.get_task(question.task_id, stream_id=confirm.talk.stream_id)
    mark_update = len(confirm.events)
    await confirm.talk.update_interaction({"policy": {"hold_after_silence_ms": 5000}})
    updated = await confirm.wait("interaction_updated", 10, mark_update)
    await confirm.close()
    calls = ctx.webhook.since(mark)
    party = [c["arguments"].get("party_size") for c in calls if isinstance(c["arguments"], dict)]
    q = asked[2] if asked else None
    a = answered[2] if answered else None
    r = done[2] if done else None
    checks["confirm: TaskQuestion kind confirm reading 2"] = (
        isinstance(q, TaskQuestion) and q.kind == "confirm" and says_number(q.prompt, "2", "two"))
    checks["confirm: answer_task -> TaskAnswerReceived accept by client"] = (
        isinstance(a, TaskAnswerReceived) and (a.outcome, a.by) == ("accept", "client"))
    # As in C1: how the task ends after the tool result is the background model's doing.
    checks["confirm: one booking, party 2, TaskResult"] = (
        len(calls) == 1 and party == [2] and isinstance(r, TaskResult) and r.state in ("completed", "failed"))
    checks["confirm: list_tasks and get_task show it with its effect"] = bool(
        q and [(t.id, t.state) for t in listed] == [(q.task_id, r.state if r else None)]
        and task is not None and task.result is not None
        and task.result.effects == ["booked a table for 2"] and task.stream_id == confirm.talk.stream_id)
    checks["update_interaction -> InteractionUpdated applied"] = bool(
        updated and isinstance(updated[2], InteractionUpdated)
        and updated[2].applied == ["policy.hold_after_silence_ms"])
    both = [relay, confirm]
    checks["13 s of silence: no zombie_detected"] = zombies_in_silence == 0
    checks["every 1.1 frame typed"] = all(not c.untyped for c in both)
    checks["no error frame"] = all(not c.errors for c in both)
    checks["no SDK reconnect"] = all(not c.zombies for c in both)

    failed = [name for name, ok in checks.items() if not ok]
    kinds = lambda c: sorted({e.type for _, e in c.events})
    ctx.add("SDK1", "the Python SDK on a live session", not failed,
            f"relay: E +{ms(created[0][0] - e_at) if created and e_at else None} ms task_created, ack "
            f"{acks[0][1].delta if acks else None!r} audible at E +{ms(ack_audio - e_at) if ack_audio and e_at else None} ms, "
            f"{len(relays)} relay sentence(s) {relay_text[:80]!r}, result {[e.state for _, e in relay.of('task_result', start)]}; "
            f"confirm: question {q.prompt if q else None!r}, answer {(a.outcome, a.by) if a else None} "
            f"{ms(answered[1] - asked[1]) if answered and asked else None} ms after the question, calls {party}, "
            f"result {r.state if r else None} effects {r.effects if r else None}, listed "
            f"{[(t.task_ref, t.state) for t in listed]}, get_task result {task.result.model_dump() if task and task.result else None}; "
            f"update {updated[2].applied if updated else None}; event types relay {kinds(relay)}, confirm {kinds(confirm)}; "
            f"warnings {[w.code for w in relay.warnings + confirm.warnings]}; 13 s silence: {silent_frames} "
            f"event(s), zombie_detected {zombies_in_silence}; untyped 1.1 frames {[c.untyped for c in both]}, "
            f"errors {[[str(e) for e in c.errors] for c in both]}, zombie_detected {[len(c.zombies) for c in both]}"
            + (f"; FAILED: {failed}" if failed else ""),
            "relay as L2; confirm question reads 2; answer_task -> accept by client; one booking with party 2; "
            "list_tasks/get_task show the task and its effect; update_interaction applied; 13 s of silence "
            "without zombie_detected; all 1.1 frames typed, no error frame, no SDK reconnect")


# ---- Provider fixes (slice fin-vendor-fixes)

# Thresholds of PF1, written before the first run. The fixes PF1's second gateway lists:
PF1_FIXES = ("gemini_tool_behavior,gemini_tool_call_cancellation,gemini_interaction_status,"
             "gemini_response_scheduling,deepgram_user_text,deepgram_tools,openai_realtime_default")
# From the stand-in's `toolCallCancellation` to the client's frame (`function_call_cancelled`,
# or `task_cancelled` in interaction mode). Loopback and one actor step; no model.
PF1_CANCEL_LIMIT_S = 2.0
# How long a session is watched for a frame that must not come.
PF1_QUIET_S = 2.5
PF1_TOOL = {"type": "function", "function": {"name": "book_table", "description": "Book a table.",
                                             "parameters": {"type": "object", "properties": {
                                                 "party": {"type": "integer"}}}}}


class VendorStandIn:
    """One loopback WebSocket server standing in for three vendors by path: `/gemini` (Gemini
    Live: answers `setup` with `setupComplete`, then withdraws a call: the client's `pf1-call`
    0.5 s later, or, when the setup declares `waav_delegate`, calls it as `pf1-delegate` and
    withdraws it 1 s later), `/deepgram` (Voice Agent: `Welcome` after `Settings`) and
    `/openai`. It records every JSON frame the gateway sent, and when it sent each withdrawal.
    On `/deepgram`, a `Settings` that declares `waav_delegate` gets DG1's script: the agent calls
    it, answers the call's response with a reply of its own, refuses the first gateway line as
    Deepgram does while it replies (`Warning` `INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH`), ends
    that reply 0.5 s later, and says every later line. A `Settings` whose prompt holds `DG2` gets
    the same refusal of the client's first typed line (mode off, fin-defects).
    Also a loopback HTTP port that accepts a request and never answers: an LLM that is still
    working when its task is cancelled."""

    def __init__(self):
        self.frames: List[Tuple[str, float, dict]] = []
        self.withdrawn: List[Tuple[str, float]] = []
        # DG1/DG2: when the stand-in ended the reply it refused a line in.
        self.reply_done: List[float] = []
        self.dg2_done: List[float] = []
        # SS1: when the stand-in reported the caller's turn it never answers.
        self.ss1_turns: List[float] = []
        # r3-s2s (20 Oct): HC1/SS2/SP1 marks, by name: when the stand-in did each step.
        self.marks: Dict[str, float] = {}
        # SP1: when the stand-in LLM was asked (the provisional run's request).
        self.llm_asked: List[float] = []
        self.llm_port = 0
        self.port = 0
        self.slow_port = 0

    async def start(self) -> "VendorStandIn":
        from websockets.asyncio.server import serve
        # The OpenAI client asks for the `realtime` subprotocol and needs it back; the others
        # ask for none.
        def subprotocol(_connection, offered):
            return "realtime" if "realtime" in offered else None
        self.server = await serve(self._connection, "127.0.0.1", 0, max_size=None,
                                  select_subprotocol=subprotocol)
        self.port = self.server.sockets[0].getsockname()[1]

        async def hang(reader, writer):
            try:
                await reader.read(65536)
                await asyncio.sleep(60)
            finally:
                writer.close()
        self.slow = await asyncio.start_server(hang, "127.0.0.1", 0)
        self.llm = await asyncio.start_server(self._llm, "127.0.0.1", 0)
        self.llm_port = self.llm.sockets[0].getsockname()[1]
        self.slow_port = self.slow.sockets[0].getsockname()[1]
        # EL1: ElevenLabs text-to-speech, for the fix gateways' ELEVENLABS_TTS_URL.
        self.elevenlabs = ElevenLabsStandIn()
        return self

    async def close(self):
        self.server.close()
        self.slow.close()
        self.llm.close()

    async def _llm(self, reader, writer):
        """SP1's background: an OpenAI-compatible chat endpoint that answers every request at
        once, streamed or not."""
        try:
            data = b""
            while b"\r\n\r\n" not in data:
                chunk = await reader.read(65536)
                if not chunk:
                    return
                data += chunk
            head, _, body = data.partition(b"\r\n\r\n")
            found = re.search(rb"(?i)content-length:\s*(\d+)", head)
            while found and len(body) < int(found[1]):
                chunk = await reader.read(65536)
                if not chunk:
                    break
                body += chunk
            self.llm_asked.append(now())
            text = "Paris is the capital of France."
            if b'"stream":true' in body.replace(b" ", b""):
                chunks = [{"id": "sp1", "object": "chat.completion.chunk", "created": 0, "model": "stand-in",
                           "choices": [{"index": 0, "delta": {"role": "assistant", "content": text},
                                        "finish_reason": None}]},
                          {"id": "sp1", "object": "chat.completion.chunk", "created": 0, "model": "stand-in",
                           "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}]
                payload = "".join(f"data: {json.dumps(c)}\n\n" for c in chunks) + "data: [DONE]\n\n"
                kind = "text/event-stream"
            else:
                payload = json.dumps({"id": "sp1", "object": "chat.completion", "created": 0, "model": "stand-in",
                                      "choices": [{"index": 0, "finish_reason": "stop",
                                                   "message": {"role": "assistant", "content": text}}]})
                kind = "application/json"
            out = payload.encode()
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: " + kind.encode() + b"\r\nContent-Length: "
                         + str(len(out)).encode() + b"\r\nConnection: close\r\n\r\n" + out)
            await writer.drain()
        except Exception:
            pass
        finally:
            writer.close()

    def mark(self) -> int:
        return len(self.frames)

    def since(self, mark: int, vendor: str) -> List[dict]:
        return [frame for kind, _, frame in self.frames[mark:] if kind == vendor]

    async def _connection(self, ws):
        path = ws.request.path
        vendor = next((v for v in ("gemini", "deepgram", "openai", "relay") if path.startswith(f"/{v}")), "?")
        jobs = []
        state: Dict[str, Any] = {"dg1": False, "dg2": False, "lines": 0}
        try:
            if vendor == "relay":
                # RL1: an OpenAI Realtime vendor behind the /v1/realtime relay.
                await ws.send(json.dumps({"type": "session.created", "event_id": "rl1-created", "session": {
                    "id": "sess_rl1", "object": "realtime.session", "type": "realtime",
                    "model": "gpt-realtime"}}))
            async for message in ws:
                if not isinstance(message, str):
                    continue
                frame = json.loads(message)
                self.frames.append((vendor, now(), frame))
                if vendor == "gemini" and "setup" in frame:
                    await ws.send(json.dumps({"setupComplete": {}}))
                    names = [d.get("name") for t in frame["setup"].get("tools", [])
                             for d in t.get("functionDeclarations", [])]
                    if "SP1" in json.dumps(frame["setup"]):
                        jobs.append(asyncio.create_task(self._gemini_sp1(ws)))
                    elif "SP2" in json.dumps(frame["setup"]):
                        jobs.append(asyncio.create_task(self._gemini_sp2(ws)))
                    elif "PF3" in json.dumps(frame["setup"]):
                        # PF3 (r3-misc): one call of the client's function, never withdrawn.
                        call = {"toolCall": {"functionCalls": [{"id": "pf3-call", "name": "book_table",
                                                                "args": {"party": 2}}]}}
                        jobs.append(asyncio.create_task(ws.send(json.dumps(call))))
                    else:
                        jobs.append(asyncio.create_task(self._gemini(ws, "waav_delegate" in names)))
                if vendor == "deepgram":
                    jobs.extend(await self._deepgram(ws, frame, state))
                if vendor == "openai":
                    jobs.extend(self._openai(ws, frame, state))
                if vendor == "relay":
                    jobs.extend(await self._relay(ws, frame, state))
        except Exception:
            pass
        finally:
            for job in jobs:
                job.cancel()

    async def _deepgram(self, ws, frame: dict, state: Dict[str, Any]) -> List[asyncio.Task]:
        kind = frame.get("type")
        send = lambda f: ws.send(json.dumps(f))
        if kind == "Settings":
            await send({"type": "Welcome", "request_id": "pf1"})
            await send({"type": "SettingsApplied"})
            names = [f.get("name") for f in frame["agent"]["think"].get("functions", [])]
            state["dg2"] = "DG2" in (frame["agent"]["think"].get("prompt") or "")
            if "SS2" in (frame["agent"]["think"].get("prompt") or ""):
                # SS2: the caller says "uh-huh" (left unanswered on purpose), then asks a
                # question; the agent answers neither.
                async def turns():
                    await asyncio.sleep(0.8)
                    await send({"type": "UserStartedSpeaking"})
                    await asyncio.sleep(0.6)
                    await send({"type": "ConversationText", "role": "user", "content": "Uh-huh."})
                    self.marks["ss2_continuer"] = now()
                    await asyncio.sleep(SS2_QUIET_S)
                    await send({"type": "UserStartedSpeaking"})
                    await asyncio.sleep(1.0)
                    await send({"type": "ConversationText", "role": "user",
                                "content": "What time does the store open?"})
                    self.marks["ss2_question"] = now()
                return [asyncio.create_task(turns())]
            if "SS1" in (frame["agent"]["think"].get("prompt") or ""):
                # SS1: the caller speaks and the agent never answers.
                async def turn():
                    await asyncio.sleep(0.8)
                    await send({"type": "UserStartedSpeaking"})
                    await asyncio.sleep(1.5)
                    await send({"type": "ConversationText", "role": "user",
                                "content": "What time does the store open?"})
                    self.ss1_turns.append(now())
                return [asyncio.create_task(turn())]
            if "waav_delegate" in names:
                state["dg1"] = True

                async def call():
                    await asyncio.sleep(0.5)
                    await send({"type": "FunctionCallRequest", "functions": [{
                        "id": "dg1-delegate", "name": "waav_delegate", "client_side": True,
                        "arguments": json.dumps({"request": "What is the capital of France?"})}]})
                return [asyncio.create_task(call())]
        elif kind == "FunctionCallResponse":
            await send({"type": "ConversationText", "role": "assistant", "content": "Let me look that up."})
            await send({"type": "AgentAudioDone"})
        elif kind == "InjectAgentMessage" and state["dg2"]:
            state["lines"] += 1
            if state["lines"] == 1:
                await send({"type": "Warning", "code": "INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH",
                            "description": "Ignored InjectAgentMessage because the agent is already in "
                                           "the middle of a reply."})

                async def end_reply():
                    await asyncio.sleep(0.5)
                    self.dg2_done.append(now())
                    await send({"type": "AgentAudioDone"})
                return [asyncio.create_task(end_reply())]
            await send({"type": "ConversationText", "role": "assistant", "content": frame.get("content", "")})
            await send({"type": "AgentAudioDone"})
        elif kind == "InjectAgentMessage" and state["dg1"]:
            state["lines"] += 1
            if state["lines"] == 1:
                await send({"type": "Warning", "code": "INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH",
                            "description": "Ignored InjectAgentMessage because the agent is already in "
                                           "the middle of a reply."})

                async def end_reply():
                    await asyncio.sleep(0.5)
                    await send({"type": "ConversationText", "role": "assistant", "content": "Still on it."})
                    self.reply_done.append(now())
                    await send({"type": "AgentAudioDone"})
                return [asyncio.create_task(end_reply())]
            words = frame.get("message") or frame.get("content") or ""
            await send({"type": "ConversationText", "role": "assistant", "content": words})
            await send({"type": "AgentAudioDone"})
        return []

    def _openai(self, ws, frame: dict, state: Dict[str, Any]) -> List[asyncio.Task]:
        """HC1: a session that asks for text replies only (the half-cascade) and whose
        instructions hold `HC1` hears the caller, writes a reply with a claim nobody backs,
        then a long reply the caller cuts in on."""
        session = frame.get("session") or {}
        if (frame.get("type") != "session.update" or state.get("hc1")
                or session.get("output_modalities") != ["text"] or "HC1" not in (session.get("instructions") or "")):
            return []
        state["hc1"] = True
        return [asyncio.create_task(self._half_cascade(ws, "hc1"))]

    async def _half_cascade(self, ws, label: str):
        """HC1, HC2: the caller's question, a reply with a claim nobody backs, then a long
        reply the caller cuts in on (marks `<label>_reply`, `<label>_long`, `<label>_cut`)."""
        send = lambda f: ws.send(json.dumps(f))

        async def reply(rid: str, text: str):
            await send({"type": "response.created", "response": {"id": rid, "object": "realtime.response",
                                                                 "status": "in_progress", "output": []}})
            part = {"response_id": rid, "item_id": f"{rid}-item", "output_index": 0, "content_index": 0}
            await send({"type": "response.output_text.delta", **part, "delta": text})
            await send({"type": "response.output_text.done", **part, "text": text})
            await send({"type": "response.done", "response": {"id": rid, "object": "realtime.response",
                                                              "status": "completed", "output": []}})

        async def turn(item: str, words: str):
            await send({"type": "input_audio_buffer.speech_started", "audio_start_ms": 0, "item_id": item})
            await asyncio.sleep(0.9)
            await send({"type": "input_audio_buffer.speech_stopped", "audio_end_ms": 900, "item_id": item})
            await send({"type": "conversation.item.input_audio_transcription.completed", "item_id": item,
                        "content_index": 0, "transcript": words})

        await asyncio.sleep(0.5)
        await turn(f"{label}-u1", "When do you open?")
        self.marks[f"{label}_reply"] = now()
        await reply(f"{label}-r1", HC1_REPLY)
        await asyncio.sleep(HC1_SECOND_S)
        await turn(f"{label}-u2", "Where are you?")
        self.marks[f"{label}_long"] = now()
        await reply(f"{label}-r2", HC1_LONG)
        await asyncio.sleep(HC1_CUT_AFTER_S)
        self.marks[f"{label}_cut"] = now()
        await send({"type": "input_audio_buffer.speech_started", "audio_start_ms": 0, "item_id": f"{label}-u3"})
        await asyncio.sleep(0.8)
        await send({"type": "input_audio_buffer.speech_stopped", "audio_end_ms": 800, "item_id": f"{label}-u3"})

    async def _relay(self, ws, frame: dict, state: Dict[str, Any]) -> List[asyncio.Task]:
        """RL1: answers each `session.update` as the vendor does; once the instructions name
        RL1, reports a caller's turn and never answers it."""
        if frame.get("type") != "session.update":
            return []
        session = frame.get("session") or {}
        await ws.send(json.dumps({"type": "session.updated", "event_id": f"rl1-{os.urandom(3).hex()}",
                                  "session": {"type": "realtime", **session}}))
        # HC2 (r4-left): the relay's half-cascade asks for text replies only.
        if (not state.get("hc2") and "HC2" in (session.get("instructions") or "")
                and session.get("output_modalities") == ["text"]):
            state["hc2"] = True
            return [asyncio.create_task(self._half_cascade(ws, "hc2"))]
        if state.get("rl1") or "RL1" not in json.dumps(session):
            return []
        state["rl1"] = True

        async def turn():
            await asyncio.sleep(1.0)
            await ws.send(json.dumps({"type": "input_audio_buffer.speech_started", "audio_start_ms": 0,
                                      "item_id": "rl1-u1"}))
            await asyncio.sleep(1.0)
            await ws.send(json.dumps({"type": "input_audio_buffer.speech_stopped", "audio_end_ms": 1000,
                                      "item_id": "rl1-u1"}))
            self.marks["rl1_turn"] = now()
            await ws.send(json.dumps({"type": "conversation.item.input_audio_transcription.completed",
                                      "item_id": "rl1-u1", "content_index": 0,
                                      "transcript": "What time does the store open?"}))
        return [asyncio.create_task(turn())]

    async def _gemini_sp1(self, ws):
        """SP1: the provider calls waav_delegate while its turn is open, then ends the turn."""
        await asyncio.sleep(0.5)
        await ws.send(json.dumps({"toolCall": {"functionCalls": [{
            "id": "sp1-delegate", "name": "waav_delegate",
            "args": {"request": "What is the capital of France?"}}]}}))
        self.marks["sp1_call"] = now()
        await asyncio.sleep(SP1_TURN_S)
        self.marks["sp1_turn"] = now()
        await ws.send(json.dumps({"serverContent": {"turnComplete": True}}))

    async def _gemini_sp2(self, ws):
        """SP2 (r4-left): a delegate call, then the caller interrupts the provider's turn
        (`interrupted`, no `turnComplete`); later a second call, interrupted too and withdrawn
        150 ms after."""
        call = lambda id_, words: ws.send(json.dumps({"toolCall": {"functionCalls": [{
            "id": id_, "name": "waav_delegate", "args": {"request": words}}]}}))
        await asyncio.sleep(0.5)
        await call("sp2-a", "What is the capital of France?")
        await asyncio.sleep(0.4)
        self.marks["sp2_interrupted"] = now()
        await ws.send(json.dumps({"serverContent": {"interrupted": True}}))
        await asyncio.sleep(SP2_GAP_S)
        await call("sp2-b", "And the capital of Spain?")
        await asyncio.sleep(0.4)
        self.marks["sp2_interrupted_b"] = now()
        await ws.send(json.dumps({"serverContent": {"interrupted": True}}))
        await asyncio.sleep(0.15)
        self.marks["sp2_withdrawn"] = now()
        await ws.send(json.dumps({"toolCallCancellation": {"ids": ["sp2-b"]}}))

    async def _gemini(self, ws, delegate: bool):
        if delegate:
            await asyncio.sleep(0.5)
            await ws.send(json.dumps({"toolCall": {"functionCalls": [{
                "id": "pf1-delegate", "name": "waav_delegate",
                "args": {"request": "Book a table for two at seven."}}]}}))
            await asyncio.sleep(1.0)
            call = "pf1-delegate"
        else:
            await asyncio.sleep(0.5)
            call = "pf1-call"
        self.withdrawn.append((call, now()))
        await ws.send(json.dumps({"toolCallCancellation": {"ids": [call]}}))


async def rt_frames(port: int, config: dict, seconds: float, then: Optional[dict] = None
                    ) -> List[Tuple[float, dict]]:
    """The JSON frames a `/realtime` client gets for `seconds` after its config, with when
    each came; `then` is sent once `session_created` is in."""
    out: List[Tuple[float, dict]] = []
    async with websockets.connect(f"ws://127.0.0.1:{port}/realtime", max_size=None,
                                  ping_interval=None) as ws:
        await ws.send(json.dumps(config))
        deadline = now() + seconds
        while now() < deadline:
            try:
                frame = await asyncio.wait_for(ws.recv(), max(0.05, deadline - now()))
            except asyncio.TimeoutError:
                break
            except Exception:
                break
            if not isinstance(frame, str):
                continue
            event = json.loads(frame)
            out.append((now(), event))
            if then is not None and event.get("type") == "session_created":
                await ws.send(json.dumps(then))
                then = None
    return out


async def pf1_gateway(ctx: Ctx, vendor: VendorStandIn, port: int) -> dict:
    """One gateway's sessions: Gemini with a client tool, Deepgram typed text, OpenAI with no
    model, and a Gemini interaction session whose delegate call the vendor withdraws."""
    out: Dict[str, Any] = {}
    mark = vendor.mark()
    gemini = await rt_frames(port, {"type": "config", "provider": "gemini", "model": "gemini-3.8-live",
                                    "tools": [PF1_TOOL]}, 0.5 + PF1_QUIET_S)
    setups = [f["setup"] for f in vendor.since(mark, "gemini") if "setup" in f]
    out["behavior"] = [d.get("behavior") for t in (setups[0].get("tools", []) if setups else [])
                       for d in t.get("functionDeclarations", [])]
    out["withdrawn"] = [e for _, e in gemini if e.get("type") == "function_call_cancelled"]
    sent_at = next((at for call, at in reversed(vendor.withdrawn) if call == "pf1-call"), None)
    got_at = next((at for at, e in gemini if e.get("type") == "function_call_cancelled"), None)
    out["withdrawn_s"] = got_at - sent_at if got_at is not None and sent_at is not None else None
    mark = vendor.mark()
    await rt_frames(port, {"type": "config", "provider": "deepgram", "tools": [PF1_TOOL]}, 2.0,
                    then={"type": "text", "text": "Is it open?"})
    out["typed"] = [f for f in vendor.since(mark, "deepgram")
                    if f.get("type") in ("InjectAgentMessage", "InjectUserMessage")]
    settings = [f for f in vendor.since(mark, "deepgram") if f.get("type") == "Settings"]
    out["dg_functions"] = [f.get("name") for f in (settings[0]["agent"]["think"].get("functions", [])
                                                   if settings else [])]
    openai = await rt_frames(port, {"type": "config", "provider": "openai"}, 1.5)
    created = [e for _, e in openai if e.get("type") == "session_created"]
    out["openai_model"] = created[0].get("model") if created else None
    out["openai_errors"] = [e for _, e in openai if e.get("type") == "error"]
    # Interaction mode: the delegate call becomes a task on an LLM that never answers.
    mark = vendor.mark()
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{vendor.slow_port}/v1",
                               "model": "never", "api_key": "local-dummy"}}}
    ix = await rt_frames(port, {"type": "config", "provider": "gemini", "model": "gemini-3.8-live",
                                "interaction": block}, 1.5 + PF1_QUIET_S + 1.0)
    created = [e for _, e in ix if e.get("type") == "session_created"]
    out["regime"] = ((created[0].get("interaction") or {}).get("regime") if created else None)
    setups = [f["setup"] for f in vendor.since(mark, "gemini") if "setup" in f]
    out["ix_behavior"] = {d.get("name"): d.get("behavior")
                          for t in (setups[0].get("tools", []) if setups else [])
                          for d in t.get("functionDeclarations", [])}
    out["ix_types"] = [e.get("type") for _, e in ix]
    out["task_cancelled"] = [e for _, e in ix if e.get("type") == "task_cancelled"]
    sent_at = next((at for call, at in reversed(vendor.withdrawn) if call == "pf1-delegate"), None)
    got_at = next((at for at, e in ix if e.get("type") == "task_cancelled"), None)
    out["cancel_s"] = got_at - sent_at if got_at is not None and sent_at is not None else None
    out["ix_withdrawn_frames"] = [e for _, e in ix if e.get("type") == "function_call_cancelled"]
    out["delegate_result"] = [f for f in vendor.since(mark, "gemini")
                              if any(r.get("id") == "pf1-delegate"
                                     for r in (f.get("toolResponse") or {}).get("functionResponses", []))]
    return out


_VENDOR: Optional[VendorStandIn] = None


async def vendor_stand_in() -> VendorStandIn:
    """The one stand-in vendor of the run: the fix gateways point at its port for good."""
    global _VENDOR
    if _VENDOR is None:
        _VENDOR = await VendorStandIn().start()
    return _VENDOR


async def pf1(ctx: Ctx):
    """Provider fixes: built and off; listed by the operator, the vendors get the documented frames."""
    vendor = await vendor_stand_in()
    off_port, on_port = ctx.stack.ensure_fix_gateways(vendor.port)
    off = await pf1_gateway(ctx, vendor, off_port)
    on = await pf1_gateway(ctx, vendor, on_port)
    in_time = lambda s: s is not None and s <= PF1_CANCEL_LIMIT_S
    checks = {
        "off: Gemini declarations carry no behavior": off["behavior"] == [None],
        "off: a withdrawn call is not passed on": not off["withdrawn"],
        # deepgram_user_text passed its probe and is on by default (gap-lifecycle, 19 Oct).
        "off: Deepgram typed text is InjectUserMessage {content} (on by default)":
            off["typed"] == [{"type": "InjectUserMessage", "content": "Is it open?"}],
        "off: OpenAI's empty model is gpt-realtime": off["openai_model"] == "gpt-realtime",
        # deepgram_tools is on by default since its probe through the gateway (r3-misc, 20 Oct).
        "off: Deepgram gets the session's function (on by default)": off["dg_functions"] == ["book_table"],
        "on: Deepgram gets the session's function": on["dg_functions"] == ["book_table"],
        "off: interaction mode keeps a withdrawn delegate call's task":
            off["regime"] == "delegating" and "task_created" in off["ix_types"] and not off["task_cancelled"],
        "on: the client's function blocks": on["behavior"] == ["BLOCKING"],
        "on: the withdrawn call reaches the client in time":
            on["withdrawn"] == [{"type": "function_call_cancelled", "call_id": "pf1-call"}]
            and in_time(on["withdrawn_s"]),
        "on: Deepgram typed text is InjectUserMessage {content}":
            on["typed"] == [{"type": "InjectUserMessage", "content": "Is it open?"}],
        "on: OpenAI's empty model is gpt-realtime-2.1":
            on["openai_model"] == "gpt-realtime-2.1" and not on["openai_errors"],
        "on: the delegate function is non-blocking":
            on["ix_behavior"].get("waav_delegate") == "NON_BLOCKING",
        # r3-s2s (20 Oct): with the withdrawal read, a delegate call is provisional until the
        # provider's turn ends (plan chapter 6, provider speculation): one withdrawn before is
        # never seen. Was: its task_created, then task_cancelled by system.
        "on: a delegate call withdrawn before the provider's turn ended is never seen: no task event, no result":
            on["regime"] == "delegating" and "task_created" not in on["ix_types"] and not on["task_cancelled"]
            and not on["ix_withdrawn_frames"] and not on["delegate_result"],
    }
    def brief(r: dict) -> str:
        fmt = lambda s: None if s is None else ms(s)
        return (f"behavior {r['behavior']}, withdrawn {r['withdrawn']} {fmt(r['withdrawn_s'])} ms, typed "
                f"{r['typed']}, deepgram functions {r['dg_functions']}, openai model {r['openai_model']!r}, regime {r['regime']!r}, delegate "
                f"behavior {r['ix_behavior'].get('waav_delegate')!r}, interaction frames {r['ix_types']}, "
                f"task_cancelled {r['task_cancelled']} {fmt(r['cancel_s'])} ms, delegate results "
                f"{len(r['delegate_result'])}")
    report(ctx, "PF1", "provider fixes off by default, on by the operator's list", checks,
           f"off: {brief(off)}; on: {brief(on)}",
           f"off gateway: today's frames (no behavior, InjectUserMessage and the Deepgram function by default, gpt-realtime, nothing passed on, "
           f"the task stays). On gateway ({PF1_FIXES}): BLOCKING / NON_BLOCKING, InjectUserMessage, "
           f"gpt-realtime-2.1, function_call_cancelled and task_cancelled (by system) each within "
           f"{PF1_CANCEL_LIMIT_S} s of the vendor's toolCallCancellation, no result on a withdrawn call; "
           f"no unwanted frame within {PF1_QUIET_S} s",
           "the vendors are a loopback stand-in speaking the documented frames; no vendor is dialled. "
           "Nova (Bedrock), Ultravox's create-call body and OpenAI's speech-to-text default have no "
           "stand-in here and are covered by unit tests; interactionStatus has no observable effect on "
           "a Gemini session (it has no gateway lines) and is covered by the executor's test")


# ---- Deepgram Voice Agent in interaction mode (slice fin-deepgram)

# Thresholds of DG1, written before the first run: from the stand-in ending the reply it refused
# a line in to the gateway sending that line again (loopback, one executor step).
DG1_RESEND_LIMIT_S = 1.0


async def dg1(ctx: Ctx):
    """Deepgram Voice Agent, interaction mode: the delegate function reaches the agent, a call's answer names its function, and a line the agent refused while replying goes out again after that reply."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    mark = vendor.mark()
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{ctx.args.background_port}/v1",
                               "model": ctx.args.background_alias, "api_key": "local-dummy"}}}
    frames = await rt_frames(off_port, {"type": "config", "provider": "deepgram",
                                        "think_provider": "anthropic", "model": "claude-haiku-4-5",
                                        "language": "en", "interaction": block}, 15.0)
    sent = vendor.since(mark, "deepgram")
    settings = next((f for f in sent if f.get("type") == "Settings"), {})
    think = (settings.get("agent") or {}).get("think", {})
    answers = [f for f in sent if f.get("type") == "FunctionCallResponse"]
    lines = [(at, f.get("message") or f.get("content")) for v, at, f in vendor.frames[mark:]
             if v == "deepgram" and f.get("type") == "InjectAgentMessage"]
    refused = lines[0][1] if lines else None
    again = [at for at, words in lines[1:] if words == refused]
    done = vendor.reply_done[0] if vendor.reply_done else None
    resend_s = again[0] - done if again and done is not None else None
    types = [e.get("type") for _, e in frames]
    created = next((e for _, e in frames if e.get("type") == "session_created"), {})
    checks = {
        "interaction mode on, regime B": (created.get("interaction") or {}).get("regime") == "provider_speech",
        "the think model's vendor and the delegate function reach the agent":
            think.get("provider") == {"type": "anthropic", "model": "claude-haiku-4-5"}
            and [f.get("name") for f in think.get("functions", [])] == ["waav_delegate"],
        "the call's answer names its function":
            len(answers) == 1 and answers[0].get("id") == "dg1-delegate" and answers[0].get("name") == "waav_delegate",
        "the task ran": "task_created" in types and "task_result" in types,
        "the refused line goes out again after the agent's reply, in time":
            resend_s is not None and 0 <= resend_s <= DG1_RESEND_LIMIT_S,
        "every line was said once accepted": len(lines) >= 2,
    }
    report(ctx, "DG1", "Deepgram Voice Agent: delegate function, named answer, a refused line sent again", checks,
           f"think {think.get('provider')}, functions {[f.get('name') for f in think.get('functions', [])]}, "
           f"answers {answers}, lines sent {[w for _, w in lines]}, resent {None if resend_s is None else ms(resend_s)} "
           f"ms after the agent's reply ended, client events {types}",
           f"Settings name think provider anthropic and carry waav_delegate (without the deepgram_tools fix); the "
           f"FunctionCallResponse carries id and name; the line the stand-in refused (Deepgram's warning) is sent "
           f"again within {DG1_RESEND_LIMIT_S} s of the AgentAudioDone that ends the agent's reply; task_created "
           f"and task_result reach the client",
           "the vendor is the loopback stand-in speaking the frames probed on Deepgram (2026-10-15); the live "
           "Deepgram runs are tests/live_testing/scripts/deepgram_matrix.py")


# Thresholds of DG2, written before the first run: as DG1's (loopback, the protocol's own step).
DG2_RESEND_LIMIT_S = 1.0


async def dg2(ctx: Ctx):
    """Deepgram Voice Agent with the mode off (fin-defects): Deepgram's warning reaches the client as a warning, not an error; the client's typed line it refused while replying goes out again after that reply; a Flux listen model gets no agent.language."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    mark = vendor.mark()
    # The client's typed text as the agent's line (deepgram_user_text, on by default since its
    # probe, turned off for this session): the frame Deepgram refuses mid-reply.
    frames = await rt_frames(off_port, {"type": "config", "provider": "deepgram", "model": "gpt-4o-mini",
                                        "language": "en", "transcription_model": "flux-general-multi",
                                        "instructions": "DG2: a phone assistant.",
                                        "provider_fixes": {"deepgram_user_text": False}}, 4.0,
                             then={"type": "text", "text": "Our kitchen closes at ten."})
    sent = vendor.since(mark, "deepgram")
    settings = next((f for f in sent if f.get("type") == "Settings"), {})
    agent = settings.get("agent") or {}
    lines = [(at, f) for v, at, f in vendor.frames[mark:] if v == "deepgram" and f.get("type") == "InjectAgentMessage"]
    done = vendor.dg2_done[0] if vendor.dg2_done else None
    again = [at for at, f in lines[1:] if lines and f == lines[0][1]]
    resend_s = again[0] - done if again and done is not None else None
    warnings = [e for _, e in frames if e.get("type") == "warning"]
    errors = [e for _, e in frames if e.get("type") == "error"]
    checks = {
        "Flux listen: no agent.language": (agent.get("listen") or {}).get("provider", {}).get("model")
            == "flux-general-multi" and "language" not in agent,
        "the warning reaches the client as a warning": len(warnings) == 1
            and warnings[0].get("code") == "provider_warning"
            and (warnings[0].get("detail") or {}).get("provider_code") == "INJECT_AGENT_MESSAGE_DURING_AGENT_SPEECH",
        "no error": not errors,
        "the refused line goes out again after the agent's reply, in time":
            resend_s is not None and 0 <= resend_s <= DG2_RESEND_LIMIT_S,
        "sent twice, no more": len(lines) == 2,
    }
    report(ctx, "DG2", "Deepgram Voice Agent, mode off: a warning, the refused typed line sent again, Flux listen", checks,
           f"agent keys {sorted(agent)}, lines sent {len(lines)}, resent "
           f"{None if resend_s is None else ms(resend_s)} ms after the reply ended, warnings {warnings}, errors {errors}",
           f"Settings with a Flux listen model carry no agent.language; Deepgram's Warning arrives as `warning` "
           f"`provider_warning` with the vendor's code, no `error`; the typed line is sent again, once, within "
           f"{DG2_RESEND_LIMIT_S} s of the AgentAudioDone that ends the reply",
           "the vendor is the loopback stand-in speaking the frames probed on Deepgram (2026-10-15/17); the live "
           "Deepgram runs are tests/live_testing/scripts/deepgram_matrix.py")


# ---- voice experience: pace, listening-sound intensity, the capture window, the echo keys, the
# front model's server

PACE_LINE = "Your table for two is booked for seven tonight, by the window, as you asked."
# 1.15 / 0.85 = 1.35: the same line at `slower` lasts at least this many times as long as at `faster`.
PACE_RATIO_MIN = 1.2
PACE_RATIO_MAX = 1.6


async def pace_line(session: Session) -> float:
    """Has the agent say PACE_LINE; returns how many seconds of its audio the client got."""
    start = len(session.events)
    await session.send({"type": "speak", "text": PACE_LINE})
    line = await spoken(session, start, "notice", 20)
    await heard(session, line, 40)
    await session.quiet(20, start)
    received = sum(len(data) for at, data in session.audio if at >= line[1] - 0.06)
    return received / 2 / AGENT_RATE


async def pc1(ctx: Ctx):
    """The caller's pace sets the speaking rate mid-session: the same line is longer at slower than at faster."""
    block = {"front": {"profile": "model", "caller": {"pace": "slower"}}}
    session = await Session.open(ctx, "PC1", block, PLAIN_PROMPT)
    await session.warm()
    slow = await pace_line(session)
    start = len(session.events)
    await session.send({"type": "interaction_update", "patch": {"front": {"caller": {"pace": "faster"}}}})
    updated = await session.wait(lambda e: e["type"] == "interaction_updated", 10, start)
    fast = await pace_line(session)
    await session.close()
    rates = ctx.stack.log_since("interaction speaking rate", session.log_mark)
    factors = [dict(re.findall(r"(\w+)=(\S+)", line)) for line in rates]
    # The factor is logged as the f32 it is (0.8500000238418579).
    taken = [(round(float(f.get("factor", "nan")), 2), f.get("taken")) for f in factors]
    ratio = slow / fast if fast else 0.0
    checks = {
        "the update is applied": bool(updated) and "front.caller.pace" in updated[2].get("applied", []),
        "the provider took both rates": taken == [(0.85, "true"), (1.15, "true")],
        "the line is longer at slower": PACE_RATIO_MIN <= ratio <= PACE_RATIO_MAX,
    }
    report(ctx, "PC1", "the caller's pace changes the speaking rate", checks,
           f"the same line: {slow:.2f} s at slower, {fast:.2f} s at faster (ratio {ratio:.2f}); rates set "
           f"{taken}; interaction_updated applied {updated[2].get('applied') if updated else None}",
           f"interaction_updated applies front.caller.pace; the gateway's debug log `interaction speaking "
           f"rate` shows factor 0.85 then 1.15, both taken; the audio of the line at slower is "
           f"{PACE_RATIO_MIN} to {PACE_RATIO_MAX} times as long as at faster (expected 1.15 / 0.85 = 1.35)",
           "the local speech server is Piper behind the OpenAI-compatible `speed` field")
    return session


async def bc2(ctx: Ctx):
    """Listening-sound intensity: `low` plays at most one in BC1's long turn, `off` none."""
    story, _ = long_story(ctx)
    found = {}
    for level in ("medium", "low", "off"):
        block = {**MODEL_FRONT, "duplex": {"backchannel": {"intensity": level}}}
        session = await Session.open(ctx, f"BC2-{level}", block, PLAIN_PROMPT)
        await session.warm()
        duplex = (session.ready.get("interaction") or {}).get("duplex", {})
        turn = await session.turn("story", story)
        starts = backchannel_starts(session, turn.said.start, (turn.e or turn.said.end) + 3)
        found[level] = (duplex.get("backchannel_intensity"),
                        [round(at - turn.said.start, 2) for at, _ in starts])
        await session.close()
    checks = {
        "the sessions echo their intensity": [found[level][0] for level in found] == ["medium", "low", "off"],
        "medium: the turn had a pause for one": len(found["medium"][1]) >= 1,
        "low: one in the turn": len(found["low"][1]) == 1,
        "off: none": not found["off"][1],
    }
    report(ctx, "BC2", "listening-sound intensity", checks,
           f"BC1's long turn: medium -> starts at {found['medium'][1]} s, low -> {found['low'][1]} s, "
           f"off -> {found['off'][1]} s",
           "`ready.interaction.duplex.backchannel_intensity` echoes the level; with medium (the control: "
           "the turn had an eligible pause in this run) at least one `audio_segment` of lane backchannel "
           "starts in the turn, with low exactly one (one a turn, after 1 s of speech), with off none",
           "Smart Turn must run in the story's 200 ms comma pauses, which a loaded machine misses (then "
           "the control fails); the 40 ms fade is measured by the actor test")
    return session


async def tp1(ctx: Ctx):
    """thinking_pauses_are_held: a long turn whose comma pauses are 1.2 s, each with the recogniser's final in it, is one turn: nothing is said and no task starts before the caller has finished."""
    story = paused_story(ctx, 1.2)
    session = await Session.open(ctx, "TP1", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    start = len(session.events)
    said = await session.say("story", story)
    # A turn held through its pauses ends a pause after the story: the agent's reply (or its
    # task) comes after that, not within `quiet`'s 0.7 s of silence after the last word.
    await session.wait(lambda e: e["type"] == "task_created"
                       or (e["type"] == "assistant_transcript" and e.get("delta")), 15, start, said.last_voiced)
    await session.quiet(60, start)
    finals = [at for at, e in session.all("stt_result", start) if e.get("is_final")]
    early_lines = [(round(at - said.start, 2), e.get("kind"), e.get("delta"))
                   for at, e in session.transcripts(start) if at < said.last_voiced]
    tasks = session.all("task_created", start)
    early_tasks = [round(at - said.start, 2) for at, _ in tasks if at < said.last_voiced]
    title = tasks[0][1].get("title", "") if tasks else ""
    lines = [(round(at - said.last_voiced, 2), e.get("kind")) for at, e in session.transcripts(start)]
    # A task title is cut at 60 characters, so the turn's words are read from the gateway's
    # `voice.turn` debug span (`bud.voice.transcript`) of this session.
    turn_words = [m.group(1) for line in ctx.stack.log_lines("bud.voice.transcript=", session.stream_id)
                  if (m := re.search(r'bud\.voice\.transcript="([^"]*)"', line))]
    await session.close()
    checks = {
        "the recogniser ended segments inside the pauses": len([at for at in finals if at < said.last_voiced]) >= 2,
        "no agent line before the caller finished": not early_lines,
        "no task before the caller finished": not early_tasks,
        "one task with the words of the whole turn": len(tasks) == 1 and "spring" in title
            and any("spring" in words and "beach" in words for words in turn_words),
        "the turn is answered": bool(lines),
    }
    report(ctx, "TP1", "thinking pauses are held", checks,
           f"story {said.last_voiced - said.start:.1f} s with 1.2 s pauses; finals at "
           f"{[round(at - said.start, 2) for at in finals]} s; tasks {len(tasks)} (early {early_tasks}), title "
           f"{title!r}; turn words {turn_words}; lines before the end {early_lines}; lines after the caller's last "
           f"word (s, kind) {lines}",
           "the local streaming recogniser sends a final after 500 ms of silence, so at least two come before "
           "the caller's last voiced moment; no `assistant_transcript` with words and no `task_created` before "
           "that moment; exactly one `task_created`, whose title holds 'spring' (first phrase), and one turn "
           "of the gateway holding 'spring' and 'beach' (last phrase); the turn gets an answer",
           "Smart Turn must say the comma pauses are not the end of the turn (BC1's listening sounds at the "
           "same pauses need the same verdict); a pause it judges finished ends the turn there")
    return session


# CL1's second voice: Piper en_GB-alan-medium (huggingface.co/rhasspy/piper-voices, MIT).
CL1_VOICES = {"lessac": None, "alan": "models/piper-alan/en_GB-alan-medium.onnx"}


def similarity(a: bytes, b: bytes) -> float:
    """The peak normalised cross-correlation of two PCM16 clips: 1.0 when one holds the other."""
    x = np.frombuffer(a[: len(a) // 2 * 2], dtype="<i2").astype(np.float64)
    y = np.frombuffer(b[: len(b) // 2 * 2], dtype="<i2").astype(np.float64)
    if not x.size or not y.size:
        return 0.0
    n = x.size + y.size
    peak = np.max(np.abs(np.fft.irfft(np.fft.rfft(x, n) * np.conj(np.fft.rfft(y, n)), n)))
    return float(peak / (np.linalg.norm(x) * np.linalg.norm(y) + 1e-9))


def paused_story(ctx: Ctx, pause_s: float = 0.9) -> bytes:
    """BC1's long turn with each comma pause stretched to `pause_s`: Smart Turn on this debug
    build answers 260 to 390 ms after the caller stops, after BC1's next phrase began."""
    pcm, spans = long_story(ctx)
    at = lambda t: int(t * CALLER_RATE) * 2
    out, cursor = bytearray(), 0
    for (_, end), (begin, _) in zip(spans, spans[1:]):
        middle = at((end + begin) / 2)
        out += pcm[cursor:middle] + bytes(at(max(0.0, pause_s - (begin - end))))
        cursor = middle
    return bytes(out + pcm[cursor:])


async def cl1(ctx: Ctx):
    """listening_sounds_in_two_voices: each Piper voice's clips are checked before they enter the bank and play in that voice; a recorded clip replaces the rendered one."""
    router: Optional[VoiceRouter] = ctx.stack.voices
    if router is None or "gateway" not in ctx.stack.started:
        raise RuntimeError("CL1 needs the gateway this run starts (--start-stack), behind the voice router")
    clips_dir = ctx.stack.log_dir / "clips"
    story = paused_story(ctx)
    found: Dict[str, dict] = {}
    for name in CL1_VOICES:
        suffix = f"-cl1{name}"
        voice = f"live-{ctx.run_id}{suffix}"
        recording = b""
        if name == "alan":
            # The operator's recording of "mhm" for this voice: Piper's "hmm" (22.05 kHz).
            pcm, rate = router.piper_pcm("cl1alan", "hmm")
            folder = clips_dir / "openai" / voice / "en"
            folder.mkdir(parents=True, exist_ok=True)
            with wave.open(str(folder / "mhm.wav"), "wb") as wav:
                wav.setnchannels(1)
                wav.setsampwidth(2)
                wav.setframerate(rate)
                wav.writeframes(pcm)
            recording = resample(pcm, rate, AGENT_RATE)
        # The streaming recogniser would end the turn at the first stretched pause (its final
        # comes after 500 ms of silence): it finalises later, as in the eager scenarios.
        features = SLOW_FINAL if ctx.args.stt == "streaming" else None
        session = await Session.open(ctx, f"CL1-{name}", MODEL_FRONT, PLAIN_PROMPT, voice=suffix,
                                     stt_features=features)
        await session.warm()
        sid = session.stream_id
        deadline = now() + 30
        while now() < deadline and not ctx.stack.log_lines("interaction listening sounds ready", sid):
            await asyncio.sleep(0.2)
        ready = ctx.stack.log_lines("interaction listening sounds ready", sid)
        clips = int(re.search(r"clips=(\d+)", ready[0]).group(1)) if ready else -1
        refused = [re.search(r'text=Some\("([^"]*)"\).*reason=(.*)$', line)
                   for line in ctx.stack.log_lines("interaction listening sound refused", sid)]
        refused = [(m.group(1), m.group(2).strip()) for m in refused if m]
        # What the request-response recogniser heard in each clip (none on a streaming one); the
        # line names no session, and the sessions run one after the other.
        heard = [re.sub(r'.*text="([^"]*)" heard="([^"]*)".*', r"\1: \2", line)
                 for line in ctx.stack.log_since("interaction listening sound heard", session.log_mark)]
        turn = await session.turn("story", story)
        starts = backchannel_starts(session, turn.said.start, (turn.e or turn.said.end) + 3)
        played = b""
        if starts:
            first = starts[0][1].get("utterance_id")
            ends = [at for at, e in session.all("audio_segment")
                    if e.get("utterance_id") == first and e.get("phase") == "end"]
            until = ends[0] if ends else starts[0][0] + 0.7
            played = b"".join(data for at, data in session.audio if starts[0][0] - 0.05 <= at <= until)
        found[name] = {"voice": voice, "clips": clips, "refused": refused, "played": played, "heard": heard,
                       "starts": [round(at - turn.said.start, 2) for at, _ in starts],
                       "recording": recording, "rendered": [t for t, _ in router.rendered.get(voice, [])]}
        await session.close()
    # Which voice each played clip came from: its best match among every clip either voice made.
    for name, it in found.items():
        mine = [pcm for _, pcm in router.rendered.get(it["voice"], [])] + ([it["recording"]] if it["recording"] else [])
        other = [pcm for n, o in found.items() if n != name for _, pcm in router.rendered.get(o["voice"], [])]
        it["own"] = max((similarity(it["played"], pcm) for pcm in mine), default=0.0)
        it["other"] = max((similarity(it["played"], pcm) for pcm in other), default=0.0)
    checks = {}
    for name, it in found.items():
        checks[f"{name}: the bank was made and holds clips"] = it["clips"] >= 1
        checks[f"{name}: a listening sound played in the turn"] = bool(it["starts"]) and frame_db(it["played"]) > AUDIBLE_DB
        checks[f"{name}: it is this voice's clip"] = it["own"] >= 0.8 and it["own"] > it["other"]
        # The words check runs only on a request-response recogniser (one upload per clip).
        checks[f"{name}: the recogniser checked the clips (request) or none (streaming)"] = \
            bool(it["heard"]) == (ctx.args.stt == "request")
        checks[f"{name}: every refused clip is too long, silent or misheard"] = all(
            re.match(r"(\d+ ms long|silent|heard )", reason) for _, reason in it["refused"])
    checks["alan: the recording replaced the rendered mhm"] = "mhm" not in found["alan"]["rendered"]
    report(ctx, "CL1", "listening sounds in two voices", checks,
           "; ".join(f"{name}: bank {it['clips']} clips, rendered {it['rendered'][:4]}..., refused {it['refused']}, "
                     f"recogniser heard {it['heard']}, "
                     f"starts {it['starts']} s, played {len(it['played']) // 48} ms, match own {it['own']:.2f} "
                     f"other {it['other']:.2f}" for name, it in found.items()),
           "per voice: the gateway logged `interaction listening sounds ready` with at least 1 clip; with the "
           "request recogniser `interaction listening sound heard` lines (none with the streaming one); every "
           "`interaction listening sound refused` gives a length over 600 ms, silence or the recogniser's "
           "words; an `audio_segment` of lane backchannel starts in the stretched story turn, louder than "
           "-50 dBFS, and its audio correlates >= 0.8 with a clip of the same voice (rendered or recorded) "
           "and better than with any clip of the other voice; alan's \"mhm\" is the operator's recording "
           "(no synthesis of it in that voice)",
           "voices: Piper en_US-lessac-medium (the speech server) and en_GB-alan-medium (the voice router); "
           "the story's comma pauses are stretched to 900 ms (BC1's 320-380 ms close before Smart Turn "
           "answers on this debug build); the streaming recogniser finalises after 1,400 ms of silence")


SCREEN_CARD = "I'm putting a secure form on your screen for your card number."


async def cw1(ctx: Ctx):
    """While a card number is asked on the screen the recogniser hears silence: a number said aloud reaches no transcript."""
    # The webhook stand-in asks for the card number (no model decides whether to ask).
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    block = {**MODEL_FRONT, "tasks": {"default_backend": "hook"}}
    session = await Session.open(ctx, "CW1", block, conversation=False, port=stand_in.port)
    await session.warm()
    start = len(session.events)
    await session.say("59_pay_bill")
    asked = await session.wait(lambda e: e["type"] == "task_question", 40, start)
    prompt = await said(session, start, SCREEN_CARD, 30)
    if prompt:
        await heard(session, prompt, 40)
    window = len(session.events)
    await session.say("53_card_number")
    dont = await said(session, window, "Please don't say that out loud. Use your screen instead.", 30)
    await asyncio.sleep(3.0)
    in_window = [e.get("transcript") for _, e in session.all("stt_result", window)]
    request_id = asked[2].get("request_id") if asked else None
    answered_from = len(session.events)
    await session.send({"type": "task_answer", "request_id": request_id, "outcome": "accept", "value": "4111"})
    received = await answer_received(session, answered_from, 20)
    result = await session.wait(lambda e: e["type"] in ("task_result", "task_cancelled"), 60, answered_from)
    await session.quiet(20, answered_from)
    after = len(session.events)
    await session.say("20_q_capital")
    heard_again = await session.wait(
        lambda e: e["type"] == "stt_result" and e.get("is_final") and e.get("transcript"), 15, after)
    await session.close()
    log = ctx.stack.log_dir / "live-backend-gateway.log"
    lines = [line for line in log.read_text(errors="replace").splitlines()
             if "interaction sensitive capture window" in line and session.stream_id in line]
    windows = [dict(re.findall(r"(\w+)=(\S+)", line)).get("capture") for line in lines]
    got = [hit["body"].get("value") for hit in stand_in.of("hook", "answer", mark)]
    digits = [text for text in in_window if text and re.search(r"\d{4}|four|one one", text.lower())]
    checks = {
        "the card number is asked on the screen": bool(asked) and asked[2].get("sensitive") is True
                                                  and asked[2].get("channels") == ["client"] and bool(prompt),
        "the recogniser's window opens and closes": windows[:2] == ["true", "false"],
        "no transcript while it is open": not in_window,
        "the caller is pointed at the screen": bool(dont),
        "the screen's answer goes to the backend": bool(received) and received.get("by") == "client"
                                                   and got == ["4111"],
        "the task completes": bool(result) and result[2].get("state") == "completed",
        "the recogniser hears the caller again": bool(heard_again),
    }
    report(ctx, "CW1", "the capture window of a sensitive value", checks,
           f"task_question sensitive {asked[2].get('sensitive') if asked else None} channels "
           f"{asked[2].get('channels') if asked else None}; screen line said {bool(prompt)}; capture window "
           f"log {windows}; stt_result while open {in_window} (with digits {digits}); the number said aloud -> "
           f"{dont[2].get('delta') if dont else None!r}; task_answer_received {received}; the webhook got {got}; "
           f"then {result[2].get('state') if result else None!r}; next turn heard "
           f"{heard_again[2].get('transcript') if heard_again else None!r}",
           "a sensitive `task_question` with channels [client] and the screen line; the gateway logs the "
           "capture window opening (capture=true) and closing (capture=false); not one `stt_result` from "
           "the screen line's end until the client answers, though the caller says the number; the caller "
           "hears the don't-say line; the client's value reaches the webhook; after the answer a spoken turn "
           "is transcribed again")
    return session


# The echo comes back as in S4: 200 ms late, 6 dB down.
ECHO_LINES = 3


def echo_words(line: str, transcripts: List[str]) -> List[str]:
    """The transcripts that hold three or more words of the agent's line: its echo, transcribed."""
    said = set(re.findall(r"[a-z']+", line.lower()))
    return [text for text in transcripts if len(set(re.findall(r"[a-z']+", (text or "").lower())) & said) >= 3]


async def echo_lines(ctx: Ctx, label: str, echo: dict, lines: int) -> dict:
    """A session whose microphone returns the agent's audio; the agent says the long line
    `lines` times. Returns the finals of each line and what the gateway says of the session."""
    session = await Session.open(ctx, label, {**MODEL_FRONT, "duplex": {"echo": echo}}, PLAIN_PROMPT)
    await session.warm()
    session.loopback = (ECHO_DELAY, ECHO_LEVEL_DB)
    finals = []
    for _ in range(lines):
        start = len(session.events)
        line = await say_long_line(session)
        await heard(session, line, 60)
        # A streaming recogniser gives the echo's final after the line's audio ends.
        await session.wait(lambda e: e["type"] == "stt_result" and e.get("is_final"), 6.0, start)
        await asyncio.sleep(1.0)
        await session.quiet(20, start)
        finals.append([e.get("transcript") for _, e in session.all("stt_result", start) if e.get("is_final")])
    session.loopback = None
    await session.close()
    summary = await ctx.summary(session)
    turned_on = ctx.stack.log_since("interaction echo canceller turned on", session.log_mark)
    return {"finals": finals, "summary": summary, "turned_on": len(turned_on),
            "echo": (session.ready.get("interaction") or {}).get("duplex", {}).get("echo", {})}


async def ec1(ctx: Ctx):
    """The echo canceller runs on the speech-to-text's audio; client_aec, phone_line and the automatic turn-on."""
    plain = await echo_lines(ctx, "EC1-gate", {"correlation_gate": True, "aec": "off"}, 1)
    client = await echo_lines(ctx, "EC1-client-aec", {"client_aec": False}, 1)
    phone = await echo_lines(ctx, "EC1-phone", {"phone_line": True}, 1)
    auto = await echo_lines(ctx, "EC1-auto", {}, ECHO_LINES)
    heard_echo = {name: [len(echo_words(LONG_LINE, finals)) for finals in run["finals"]]
                  for name, run in (("gate", plain), ("client_aec", client), ("phone", phone), ("auto", auto))}
    canceller = {name: run["summary"].get("echo_canceller") for name, run in
                 (("gate", plain), ("client_aec", client), ("phone", phone), ("auto", auto))}
    # A request-response recogniser hears only the turns the analyser opens, and the gate opens
    # none on the echo: the control is judged on the streaming recogniser, which hears it all.
    streaming = ctx.args.stt == "streaming"
    checks = {
        "without the canceller the recogniser hears the echo": heard_echo["gate"][0] >= 1 or not streaming,
        "a client with client_aec false gets the canceller and no echo is transcribed":
            canceller["client_aec"] == "true" and heard_echo["client_aec"][0] == 0,
        "a client's phone_line gets the canceller and no echo is transcribed":
            phone["summary"].get("phone_leg") == "true" and canceller["phone"] == "true"
            and heard_echo["phone"][0] == 0,
        "auto turns the canceller on after two stretches of echo": auto["turned_on"] == 1
            and heard_echo["auto"][-1] == 0,
        "the session echoes the keys": client["echo"].get("client_aec") is False
            and phone["echo"].get("phone_line") is True and auto["echo"].get("max_bulk_delay_ms") == 1000,
    }
    report(ctx, "EC1", "the echo canceller on the speech-to-text; the published echo keys", checks,
           f"finals holding the line's words, per line: {heard_echo}; echo_canceller in the session summary "
           f"{canceller}; phone_leg {phone['summary'].get('phone_leg')}; canceller turned on in the auto session "
           f"{auto['turned_on']} time(s); finals of the gate session {plain['finals']}; of the client_aec "
           f"session {client['finals']}",
           "with the gate alone at least one final holds 3 or more words of the agent's line (its echo); "
           "with client_aec false, and with phone_line true, the summary says echo_canceller true (phone_leg "
           "true for the latter) and no final holds them; with neither set the gateway logs `interaction echo "
           f"canceller turned on` once over {ECHO_LINES} lines and the last line's echo is not transcribed",
           "the echo is the client's own playback, 200 ms later and 6 dB down (S4); the control (gate "
           "alone) is judged on the streaming recogniser only")
    return auto


# The supervised front model is ready again this soon after it was killed.
FRONT_BACK_LIMIT_S = 60.0


def front_log(ctx: Ctx) -> List[str]:
    """What the front gateway of this run has logged."""
    try:
        with open(ctx.stack.log_dir / "live-front-gateway.log", "rb") as log:
            log.seek(getattr(ctx.stack, "front_log_start", 0))
            return log.read().decode(errors="replace").splitlines()
    except OSError:
        return []


async def until_log(ctx: Ctx, needle: str, count: int, timeout: float) -> Optional[float]:
    """When the front gateway's log holds `count` lines with `needle`; None after `timeout`."""
    deadline = now() + timeout
    while now() < deadline:
        if len([line for line in front_log(ctx) if needle in line]) >= count:
            return now()
        await asyncio.sleep(0.25)
    return None


def child_pids(port: int) -> List[int]:
    listing = subprocess.run(["pgrep", "-f", f"llama-server .*--port {port} "], capture_output=True, text=True)
    return [int(pid) for pid in listing.stdout.split()]


async def fs1(ctx: Ctx):
    """The gateway runs the front model's server: a session loses the model when it is killed and takes it up again after the restart."""
    # The model runs once on this machine's GPU: the stack's own front model server makes room.
    freed = ctx.stack.stop_front()
    try:
        await fs1_run(ctx)
    finally:
        if freed:
            ctx.stack.start_front()


async def fs1_run(ctx: Ctx):
    port, front = ctx.stack.ensure_front_gateway()
    started = now()
    first_ready = await until_log(ctx, "front model ready", 1, 150)
    session = await Session.open(ctx, "FS1", {"front": {"profile": "model"}}, PLAIN_PROMPT, port=port)
    await session.warm()

    def settled() -> List[str]:
        return [dict(re.findall(r"(\w+)=(\S+)", line)).get("source", "?") for line in front_log(ctx)
                if "interaction turn settled" in line and session.stream_id in line]

    # Three different questions: a repeated one is taken for the agent's own echo.
    await session.turn("20_q_capital")
    before = settled()
    pids = child_pids(front)
    killed = now()
    for pid in pids:
        os.kill(pid, signal.SIGKILL)
    lost_from = len(session.events)
    await session.turn("21_q_week")
    lost = await session.wait(lambda e: e["type"] == "interaction_updated"
                              and "front_unavailable" in e.get("degraded", []), 20, lost_from)
    lost_turn = settled()[len(before):]
    back_at = await until_log(ctx, "front model ready", 2, FRONT_BACK_LIMIT_S + 30)
    await asyncio.sleep(1.0)
    back_from = len(session.events)
    await session.turn("22_q_sky")
    # The session takes the model up as soon as the supervisor's new generation is ready
    # (chapter 10's re-ask of the waiting turn), often before this script's next turn: the
    # event is looked for from the loss on.
    back = await session.wait(lambda e: e["type"] == "interaction_updated"
                              and "front_unavailable" not in e.get("degraded", []), 20, lost_from)
    back_turn = settled()[len(before) + len(lost_turn):]
    completed = [e.get("state") for _, e in session.all("task_result")]
    await session.close()
    # The gateway stops its model server when it stops.
    process = ctx.stack.started.pop("front-gateway")
    process.terminate()
    process.wait(timeout=40)
    deadline = now() + 10
    while child_pids(front) and now() < deadline:
        await asyncio.sleep(0.2)
    left = child_pids(front)
    restart_s = back_at - killed if back_at else None
    asked = [line for line in front_log(ctx) if "interaction front act" in line]
    checks = {
        "the gateway starts the model and it gets ready": first_ready is not None and len(pids) == 1,
        "killed, the session decides nothing and says so": bool(lost)
            and all(source == "Model" for source in lost_turn),
        "it is started again and ready": restart_s is not None and restart_s <= FRONT_BACK_LIMIT_S,
        "the session takes the model up again and says so": bool(back),
        "the turns before and after complete": completed.count("completed") >= 2,
        "the model server stops with the gateway": not left,
    }
    report(ctx, "FS1", "the front model's server run by the gateway", checks,
           f"ready {round(first_ready - started, 1) if first_ready else None} s after the gateway; child pids "
           f"{pids}; turn sources before the kill {before}, after it {lost_turn}, after the restart {back_turn} "
           f"(acts the model named in its budget: {len(asked)}); "
           f"degraded event {lost[2] if lost else None}; ready again {round(restart_s, 1) if restart_s else None} "
           f"s after the SIGKILL; back event {back[2] if back else None}; task states {completed}; model "
           f"server processes left after the gateway stopped: {left}",
           f"one llama-server child, ready (log `front model ready`); after a SIGKILL of it the next turn "
           f"gives interaction_updated with front_unavailable and is settled by nothing else; `front "
           f"model ready` again within {FRONT_BACK_LIMIT_S:.0f} s; the turn after it gives "
           f"interaction_updated without front_unavailable (the model is asked again); the tasks of "
           f"the turns before and after completed; no llama-server on the port 10 s after the gateway's SIGTERM",
           "the model is this run's front model on the GPU, started by the gateway's supervisor; whether "
           "it names its act within the 300 ms budget on this machine is L8's measure, reported here")
    return session


# MA1's operator prompt for the orchestrator (scratch orch-probe: gpt-oss-20b, temperature 0.2,
# the gateway's rules and tools). Without an operator prompt the model called ask_caller first in
# all 45 tries of a flight or test request (with or without the details, with or without a rule
# sentence added to the rules or to ask_caller's description). With this prompt: 10 of 10 bare
# requests went to the right agent; with the persona's preamble in front, 4 of 5 for "a flight to
# Delhi on Friday", and live (with the call's history) it asked every time; a rule sentence naming
# ask_caller brought it back to 10 of 10 asking. With the details in the request (clips 63, 64),
# the preamble and this prompt: 10 of 10 to the right agent at once.
ORCHESTRATOR_PROMPT = (
    "You are the caller's assistant on a phone call, and you hand their requests to agents. The "
    "agents know the caller's account, travel profile and code repositories, and they ask the "
    "caller themselves for anything else they need, so you never ask the caller for the details "
    "of a task: give the agent the request as the caller said it.")


async def answer_questions(session: Session, start: int, answered: List[float]):
    """MA1's caller answers each question a request asks, once it has been put: an orchestrator
    may ask for a preference (live, gpt-oss-20b asked for the airline or seat class after it had
    started the booking). Runs until cancelled; `answered` gets the time of each answer."""
    cursor = start
    while True:
        found = await session.wait(lambda e: e["type"] == "task_question", 3600, cursor)
        if found is None:
            return
        index = found[0]
        await session.wait(lambda e: e["type"] == "assistant_transcript"
                           and e.get("kind") == "question", 20, index)
        await asyncio.sleep(0.5)
        await session.say("65_no_preference")
        answered.append(now())
        cursor = index + 1


async def settled(session: Session, start: int, timeout: float = 120.0) -> bool:
    """Quiet, and still no new agent audio 5 s later. `task_result` marks the end of a task,
    not of its speech: live (MA1 run 8, load 23) a request's last two sentences were still in
    synthesis when its result came and `quiet` returned, and the session was closed on them."""
    deadline = now() + timeout
    while now() < deadline:
        await session.quiet(max(1.0, deadline - now()), start)
        heard = session.last_audio()
        await asyncio.sleep(5.0)
        if session.last_audio() == heard and not session.open_tasks(start):
            return True
    return False


async def orchestrated_session(ctx: Ctx, label: str, limits: Optional[dict] = None,
                               block: Optional[dict] = None,
                               tasks_extra: Optional[dict] = None,
                               prompt: str = ORCHESTRATOR_PROMPT) -> Session:
    """A session of MA1: its requests go to the orchestrator model (`Stack.ensure_orchestrator`)
    with the stand-ins' `browser` and `coder` agents as its tools. `block` is laid over the
    session's interaction block."""
    stand_in = ctx.stack.ensure_backends()
    port = ctx.stack.ensure_orchestrator()
    tasks: Dict[str, Any] = {
        "orchestrator": {"base_url": f"http://127.0.0.1:{port}/v1", "model": "gpt-oss-20b",
                         "api_key": "local-dummy", "temperature": 0.2,
                         "system_prompt": prompt,
                         # Live (runs 7 and 9, load 21-26) its default reasoning took 17-44 s
                         # before the first agent call, so one agent had finished before the
                         # other started. Low: 27-40 reasoning characters and about 1 s a
                         # call alone; live (run 11) 3.5 and 10.2 s from request to agent.
                         "reasoning_effort": "low"},
        "agents": ["browser", "coder"],
    }
    if limits:
        tasks["limits"] = limits
    tasks.update(tasks_extra or {})
    return await Session.open(ctx, label, {**MODEL_FRONT, "tasks": tasks, **(block or {})},
                              port=stand_in.port)


def segments_of(session: Session, start: int) -> Dict[str, Tuple[float, float, str]]:
    """Each utterance's audio: (start, end, kind) by `utterance_id`, from `audio_segment`."""
    spans: Dict[str, List[Any]] = {}
    for at, event in session.all("audio_segment", start):
        span = spans.setdefault(event["utterance_id"], [None, None, event.get("kind", "")])
        span[0 if event["phase"] == "start" else 1] = at
    return {key: (span[0], span[1] if span[1] is not None else now(), span[2])
            for key, span in spans.items() if span[0] is not None}


def updates_cut_nothing(session: Session, start: int) -> Tuple[bool, str]:
    """No update or offer starts while an answer (a relayed line) plays, and no answer line
    starts while an update plays: the heard timeline of `audio_segment`."""
    spans = segments_of(session, start)
    answers = [(b, e) for b, e, kind in spans.values() if kind == "relay"]
    updates = [(b, e) for b, e, kind in spans.values() if kind == "update"]
    inside = lambda at, spans: any(b < at < e for b, e in spans)
    cut = [b for b, _ in updates if inside(b, answers)] + [b for b, _ in answers if inside(b, updates)]
    return not cut, f"{len(updates)} update lines, {len(answers)} answer lines, {len(cut)} overlapping"


def outputs_before_updates(session: Session, start: int) -> Tuple[bool, str]:
    """Each child's `task_output` came before any line of the agent about that task."""
    outputs = {e["task_id"]: at for at, e in session.all("task_output", start)
               if e.get("parent_id") is not None}
    lines = [(at, e) for at, e in session.transcripts(start, "update")]
    late = [task for task, at in outputs.items()
            if any(e.get("task_id") == task and said < at for said, e in lines)]
    spoken = [e.get("task_id") for _, e in lines]
    return bool(outputs) and not late, (f"{len(outputs)} child outputs, update lines for "
                                        f"{sum(1 for task in outputs if task in spoken)}, {len(late)} after")


async def updates_said(session: Session, start: int, timeout: float = 60.0) -> bool:
    """Every child with a `task_output` has had an update line said about it (not an offer)."""
    deadline = now() + timeout
    while now() < deadline:
        children = {e["task_id"] for _, e in session.all("task_output", start) if e.get("parent_id")}
        said = {e.get("task_id") for _, e in session.transcripts(start, "update")
                if "Want" not in e.get("delta", "")}
        if children <= said:
            return True
        await asyncio.sleep(0.5)
    return False


def numbers_in(text: str) -> List[str]:
    return [run.lstrip("0") or "0" for run in re.findall(r"\d+", text)]


def narrator_lines(ctx: Ctx, stream_id: str) -> List[Tuple[int, str]]:
    """The backend gateway's `interaction narrator` lines of a session: (ms, line)."""
    path = ctx.stack.log_dir / "live-backend-gateway.log"
    found = []
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return found
    for line in text.splitlines():
        if "interaction narrator" in line and stream_id in line:
            ms = re.search(r"\bms=(\d+)", line)
            said = re.search(r'\bline="?(.*?)"?$', line)
            found.append((int(ms.group(1)) if ms else -1, said.group(1) if said else ""))
    return found


async def ma1(ctx: Ctx):
    """MA1 many agents: an orchestrator runs two agents at once and answers a question meanwhile"""
    stand_in = ctx.stack.ensure_backends()
    checks: Dict[str, bool] = {}
    # Voice only: a result is then said as its update with the result's words (with a screen
    # attached the update points at the screen instead, chapter 8).
    session = await orchestrated_session(ctx, "MA1", block={"hitl": {"channels": ["voice"]}})
    await session.warm()
    start, mark = len(session.events), len(stand_in.seen)
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    answers: List[float] = []
    caller = asyncio.create_task(answer_questions(session, start, answers))
    # Three requests in quick succession: each as soon as the one before was heard.
    finals = []
    for clip in ("63_book_flight_from", "64_fix_named_test", "62_capital_peru"):
        said_from = len(session.events)
        await session.say(clip)
        final = await session.wait(is_final, 15, said_from)
        finals.append(final[1] if final else None)
        await asyncio.sleep(1.5)
    # Everything ends: the orchestrator's three tasks and its two children, and what is said.
    await settled(session, start)
    # An update that was offered and not taken is said once the call is idle (4 s after the
    # last line, and the offer's answer window): wait for every child's update, then settle.
    await updates_said(session, start)
    await settled(session, start)
    caller.cancel()
    await session.close()
    ctx.record(session)

    created = [e for _, e in session.all("task_created", start)]
    results = {e["task_id"]: (at, e) for at, e in session.all("task_result", start)}
    requests = [e for e in created if e.get("parent_id") is None]
    children = [e for e in created if e.get("parent_id") is not None]
    by_id = {e["task_id"]: e for e in created}
    agents = sorted(str(child.get("agent")) for child in children)
    jobs = [job for job in stand_in.jobs.values()
            if job["created"] >= session.opened and ("flight" in job["request"].lower()
                                                     or "test" in job["request"].lower())]
    intervals = sorted((job["created"], job.get("ended", now()), job["wire"]) for job in jobs)
    overlap = (len(intervals) >= 2
               and max(begin for begin, _, _ in intervals) < min(end for _, end, _ in intervals))
    traced = [(hit.get("traceparent") or "", hit) for hit in stand_in.seen[mark:]
              if hit["wire"] in ("hook", "a2a")]
    traces = {child["trace_id"] for child in children}
    lines = [(at, e.get("delta", "")) for at, e in session.transcripts(start) if e.get("delta")]
    lima = [at for at, text in lines if "lima" in text.lower()]
    last_job_end = max((end for _, end, _ in intervals), default=0.0)
    first_job_end = min((end for _, end, _ in intervals), default=0.0)
    said = " | ".join(text for _, text in lines)
    checks["three caller requests, each its own top-level task"] = len(requests) == 3
    checks["two children, one on each agent, under the request that asked for it"] = (
        agents == ["browser", "coder"]
        and all(child["parent_id"] in by_id and by_id[child["parent_id"]].get("parent_id") is None
                for child in children))
    checks["children carry their request's trace_id (32 hex digits)"] = bool(children) and all(
        re.fullmatch(r"[0-9a-f]{32}", child.get("trace_id") or "")
        and child["trace_id"] == by_id[child["parent_id"]]["trace_id"] for child in children)
    checks["every request to an agent carries its task's traceparent"] = bool(traced) and all(
        any(header.startswith(f"00-{trace}-") for trace in traces) for header, _ in traced)
    checks["the two agents ran at the same time (overlapping wall-clock intervals)"] = overlap
    checks["the general question was answered while an agent still ran"] = (
        bool(lima) and lima[0] < last_job_end)
    checks["each agent's result reached the caller"] = (
        "9:40" in said or "nine forty" in said.lower()) and (
        "test" in said.lower() and ("fix" in said.lower() or "pass" in said.lower()))
    checks["every task completed"] = (
        len(created) == 5 and all(results.get(e["task_id"], (0, {}))[1].get("state") == "completed"
                                  for e in created))
    published, published_note = outputs_before_updates(session, start)
    checks["every child's task_output came before any update about it"] = published
    uncut, uncut_note = updates_cut_nothing(session, start)
    checks["no update started while an answer played, nor an answer while an update played"] = uncut

    # The same with max_live 1: the question waits behind the flight; it is not refused.
    limited = await orchestrated_session(ctx, "MA1-limited", {"max_live": 1})
    await limited.warm()
    start = len(limited.events)
    said_from = len(limited.events)
    limited_answered: List[float] = []
    limited_caller = asyncio.create_task(answer_questions(limited, start, limited_answered))
    await limited.say("63_book_flight_from")
    await limited.wait(is_final, 15, said_from)
    await limited.wait(lambda e: e["type"] == "task_created" and e.get("parent_id") is not None, 30, start)
    await asyncio.sleep(1.0)
    await limited.say("62_capital_peru")
    await settled(limited, start)
    limited_caller.cancel()
    await limited.close()
    ctx.record(limited)
    queued = [e for _, e in limited.all("task_created", start)
              if e.get("parent_id") is None and e.get("state") == "submitted"]
    refused = [e for _, e in limited.all("task_result", start) if e.get("state") == "rejected"]
    heard_queued = [e.get("delta", "") for _, e in limited.transcripts(start)
                    if "right after" in e.get("delta", "") or "queued" in e.get("delta", "")]
    limited_results = [e for _, e in limited.all("task_result", start)]
    checks["max_live 1: the second request is queued, not refused, and the caller hears so"] = (
        len(queued) == 1 and not refused and bool(heard_queued))
    checks["max_live 1: both requests complete in the end"] = (
        sum(1 for e in limited_results if e.get("state") == "completed") >= 3)

    update_checks, update_note = await ma1_updates(ctx, is_final)
    checks.update(update_checks)

    jobs_text = "; ".join(f"{wire} {begin - session.opened:.1f}-{end - session.opened:.1f} s"
                          for begin, end, wire in intervals)
    answered = f"{lima[0] - session.opened:.1f} s" if lima else "never"
    ctx.add("MA1", "an orchestrator runs two agents at once and answers a question meanwhile",
            all(checks.values()),
            f"agent jobs {jobs_text} (first ended {first_job_end - session.opened:.1f} s); Lima "
            f"said at {answered}; tasks {len(requests)} requests + {len(children)} children; "
            f"agent requests {len(traced)} with traceparent; queued {len(queued)}, refused {len(refused)}, "
            f"queued line {heard_queued[:1]}; questions answered {len(answers)} + "
            f"{len(limited_answered)} (max_live 1); first session: {published_note}, {uncut_note}; "
            f"{update_note}; " + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks),
            f"said: {said[:600]}")


async def ma1_updates(ctx: Ctx, is_final: Callable[[dict], bool]) -> Tuple[Dict[str, bool], str]:
    """MA1's third session (chapter 8's update inbox): the two agents end while the agent
    answers the general question. Their results are published at once; the answer is not
    cut; at its end one offer (`tasks.announce` ask, the default) is heard; the caller's "yes"
    plays the front's update. The model front words it (the narrator); the client has no
    screen."""
    stand_in = ctx.stack.ensure_backends()
    checks: Dict[str, bool] = {}
    block = {"front": {"profile": "model"}, "hitl": {"channels": ["voice"]}}
    session = await orchestrated_session(ctx, "MA1-updates", block=block)
    await session.warm()
    start = len(session.events)
    stand_in.hold = threading.Event()
    answers: List[float] = []
    caller = asyncio.create_task(answer_questions(session, start, answers))
    try:
        for clip in ("63_book_flight_from", "64_fix_named_test"):
            said_from = len(session.events)
            await session.say(clip)
            await session.wait(is_final, 15, said_from)
            await asyncio.sleep(1.5)
        asked_from = len(session.events)
        await session.say("62_capital_peru")
        asked = await session.wait(is_final, 15, asked_from)
        # The agents end once the question is asked and both have their task: their results
        # come while it is answered (if the orchestrator has not answered it by then).
        third = lambda e: e["type"] == "task_created" and e.get("parent_id") is None
        await session.wait(lambda e: third(e) and "peru" in e.get("title", "").lower(), 30, asked_from)
        children_of = lambda: [e for _, e in session.all("task_created", start) if e.get("parent_id")]
        deadline = now() + 60
        while len(children_of()) < 2 and now() < deadline:
            await asyncio.sleep(0.1)
        released = now()
        stand_in.hold.set()
        offer_line = lambda e: (e["type"] == "assistant_transcript" and e.get("kind") == "update"
                                and "Want" in e.get("delta", ""))
        offer = await session.wait(offer_line, 120, asked_from)
        offered_heard = None
        if offer is not None:
            heard = lambda e: (e["type"] == "assistant_transcript" and e.get("heard") == "full"
                               and e.get("utterance_id") == offer[2].get("utterance_id"))
            offered_heard = await session.wait(heard, 30, offer[0])
        yes_at = now()
        await asyncio.sleep(0.4)
        await session.say("66_yes_please")
        await settled(session, start)
    finally:
        stand_in.hold = None
        caller.cancel()
    await session.close()
    ctx.record(session)

    created = [e for _, e in session.all("task_created", start)]
    children = {e["task_id"]: e for e in created if e.get("parent_id") is not None}
    capital = [e for e in created if e.get("parent_id") is None and "peru" in e.get("title", "").lower()]
    spans = segments_of(session, start)
    relays = [(at, e) for at, e in session.transcripts(start, "relay")]
    capital_ids = {e["task_id"] for e in capital}
    answer_ids = {e["utterance_id"] for _, e in relays if e.get("task_id") in capital_ids}
    answer_end = max((spans[u][1] for u in answer_ids if u in spans), default=0.0)
    question_time = asked[1] if asked else 0.0
    outputs = {e["task_id"]: (at, e) for at, e in session.all("task_output", start)
               if e["task_id"] in children}
    checks["updates: both children's task_output came while the general question was being "
           "answered (after it was asked, before its answer's audio ended)"] = (
        len(outputs) == 2 and answer_end > 0
        and all(question_time < at < answer_end for at, _ in outputs.values()))
    published, published_note = outputs_before_updates(session, start)
    checks["updates: each task_output came before its spoken update"] = published
    uncut, uncut_note = updates_cut_nothing(session, start)
    checks["updates: no update cut an answer (heard timeline)"] = uncut
    offer_span = spans.get(offer[2]["utterance_id"]) if offer else None
    checks["updates: the offer was heard at the end of the answer, before the yes"] = (
        offer_span is not None and offered_heard is not None and offer_span[0] >= answer_end
        and offer_span[1] <= yes_at)
    after_yes = [(at, e) for at, e in session.transcripts(start, "update")
                 if at > yes_at and e.get("task_id") in children]
    words = [len(e["delta"].split()) for _, e in after_yes]
    result_numbers = {e["task_id"]: set(numbers_in(e.get("text", ""))) for _, e in outputs.values()}
    foreign = [e["delta"] for _, e in after_yes
               if not set(numbers_in(e["delta"])) <= result_numbers.get(e["task_id"], set())]
    checks["updates: the yes played the updates, each at most 20 words with no number the result lacks"] = (
        len(after_yes) >= 1 and all(n <= 20 for n in words) and not foreign)
    narrated = narrator_lines(ctx, session.stream_id)
    by_model = [line for _, line in narrated if line and any(line in e["delta"] for _, e in after_yes)]
    latency = ", ".join(f"{ms} ms" for ms, _ in narrated) or "none logged"
    note = (f"updates session: offer {offer[2]['delta'] if offer else None!r} at "
            f"{(offer_span[0] - session.opened) if offer_span else -1:.1f} s, answer ended "
            f"{answer_end - session.opened:.1f} s, outputs at "
            f"{', '.join(f'{at - session.opened:.1f}' for at, _ in outputs.values())} s (asked "
            f"{question_time - session.opened:.1f} s, released {released - session.opened:.1f} s); "
            f"after yes {[e['delta'] for _, e in after_yes]} ({words} words); narrator {latency}, "
            f"{len(by_model)} of {len(after_yes)} in the model's words; {published_note}; {uncut_note}")
    return checks, note


async def ma4(ctx: Ctx):
    """MA4 GET /v1/tasks/events: a second client receives each result as it completes"""
    stand_in = ctx.stack.ensure_backends()
    session = await orchestrated_session(ctx, "MA4")
    await session.warm()
    start = len(session.events)
    received: List[Tuple[float, str, dict]] = []
    stop = threading.Event()
    url = f"http://127.0.0.1:{stand_in.port}/v1/tasks/events?stream_id={session.stream_id}"

    def read():
        # A second client of the caller's, apart from the call, reading the stream as it comes.
        try:
            with urllib.request.urlopen(url, timeout=600) as response:
                name, data = "", ""
                for raw in response:
                    if stop.is_set():
                        return
                    line = raw.decode("utf-8", "replace").rstrip("\n")
                    if line.startswith("event:"):
                        name = line[6:].strip()
                    elif line.startswith("data:"):
                        data = line[5:].strip()
                    elif not line and name:
                        received.append((now(), name, json.loads(data) if data else {}))
                        name, data = "", ""
        except Exception as error:  # the stream's end at the close is not a failure
            received.append((now(), "error", {"error": repr(error)}))

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    await asyncio.sleep(0.5)
    stand_in.hold = threading.Event()
    answers: List[float] = []
    caller = asyncio.create_task(answer_questions(session, start, answers))
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    try:
        for clip in ("63_book_flight_from", "64_fix_named_test"):
            said_from = len(session.events)
            await session.say(clip)
            await session.wait(is_final, 15, said_from)
            await asyncio.sleep(1.5)
        children = lambda: [e for _, e in session.all("task_created", start) if e.get("parent_id")]
        deadline = now() + 120
        while len(children()) < 2 and now() < deadline:
            await asyncio.sleep(0.2)
        stand_in.hold.set()
        await settled(session, start)
    finally:
        stand_in.hold = None
        caller.cancel()
    await session.close()
    stop.set()
    ctx.record(session)

    kids = {e["task_id"]: e for _, e in session.all("task_created", start) if e.get("parent_id")}
    jobs = {job["request"]: job for job in stand_in.jobs.values() if job["created"] >= session.opened}
    socket_outputs = {e["task_id"]: at for at, e in session.all("task_output", start)}
    streamed = {e.get("task_id"): at for at, name, e in received if name == "task_output"}
    names = [name for _, name, _ in received]
    lags = [streamed[task] - socket_outputs[task] for task in kids if task in streamed and task in socket_outputs]
    # The stand-in job each child ran: its request starts with the child's title.
    ended = {task: job.get("ended") for task, child in kids.items() for request, job in jobs.items()
             if request.startswith(child.get("title", "\0"))}
    after_end = [streamed[task] - end for task, end in ended.items() if end and task in streamed]
    checks = {
        "the stream carried task_created, task_output and task_result of the session": all(
            name in names for name in ("task_created", "task_output", "task_result")),
        "each child's task_output arrived on the stream": bool(kids) and all(task in streamed for task in kids),
        "within 1 s of the socket's own task_output": bool(lags) and all(abs(lag) < 1.0 for lag in lags),
        "every streamed event names the session": all(
            e.get("stream_id") == session.stream_id for _, name, e in received if name != "error"),
    }
    ctx.add("MA4", "GET /v1/tasks/events streams each result to a second client as it completes",
            all(checks.values()),
            f"{len(received)} events ({', '.join(sorted(set(names)))}); stream minus socket "
            f"{', '.join(f'{lag * 1000:+.0f} ms' for lag in lags)}; after the agent's job ended "
            f"{', '.join(f'{lag * 1000:.0f} ms' for lag in after_end)}; "
            + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks))


# MA7's operator prompt: the orchestrator knows its agents and tools only by what they say of
# themselves (no catalog description).
SELF_DESCRIBED_PROMPT = (
    "You are the caller's assistant on a phone call. Your tools are agents and services of the "
    "caller's: for each request, call at once the one whose description fits it, with its "
    "arguments filled in from what the caller said. Never ask the caller for details or for a "
    "confirmation yourself: the agents ask for what they need, and the phone system confirms "
    "what needs confirming.")


async def answer_aloud(session: Session, question: Tuple[int, float, dict], clip: str,
                       is_final: Callable[[dict], bool]) -> Optional[float]:
    """Waits for the question's last line to be played, then says `clip`: when the caller
    began to say it (None when no final came for it). The gateway may act on the answer
    before the client reads the final (`stt_result` and the agent's request race)."""
    index = question[0]
    line = await session.wait(lambda e: e["type"] == "assistant_transcript" and e.get("delta")
                              and e.get("kind") in ("question", "confirm")
                              and e["delta"].rstrip().endswith("?"), 30, index)
    if line:
        utterance = line[2].get("utterance_id")
        await session.wait(lambda e: e["type"] == "audio_segment" and e.get("phase") == "end"
                           and e.get("utterance_id") == utterance, 30, line[0])
    await asyncio.sleep(0.4)
    said_from, began = len(session.events), now()
    await session.say(clip)
    final = await session.wait(is_final, 15, said_from)
    return began if final else None


async def ma7(ctx: Ctx):
    """MA7 a standalone agent: an A2A agent and an MCP server known by what they say of themselves"""
    stand_in = ctx.stack.ensure_backends()
    checks: Dict[str, bool] = {}
    mark = len(stand_in.seen)
    session = await orchestrated_session(ctx, "MA7", block={"hitl": {"channels": ["voice"]}},
                                         tasks_extra={"agents": ["planner", "notes"]},
                                         prompt=SELF_DESCRIBED_PROMPT)
    await session.warm()
    start = len(session.events)
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    child = lambda agent: (lambda e: e["type"] == "task_created" and e.get("agent") == agent)
    ended = lambda task: (lambda e: e["type"] == "task_result" and e.get("task_id") == task)
    task_of = lambda found: found[2]["task_id"] if found else ""
    budget_at = yes_at = None
    plan = search = delete = remind = confirm = None
    try:
        # The planner streams its task and asks the caller's budget.
        await say_heard(session, "74_hotel_goa", is_final)
        plan = await session.wait(child("planner"), 120, start)
        asked = await session.wait(lambda e: e["type"] == "task_question"
                                   and e.get("task_id") == task_of(plan), 60, start)
        if asked:
            budget_at = await answer_aloud(session, asked, "75_budget", is_final)
        await session.wait(ended(task_of(plan)), 60, start)
        await settled(session, start, timeout=120)
        # A read-only tool: no question.
        searched_from = len(session.events)
        await say_heard(session, "76_search_dentist", is_final)
        search = await session.wait(child("search_notes"), 120, searched_from)
        await session.wait(ended(task_of(search)), 60, searched_from)
        await settled(session, searched_from, timeout=120)
        # A destructive tool: the caller confirms it before it is sent.
        deleted_from = len(session.events)
        await say_heard(session, "77_delete_note", is_final)
        confirm = await session.wait(lambda e: e["type"] == "task_question"
                                     and e.get("kind") == "confirm", 120, deleted_from)
        if confirm:
            yes_at = await answer_aloud(session, confirm, "78_yes_go_ahead", is_final)
        delete = await session.wait(child("delete_note"), 60, deleted_from)
        await session.wait(ended(task_of(delete)), 60, deleted_from)
        await settled(session, deleted_from, timeout=120)
        # A tool of several fields, with its progress on the reply.
        reminded_from = len(session.events)
        await say_heard(session, "79_reminder", is_final)
        remind = await session.wait(child("create_reminder"), 120, reminded_from)
        await session.wait(ended(task_of(remind)), 60, reminded_from)
        await updates_said(session, start, 60)
        await settled(session, reminded_from, timeout=120)
    finally:
        await session.close()
    ctx.record(session)

    seen = stand_in.seen[mark:]
    of = lambda wire, method: [hit for hit in seen if hit["wire"] == wire and hit["method"] == method]
    calls = lambda name: [hit for hit in of("notes", "tools/call")
                          if (hit["body"].get("params") or {}).get("name") == name]
    created = [e for _, e in session.all("task_created", start)]
    children = [e for e in created if e.get("parent_id")]
    results = {e["task_id"]: e for _, e in session.all("task_result", start)}
    progress = [(at, e) for at, e in session.all("task_progress", start)]
    plan_job = next((job for job in stand_in.jobs.values()
                     if job["wire"] == "agent" and job["created"] >= session.opened), {})
    streams = of("agent", "SendStreamingMessage")
    plan_progress = [at for at, e in progress if e.get("task_id") == task_of(plan)
                     and e.get("text") == "Looking at hotels in Goa."]
    heard_progress = [e.get("delta") for _, e in session.transcripts(start)
                      if "hotels in Goa" in e.get("delta", "")]
    budget_q = next((at for at, e in session.all("task_question", start)
                     if e.get("task_id") == task_of(plan)), None)
    said = " | ".join(e.get("delta", "") for _, e in session.transcripts(start))
    deletes, reminders = calls("delete_note"), calls("create_reminder")
    reminder_args = (reminders[0]["body"].get("params") or {}).get("arguments", {}) if reminders else {}
    delete_args = (deletes[0]["body"].get("params") or {}).get("arguments", {}) if deletes else {}
    discovered = of("agent", "card") + of("notes", "tools/list")
    first_child_at = min((at for at, e in session.all("task_created", start) if e.get("parent_id")),
                         default=now())
    checks["the card and the tool list were read before the session's first agent call"] = (
        bool(of("agent", "card")) and bool(of("notes", "tools/list"))
        and all(hit["t"] < first_child_at for hit in discovered))
    checks["the orchestrator chose the planner and the three tools by their own descriptions"] = (
        [e.get("agent") for e in children] == ["planner", "search_notes", "delete_note", "create_reminder"])
    checks["every call is a child task with its request's trace and the entry's background"] = bool(children) and all(
        e.get("trace_id") == next((p.get("trace_id") for p in created if p["task_id"] == e["parent_id"]), None)
        and e.get("background") == ("a2a:planner" if e.get("agent") == "planner" else "mcp:notes")
        for e in children)
    checks["the planner's task streamed (SendStreamingMessage, its key in X-Agent-Key)"] = (
        len(streams) >= 2 and all(hit["ok"] for hit in streams) and not of("agent", "GetTask"))
    checks["its progress came as it streamed, before its question"] = (
        bool(plan_progress) and budget_q is not None and plan_progress[0] < budget_q)
    checks["its question was asked aloud and answered by voice"] = (
        "budget" in said.lower() and budget_at is not None
        and any(word in str(plan_job.get("answer", "")).lower() for word in ("two hundred", "200")))
    checks["the read-only tool was called without a question"] = bool(search) and (
        search[2].get("side_effects") is False and bool(calls("search_notes"))
        and not [1 for _, e in session.all("task_question", start) if e.get("task_id") == task_of(search)])
    checks["the destructive call was read back from its arguments"] = bool(confirm) and (
        "shopping list" in str(confirm[2].get("prompt", "")).lower())
    answered = next((at for at, e in session.all("task_answer_received", start)
                     if confirm and e.get("request_id") == confirm[2].get("request_id")), None)
    checks["it reached the server once, only after the caller began to say yes"] = (
        yes_at is not None and len(deletes) == 1 and deletes[0]["t"] > yes_at
        and "shopping" in str(delete_args.get("title", "")).lower())
    checks["the tool of several fields got them from the caller's words"] = (
        "mom" in str(reminder_args.get("what", "")).lower()
        and "fri" in str(reminder_args.get("day", "")).lower()
        and any(t in str(reminder_args.get("time", "")).lower() for t in ("6", "six"))
        and str(reminder_args.get("priority", "")).lower() == "high")
    checks["its progress notification was the task's progress"] = any(
        e.get("task_id") == task_of(remind) and e.get("text") == "Saving the reminder." for _, e in progress)
    checks["every task completed"] = bool(created) and all(
        results.get(e["task_id"], {}).get("state") == "completed" for e in created)
    t = lambda at: f"{at - session.opened:.1f} s" if at else "never"
    ctx.add("MA7", "a standalone agent: A2A and MCP agents known by what they say of themselves",
            all(checks.values()),
            f"children {[e.get('agent') for e in children]}; streams {len(streams)}; planner progress at "
            f"{t(plan_progress[0] if plan_progress else None)}, question {t(budget_q)}, answered {t(budget_at)} "
            f"({plan_job.get('answer')!r}); progress heard {heard_progress[:1]}; confirm prompt "
            f"{confirm[2].get('prompt') if confirm else None!r}; yes began {t(yes_at)}, answer received "
            f"{t(answered)}, delete calls {[t(hit['t']) for hit in deletes]} {delete_args}; reminder "
            f"{reminder_args}; created {[(e.get('agent'), e.get('follows') is not None) for e in created]}; "
            + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks),
            f"said: {said[:700]}")


async def say_heard(session: Session, clip: str, is_final: Callable[[dict], bool]):
    """Says `clip`, and once more when the recogniser gave no final for it in 15 s: the first
    clip of the first session after the stack started went unheard in 3 of 6 MA2/MA3 runs
    (fin-followup runs 5, 11, 13; no `stt_result` at all), as MA1 saw at load 25."""
    for _ in range(2):
        said_from = len(session.events)
        await session.say(clip)
        if await session.wait(is_final, 15, said_from) is not None:
            return


async def ma2(ctx: Ctx, deadline_ms: int = 5000, label: str = "MA2"):
    """MA2 a question with a 5 s deadline breaks into the agent's answer, then the answer resumes"""
    stand_in = ctx.stack.ensure_backends()
    checks: Dict[str, bool] = {}
    stand_in.seat_deadline_ms = deadline_ms
    session = await orchestrated_session(ctx, label, block={"hitl": {"channels": ["voice"]}})
    await session.warm()
    start = len(session.events)
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    stand_in.ask_seat = threading.Event()
    asked = answered = None
    try:
        await say_heard(session, "63_book_flight_from", is_final)
        await session.wait(lambda e: e["type"] == "task_created" and e.get("agent") == "browser",
                           120, start)
        # The booking is under way (the job waits to ask): the orchestrator's line is said.
        await asyncio.sleep(6.0)
        lima_from = len(session.events)
        await session.say("67_lima_four_sentences")
        await session.wait(is_final, 15, lima_from)
        lima = await session.wait(lambda e: e["type"] == "task_created" and e.get("parent_id") is None
                                  and "lima" in e.get("title", "").lower(), 30, lima_from)
        lima_id = lima[2]["task_id"] if lima else None
        # The airline asks once the answer has begun to play: mid-answer.
        await session.wait(lambda e: e["type"] == "assistant_transcript" and e.get("kind") == "relay"
                           and e.get("task_id") == lima_id and e.get("delta"), 180, lima_from)
        await asyncio.sleep(0.8)
        stand_in.ask_seat.set()
        asked = await session.wait(lambda e: e["type"] == "task_question", 10, lima_from)
        question_line = await session.wait(lambda e: e["type"] == "assistant_transcript"
                                           and e.get("kind") == "question" and e.get("delta"), 10, lima_from)
        if question_line is not None:
            await session.wait(lambda e: e["type"] == "assistant_transcript"
                               and e.get("utterance_id") == question_line[2]["utterance_id"]
                               and e.get("heard") == "full", 10, question_line[0])
            # Heard by the caller: the pump's 150 ms lead and the client's 150 ms lag after the
            # last audio was delivered, and the 150 ms a reply must come after it (run 13: at
            # 0.25 s the answer began too early and the question was asked again).
            await asyncio.sleep(0.6)
            await session.say("68_window_please")
        answered = await session.wait(lambda e: e["type"] == "task_answer_received", 15, lima_from)
        await settled(session, start, timeout=180)
    finally:
        stand_in.ask_seat = None
    await session.close()
    ctx.record(session)

    job = next((job for job in stand_in.jobs.values() if job["created"] >= session.opened
                and "flight" in job["request"].lower()), {})
    lines = session.transcripts(start)
    by_id = lambda kind: [(at, e) for at, e in lines if e.get("kind") == kind]
    lead_in = [(at, e) for at, e in lines if e.get("delta") == "Quick question."]
    anyway = [(at, e) for at, e in lines if e.get("delta") == "Anyway,"]
    relays = [(at, e) for at, e in by_id("relay") if e.get("task_id") == (lima[2]["task_id"] if lima else "")]
    cut = [e for _, e in session.all("assistant_transcript", start)
           if e.get("heard") == "partial" and e.get("kind") == "relay"]
    cut_texts = {e2.get("delta") for _, e2 in relays
                 for e in cut if e2.get("utterance_id") == e.get("utterance_id")}
    asked_at = asked[1] if asked else 0.0
    q_event = asked[2] if asked else {}
    question_at = question_line[1] if question_line else 0.0
    stopped = [e for _, e in session.all("agent_audio_stopped", start) if e.get("reason") == "policy"]
    cut_at = next((at for at, e in session.all("agent_audio_stopped", start) if e.get("reason") == "policy"), None)
    answer_said = next((at for at, e in session.all("stt_result", lima_from)
                        if e.get("is_final") and "window" in e.get("transcript", "").lower()), None)
    resumed = [at for at, e in relays if anyway and at > anyway[0][0] and e.get("delta") in cut_texts]
    seconds = deadline_ms / 1000
    checks[f"the question is on the screen at once with its {seconds:g} s"] = bool(q_event) and (
        q_event.get("expires_at_ms", 0) - q_event.get("t_ms", 0) == deadline_ms)
    # The lead-in ("Quick question.") only when 6 s or more are left (the gateway's
    # LEAD_IN_LEFT_MS): with 5 s it made every answer late (r5-latency).
    if deadline_ms >= 6000:
        checks["it broke into the answer at a clause boundary (a cut of the answer, a lead-in)"] = (
            bool(stopped) and bool(lead_in) and bool(relays) and relays[0][0] < lead_in[0][0])
    else:
        checks["it broke into the answer at a clause boundary (a cut of the answer, the question "
               "with no lead-in)"] = (
            bool(stopped) and not lead_in and bool(relays) and bool(question_line)
            and relays[0][0] < question_at)
    checks["asked within the deadline"] = bool(question_line) and question_at - asked_at < seconds
    checks["answered by voice"] = bool(answered) and answered[2].get("by") == "voice" and \
        answered[2].get("outcome") == "accept"
    checks["the agent had the answer before its deadline"] = (
        "answered_at" in job and job["answered_at"] - job["asked_at"] < seconds
        and "window" in str(job.get("answer", "")).lower())
    checks["the interrupted answer resumed after 'Anyway,' with the sentence that was cut"] = (
        bool(anyway) and bool(cut_texts) and bool(resumed))
    timeline = (f"question {asked_at - session.opened:.1f} s, policy cut "
                f"{(cut_at - asked_at) if cut_at else -1:+.2f} s, asked aloud "
                f"{question_at - asked_at:+.2f} s, lead-in "
                f"{(lead_in[0][0] - asked_at) if lead_in else -1:+.2f} s, caller's answer final "
                f"{(answer_said - asked_at) if answer_said else -1:+.2f} s, agent answered "
                f"{(job.get('answered_at', 0) - job.get('asked_at', 0)):.2f} s after asking "
                f"({job.get('answer')!r}); cut lines {sorted(t for t in cut_texts if t)[:2]}; "
                f"anyway {len(anyway)}, resumed {len(resumed)}")
    ctx.add(label, f"a question with a {seconds:g} s deadline breaks into the answer, then the answer resumes",
            all(checks.values()), timeline + "; " + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks),
            " | ".join(e.get("delta", "") for _, e in lines)[:600])


async def ma2l(ctx: Ctx):
    """MA2L MA2 with an 8 s deadline (the same checks; reported apart, MA2 keeps its 5 s)"""
    await ma2(ctx, 8000, "MA2L")


async def ma3(ctx: Ctx):
    """MA3 follow-ups: a question answered from the result, an action on the same A2A context"""
    stand_in = ctx.stack.ensure_backends()
    checks: Dict[str, bool] = {}
    block = {"front": {"profile": "model"}, "hitl": {"channels": ["voice"]}}
    session = await orchestrated_session(ctx, "MA3", block=block, tasks_extra={"announce": "tell"})
    await session.warm()
    start, mark = len(session.events), len(stand_in.seen)
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    answers: List[float] = []
    caller = asyncio.create_task(answer_questions(session, start, answers))
    try:
        await say_heard(session, "63_book_flight_from", is_final)
        flight = await session.wait(lambda e: e["type"] == "task_created" and e.get("agent") == "browser",
                                    120, start)
        flight_id = flight[2]["task_id"] if flight else ""
        await session.wait(lambda e: e["type"] == "task_result" and e.get("task_id") == flight_id, 120, start)
        await updates_said(session, start, 60)
        await settled(session, start, timeout=120)
        # A question the stored result answers: no new task.
        before = len([1 for _, e in session.all("task_created", start)])
        asked_from = len(session.events)
        await session.say("69_when_leave")
        await session.wait(is_final, 15, asked_from)
        await settled(session, asked_from, timeout=60)
        after_question = len([1 for _, e in session.all("task_created", start)])
        answer_lines = [e.get("delta", "") for _, e in session.transcripts(asked_from)]
        # More work for the same agent: a follow-up on the flight's A2A context.
        more_from = len(session.events)
        await session.say("70_window_too")
        await session.wait(is_final, 15, more_from)
        follow = await session.wait(lambda e: e["type"] == "task_created" and e.get("follows"), 60, more_from)
        if follow:
            await session.wait(lambda e: e["type"] == "task_result" and e.get("task_id") == follow[2]["task_id"],
                               120, more_from)
        await settled(session, more_from, timeout=120)
    finally:
        caller.cancel()
    await session.close()
    ctx.record(session)

    sends = [hit for hit in stand_in.of("a2a", "SendMessage", mark)]
    first = next((hit for hit in sends if "flight" in json.dumps(hit["body"]).lower()), None)
    flight_job = next((job for job in stand_in.jobs.values() if job["created"] >= session.opened
                       and "flight" in job["request"].lower() and job["wire"] == "a2a"), {})
    later = [hit for hit in sends if "window" in json.dumps(hit["body"]).lower()]
    later_context = (later[0]["body"].get("params", {}).get("message", {}).get("contextId")
                     if later else None)
    said_answer = " ".join(answer_lines)
    checks["the follow-up question was answered from the result without a new task"] = (
        after_question == before and ("9:40" in said_answer or "nine forty" in said_answer.lower()))
    checks["the follow-up action is a task on the same agent that follows the flight"] = bool(follow) and (
        follow[2].get("agent") == "browser" and follow[2].get("follows") == flight_id
        and follow[2].get("parent_id") is None)
    checks["it continued the flight's A2A contextId"] = bool(later_context) and (
        later_context == flight_job.get("context"))
    ctx.add("MA3", "a follow-up question answered from the result; a follow-up action on the same A2A context",
            all(checks.values()),
            f"answer to the question {said_answer[:160]!r}; tasks before/after the question {before}/{after_question}; "
            f"follow-up {follow[2].get('task_ref') if follow else None} follows {follow[2].get('follows') if follow else None}; "
            f"contextId flight {flight_job.get('context')} follow-up {later_context}; "
            + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks))


async def ma5(ctx: Ctx):
    """MA5 a hesitant request, said in two goes, stays one task"""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    block = {**MODEL_FRONT, "tasks": {"default_backend": "agent"}, "hitl": {"channels": ["voice"]}}
    session = await Session.open(ctx, "MA5", block, port=stand_in.port)
    await session.warm()
    start = len(session.events)
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    # The caller pauses 0.9 s, long enough for the end of a turn, and goes on (the experience
    # evaluation's disfluent requests went on 0.1 to 1.0 s after their task was made).
    await session.say("71_apartment_first")
    await asyncio.sleep(0.9)
    await session.say("72_apartment_more")
    await settled(session, start, timeout=60)
    await session.close()
    ctx.record(session)
    created = [e for _, e in session.all("task_created", start)]
    results = [e for _, e in session.all("task_result", start)]
    sends = [" ".join(p.get("text", "") for p in hit["body"].get("params", {}).get("message", {}).get("parts", []))
             for hit in stand_in.of("a2a", "SendMessage", mark)]
    joined = [text for text in sends if "apartment" in text.lower() and "budget" in text.lower()]
    turns = [e for _, e in session.all("stt_result", start) if e.get("is_final")]
    revision = results[-1].get("revision", 1) if results else 0
    checks = {
        "one task for the request said in two goes": len(created) == 1,
        "the agent's last request holds both parts": bool(sends) and sends[-1] in joined,
        # Two finals: two turns, so the second revised the task (one turn needs no revision).
        "it completed (revised when the request came in two turns)": bool(results)
        and results[-1].get("state") == "completed" and (len(turns) < 2 or revision >= 2),
    }
    ctx.add("MA5", "a hesitant request stays one task", all(checks.values()),
            f"{len(created)} task(s), {len(turns)} finals, revision {revision}; requests to the agent "
            f"{sends}; " + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks))


async def ma6(ctx: Ctx):
    """MA6 a caller with a resume key calls back on a new stream_id and gets the pending result"""
    stand_in = ctx.stack.ensure_backends()
    key = f"live-caller-{ctx.run_id}-{os.urandom(3).hex()}"
    block = {**MODEL_FRONT, "tasks": {"default_backend": "agent", "resume_key": key,
                                   "reconnect_grace_s": 60},
             "hitl": {"channels": ["voice"]}}
    first = await Session.open(ctx, "MA6-first", block, port=stand_in.port)
    await first.warm()
    start = len(first.events)
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    await first.say("73_parcel_status")
    await first.wait(is_final, 15, start)
    created = await first.wait(lambda e: e["type"] == "task_created", 30, start)
    task_id = created[2]["task_id"] if created else None
    echoed = first.ready.get("interaction", {}).get("tasks", {}).get("resume")
    # The caller hangs up while the check runs, and calls back from another device.
    await first.close()
    await asyncio.sleep(1.0)
    back = await Session.open(ctx, "MA6-back", block, port=stand_in.port,
                              extra={"stream_id": f"{first.stream_id}-callback"})
    begun = len(back.events)
    result = await back.wait(lambda e: e["type"] == "task_result", 60, begun)
    heard = await back.wait(lambda e: e["type"] == "assistant_transcript"
                            and "Thursday" in e.get("delta", ""), 30, begun)
    await settled(back, begun, timeout=30)
    await back.close()
    ctx.record(back)
    checks = {
        "ready echoes that a resume key is set, never the key": echoed is True
        and key not in json.dumps(first.ready),
        "the call-back is another stream_id": back.stream_id != first.stream_id,
        "it got the task's outcome": bool(result) and result[2].get("task_id") == task_id
        and result[2].get("state") == "completed",
        "and heard it": heard is not None,
    }
    ctx.add("MA6", "a resume key brings a pending result to a call-back on a new stream_id",
            all(checks.values()),
            f"task {task_id}; first {first.stream_id}, back {back.stream_id}; result "
            f"{(result[1] - back.opened) if result else -1:.1f} s after the call-back; "
            + "; ".join(f"{k}: {v}" for k, v in checks.items() if not v),
            "all hold: " + "; ".join(checks))


# ---- conversation controls (plan chapter 9)

CODE_LINE = "Your confirmation code is 4 4 1 7."


def controls_of(session: Session, start: int = 0) -> List[dict]:
    return [e for _, e in session.all("control_applied", start)]


async def control_turn(session: Session, clip: str, timeout: float = 20.0) -> Optional[dict]:
    """A caller turn that should be a control: the `control_applied` it gives."""
    start = len(session.events)
    await session.turn(clip, settle=20)
    found = await session.wait(lambda e: e["type"] == "control_applied", timeout, start)
    return found[2] if found else None


async def ct1(ctx: Ctx):
    """Spoken controls: slower and faster change the same line's length, say that again, the number, stop with the cut marker in the backend's next request."""
    stand_in = ctx.stack.ensure_backends()
    session = await backend_session(ctx, "CT1", "hook")
    await session.warm()
    checks: Dict[str, bool] = {}
    base = await pace_line(session)
    slower = await control_turn(session, "80_slower_please")
    slow = await pace_line(session)
    faster = await control_turn(session, "81_faster_please")
    fast = await pace_line(session)
    checks["'slower' is a rate control the gateway applied"] = bool(slower) and (
        slower.get("kind"), slower.get("value"), slower.get("outcome")) == ("rate", "slower", "applied")
    checks["'faster' is a rate control the gateway applied"] = bool(faster) and (
        faster.get("kind"), faster.get("value"), faster.get("outcome")) == ("rate", "faster", "applied")
    checks["the line is longer after slower (>= 1.08 x) and back after faster (0.92-1.08 x)"] = (
        base > 0 and slow / base >= 1.08 and 0.92 <= fast / base <= 1.08)

    # Say that again: the last line the caller heard; the number: only its number.
    start = len(session.events)
    await session.send({"type": "speak", "text": CODE_LINE})
    await heard(session, await spoken(session, start, "notice", 20), 30)
    await session.quiet(20, start)
    again_from = len(session.events)
    again = await control_turn(session, "82_say_again")
    await session.quiet(20, again_from)
    repeated = [e["delta"] for _, e in session.transcripts(again_from) if e.get("delta")]
    number_from = len(session.events)
    number = await control_turn(session, "83_the_number")
    await session.quiet(20, number_from)
    number_lines = [e["delta"] for _, e in session.transcripts(number_from) if e.get("delta")]
    checks["'say that again' repeats the last heard line"] = bool(again) and again.get("kind") == "repeat" \
        and CODE_LINE in repeated
    checks["'what was the number' says only the number"] = bool(number) and number.get("value") == "number" \
        and number_lines == ["4 4 1 7"]

    # Stop: the agent is cut on the front's reading of the interim words; the backend's next
    # request has the heard part with the cut marker, the control's note and the state.
    line = await say_long_line(session)
    await asyncio.sleep(1.5)
    pcm = load_wav(ctx.caller("44_stop_talking"))
    said = await session.say("44_stop_talking", pcm, block=False)
    voiced = said.start + first_voiced_ms(pcm) / 1000
    stopped = await session.wait(
        lambda e: e["type"] == "agent_audio_stopped" and e.get("reason") != "client_clear", 8, line[0])
    words = await session.wait(
        lambda e: e["type"] == "stt_result" and "stop" in e.get("transcript", "").lower(), 8, line[0])
    stop = await session.wait(lambda e: e["type"] == "control_applied" and e.get("kind") == "stop", 15, line[0])
    await session.quiet(20, line[0])
    mark = len(stand_in.seen)
    await session.turn("20_q_capital")
    tasks = [hit["body"] for hit in stand_in.of("hook", None, mark) if hit["body"].get("type") == "task"]
    history = tasks[0].get("history", []) if tasks else []
    contents = [turn.get("content", "") for turn in history]
    state = (tasks[0].get("metadata") or {}).get("waav_state") if tasks else None
    await session.close()
    cut_ms = ms(stopped[1] - voiced) if stopped else None
    after_words = ms(stopped[1] - words[1]) if stopped and words else None
    checks["'stop talking' over the agent cuts it (barge_in) and is a stop control"] = (
        bool(stopped) and stopped[2].get("reason") == "barge_in" and bool(stop))
    checks["the backend's next request has the cut marker, the note and the state"] = (
        any(c.endswith("[cut off by the caller]") for c in contents)
        and "[the caller asked the agent to stop talking]" in contents
        and bool(state) and state.get("rate") == 1.0)
    report(ctx, "CT1", "spoken controls: rate, repeat, slot repeat, stop with the cut marker", checks,
           f"line {base:.2f} s, after slower {slow:.2f} s ({slow / base if base else 0:.2f} x), after faster "
           f"{fast:.2f} s ({fast / base if base else 0:.2f} x); controls {[(c.get('kind'), c.get('value'), c.get('outcome')) for c in controls_of(session)]}; "
           f"repeated lines {repeated}; number lines {number_lines}; stop: cut {cut_ms} ms after the cue's "
           f"first voiced frame, {after_words} ms after the first transcript with 'stop'; next request history "
           f"{contents[-6:]}; waav_state {state}",
           "control_applied rate slower/faster applied; the same client line >= 1.08 x as long after slower "
           "(expected 1 / 0.85 = 1.18) and 0.92-1.08 x after faster; the code line said again; only "
           "'4 4 1 7' after 'what was the number'; agent_audio_stopped barge_in and control_applied stop; the "
           "webhook's next `task` history holds an agent line ending '[cut off by the caller]' and the note "
           "'[the caller asked the agent to stop talking]', metadata.waav_state rate 1.0",
           "every caller turn is decided by the front model (no phrase list); the cut times are reported "
           "against S1's lexicon path, not checked here")
    return session


async def ct2(ctx: Ctx):
    """Language controls: Spanish on Deepgram nova-3 multi with an Aura-2 Spanish voice (partial: no reviewed pack), Chinese answered honestly as unsupported."""
    port = ctx.stack.ensure_controls_gateway()
    if port is None:
        ctx.add("CT2", "a language switch and an honest unsupported", False,
                "NOT RUN: DEEPGRAM_API_KEY is not in the environment nor in --deepgram-env",
                "the Spanish switch needs a multilingual recogniser and a voice list; neither is local")
        return None
    extra = {
        "stt_config": {"provider": "deepgram", "model": "nova-3", "language": "multi",
                       "sample_rate": CALLER_RATE, "channels": 1, "punctuation": True, "encoding": "linear16"},
        "tts_config": {"provider": "deepgram", "voice_id": "aura-2-thalia-en", "audio_format": "pcm",
                       "sample_rate": AGENT_RATE},
    }
    session = await Session.open(ctx, "CT2", MODEL_FRONT, PLAIN_PROMPT, extra=extra, port=port)
    await session.warm()
    spanish = await control_turn(session, "84_in_spanish", 30)
    await session.quiet(20)
    updated = [e.get("state") for _, e in session.all("interaction_updated") if e.get("state")]
    state = updated[-1] if updated else {}
    answer_from = len(session.events)
    await session.turn("87_store_open")
    answer = " ".join(e["delta"] for _, e in session.transcripts(answer_from, "relay") if e.get("delta"))
    chinese_from = len(session.events)
    chinese = await control_turn(session, "85_in_chinese", 30)
    await session.quiet(20, chinese_from)
    honest = [e["delta"] for _, e in session.transcripts(chinese_from) if e.get("delta")]
    await session.close()
    spanish_words = sum(1 for word in re.findall(r"[a-záéíóúñ]+", answer.lower())
                        if word in {"la", "el", "de", "que", "abre", "tienda", "las", "los", "a", "en", "es", "una"})
    checks = {
        "'talk to me in Spanish' is a language control, partial (no reviewed pack)": bool(spanish)
            and (spanish.get("kind"), spanish.get("value"), spanish.get("outcome")) == ("language", "es", "partial"),
        "the state names Spanish and an Aura-2 Spanish voice": state.get("language") == "es"
            and str(state.get("voice", "")).endswith("-es"),
        "the next answer is in Spanish": spanish_words >= 3,
        "'talk to me in Chinese' is unsupported and said honestly": bool(chinese)
            and (chinese.get("kind"), chinese.get("value"), chinese.get("outcome")) == ("language", "zh", "unsupported")
            and any("I can only speak" in line for line in honest),
    }
    report(ctx, "CT2", "a language switch with a recogniser and a voice; an honest unsupported", checks,
           f"spanish {spanish}; state {state}; next answer {answer!r} ({spanish_words} Spanish function words); "
           f"chinese {chinese}; lines after it {honest}",
           "control_applied language es partial; state language es with an `*-es` voice; the next relayed "
           "answer has >= 3 Spanish function words; control_applied language zh unsupported and a line "
           "'Sorry, I can only speak ...'",
           "Deepgram nova-3 multi + Aura-2 on a gateway of this run (`--controls-gateway-port`) whose operator "
           "block allows es and zh with answers_only; spend is recorded by the caller of this script")
    return session


async def ct3(ctx: Ctx):
    """A client's `control` message, and an orchestrator-issued conversation_control."""
    session = await Session.open(ctx, "CT3", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.send({"type": "control", "kind": "verbosity", "value": "brief"})
    await session.send({"type": "control", "kind": "rate", "value": "loud"})
    applied = await session.wait(lambda e: e["type"] == "control_applied", 10, start)
    error = await session.wait(lambda e: e["type"] == "error" and "control" in e.get("message", ""), 10, start)
    updated = await session.wait(lambda e: e["type"] == "interaction_updated" and e.get("state"), 10, start)
    await session.close()
    # The front takes a caller's spoken wish itself (CT1), so the orchestrator is the one that
    # decides here: its prompt asks for brief answers before the first one.
    prompt = ORCHESTRATOR_PROMPT + (" Before you answer the caller's first question, call the tool "
                                    "conversation_control with kind verbosity and value brief.")
    orch = await orchestrated_session(ctx, "CT3-orchestrator", prompt=prompt)
    await orch.warm()
    o_start = len(orch.events)
    await orch.turn("62_capital_peru", settle=90)
    by_agent = await orch.wait(lambda e: e["type"] == "control_applied" and e.get("source") == "agent", 60, o_start)
    o_updated = [e.get("state") for _, e in orch.all("interaction_updated", o_start) if e.get("state")]
    await orch.close()
    checks = {
        "the client's control is applied and reported": bool(applied)
            and (applied[2].get("kind"), applied[2].get("source"), applied[2].get("outcome")) == ("verbosity", "client", "applied"),
        "a malformed control is refused with an error": bool(error),
        "the state follows (verbosity brief)": bool(updated) and updated[2]["state"].get("verbosity") == "brief",
        "the orchestrator's conversation_control is applied": bool(by_agent)
            and (by_agent[2].get("kind"), by_agent[2].get("outcome")) == ("verbosity", "applied"),
    }
    report(ctx, "CT3", "a client control and an orchestrator-issued control", checks,
           f"client: {applied[2] if applied else None}; error {error[2] if error else None}; state "
           f"{updated[2]['state'] if updated else None}; orchestrator: {by_agent[2] if by_agent else None}; "
           f"states {o_updated[-1:] }",
           "control_applied verbosity client applied; `error` for rate 'loud'; interaction_updated state "
           "verbosity brief; control_applied source agent verbosity applied on the orchestrator session",
           "the orchestrator is MA1's orchestrator model with one rule sentence added to its prompt; "
           "the caller asks a plain question (a spoken wish is the front's, CT1)")
    return orch


# ---- the controls of the gap round: a person, another voice, a line again slower or
# louder, the recogniser in the new language (plan chapter 9)

PERSON_LINE = "Of course, I'm putting you through to a person now."
NO_PERSON_LINE = "Sorry, there's no one I can put you through to on this line."
VOICE_LINE = "Okay, I'll speak in this voice now."
# CT7's Spanish caller, synthesised once through the controls gateway's own Aura-2 Spanish
# voice (`/speak`: the key stays in that gateway).
SPANISH_CALLER = ("93_store_open_es", "¿A qué hora abre la tienda?", "aura-2-celeste-es")
VENDOR_CLIPS[SPANISH_CALLER[0]] = SPANISH_CALLER[1:]


async def line_measure(session: Session, line) -> Tuple[float, float]:
    """Seconds of a line's audio the client got, and its speech level (the 90th percentile of
    its 20 ms frames' dBFS)."""
    end = await heard(session, line, 40)
    since, until = line[1] - 0.06, end + 0.05
    seconds = sum(len(data) for at, data in session.audio if since <= at <= until) / 2 / AGENT_RATE
    levels = sorted(db for _, db in session.levels(since, until) if db > AUDIBLE_DB)
    return seconds, (levels[int(len(levels) * 0.9)] if levels else -99.0)


async def said_line(session: Session, start: int, text: str, timeout: float = 30.0):
    found = await session.wait(lambda e: e["type"] == "assistant_transcript" and e.get("delta") == text,
                               timeout, start)
    if found is None:
        raise RuntimeError(f"{text!r} was not said within {timeout:.0f} s")
    return found


async def ct4(ctx: Ctx):
    """'Let me talk to a person': with controls.human.url the operator gets a signed request and answers on the callback route: human_connected is the persona's line and the end, human_unavailable an honest line and the call goes on; without a transfer, an honest line."""
    hook = Webhook(ctx.args.tool_secret)
    port = ctx.stack.ensure_human_gateway(hook.url)
    secret = ctx.args.tool_secret

    async def ask(label: str, answer: str):
        session = await Session.open(ctx, label, MODEL_FRONT, PLAIN_PROMPT, port=port)
        await session.warm()
        start, mark = len(session.events), len(hook.calls)
        applied = await control_turn(session, "90_a_person", 30)
        requested = await session.wait(lambda e: e["type"] == "human_requested", 15, start)
        finding = await said_line(session, start, FINDING_LINE)
        deadline = now() + 10
        while now() < deadline and not [c for c in hook.calls[mark:]
                                        if (c.get("body") or {}).get("type") == "human_requested"]:
            await asyncio.sleep(0.1)
        pushes = [c for c in hook.calls[mark:] if (c.get("body") or {}).get("type") == "human_requested"]
        body = pushes[0]["body"] if pushes else {}
        # The person (or nobody) answers 2 s after the request.
        await asyncio.sleep(2.0)
        answered_at = now()
        status = await asyncio.to_thread(signed_callback, port, str(body.get("task_id")),
                                         {"type": answer}, secret)
        return session, start, applied, requested, finding, pushes, body, answered_at, status

    (session, start, applied, requested, finding, pushes, body, answered_at,
     status) = await ask("CT4", "human_connected")
    line = await said_line(session, start, PERSON_LINE)
    outcome = await session.wait(lambda e: e["type"] == "human_answered", 10, start)
    deadline = now() + 30
    while session.closed is None and now() < deadline:
        await asyncio.sleep(0.2)
    closed = session.closed
    try:
        await session.close()
    except Exception:
        pass
    history = [turn.get("content") or "" for turn in body.get("history", [])]
    # Nobody free: the operator says so on the route, and the call goes on.
    (busy, b_start, _, _, b_finding, b_pushes, b_body, _, b_status) = await ask("CT4-unavailable",
                                                                            "human_unavailable")
    no_one = await said_line(busy, b_start, NO_ANSWER_LINE)
    b_outcome = await busy.wait(lambda e: e["type"] == "human_answered", 10, b_start)
    await busy.quiet(5, b_start)
    busy_open = busy.closed is None
    await busy.close()
    # The main gateway has no transfer: said honestly, the call goes on.
    plain = await Session.open(ctx, "CT4-none", MODEL_FRONT, PLAIN_PROMPT)
    await plain.warm()
    p_start = len(plain.events)
    none = await control_turn(plain, "90_a_person", 30)
    honest = await said_line(plain, p_start, NO_PERSON_LINE)
    await plain.quiet(10, p_start)
    still_open = plain.closed is None
    await plain.close()
    task_id = str(body.get("task_id"))
    checks = {
        "'let me talk to a person' is a human control, applied": bool(applied)
            and (applied.get("kind"), applied.get("outcome")) == ("human", "applied"),
        "the client gets human_requested via webhook": bool(requested) and requested[2].get("via") == "webhook",
        "the caller hears that a person is being found": bool(finding),
        "the operator's stand-in got one signed request with the stream, state, history and its callback":
            len(pushes) == 1 and pushes[0]["signed"] and pushes[0]["signature_ok"]
            and body.get("stream_id") == session.stream_id and bool(body.get("state"))
            and any("person" in turn.lower() for turn in history) and task_id.startswith("human_")
            and body.get("callback") == f"/v1/tasks/{task_id}/callback",
        "human_connected on the callback route (202): the persona's line after it, then the close (1000 transferred)":
            status == 202 and bool(line) and line[1] >= answered_at and bool(outcome)
            and outcome[2].get("outcome") == "connected" and bool(closed) and closed[1] == 1000
            and closed[2] == "transferred" and closed[0] >= line[1],
        "human_unavailable: the honest line, and the call goes on": b_status == 202 and bool(b_finding)
            and len(b_pushes) == 1 and bool(no_one) and bool(b_outcome)
            and b_outcome[2].get("outcome") == "unavailable" and busy_open,
        "without a transfer: unsupported, said, the call goes on": bool(none)
            and (none.get("kind"), none.get("outcome")) == ("human", "unsupported") and bool(honest) and still_open,
    }
    report(ctx, "CT4", "a person: the operator's signed request, its answer on the callback route, the line and the end; else an honest line", checks,
           f"applied {applied}; human_requested {requested[2] if requested else None}; finding line at "
           f"{(finding[1] - session.opened) if finding else -1:.1f} s; callback {status}; the line "
           f"{(line[1] - answered_at) if line else -1:.2f} s after it; human_answered {outcome[2] if outcome else None}; "
           f"close {closed[1:] if closed else None} {(closed[0] - line[1]) if closed and line else -1:.1f} s after "
           f"the line began; pushes {[(c['signed'], c['signature_ok'], c['status']) for c in pushes]}; task_id "
           f"{task_id}; history {history[-4:]}; unavailable: callback {b_status}, "
           f"{no_one[2].get('delta') if no_one else None}, {b_outcome[2] if b_outcome else None}, open {busy_open}; "
           f"without a transfer {none}, {honest[2].get('delta') if honest else None}",
           f"control_applied human applied; human_requested via webhook; {FINDING_LINE!r}; exactly one signed "
           "(valid X-WaaV-Signature) POST with type human_requested, the session's stream_id, state, a history "
           "holding the caller's words, task_id human_* and callback /v1/tasks/<task_id>/callback; the signed "
           f"human_connected callback answered 202, then {PERSON_LINE!r} and close 1000 'transferred' after it; "
           f"human_unavailable: 202, {NO_ANSWER_LINE!r}, human_answered unavailable, the socket open 5 s later; "
           f"the main gateway: human unsupported and {NO_PERSON_LINE!r}, socket open",
           f"a gateway of this run on --human-gateway-port whose operator block sets controls.human.url to a "
           f"local webhook stand-in (wait_s {CT4_WAIT_S}); the scenario posts the operator's answers; every turn "
           "decided by the model front")
    return None


async def ct5(ctx: Ctx):
    """'Could you switch to a male voice?' on Deepgram Aura-2: another voice of the provider's list, the persona's line in it, the listening sounds and failure lines made again in it."""
    port = ctx.stack.ensure_controls_gateway()
    if port is None:
        ctx.add("CT5", "another voice", False, "NOT RUN: DEEPGRAM_API_KEY is not in the environment nor in --deepgram-env",
                "the voice list is Deepgram's; no local voice list exists")
        return None
    extra = {"tts_config": {"provider": "deepgram", "voice_id": "aura-2-thalia-en", "audio_format": "pcm",
                            "sample_rate": AGENT_RATE}}
    mark = ctx.stack.log_size_of("live-controls-gateway.log")
    session = await Session.open(ctx, "CT5", MODEL_FRONT, PLAIN_PROMPT, extra=extra, port=port)
    await session.warm()
    start = len(session.events)
    voice = await control_turn(session, "91_male_voice", 45)
    line = await said_line(session, start, VOICE_LINE, 45)
    await session.quiet(20, start)
    updated = [e.get("state") for _, e in session.all("interaction_updated", start) if e.get("state")]
    state = updated[-1] if updated else {}
    # The lines said when a service fails are made again in the new voice.
    switched_at = [l for l in ctx.stack.log_since_of("live-controls-gateway.log", "interaction control", mark)
                   if session.stream_id in l]
    deadline = now() + 90
    ready = []
    while now() < deadline:
        ready = ctx.stack.log_since_of("live-controls-gateway.log", "interaction failure lines ready", mark)
        ready = [l for l in ready if session.stream_id in l and switched_at and l > switched_at[-1]]
        if ready:
            break
        await asyncio.sleep(0.5)
    renders = ctx.stack.log_since_of("live-controls-gateway.log", "interaction voice", mark)
    await session.close()
    kept = lambda voice: sorted(p.name for p in (ctx.stack.log_dir / "controls-clips" / "deepgram" / voice
                                                 / "en" / "failure" / "cache").glob("*.wav"))
    chosen = str(state.get("voice", ""))
    checks = {
        "a voice control (male), applied": bool(voice)
            and (voice.get("kind"), voice.get("value"), voice.get("outcome")) == ("voice", "male", "applied"),
        "the state names another Aura-2 English voice": chosen.startswith("aura-2-") and chosen.endswith("-en")
            and chosen != "aura-2-thalia-en",
        "the persona's line is said": bool(line),
        "the failure lines are made again for the new voice (all nine kept under it)": len(ready) >= 1
            and len(kept(chosen)) == 9,
    }
    report(ctx, "CT5", "another voice of the provider; the clips and failure lines again in it", checks,
           f"control {voice}; state voice {chosen}; line at {(line[1] - session.opened) if line else -1:.1f} s; "
           f"'failure lines ready' x{len(ready)} for this session; kept for {chosen}: {len(kept(chosen))}, "
           f"for thalia: {len(kept('aura-2-thalia-en'))}; voice log {renders[-2:]}",
           "control_applied voice male applied; state.voice an aura-2-*-en voice other than thalia; the line "
           f"{VOICE_LINE!r}; after it the gateway logs 'interaction failure lines ready' and keeps all nine failure "
           "lines under the new voice (clips_dir cache)",
           "Deepgram Aura-2 speech (voice list from Deepgram /v1/models) on the controls gateway, the "
           "local recogniser; spend recorded by the caller of this script")
    return None


async def ct6(ctx: Ctx):
    """'Please say that last part again more slowly' (spoken) and a `repeat louder` (the client's control): that line only, slower or louder. CT6L: the front's reading of 'Can you say that again louder please?'."""
    session = await Session.open(ctx, "CT6", MODEL_FRONT, PLAIN_PROMPT)
    await session.warm()

    async def client_line():
        start = len(session.events)
        await session.send({"type": "speak", "text": PACE_LINE})
        line = await spoken(session, start, "notice", 20)
        measured = await line_measure(session, line)
        await session.quiet(20, start)
        return measured

    async def again(start: int):
        line = await said_line(session, start, PACE_LINE, 30)
        measured = await line_measure(session, line)
        await session.quiet(20, start)
        return measured

    base = await client_line()
    start = len(session.events)
    slow_control = await control_turn(session, "94_last_part_slowly", 30)
    slow = await again(start)
    after_slow = await client_line()
    # The front's reading of a spoken "again, louder" (reported as CT6L); a volume control it
    # gave instead is undone so the session's level stays the reference.
    start = len(session.events)
    spoken_loud = await control_turn(session, "95_again_louder", 30)
    await session.quiet(20, start)
    if spoken_loud and spoken_loud.get("kind") == "volume":
        await session.send({"type": "control", "kind": "volume", "value": "normal"})
        await session.wait(lambda e: e["type"] == "control_applied" and e.get("value") == "normal", 10, start)
    await session.quiet(20, start)
    after_spoken = await client_line()
    # The same per-line step from the client app.
    start = len(session.events)
    await session.send({"type": "control", "kind": "repeat", "value": "louder"})
    loud_found = await session.wait(lambda e: e["type"] == "control_applied" and e.get("kind") == "repeat", 10, start)
    loud_control = loud_found[2] if loud_found else None
    loud = await again(start)
    after_loud = await client_line()
    updated = [e.get("state") for _, e in session.all("interaction_updated") if e.get("state")]
    state = updated[-1] if updated else {}
    await session.close()
    ratio = lambda measured: measured[0] / base[0] if base[0] else 0.0
    reference = after_spoken[1]
    checks = {
        "'that last part again more slowly' is repeat slower": bool(slow_control)
            and (slow_control.get("kind"), slow_control.get("value")) == ("repeat", "slower")
            and slow_control.get("outcome") == "applied",
        "the line said again is longer (>= 1.08 x)": ratio(slow) >= 1.08,
        "the next line is at the session's rate (0.92-1.08 x)": 0.92 <= ratio(after_slow) <= 1.08,
        "the client's repeat louder is applied": bool(loud_control)
            and (loud_control.get("value"), loud_control.get("outcome")) == ("louder", "applied"),
        "the line said again is louder (+2 to +4 dB)": 2.0 <= loud[1] - reference <= 4.0,
        "the next line is at the session's volume (within 1 dB)": abs(after_loud[1] - reference) <= 1.0,
        "the session's rate and volume did not move": state.get("rate") == 1.0 and state.get("volume_db") == 0.0,
    }
    report(ctx, "CT6", "a line again slower or louder, that line only", checks,
           f"line {base[0]:.2f} s {base[1]:.1f} dBFS; again slower {slow[0]:.2f} s ({ratio(slow):.2f} x); "
           f"next {after_slow[0]:.2f} s ({ratio(after_slow):.2f} x); reference {reference:.1f} dBFS; again louder "
           f"{loud[1]:.1f} dBFS ({loud[1] - reference:+.1f} dB); next {after_loud[1]:.1f} dBFS "
           f"({after_loud[1] - reference:+.1f} dB); controls "
           f"{[(c.get('kind'), c.get('value'), c.get('source'), c.get('outcome')) for c in controls_of(session)]}; "
           f"state rate {state.get('rate')} volume {state.get('volume_db')}",
           "repeat slower (spoken) applied, the line again >= 1.08 x as long (expected 1 / 0.85 = 1.18) and "
           "the next 0.92-1.08 x; repeat louder (client) applied, the line again +2 to +4 dB (expected 3) "
           "and the next within 1 dB; state rate 1.0, volume 0",
           "the local voice (rate taken by the OpenAI-compatible server); levels are the 90th percentile "
           "of 20 ms frames")
    ctx.add("CT6L", "the front reads 'Can you say that again louder please?' as repeat louder",
            bool(spoken_loud) and (spoken_loud.get("kind"), spoken_loud.get("value")) == ("repeat", "louder"),
            f"control {spoken_loud}", "control_applied repeat louder",
            "the front model's reading, in this session's context (the gate set reads the same words as "
            "repeat louder)")
    return None


async def ct7(ctx: Ctx):
    """A language switch on Deepgram nova-3 in English: a second recogniser opened in Spanish, swapped in at a pause; the Spanish caller is heard in Spanish."""
    port = ctx.stack.ensure_controls_gateway()
    if port is None:
        ctx.add("CT7", "the recogniser follows a language switch", False,
                "NOT RUN: DEEPGRAM_API_KEY is not in the environment nor in --deepgram-env",
                "the recogniser is Deepgram nova-3")
        return None
    name = SPANISH_CALLER[0]
    ctx.caller(name)
    extra = {
        "stt_config": {"provider": "deepgram", "model": "nova-3", "language": "en",
                       "sample_rate": CALLER_RATE, "channels": 1, "punctuation": True, "encoding": "linear16"},
        "tts_config": {"provider": "deepgram", "voice_id": "aura-2-thalia-en", "audio_format": "pcm",
                       "sample_rate": AGENT_RATE},
    }
    mark = ctx.stack.log_size_of("live-controls-gateway.log")
    session = await Session.open(ctx, "CT7", MODEL_FRONT, PLAIN_PROMPT, extra=extra, port=port)
    await session.warm()
    spanish = await control_turn(session, "84_in_spanish", 45)
    await session.quiet(20)
    opened = ctx.stack.log_since_of("live-controls-gateway.log", "interaction recogniser opening in the new language", mark)
    switched = ctx.stack.log_since_of("live-controls-gateway.log", "interaction recogniser switched", mark)
    heard_from = len(session.events)
    await session.turn(name, settle=30)
    finals = [e.get("transcript", "") for _, e in session.all("stt_result", heard_from) if e.get("is_final")]
    answer = " ".join(e["delta"] for _, e in session.transcripts(heard_from, "relay") if e.get("delta"))
    await session.close()
    words = " ".join(finals).lower()
    checks = {
        "'talk to me in Spanish' is a language control, partial": bool(spanish)
            and (spanish.get("kind"), spanish.get("value"), spanish.get("outcome")) == ("language", "es", "partial"),
        "a recogniser was opened in Spanish and swapped in": any("native=es" in l for l in opened) and bool(switched),
        "the Spanish caller is transcribed in Spanish": ("tienda" in words or "abre" in words),
        "and answered": bool(answer),
    }
    report(ctx, "CT7", "the recogniser follows a language switch (a second one, swapped at a pause)", checks,
           f"control {spanish}; opening {[l[-120:] for l in opened]}; switched {len(switched)}; finals {finals}; "
           f"answer {answer!r}",
           "control_applied language es partial; the gateway logs 'interaction recogniser opening in the new "
           "language' (native=es) and 'interaction recogniser switched'; a final transcript of the Spanish "
           "caller holds 'tienda' or 'abre'; a relayed answer follows",
           "Deepgram nova-3 `en` and Aura-2 on the controls gateway (es and zh, answers_only); the Spanish "
           "caller clip is Aura-2 celeste-es made through that gateway's /speak; spend recorded by the caller")
    return None


# ---- when services fail (plan chapter 10)

# SF3's voice: Piper en_GB-alan-medium behind the voice router, whose process SF3 kills.
SF3_VOICE = "sf3alan"
# SF2's background holds its answer this long.
SF2_HOLD_S = 40.0


def failure_notes(session: Session, start: int = 0) -> List[Tuple[float, dict]]:
    """The failure lines the session was told of (`interaction_updated.failure`)."""
    return [(at, e["failure"]) for at, e in session.all("interaction_updated", start)
            if isinstance(e.get("failure"), dict)]


def failure_segments(session: Session, start: int = 0) -> List[Tuple[float, float, str]]:
    """(start, end, utterance id) of each failure line's audio; end is the start + 8 s when no
    end came."""
    starts = [(at, e["utterance_id"]) for at, e in session.all("audio_segment", start)
              if e.get("kind") == "failure" and e.get("phase") == "start"]
    out = []
    for at, utt in starts:
        ends = [t for t, e in session.all("audio_segment", start)
                if e.get("utterance_id") == utt and e.get("phase") == "end"]
        out.append((at, ends[0] if ends else at + 8.0, utt))
    return out


def audio_between(session: Session, since: float, until: float) -> bytes:
    return b"".join(data for at, data in session.audio if since <= at <= until)


async def wait_failure(session: Session, kinds: Tuple[str, ...], start: int, timeout: float) -> Optional[Tuple[float, dict]]:
    found = await session.wait(lambda e: e["type"] == "interaction_updated"
                               and isinstance(e.get("failure"), dict)
                               and e["failure"].get("kind") in kinds, timeout, start)
    return (found[1], found[2]["failure"]) if found else None


async def failure_lines_ready(ctx: Ctx, session: Session, timeout: float = 90.0) -> Optional[float]:
    """Seconds until the gateway logged the session's failure lines made (`interaction failure
    lines ready`); None when not within `timeout` (the server's default set then stands in)."""
    start = now()
    while now() - start < timeout:
        if ctx.stack.log_lines("interaction failure lines ready", session.stream_id):
            return now() - start
        await asyncio.sleep(0.2)
    return None


def failure_summary(session: Session, start: int, t0: float) -> str:
    return ", ".join(f"{n['line']}@{at - t0:.1f}s" for at, n in failure_notes(session, start))


class StallingLlm:
    """SF2's background: an OpenAI-compatible chat server that holds every answer `hold_s`
    (a stand-in for a background LLM that stalls), then streams one sentence."""

    def __init__(self, hold_s: float):
        outer = self
        self.hold_s = hold_s
        self.requests: List[float] = []
        self.hung_up: List[float] = []

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_POST(self):
                asked = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                outer.requests.append(now())
                time.sleep(outer.hold_s)
                chunk = {"id": "sf2", "object": "chat.completion.chunk", "model": "sf2",
                         "choices": [{"index": 0, "delta": {"content": "It opens at nine."},
                                      "finish_reason": None}]}
                done = {**chunk, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
                body = (f"data: {json.dumps(chunk)}\n\ndata: {json.dumps(done)}\n\n"
                        "data: [DONE]\n\n").encode()
                kind = "text/event-stream"
                try:
                    streamed = json.loads(asked or b"{}").get("stream", True)
                except ValueError:
                    streamed = True
                if streamed is False:
                    # SF5's DAG node asks for one answer, not a stream.
                    body = json.dumps({"id": "sf2", "object": "chat.completion", "created": int(time.time()),
                                       "model": "sf2", "choices": [
                        {"index": 0, "message": {"role": "assistant", "content": "It opens at nine."},
                         "finish_reason": "stop"}],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 5, "total_tokens": 6}}).encode()
                    kind = "application/json"
                try:
                    self.send_response(200)
                    self.send_header("Content-Type", kind)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    outer.hung_up.append(now())

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/v1"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()


async def sf1(ctx: Ctx):
    """SF1: the front model's server is killed mid-call: the waiting turn gets a stall line, then the back line once the gateway's supervisor restarted it, and the turn is answered."""
    freed = ctx.stack.stop_front()
    try:
        await sf1_run(ctx)
    finally:
        if freed:
            ctx.stack.start_front()


async def sf1_run(ctx: Ctx):
    port, front = ctx.stack.ensure_front_gateway()
    first_ready = await until_log(ctx, "front model ready", 1, 150)
    session = await Session.open(ctx, "SF1", MODEL_FRONT, PLAIN_PROMPT, port=port)
    try:
        await session.warm()
        # The session's failure lines are made at its start (this gateway has no default set).
        made = await until_log(ctx, f"interaction failure lines ready stream_id={session.stream_id}", 1, 90)
        await session.turn("20_q_capital")
        pids = child_pids(front)
        killed = now()
        for pid in pids:
            os.kill(pid, signal.SIGKILL)
        start = len(session.events)
        said = await session.say("21_q_week")
        stall = await wait_failure(session, ("stall",), start, 30)
        back_at = await until_log(ctx, "front model ready", 2, FRONT_BACK_LIMIT_S + 30)
        back = await wait_failure(session, ("back",), start, 60)
        answered = await session.wait(lambda e: e["type"] == "task_result" and e.get("state") == "completed",
                                      90, start)
        await session.quiet(20, start)
        segments = failure_segments(session, start)
        heard = audio_between(session, segments[0][0], segments[0][1]) if segments else b""
        summary = failure_summary(session, start, said.end)
    finally:
        await session.close()
        process = ctx.stack.started.pop("front-gateway", None)
        if process is not None:
            process.terminate()
            process.wait(timeout=40)
    checks = {
        "the gateway's model is ready, then killed": first_ready is not None and len(pids) == 1,
        "the session's failure lines were made at its start": made is not None,
        "the waiting turn gets a stall line about the front": bool(stall) and stall[1]["service"] == "front",
        "the stall line starts after the caller stopped": bool(segments) and segments[0][0] >= said.end,
        "it is heard": frame_db(heard) > AUDIBLE_DB,
        "the supervisor starts the model again": back_at is not None,
        "the back line once it is back": bool(back) and back_at is not None and back[0] >= back_at - 1.0,
        # After the back line, the socket's own sign of the restart: back_at is when a 250 ms poll
        # of the log saw it, which a turn answered at once (r16-verify: 100 ms after) precedes.
        "the turn is answered after the restart": bool(answered) and bool(back) and back_at is not None
            and answered[1] > back[0],
    }
    report(ctx, "SF1", "the front model's server killed mid-call", checks,
           f"killed {pids}; caller's turn ended {said.end - killed:.1f} s after the kill; failure lines "
           f"[{summary}] (seconds after the turn); stall line {len(heard) // 48} ms at "
           f"{frame_db(heard):.1f} dBFS; ready again {back_at - killed:.1f} s after the kill" if back_at else
           f"killed {pids}; failure lines [{summary}]; the model was not ready again",
           "after a SIGKILL of the gateway-run front llama-server, the next caller turn gets "
           "interaction_updated.failure kind stall (service front) whose audio_segment starts after the "
           "caller's speech ended and is louder than -50 dBFS; `front model ready` again; a back line at "
           "or after that; the turn's task completes after it",
           "the front gateway of FS1 (this run's front model, started by the gateway's supervisor)")


async def sf2(ctx: Ctx):
    """SF2: the background LLM stalls (a stand-in holds its answer for 40 s): stall lines at most every 8 s, at most three, then the exit line, session_failed and the socket closed."""
    llm = StallingLlm(SF2_HOLD_S)
    session = await Session.open(ctx, "SF2", MODEL_FRONT, extra={"conversation_config": {
        "base_url": llm.url, "model": "sf2-stall", "api_key": "local-dummy",
        "system_prompt": PLAIN_PROMPT}})
    try:
        await session.warm()
        made = await failure_lines_ready(ctx, session)
        start = len(session.events)
        turn = await session.turn("87_store_open", settle=0)
        e = turn.e or turn.said.end
        failed = await session.wait(lambda ev: ev["type"] == "session_failed", 75, start)
        deadline = now() + 10
        while session.closed is None and now() < deadline:
            await asyncio.sleep(0.1)
        notes = failure_notes(session, start)
        segments = failure_segments(session, start)
        acks = [at for at, ev in session.transcripts(start, "ack")]
        summary = failure_summary(session, start, e)
    finally:
        await session.close()
        llm.close()
    stalls = [(at, n) for at, n in notes if n["kind"] == "stall"]
    exits = [(at, n) for at, n in notes if n["kind"] in ("exit", "exit_follow_up")]
    starts = [seg[0] for seg in segments]
    gaps = [b - a for a, b in zip(starts, starts[1:len(stalls)])]
    failed_at = failed[1] if failed else None
    closed = session.closed
    checks = {
        "the request reached the background": bool(llm.requests),
        "the turn is acknowledged": bool(acks),
        "at most three stall lines, each another": 1 <= len(stalls) <= 3
            and len({n["line"] for _, n in stalls}) == len(stalls),
        "the first comes after the stall time": bool(starts) and starts[0] >= e + 2.0,
        "stall lines 8 s apart": all(gap >= 7.8 for gap in gaps),
        "the exit line 30 s after the first stall, before the answer": len(exits) == 1 and bool(starts)
            and exits[0][0] >= starts[0] + 29.5 and exits[0][0] < e + SF2_HOLD_S,
        "exit without the promise (no follow-up channel)": bool(exits) and exits[0][1]["line"] == "exit",
        "session_failed names the background and the pending task": bool(failed)
            and failed[2]["service"] == "background" and len(failed[2]["pending"]) == 1,
        "the gateway closes the socket after it (1011)": closed is not None and closed[1] == 1011
            and failed_at is not None and closed[0] >= failed_at,
    }
    report(ctx, "SF2", "a stalling background", checks,
           f"lines made {round(made, 1) if made is not None else None} s after the warm-up; "
           f"failure lines [{summary}] (seconds after E); stall gaps {[round(g, 1) for g in gaps]} s; "
           f"session_failed {round(failed_at - e, 1) if failed_at else None} s after E "
           f"({failed[2].get('reason') if failed else None}); close {closed[1:] if closed else None}; "
           f"the stand-in got {len(llm.requests)} request(s), the gateway hung up on {len(llm.hung_up)}",
           "an acknowledgement; 1-3 stall lines (distinct ids), the first >= 2.0 s after E (the first "
           "token is due 2.5 s after the run started), >= 7.8 s apart; one exit line (`exit`: no push "
           "address) >= 29.5 s after the first stall line and before the stand-in's answer at 40 s; "
           "session_failed with service background and the one live task; close code 1011 after it",
           f"the background is a stand-in OpenAI-compatible server in this script holding each answer "
           f"{SF2_HOLD_S:.0f} s; failure.stall_after_ms 2500 and max_wait_s 30 (the defaults)")


async def sf3(ctx: Ctx):
    """SF3: the local text-to-speech is killed mid-call: the stall line made at session start is still heard in the session's voice."""
    router: Optional[VoiceRouter] = ctx.stack.voices
    if router is None or "gateway" not in ctx.stack.started:
        raise RuntimeError("SF3 needs the gateway this run starts (--start-stack), behind the voice router")
    suffix = f"-{SF3_VOICE}"
    voice = f"live-{ctx.run_id}{suffix}"
    stall_text = "Sorry, my systems are acting up. Bear with me a moment."
    session = await Session.open(ctx, "SF3", MODEL_FRONT, PLAIN_PROMPT, voice=suffix)
    killed_pid = None
    try:
        await session.warm()
        made = await failure_lines_ready(ctx, session) is not None
        await session.turn("20_q_capital")
        killed = now()
        killed_pid = router.kill(SF3_VOICE)
        start = len(session.events)
        said = await session.say("21_q_week")
        stall = await wait_failure(session, ("stall",), start, 30)
        await asyncio.sleep(4.0)
        segments = failure_segments(session, start)
        heard = audio_between(session, segments[0][0] - 0.05, segments[0][1]) if segments else b""
        failed = await session.wait(lambda ev: ev["type"] == "session_failed", 60, start)
        summary = failure_summary(session, start, said.end)
        errors = [e.get("code") for e in session.errors()]
    finally:
        await session.close()
        router.revive(SF3_VOICE)
    rendered = router.rendered.get(voice, [])
    own = [pcm for text, pcm in rendered if text == stall_text]
    others = [pcm for v, made_lines in router.rendered.items() if v != voice for _, pcm in made_lines][:40]
    match_own = max((similarity(heard, pcm) for pcm in own), default=0.0)
    match_other = max((similarity(heard, pcm) for pcm in others), default=0.0)
    checks = {
        "the failure lines were made in the session's voice at its start": made and bool(own),
        "the voice's Piper was killed": killed_pid is not None,
        "a stall line about the voice": bool(stall) and stall[1]["service"] == "voice",
        "it is heard": frame_db(heard) > AUDIBLE_DB,
        "in the session's own voice (its recording at session start)": match_own >= 0.8 and match_own > match_other,
        "the call ends after the wait (the voice stays down)": bool(failed) and failed[2]["service"] == "voice",
    }
    report(ctx, "SF3", "the text-to-speech killed mid-call", checks,
           f"Piper pid {killed_pid} killed {said.end - killed:.1f} s before the turn ended; failure lines "
           f"[{summary}]; stall line {len(heard) // 48} ms at {frame_db(heard):.1f} dBFS, match with the "
           f"session voice's rendering {match_own:.2f}, with the other voice's lines {match_other:.2f}; "
           f"errors {errors}; session_failed {failed[2].get('reason') if failed else None}",
           "`interaction failure lines ready` for the session and the router rendered the stall line in its "
           "voice; after the Piper process of that voice is SIGKILLed (the router then answers 503 for it), "
           "the next turn gets a stall line (service voice), louder than -50 dBFS, whose audio correlates "
           ">= 0.8 with the rendering made at session start and better than with any other voice's line; "
           "session_failed (service voice) after max_wait",
           "voice en_GB-alan-medium through the script's voice router; the speech server (8124) is not touched")


async def sf4(ctx: Ctx):
    """SF4: the streaming recogniser stand-in is killed mid-call: the hearing line, then the exit line, session_failed and the socket closed."""
    port = ctx.args.stt_port + 30
    if port_open(port):
        raise RuntimeError(f"SF4's recogniser port {port} is taken")
    argv = [sys.executable, str(Path(__file__).with_name("local_streaming_stt.py")),
            "--port", str(port), "--whisper", ctx.args.audio_url]
    ctx.stack._spawn("sf4-stt", argv, {}, ctx.stack.log_dir / "live-sf4-stt.log", ctx.stack.log_dir)
    ctx.stack._wait("sf4-stt", lambda: port_open(port), 20)
    stt = {"provider": "deepgram", "model": "nova-2", "api_key": "local-dummy", "language": "en",
           "sample_rate": CALLER_RATE, "channels": 1, "punctuation": True, "encoding": "linear16",
           "extras": {"endpoint_override": f"ws://127.0.0.1:{port}"}}
    session = await Session.open(ctx, "SF4", MODEL_FRONT, PLAIN_PROMPT, extra={"stt_config": stt})
    try:
        await session.warm()
        await failure_lines_ready(ctx, session)
        first = await session.turn("20_q_capital")
        killed = now()
        ctx.stack.kill("sf4-stt")
        start = len(session.events)
        said = await session.say("56_story")
        hearing = await wait_failure(session, ("hearing",), start, 30)
        failed = await session.wait(lambda ev: ev["type"] == "session_failed", 75, start)
        deadline = now() + 10
        while session.closed is None and now() < deadline:
            await asyncio.sleep(0.1)
        segments = failure_segments(session, start)
        notes = failure_notes(session, start)
        summary = failure_summary(session, start, said.start)
    finally:
        await session.close()
        if "sf4-stt" in ctx.stack.started:
            ctx.stack.kill("sf4-stt")
    exits = [(at, n) for at, n in notes if n["kind"] in ("exit", "exit_follow_up")]
    closed = session.closed
    heard = audio_between(session, segments[0][0], segments[0][1]) if segments else b""
    checks = {
        "the recogniser worked before the kill": first.e is not None,
        "the hearing line about the recogniser": bool(hearing) and hearing[1]["service"] == "recogniser",
        "it is heard": frame_db(heard) > AUDIBLE_DB,
        "the exit line 30 s after the first": len(exits) == 1 and bool(segments)
            and exits[0][0] >= segments[0][0] + 29.5,
        "session_failed names the recogniser": bool(failed) and failed[2]["service"] == "recogniser",
        "the gateway closes the socket (1011)": closed is not None and closed[1] == 1011,
    }
    report(ctx, "SF4", "the streaming recogniser killed mid-call", checks,
           f"killed {said.start - killed:.1f} s before the caller began; failure lines [{summary}] "
           f"(seconds after the caller began a {said.end - said.start:.1f} s story); hearing line "
           f"{len(heard) // 48} ms at {frame_db(heard):.1f} dBFS; session_failed "
           f"{failed[2].get('reason') if failed else None}; close {closed[1:] if closed else None}",
           "after its own streaming recogniser stand-in is SIGKILLed, the caller's next speech gets a hearing "
           "line (service recogniser) louder than -50 dBFS; one exit line >= 29.5 s after the first failure "
           "line; session_failed with service recogniser; close code 1011",
           f"a local_streaming_stt.py of this scenario on port {port} (the run's own recogniser is not touched)")


# ---- gap-lifecycle (19 Oct): the failure lines of speech-to-speech sessions, every background's
# first-token deadline, ready after the analyser's models, per-session provider fixes, ElevenLabs'
# streaming endpoint.

SF5_HOLD_S = 6.0


async def sf5(ctx: Ctx):
    """SF5: the first-token deadline of a DAG background: the graph's LLM node holds its answer 6 s, the caller hears a stall line (service background) after the stall time, then the answer."""
    llm = StallingLlm(SF5_HOLD_S)
    definition = dag_definition(ctx, False)
    for node in definition["nodes"]:
        if node["id"] == "llm":
            node.update({"base_url": llm.url, "model": "sf5-stall"})
    session = await Session.open(ctx, "SF5", MODEL_FRONT, conversation=False,
                                 extra={"dag_config": {"definition": definition}})
    try:
        await session.warm()
        made = await failure_lines_ready(ctx, session)
        start = len(session.events)
        turn = await session.turn("87_store_open", settle=0)
        e = turn.e or turn.said.end
        result = await session.wait(lambda ev: ev["type"] == "task_result", SF5_HOLD_S + 30, start)
        await session.quiet(8, start)
        notes = failure_notes(session, start)
        segments = failure_segments(session, start)
        relayed = session.transcripts(start, "relay")
        backend = session.ready.get("interaction", {}).get("tasks", {}).get("backend")
    finally:
        await session.close()
        llm.close()
    stalls = [(at, n) for at, n in notes if n["kind"] == "stall"]
    answer_at = relayed[0][0] if relayed else None
    checks = {
        "the session's background is the DAG": backend == "dag",
        "the graph's LLM got the request": bool(llm.requests),
        "a stall line about the background": bool(stalls) and stalls[0][1]["service"] == "background",
        "it starts after the stall time": bool(segments) and segments[0][0] >= e + 2.0,
        "before the answer": bool(stalls) and answer_at is not None and stalls[0][0] < answer_at,
        "the answer is said and the task completed": answer_at is not None
            and "nine" in " ".join(ev["delta"] for _, ev in relayed).lower()
            and bool(result) and result[2].get("state") == "completed",
        "no session_failed": not session.all("session_failed", start),
    }
    report(ctx, "SF5", "a DAG background that holds its first token", checks,
           f"lines made {round(made, 1) if made is not None else None} s after the warm-up; failure lines "
           f"[{failure_summary(session, start, e)}] (seconds after E); answer "
           f"{round(answer_at - e, 1) if answer_at else None} s after E; stand-in held {SF5_HOLD_S} s",
           "a stall line (service background) >= 2.0 s after E (the first token is due 2.5 s after the run "
           "started) and before the answer; the answer said; the task completed; no session_failed",
           "the DAG's llm_endpoint node streams from an in-script stand-in that holds 6 s")


async def rt_session(port: int, config: dict, seconds: float, then: Optional[dict] = None
                     ) -> Tuple[List[Tuple[float, dict]], List[Tuple[float, bytes]], Optional[float]]:
    """A `/realtime` client: the JSON frames and the binary audio it gets for `seconds` (or until
    the gateway closes the socket), with when each came, and when the socket closed."""
    events: List[Tuple[float, dict]] = []
    audio: List[Tuple[float, bytes]] = []
    closed = None
    async with websockets.connect(f"ws://127.0.0.1:{port}/realtime", max_size=None,
                                  ping_interval=None) as ws:
        await ws.send(json.dumps(config))
        deadline = now() + seconds
        while now() < deadline:
            try:
                frame = await asyncio.wait_for(ws.recv(), max(0.05, deadline - now()))
            except asyncio.TimeoutError:
                break
            except Exception:
                closed = now()
                break
            if isinstance(frame, bytes):
                audio.append((now(), frame))
                continue
            event = json.loads(frame)
            events.append((now(), event))
            if then is not None and event.get("type") == "session_created":
                await ws.send(json.dumps(then))
                then = None
    return events, audio, closed


async def ss1(ctx: Ctx):
    """SS1: a speech-to-speech provider that stops answering (the in-script Deepgram Voice Agent stand-in reports the caller's turn and never replies): stall lines from the server's default set in the session's output audio, the exit line after failure.max_wait_s, session_failed (service voice) and the socket closed."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    log = ctx.stack.log_dir / "live-fix-gateway-off.log"
    deadline = now() + 60
    while now() < deadline and "interaction default failure lines ready" not in log.read_text(errors="replace"):
        await asyncio.sleep(0.5)
    default_ready = "interaction default failure lines ready" in log.read_text(errors="replace")
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{vendor.slow_port}/v1", "model": "never",
                               "api_key": "local-dummy"}},
             "failure": {"max_wait_s": 10}}
    vendor.ss1_turns.clear()
    events, audio, closed = await rt_session(off_port, {
        "type": "config", "provider": "deepgram", "language": "en",
        "instructions": "SS1: you answer callers of the Larkspur shop.", "interaction": block}, 45.0)
    turn_end = vendor.ss1_turns[0] if vendor.ss1_turns else None
    notes = [(at, e["failure"]) for at, e in events
             if e.get("type") == "interaction_updated" and isinstance(e.get("failure"), dict)]
    stalls = [(at, n) for at, n in notes if n["kind"] == "stall"]
    exits = [(at, n) for at, n in notes if n["kind"] in ("exit", "exit_follow_up")]
    failed = [(at, e) for at, e in events if e.get("type") == "session_failed"]
    created = [e for _, e in events if e.get("type") == "session_created"]
    regime = (created[0].get("interaction") or {}).get("regime") if created else None
    first_stall = stalls[0][0] if stalls else None
    stall_audio = b"".join(pcm for at, pcm in audio if first_stall is not None and first_stall - 0.5 <= at
                           and (not exits or at < exits[0][0]))
    checks = {
        "the gateway made its default set at start": default_ready,
        "interaction mode on the provider": regime in ("provider_speech", "delegating"),
        "the stand-in reported the caller's turn": turn_end is not None,
        "a stall line about the voice after the stall time": bool(stalls) and stalls[0][1]["service"] == "voice"
            and turn_end is not None and first_stall >= turn_end + 2.0,
        "degraded names the voice": any("voice_unavailable" in (e.get("degraded") or []) for _, e in events
                                        if e.get("type") == "interaction_updated"),
        "the stall line is heard in the session's audio": frame_db(stall_audio) > AUDIBLE_DB,
        "at most three stall lines, each another": 1 <= len(stalls) <= 3
            and len({n["line"] for _, n in stalls}) == len(stalls),
        "the exit line max_wait_s after the first stall": len(exits) == 1 and first_stall is not None
            and exits[0][0] >= first_stall + 9.5,
        "session_failed names the voice": len(failed) == 1 and failed[0][1].get("service") == "voice",
        "the gateway closes the socket after it": closed is not None and bool(failed) and closed >= failed[0][0],
    }
    report(ctx, "SS1", "a speech-to-speech provider that stops answering", checks,
           f"regime {regime}; failure lines "
           + ", ".join(f"{n['line']}@{at - turn_end:.1f}s" for at, n in notes if turn_end) +
           f" (seconds after the caller's turn); stall audio {len(stall_audio) // 48} ms at "
           f"{frame_db(stall_audio):.1f} dBFS; session_failed "
           f"{round(failed[0][0] - turn_end, 1) if failed and turn_end else None} s; closed "
           f"{round(closed - turn_end, 1) if closed and turn_end else None} s",
           "the default set made at start; a stall line (service voice) >= 2.0 s after the caller's turn, "
           "louder than -50 dBFS in the socket's binary audio; 1-3 distinct stall lines; one exit line >= 9.5 s "
           "after the first stall (failure.max_wait_s 10); session_failed service voice; the socket closed after it",
           "fix-gateway-off (its default set in the speech server's voice), Deepgram Voice Agent stand-in")


# ---- r3-s2s (20 Oct): the half-cascade, provider speculation and scheduling, a continuer the
# provider leaves unanswered, and a room voice's keys, words, update and topic controls.
# Thresholds written before the first run.

HC1_INSTRUCTIONS = "HC1: You answer callers of the Larkspur cafe. We open at 9 in the morning."
HC1_REPLY = "We open at 9 in the morning. Your table for four is booked."
# No number in it (one nobody gave would be caught as an unsourced number; run 2 had two).
HC1_LONG = ("We are on Main Street, next to the old station. We have a terrace with a view of the "
            "park. We serve brunch on Sundays. Parking is free behind the building.")
HC1_SECOND_S = 8.0
# The caller cuts in this long after the long reply was written (its first sentence plays).
HC1_CUT_AFTER_S = 2.5
# The gateway's voice (the speech server's Piper) says the first sentence within this long.
HC1_FIRST_AUDIO_S = 5.0
# After the cut no audio of that reply comes later than this.
HC1_CUT_SLACK_S = 0.5
SS2_QUIET_S = 8.0
SP1_TURN_S = 1.0
# From the provider's turnComplete to task_created (one actor step on loopback).
SP1_ADOPT_LIMIT_S = 2.0


def gateway_voice_lines(log: Path, since: int) -> List[str]:
    """The words the gateway's own voice said (`interaction gateway voice line`), from offset `since`."""
    with open(log, "rb") as handle:
        handle.seek(since)
        text = handle.read().decode("utf-8", "replace")
    return [m[1] for m in re.finditer(r'interaction gateway voice line .*?words=(.*?)(?: \w+=|$)', text, re.M)]


async def hc1(ctx: Ctx):
    """HC1: the half-cascade on /realtime (the OpenAI stand-in writes text replies; the gateway checks each sentence and says it in its own voice): a claim nobody backs is never said, the correction is; the caller cutting in stops the voice."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    log = ctx.stack.log_dir / "live-fix-gateway-off.log"
    since = log.stat().st_size
    mark = vendor.mark()
    vendor.marks.clear()
    block = {"speech": {"voice_owner": "gateway"},
             "tasks": {"llm": {"base_url": f"http://127.0.0.1:{vendor.slow_port}/v1", "model": "never",
                               "api_key": "local-dummy"}}}
    events, audio, _ = await rt_session(off_port, {
        "type": "config", "provider": "openai", "instructions": HC1_INSTRUCTIONS, "interaction": block},
        HC1_SECOND_S + HC1_CUT_AFTER_S + 8.0)
    created = [e for _, e in events if e.get("type") == "session_created"]
    echo = (created[0].get("interaction") or {}) if created else {}
    updates = [f for f in vendor.since(mark, "openai") if f.get("type") == "session.update"]
    modalities = [(u.get("session") or {}).get("output_modalities") for u in updates]
    reply, long_at, cut = (vendor.marks.get(k) for k in ("hc1_reply", "hc1_long", "hc1_cut"))
    claims = [e for _, e in events if e.get("type") == "unverified_claim"]
    first_audio = next((at for at, _ in audio if reply and at >= reply), None)
    first_turn_audio = b"".join(pcm for at, pcm in audio if reply and reply <= at < (long_at or reply + 99))
    stopped = [(at, e) for at, e in events if e.get("type") == "agent_audio_stopped"]
    stopped_at = next((at for at, e in stopped if cut and at >= cut), None)
    # The stand-in does not answer the caller's cut-in turn, so 3.2-3.3 s later the gateway may
    # say a stall line (r7-verify: the one "late" frame of three runs): a line of its own, not
    # that reply. Audio from 0.25 s before its note on is not counted.
    stall_at = next((at for at, e in events if cut and at >= cut and e.get("type") == "interaction_updated"
                     and isinstance(e.get("failure"), dict)), None)
    late = [at for at, _ in audio if stopped_at and at > stopped_at + HC1_CUT_SLACK_S
            and (stall_at is None or at < stall_at - 0.25)]
    said = gateway_voice_lines(log, since)
    checks = {
        "the echo says the gateway's voice and enforced truth": echo.get("regime") == "provider_speech"
            and (echo.get("speech") or {}).get("voice_owner") == "gateway"
            and (echo.get("speech") or {}).get("truth") == "enforced",
        "the provider was asked for text replies only": bool(modalities) and all(m == ["text"] for m in modalities),
        "the claim was caught before it was said": len([c for c in claims if "booked" in c.get("text", "")]) == 1
            and all(c.get("output_stopped") is True and c.get("corrected") is True for c in claims),
        "the first sentence and the correction were said, the claim not":
            any(w.startswith("We open at 9") for w in said) and any(w.startswith("Just to be clear") for w in said)
            and not any("booked" in w for w in said),
        f"the gateway's voice is heard within {HC1_FIRST_AUDIO_S:.0f} s of the reply":
            first_audio is not None and first_audio - reply <= HC1_FIRST_AUDIO_S and frame_db(first_turn_audio) > AUDIBLE_DB,
        "the caller cutting in stops the voice (barge_in) and nothing of that reply plays after":
            stopped_at is not None and any(e.get("reason") == "barge_in" for at, e in stopped if at >= cut)
            and not late,
    }
    report(ctx, "HC1", "the half-cascade on /realtime: the provider's text in the gateway's voice", checks,
           f"echo regime {echo.get('regime')} speech {echo.get('speech')}; modalities {modalities}; claims "
           f"{[(c.get('rule'), c.get('text'), c.get('output_stopped')) for c in claims]}; said in the gateway's voice "
           f"{said}; first audio {ms(first_audio - reply) if first_audio and reply else None} ms after the reply, "
           f"{len(first_turn_audio) // 48} ms at {frame_db(first_turn_audio):.1f} dBFS; cut at "
           f"{round(cut - long_at, 1) if cut and long_at else None} s into the long reply, agent_audio_stopped "
           f"{ms(stopped_at - cut) if stopped_at and cut else None} ms after; audio frames later than "
           f"{HC1_CUT_SLACK_S} s after it: {len(late)} (a stall line's note "
           f"{round(stall_at - stopped_at, 1) if stall_at and stopped_at else None} s after it, its audio not counted)",
           f"echo provider_speech, voice_owner gateway, truth enforced; every session.update asks output_modalities "
           f"[text]; one unverified_claim for the booked sentence with output_stopped and corrected; the gateway's "
           f"voice said 'We open at 9...' and the correction, never the booked sentence; its first audio within "
           f"{HC1_FIRST_AUDIO_S:.0f} s of the reply and louder than {AUDIBLE_DB} dBFS; after the caller's speech "
           f"started in the long reply, agent_audio_stopped (barge_in) and no audio later than {HC1_CUT_SLACK_S} s after it",
           "fix-gateway-off, the OpenAI stand-in (text replies, no audio of its own); the gateway's voice is the "
           "server's failure_voice (the speech server's Piper)")


async def ss2(ctx: Ctx):
    """SS2: a caller's "uh-huh" the provider leaves unanswered on purpose gets no stall line; the caller's question it leaves unanswered does."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    log = ctx.stack.log_dir / "live-fix-gateway-off.log"
    deadline = now() + 60
    while now() < deadline and "interaction default failure lines ready" not in log.read_text(errors="replace"):
        await asyncio.sleep(0.5)
    vendor.marks.clear()
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{vendor.slow_port}/v1", "model": "never",
                               "api_key": "local-dummy"}}}
    events, _, _ = await rt_session(off_port, {
        "type": "config", "provider": "deepgram", "language": "en",
        "instructions": "SS2: you answer callers of the Larkspur shop.", "interaction": block},
        SS2_QUIET_S + 12.0)
    continuer, question = vendor.marks.get("ss2_continuer"), vendor.marks.get("ss2_question")
    notes = [(at, e["failure"]) for at, e in events
             if e.get("type") == "interaction_updated" and isinstance(e.get("failure"), dict)]
    before = [n for at, n in notes if question is None or at < question]
    after = [(at, n) for at, n in notes if question is not None and at >= question]
    checks = {
        "the stand-in said both turns": continuer is not None and question is not None,
        f"no stall line in the {SS2_QUIET_S:.0f} s after the unanswered uh-huh": not before,
        "a stall line 2-8 s after the unanswered question": bool(after) and after[0][1]["kind"] == "stall"
            and 2.0 <= after[0][0] - question <= 8.0,
    }
    report(ctx, "SS2", "a continuer the provider leaves unanswered", checks,
           f"failure lines {[(n['kind'], round(at - continuer, 1)) for at, n in notes] if continuer else notes} "
           f"(seconds after the uh-huh); the question at {round(question - continuer, 1) if question and continuer else None} s",
           f"no failure line between the caller's 'Uh-huh.' and the question {SS2_QUIET_S:.0f} s later; a stall "
           f"line 2-8 s after the unanswered question",
           "fix-gateway-off, Deepgram Voice Agent stand-in (it answers neither turn)")


async def sp1(ctx: Ctx):
    """SP1: provider speculation and Gemini's delivery scheduling (fix gateway with the fixes): a delegate call is provisional (its run starts, nothing reaches the client) until the provider's turn completes; then task_created speculative; the result goes back with scheduling."""
    vendor = await vendor_stand_in()
    _, on_port = ctx.stack.ensure_fix_gateways(vendor.port)
    mark = vendor.mark()
    vendor.marks.clear()
    vendor.llm_asked.clear()
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{vendor.llm_port}/v1", "model": "stand-in",
                               "api_key": "local-dummy"}}}
    events = await rt_frames(on_port, {"type": "config", "provider": "gemini", "model": "gemini-3.8-live",
                                       "instructions": "SP1: you answer questions.", "interaction": block}, 8.0)
    call, turn = vendor.marks.get("sp1_call"), vendor.marks.get("sp1_turn")
    created = [(at, e) for at, e in events if e.get("type") == "task_created"]
    responses = [r for f in vendor.since(mark, "gemini")
                 for r in (f.get("toolResponse") or {}).get("functionResponses", []) if r.get("id") == "sp1-delegate"]
    asked = [at for at in vendor.llm_asked if call and at >= call]
    checks = {
        "the call's run starts before the turn ends": bool(asked) and turn is not None and asked[0] < turn,
        "nothing reaches the client before the turn ends": bool(created) and turn is not None and created[0][0] >= turn,
        f"task_created speculative within {SP1_ADOPT_LIMIT_S:.0f} s of turnComplete": len(created) == 1
            and created[0][1].get("speculative") is True and created[0][0] - turn <= SP1_ADOPT_LIMIT_S,
        "the result goes back once, with scheduling WHEN_IDLE": len(responses) == 1
            and responses[0].get("scheduling") == "WHEN_IDLE"
            and "Paris" in json.dumps(responses[0].get("response"))
            and "waav_scheduling" not in json.dumps(responses[0]),
    }
    report(ctx, "SP1", "provider speculation and delivery scheduling on Gemini", checks,
           f"llm asked {[ms(at - call) for at in asked] if call else asked} ms after the call; turnComplete "
           f"{ms(turn - call) if turn and call else None} ms after it; task_created "
           f"{[(ms(at - turn) if turn else None, e.get('speculative')) for at, e in created]} ms after the turn; "
           f"results {responses}",
           f"the stand-in LLM asked before turnComplete; no task_created before it, one with speculative true "
           f"<= {SP1_ADOPT_LIMIT_S:.0f} s after it; one functionResponse for the call with scheduling WHEN_IDLE "
           f"and the answer, no waav_scheduling key left in it",
           "fix-gateway-on (gemini_tool_call_cancellation, gemini_response_scheduling listed), Gemini stand-in, "
           "in-script LLM stand-in")


async def rl1(ctx: Ctx):
    """RL1: the /v1/realtime relay over a vendor that stops answering (the in-script stand-in reports the caller's turn and never replies): the failure lines reach the client as gateway responses, each line has its bud.audio.segment, then bud.session.failed and the relay closes."""
    vendor = await vendor_stand_in()
    ctx.stack.ensure_bud()
    log = ctx.stack.log_dir / "live-bud-gateway.log"
    deadline = now() + 60
    while now() < deadline and "interaction default failure lines ready" not in log.read_text(errors="replace"):
        await asyncio.sleep(0.5)
    default_ready = "interaction default failure lines ready" in log.read_text(errors="replace")
    vendor.marks.clear()
    session = await Realtime.open(ctx, "RL1", "vendor", model="rt-silent", mic=False, session={
        "instructions": "RL1: you answer callers of the Larkspur shop.",
        "audio": {"input": {"turn_detection": None}}})
    deadline = now() + 40
    while now() < deadline and not session.ws.close_code:
        await asyncio.sleep(0.2)
    closed = now() if session.ws.close_code else None
    code = session.ws.close_code
    turn = vendor.marks.get("rl1_turn")
    wire = session.wire
    gateway = [(at, e) for at, e in wire if e.get("type") == "response.created"
               and ((e.get("response") or {}).get("metadata") or {}).get("bud_source") == "gateway"]
    deltas = [(at, e) for at, e in wire if e.get("type") == "response.output_audio.delta"
              and str(e.get("response_id", "")).startswith("resp_waav_")]
    notes = [(at, e["failure"]) for at, e in wire if e.get("type") == "bud.interaction.updated"
             and isinstance(e.get("failure"), dict)]
    stalls = [(at, n) for at, n in notes if n["kind"] == "stall"]
    exits = [(at, n) for at, n in notes if n["kind"] in ("exit", "exit_follow_up")]
    segments = [e for _, e in wire if e.get("type") == "bud.audio.segment"]
    failed = [(at, e) for at, e in wire if e.get("type") == "bud.session.failed"]
    checks = {
        "the relay's gateway made its default set at start": default_ready,
        "interaction mode on the relay": bool(session.ready.get("interaction")),
        "a stall line about the voice >= 2 s after the caller's turn": bool(stalls) and turn is not None
            and stalls[0][1]["service"] == "voice" and stalls[0][0] >= turn + 2.0,
        "each failure line is a gateway response with its audio": len(gateway) == len(notes) >= 2
            and len(deltas) >= len(gateway),
        "each line has its bud.audio.segment start and end (kind failure)":
            len([e for e in segments if e.get("phase") == "start" and e.get("kind") == "failure"]) == len(notes)
            and len([e for e in segments if e.get("phase") == "end"]) >= len(notes) - 1,
        "the exit line max_wait_s after the first stall": len(exits) == 1 and bool(stalls)
            and exits[0][0] >= stalls[0][0] + 9.5,
        "bud.session.failed names the voice, then the relay closes (1011)": len(failed) == 1
            and failed[0][1].get("service") == "voice" and code == 1011 and closed is not None,
    }
    await session.close()
    report(ctx, "RL1", "the /v1/realtime relay over a vendor that stops answering", checks,
           f"failure lines {[(n['line'], round(at - turn, 1)) for at, n in notes] if turn else notes} (s after the "
           f"turn); gateway responses {len(gateway)}, their audio deltas {len(deltas)}; segments "
           f"{[(e.get('phase'), e.get('kind'), e.get('utterance_id')) for e in segments]}; session failed "
           f"{round(failed[0][0] - turn, 1) if failed and turn else None} s; close code {code}",
           "the default set made at start; a stall line (service voice) >= 2.0 s after the vendor's speech_stopped; "
           "every failure line one response.created with metadata bud_source gateway and its output_audio deltas; "
           "a bud.audio.segment start (kind failure) per line and its end; one exit line >= 9.5 s after the first "
           "stall (max_wait_s 10); bud.session.failed service voice; the relay closed with 1011",
           "the Bud-mode gateway's relay to the in-script vendor stand-in (deployment rt-silent)")


async def rv2(ctx: Ctx):
    """RV2: a room's realtime voice (the GA cascade, manual turns) with a SIP caller: the words reach the socket, a keypad 1 accepts the provider's confirmation, interaction_update applies, a stop and task_seen on the room's topic reach the session."""
    try:
        import livekit.rtc  # noqa: F401
    except ImportError:
        raise RuntimeError("RV2 needs the LiveKit client: pip install livekit livekit-api")
    ctx.stack.ensure_livekit()
    port = ctx.stack.ensure_regime()
    mark, log_mark = len(ctx.webhook.calls), regime_log_size(ctx)
    block = regime_config(ctx, irreversible=False)["interaction"]
    block["speech"] = {"realtime": {"provider": "openai", "instructions": ROOM_VOICE_FRONT,
                                    "turn_detection": {"mode": "manual"}}}
    block["policy"] = {"hitl_answer_timeout_ms": 60000}
    block["hitl"] = {"channels": ["voice", "client", "dtmf"]}
    session = await Session.open(ctx, "RV2", block, mic=False, conversation=False, port=port,
                                 livekit={"room_name": f"rv2-{ctx.run_id}"},
                                 extra={"stt_config": None, "tts_config": None})
    phone = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "sip_caller", kind="sip").join()
    await asyncio.sleep(3.0)
    checks: Dict[str, bool] = {}
    start = len(session.events)
    said = await phone.say("10_book_table")
    asked = await session.wait(lambda e: e["type"] == "task_question", 90, after=said.start)
    words = session.all("stt_result", start)
    lines = [e for _, e in session.all("assistant_transcript", start) if e.get("source") == "provider"]
    checks["the caller's words reach the socket as stt_result"] = any(e.get("is_final") for _, e in words)
    pressed = None
    answered = None
    if asked:
        await asyncio.sleep(2.0)
        pressed = now()
        await phone.room.local_participant.publish_dtmf(code=1, digit="1")
        answered = await session.wait(lambda e: e["type"] == "task_answer_received", KEY_ANSWER_LIMIT_S, asked[0])
    checks[f"a keypad 1 accepts the confirmation within {KEY_ANSWER_LIMIT_S:.0f} s"] = (
        answered is not None and answered[2].get("by") == "dtmf" and answered[2].get("outcome") == "accept")
    result = await session.wait(lambda e: e["type"] == "task_result", 90, start)
    calls = [c for c in ctx.webhook.since(mark) if c["tool"] == "book_table"]
    checks["the booking is made once"] = result is not None and len(calls) == 1
    await asyncio.sleep(4.0)
    lines = [e for _, e in session.all("assistant_transcript", start) if e.get("source") == "provider"]
    checks["the provider's words reach the socket as assistant_transcript (source provider)"] = bool(lines)
    # r4-left: the phone caller hears the keypad hint with the question (the provider's line,
    # or the gateway's voice in the regime gateway's log); the screen's prompt is the question.
    with open(ctx.stack.log_dir / "live-regime-gateway.log", "rb") as handle:
        handle.seek(log_mark)
        voiced = [line for line in handle.read().decode(errors="replace").splitlines()
                  if "interaction gateway voice line" in line and KEYPAD_HINT in line]
    hinted = [e.get("delta") for e in lines if KEYPAD_HINT in (e.get("delta") or "")] + voiced
    checks["the question is said with the keypad hint, the screen's prompt without it"] = (
        bool(hinted) and asked is not None and "press" not in (asked[2].get("prompt") or ""))
    up_from = len(session.events)
    await session.send({"type": "interaction_update", "patch": {"policy": {"hitl_answer_timeout_ms": 30000},
                                                                "regime": "cascade"}})
    updated = await session.wait(lambda e: e["type"] == "interaction_updated" and e.get("applied") is not None
                                 and not e.get("failure"), 10, up_from)
    checks["interaction_update applies the policy and refuses the regime"] = (
        updated is not None and updated[2].get("applied") == ["policy.hitl_answer_timeout_ms"]
        and [r.get("path") for r in updated[2].get("refused", [])] == ["regime"])
    publish = phone.room.local_participant.publish_data
    topic_from, room_from = len(session.events), len(phone.events)
    sent = now()
    await publish(json.dumps({"type": "control", "kind": "stop"}), reliable=True, topic=ROOM_TOPIC)
    applied = await session.wait(lambda e: e["type"] == "control_applied", 10, topic_from)
    deadline = now() + 5
    while now() < deadline and not any(e.get("type") == "control_applied" for _, e in phone.events[room_from:]):
        await asyncio.sleep(0.05)
    room_applied = [e for _, e in phone.events[room_from:] if e.get("type") == "control_applied"]
    task_id = result[2].get("task_id") if result else "task_rv2"
    await publish(json.dumps({"type": "task_seen", "task_id": task_id}), reliable=True, topic=ROOM_TOPIC)
    await asyncio.sleep(1.5)
    errors = [e for _, e in session.events[topic_from:] if e["type"] == "error"]
    texts = [e for _, e in session.events[topic_from:] if e["type"] == "message"]
    checks["a stop on the room's topic is applied, and control_applied goes back on the topic"] = (
        applied is not None and (applied[2].get("kind"), applied[2].get("outcome")) == ("stop", "applied")
        and len(room_applied) == 1)
    checks["task_seen on the topic is the session's: no error, no data message"] = not errors and not texts
    missing, kinds = room_topic_checks(session, phone)
    checks["every interaction event of the socket is on the room's topic too"] = not missing
    await phone.leave()
    ctx.record(session)
    await session.close()
    report(ctx, "RV2", "a room's realtime voice: words, keypad, update and topic controls", checks,
           f"question {'+%d ms' % ms(asked[1] - said.start) if asked else None} after the caller's turn "
           f"(prompt {asked[2].get('prompt') if asked else None!r}; lines with the hint {len(hinted)}); key 1 -> "
           f"{answered[2].get('outcome') + ' by ' + answered[2].get('by') if answered else None} "
           f"{ms(answered[1] - pressed) if answered and pressed else None} ms; stt_result {[e.get('transcript') for _, e in words]}; "
           f"provider lines {[e.get('delta') for e in lines]}; update {updated[2] if updated else None}; "
           f"control {applied[2] if applied else None} {ms(applied[1] - sent) if applied else None} ms, on the topic "
           f"{len(room_applied)}; errors {errors}; missing on the topic {missing[:3]}; room kinds {kinds}",
           f"stt_result final on the socket; task_answer_received accept by dtmf within {KEY_ANSWER_LIMIT_S:.0f} s of "
           f"the key; one book_table call; assistant_transcript with source provider; interaction_updated applied "
           f"[policy.hitl_answer_timeout_ms], refused [regime]; control_applied stop applied on the socket and once "
           f"on the topic; no error or data message after task_seen; no socket interaction event missing on the topic",
           "the regime gateway's GA cascade is the vendor (local Whisper, Piper, background llama-server); the "
           "caller is a LiveKit participant of kind sip")


async def pf2(ctx: Ctx):
    """PF2: deepgram_user_text is on by default (its probe passed); a /realtime session turns it off for itself, and may not turn another fix on without the operator's provider_fixes_per_session."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    typed = {}
    for label, own in (("default", None), ("off", {"deepgram_user_text": False})):
        mark = vendor.mark()
        config = {"type": "config", "provider": "deepgram"}
        if own is not None:
            config["provider_fixes"] = own
        await rt_frames(off_port, config, 2.0, then={"type": "text", "text": "Is it open?"})
        typed[label] = [f for f in vendor.since(mark, "deepgram")
                        if f.get("type") in ("InjectAgentMessage", "InjectUserMessage")]
    mark = vendor.mark()
    gemini = await rt_frames(off_port, {"type": "config", "provider": "gemini", "model": "gemini-3.8-live",
                                        "tools": [PF1_TOOL], "provider_fixes": {"gemini_tool_behavior": True}},
                             0.5 + PF1_QUIET_S)
    setups = [f["setup"] for f in vendor.since(mark, "gemini") if "setup" in f]
    behavior = [d.get("behavior") for t in (setups[0].get("tools", []) if setups else [])
                for d in t.get("functionDeclarations", [])]
    warnings = [e for _, e in gemini if e.get("type") == "warning"]
    checks = {
        "typed text is the user's by default": typed["default"] == [{"type": "InjectUserMessage", "content": "Is it open?"}],
        "a session turns the fix off for itself": typed["off"] == [{"type": "InjectAgentMessage", "content": "Is it open?"}],
        "a session may not turn a fix on": behavior == [None],
        "it is told so": [(w.get("code"), (w.get("detail") or {}).get("path")) for w in warnings]
            == [("interaction_override_refused", "provider_fixes.gemini_tool_behavior")],
    }
    report(ctx, "PF2", "per-session provider fixes, deepgram_user_text on by default", checks,
           f"typed {typed}; gemini behavior {behavior}; warnings {[w.get('code') for w in warnings]}",
           "default InjectUserMessage; with provider_fixes {deepgram_user_text: false} InjectAgentMessage; "
           "{gemini_tool_behavior: true} without provider_fixes_per_session: no behavior and one warning "
           "interaction_override_refused at provider_fixes.gemini_tool_behavior",
           "fix-gateway-off (no operator fixes), in-script vendor stand-in")


class ElevenLabsStandIn:
    """EL1's ElevenLabs text-to-speech: records each request and streams PCM in four 100 ms chunks
    150 ms apart (the convert endpoint the same way), so a client sees whether the audio came
    through as it was made."""

    def __init__(self):
        outer = self
        self.requests: List[dict] = []

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_HEAD(self):
                self.send_response(200)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                hit = {"t": now(), "path": self.path, "key": self.headers.get("xi-api-key"),
                       "body": json.loads(body or b"{}"), "chunks": []}
                outer.requests.append(hit)
                rate = 16000
                tone = b"".join(int(8000 * math.sin(2 * math.pi * 440 * n / rate)).to_bytes(2, "little", signed=True)
                                for n in range(rate // 10))
                self.send_response(200)
                self.send_header("Content-Type", "audio/pcm")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                try:
                    for _ in range(4):
                        self.wfile.write(f"{len(tone):x}\r\n".encode() + tone + b"\r\n")
                        self.wfile.flush()
                        hit["chunks"].append(now())
                        time.sleep(0.15)
                    self.wfile.write(b"0\r\n\r\n")
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/v1/text-to-speech"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


async def ws_speak(port: int, tts: dict, text: str, seconds: float = 6.0
                   ) -> Tuple[List[dict], List[Tuple[float, bytes]]]:
    """A plain `/ws` session (no interaction) that only speaks `text` in `tts`: its JSON events
    and the binary audio it got, with when each came."""
    config = {"type": "config", "stream_id": f"el1-{int(time.time() * 1000)}",
              "stt_config": {"provider": "openai", "model": "whisper-1", "language": "en",
                             "sample_rate": CALLER_RATE, "channels": 1, "punctuation": True,
                             "encoding": "linear16"},
              "tts_config": tts}
    events: List[dict] = []
    audio: List[Tuple[float, bytes]] = []
    async with websockets.connect(f"ws://127.0.0.1:{port}/ws", max_size=None, ping_interval=None) as ws:
        await ws.send(json.dumps(config))
        deadline = now() + seconds + 10
        spoke = False
        while now() < deadline:
            try:
                frame = await asyncio.wait_for(ws.recv(), max(0.05, deadline - now()))
            except (asyncio.TimeoutError, Exception):
                break
            if isinstance(frame, bytes):
                audio.append((now(), frame))
                continue
            event = json.loads(frame)
            events.append(event)
            if event.get("type") == "ready" and not spoke:
                await ws.send(json.dumps({"type": "speak", "text": text}))
                spoke, deadline = True, now() + seconds
    return events, audio


async def el1(ctx: Ctx):
    """EL1: ElevenLabs text-to-speech with features.streaming goes to /v1/text-to-speech/{voice}/stream (the gateway's ELEVENLABS_TTS_URL: an in-script stand-in) with the convert endpoint's request, the audio reaches the /ws client as it comes, no streaming warning; without the feature the convert endpoint is used as before."""
    vendor = await vendor_stand_in()
    off_port, _ = ctx.stack.ensure_fix_gateways(vendor.port)
    el = vendor.elevenlabs
    tts = {"provider": "elevenlabs", "voice_id": "el1-voice", "model": "eleven_flash_v2_5",
           "audio_format": "linear16", "sample_rate": 16000}
    out = {}
    # The fix gateways' speech cache (~/.cache/waav-gateway) outlives the run: a line of this
    # run's own words, else an earlier run's audio is played and the stand-in is never asked.
    said = lambda label: f"Your table is booked ({label} {ctx.run_id})."
    for label, features in (("stream", {"streaming": True}), ("convert", {})):
        mark = len(el.requests)
        events, audio = await ws_speak(off_port, {**tts, "features": features}, said(label))
        hit = el.requests[mark] if len(el.requests) > mark else {}
        first = audio[0][0] if audio else None
        out[label] = {"ready": any(e.get("type") == "ready" for e in events),
                      "warnings": [e.get("code") for e in events if e.get("type") == "config_warning"],
                      "bytes": sum(len(pcm) for _, pcm in audio), "path": hit.get("path"),
                      "key": hit.get("key"), "body": hit.get("body"),
                      "first_before_last": first is not None and bool(hit.get("chunks")) and first < hit["chunks"][-1]}
    stream, convert = out["stream"], out["convert"]
    # POST /speak takes the same features (r3-misc).
    mark = len(el.requests)
    spoken = await asyncio.to_thread(http_speak, off_port, {**tts, "features": {"streaming": True}}, said("speak"))
    speak_hit = el.requests[mark] if len(el.requests) > mark else {}
    checks = {
        "streaming: the stream endpoint, with the session's voice": stream["ready"]
            and (stream["path"] or "").startswith("/v1/text-to-speech/el1-voice/stream?output_format=pcm_16000"),
        "the convert endpoint's body": stream["body"] == {"text": said("stream"),
                                                          "model_id": "eleven_flash_v2_5"},
        "the server's key": stream["key"] == "el1-key",
        "no streaming warning": "tts_streaming_unsupported_or_degraded" not in stream["warnings"],
        "the audio reaches the client as it is made": stream["first_before_last"] and stream["bytes"] >= 12800,
        "without the feature: the convert endpoint": convert["ready"]
            and (convert["path"] or "").startswith("/v1/text-to-speech/el1-voice?output_format=pcm_16000"),
        "POST /speak with features.streaming: the stream endpoint, the audio back": spoken[0] == 200
            and spoken[1] >= 12800
            and (speak_hit.get("path") or "").startswith("/v1/text-to-speech/el1-voice/stream?output_format=pcm_16000")
            and speak_hit.get("body") == {"text": said("speak"), "model_id": "eleven_flash_v2_5"},
    }
    report(ctx, "EL1", "ElevenLabs' HTTP streaming endpoint", checks,
           f"stream {stream['path']} {stream['bytes']} bytes, first audio before the vendor's last chunk "
           f"{stream['first_before_last']}, warnings {stream['warnings']}; convert {convert['path']}; POST /speak "
           f"{spoken[0]} {spoken[1]} bytes via {speak_hit.get('path')}",
           "features.streaming: POST .../el1-voice/stream?output_format=pcm_16000 with the convert body and the "
           "server key, >= 4 x 3,200 bytes to the /ws client, the first before the stand-in sent its last chunk, "
           "no tts_streaming_unsupported_or_degraded; without it POST .../el1-voice?output_format=pcm_16000",
           "fix-gateway-off with ELEVENLABS_TTS_URL at the in-script stand-in (no ElevenLabs credit is spent)")


async def rd1(ctx: Ctx):
    """RD1: `ready` comes once the session's analyser models (Silero, Smart Turn) are loaded, at most 2 s after the gateway set the session up."""
    start = now()
    session = await Session.open(ctx, "RD1", MODEL_FRONT)
    ready_s = now() - start
    try:
        await session.warm()
        lines = ctx.stack.log_lines("Interaction mode started", session.stream_id)
        turn = await session.turn("20_q_capital")
        answered = bool(session.transcripts(turn.first_event))
    finally:
        await session.close()
    loaded = bool(lines) and "models_loaded=true" in lines[0]
    checks = {
        "the gateway logged the session's models loaded before ready": loaded,
        "ready within the bound (2 s) plus the session's set-up": ready_s <= 10.0,
        "the first turn is heard and answered": answered,
    }
    report(ctx, "RD1", "ready after the analyser's models", checks,
           f"config to ready {ready_s:.2f} s; log {lines[0][-120:] if lines else None}",
           "`Interaction mode started ... models_loaded=true` for the session; ready <= 10 s after the config; "
           "the first turn answered", "main gateway")


# ---- protocols and events beyond one socket (plan chapters 3, 8, 9)

def read_task_events(url: str, received: List[Tuple[float, str, dict]], stop: threading.Event):
    """`GET /v1/tasks/events` read as it comes: (time, event name, data) of each event."""
    try:
        with urllib.request.urlopen(url, timeout=600) as response:
            name, data = "", ""
            for raw in response:
                if stop.is_set():
                    return
                line = raw.decode("utf-8", "replace").rstrip("\n")
                if line.startswith("event:"):
                    name = line[6:].strip()
                elif line.startswith("data:"):
                    data = line[5:].strip()
                elif not line and name:
                    received.append((now(), name, json.loads(data) if data.startswith("{") else {}))
                    name, data = "", ""
    except Exception as error:  # the stream's end at the close is not a failure
        received.append((now(), "error", {"error": repr(error)}))


async def pr1(ctx: Ctx):
    """PR1: MCP initialize and an OAuth client's token; an elicitation on a call's own reply is the caller's question, one on a reply no session waits on is refused."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    session = await backend_session(ctx, "PR1", "authed")
    await session.warm()
    start = len(session.events)
    await session.say("50_order_flowers")
    asked, after = await question(session, start)
    await heard(session, await spoken(session, after - 1, "question"))
    await asyncio.sleep(0.45)
    answer_from = len(session.events)
    await session.say("51_tulips")
    answered = await answer_received(session, answer_from)
    result = await session.wait(lambda e: e["type"] == "task_result", 60, answer_from)
    await session.quiet(30, answer_from)
    # Past half the token's life: the next call has a token fetched anew, on the same session.
    await asyncio.sleep(OAUTH_TOKEN_S)
    second_from = len(session.events)
    await session.say("20_q_capital")
    second = await session.wait(lambda e: e["type"] == "task_result", 60, second_from)
    await session.quiet(30, second_from)
    await session.close()
    hits = stand_in.of("mcpauth", None, mark)
    methods = [hit["method"] for hit in hits]
    inits = [hit for hit in hits if hit["method"] == "initialize"]
    calls = [hit for hit in hits if hit["method"] == "tools/call"]
    responses = [hit for hit in hits if hit["method"] == "response"]
    tokens = stand_in.of("token", None, mark)
    # An orchestrator session whose agent is the eliciting server: its start reads the list.
    refused_mark = len(stand_in.seen)
    block = {**MODEL_FRONT, "tasks": {"orchestrator": ctx.args.background_alias, "agents": ["elicitor"]}}
    orchestrated = await Session.open(ctx, "PR1-refused", block, mic=False, port=stand_in.port)
    await asyncio.sleep(1.0)
    await orchestrated.close()
    elicitor = stand_in.of("elicitor", None, refused_mark)
    refusals = [hit for hit in elicitor if hit["method"] == "response"]
    lists = [hit for hit in elicitor if hit["method"] == "tools/list"]
    refusal = refusals[0]["body"] if refusals else {}
    waited = (stand_in.elicited.get(str(refusal.get("id"))) or {}).get("answered_after")
    summary = (result[2].get("summary") or "") if result else ""
    checks = {
        "initialize once, first, with the client's version and capabilities": len(inits) == 1
            and bool(methods) and methods[0] == "initialize"
            and inits[0]["body"]["params"].get("protocolVersion") == MCP_VERSION
            and "elicitation" in (inits[0]["body"]["params"].get("capabilities") or {}),
        "every later request carries the server's session and an issued token": len(hits) >= 4
            and all(hit["ok"] for hit in hits) and all(hit.get("session") == inits[0].get("session")
                                                      for hit in hits[1:]) if inits else False,
        "the token comes from the client's credentials and is fetched again past half its life":
            len(tokens) >= 2 and all(hit["ok"] for hit in tokens) and len(calls) == 2
            and calls[0].get("token") != calls[1].get("token"),
        "the elicitation on the call's reply is the caller's choice": asked.get("kind") == "choose"
            and [str(o).lower() for o in asked.get("options") or []] == ["roses", "tulips"],
        "the caller's answer is the JSON-RPC response with the elicitation's id": bool(answered)
            and answered.get("outcome") == "accept" and len(responses) == 1
            and responses[0]["body"].get("result") == {"action": "accept", "content": {"flowers": "tulips"}},
        "the task completes with the server's answer": bool(result)
            and result[2].get("state") == "completed" and "tulips" in summary.lower(),
        "the second call completes": bool(second) and second[2].get("state") == "completed",
        # The question after the order is a new request, not a follow-up of the order (the
        # front's follow_up is read apart: gap-protocols saw it twice before that reading).
        "the question after the order is a request of its own": len(calls) == 2
            and "(Earlier" not in json.dumps(calls[1]["body"]),
        "an elicitation on a reply no session waits on is refused (-32601) at once, and the list read":
            (refusal.get("error") or {}).get("code") == -32601 and len(lists) == 1
            and waited is not None and waited <= 1.0,
    }
    report(ctx, "PR1", "MCP initialize, OAuth client credentials, elicitations in and outside a call", checks,
           f"requests {methods}; sessions {[hit.get('session') for hit in hits]}; tokens fetched {len(tokens)} "
           f"(calls used {[hit.get('token') for hit in calls]}); question {asked.get('kind')!r} "
           f"{asked.get('prompt')!r} {asked.get('options')}; the server got "
           f"{[hit['body'].get('result') for hit in responses]}; task {result[2].get('state') if result else None!r} "
           f"{summary!r}; second {second[2].get('state') if second else None!r}; outside a call: "
           f"{refusal.get('error')} after {ms(waited) if waited is not None else None} ms, list read {len(lists)}",
           "one initialize (version 2026-07-28, elicitation capability) before anything else; Mcp-Session-Id "
           "and a token the authorisation server issued on every later request; two tokens for two calls "
           f"{OAUTH_TOKEN_S} s apart (renewed half way); the reply's elicitation a choose question, its answer "
           "the JSON-RPC result {action: accept, content: {flowers: tulips}}; the tool list's elicitation "
           "answered with error -32601 within 1,000 ms")


async def pr2(ctx: Ctx):
    """PR2: A2A: the extended card is read with the entry's credential, and a long task's updates come as the agent's pushes to the gateway's callback route."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    session = await backend_session(ctx, "PR2", "pusher")
    await session.warm()
    start = len(session.events)
    await session.say("73_parcel_status")
    created = await session.wait(lambda e: e["type"] == "task_created", 20, start)
    progress = await session.wait(
        lambda e: e["type"] == "task_progress" and e.get("text") == "Asking the courier.", 20, start)
    result = await session.wait(lambda e: e["type"] == "task_result", 60, start)
    await session.quiet(30, start)
    await session.close()
    cards = stand_in.of("push", "card", mark)
    extended = stand_in.of("push", "GetExtendedAgentCard", mark)
    reads = stand_in.of("push", "GetTask", mark)
    job = stand_in.last("push") or {}
    config = job.get("push_config") or {}
    task_id = created[2]["task_id"] if created else "?"
    pushes = job.get("pushes") or []
    ran = (job.get("ended", now()) - job.get("created", now()))
    checks = {
        "the card is read, then its extended card with the entry's credential": bool(cards)
            and len(extended) == 1 and extended[0]["ok"],
        "the task's first message carries a push configuration for its callback route":
            config.get("url") == f"{stand_in.gateway}/v1/tasks/{task_id}/callback"
            and len(config.get("token") or "") == 64,
        "every push is taken (202), one with another token refused (401)": len(pushes) >= 2
            and all(status == 202 for _, _, status in pushes) and job.get("forged") == [401],
        "a pushed status update is the task's progress": progress is not None,
        "the pushed task is its result": bool(result) and result[2].get("state") == "completed"
            and norm(result[2].get("summary", "")) == norm(job.get("result") or "-"),
        "the task is read only now and then meanwhile (every 5 s)": len(reads) <= 1 + int(ran / 5.0),
    }
    report(ctx, "PR2", "A2A extended card and push notifications", checks,
           f"card reads {len(cards)}, extended {[hit['ok'] for hit in extended]}; push config url "
           f"{config.get('url')!r}, token of {len(config.get('token') or '')} characters; pushes "
           f"{[(state, status) for _, state, status in pushes]}, forged {job.get('forged')}; progress "
           f"{bool(progress)}; result {result[2].get('state') if result else None!r} "
           f"{result[2].get('summary') if result else None!r}; GetTask {len(reads)} times in {ran:.1f} s",
           "the public card then GetExtendedAgentCard with the bearer token; pushNotificationConfig url "
           "<gateway>/v1/tasks/<task_id>/callback with a 64-character token; every push 202, the forged one 401; "
           "task_progress from the pushed status; task_result completed with the agent's words; at most "
           "1 + run/5 s GetTask reads")


async def pr3(ctx: Ctx):
    """PR3: a webhook and an A2A agent ask for conversation controls on their own wires and hear control_applied there; tasks.announce changes mid-session."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    runs = {}
    for label, entry, line in (("PR3-hook", "ctlhook", "The store opens at nine in the morning."),
                               ("PR3-agent", "ctlagent", "Nine in the morning.")):
        session = await backend_session(ctx, label, entry)
        await session.warm()
        start = len(session.events)
        await session.say("87_store_open")
        applied = await session.wait(lambda e: e["type"] == "control_applied", 30, start)
        answer = await said(session, start, line, 40)
        result = await session.wait(lambda e: e["type"] == "task_result", 40, start)
        await session.quiet(30, start)
        states = [e.get("state") or {} for _, e in session.all("interaction_updated", start)]
        update = None
        if entry == "ctlhook":
            # How later updates reach the caller, changed during the session.
            update_from = len(session.events)
            await session.send({"type": "interaction_update", "patch": {"tasks": {"announce": "screen"}}})
            update = await session.wait(lambda e: e["type"] == "interaction_updated"
                                        and e.get("applied") == ["tasks.announce"], 10, update_from)
        await session.close()
        runs[entry] = (applied[2] if applied else {}, answer, result, states, update)
    hook_told = stand_in.of("ctlhook", "control_applied", mark)
    agent_told = stand_in.of("ctlagent", "control_applied", mark)
    hook_applied, hook_line, hook_result, hook_states, update = runs["ctlhook"]
    agent_applied, agent_line, agent_result, agent_states, _ = runs["ctlagent"]
    told_body = hook_told[0]["body"] if hook_told else {}
    agent_parts = (agent_told[0]["body"]["params"]["message"]["parts"] if agent_told else [{}])
    checks = {
        "the webhook's control is applied by the one executor": (hook_applied.get("kind"),
            hook_applied.get("value"), hook_applied.get("source"), hook_applied.get("outcome"))
            == ("rate", "slower", "agent", "applied")
            and any((state.get("rate") or 1.0) < 1.0 for state in hook_states),
        "its control_applied goes back to the webhook, signed, with its request_id": len(hook_told) == 1
            and hook_told[0]["ok"] and told_body.get("request_id") == "slow-1"
            and told_body.get("outcome") == "applied"
            and (hook_told[0].get("idempotency_key") or "").endswith(":control_applied:slow-1"),
        "the webhook's answer after it is said": bool(hook_line) and bool(hook_result)
            and hook_result[2].get("state") == "completed",
        "the agent's control (a data part) is applied once": (agent_applied.get("kind"),
            agent_applied.get("source"), agent_applied.get("outcome")) == ("verbosity", "agent", "applied"),
        "its control_applied is a message to the agent's task": len(agent_told) == 1
            and agent_told[0]["body"]["params"]["message"].get("taskId") == "ctl-task"
            and agent_parts[0].get("data", {}).get("request_id") == "brief-1",
        "the agent's answer after it is said": bool(agent_line) and bool(agent_result)
            and agent_result[2].get("state") == "completed",
        "tasks.announce changes mid-session": bool(update)
            and (update[2].get("state") or {}).get("updates") == "screen",
    }
    report(ctx, "PR3", "controls from a webhook and an A2A agent; tasks.announce mid-session", checks,
           f"webhook: control_applied {hook_applied}; rates {[s.get('rate') for s in hook_states]}; the webhook "
           f"got {told_body} (key {hook_told[0].get('idempotency_key') if hook_told else None}); said "
           f"{bool(hook_line)}; agent: control_applied {agent_applied}; the agent got "
           f"{agent_parts[0].get('data') if agent_told else None} ({len(agent_told)} message(s)); said "
           f"{bool(agent_line)}; announce update {update[2] if update else None}",
           "control_applied source agent outcome applied for rate slower (state rate < 1) and verbosity brief; "
           "one signed control_applied POST to the webhook with request_id slow-1, one A2A message to task "
           "ctl-task with a control_applied data part (request_id brief-1); both answers said; "
           "interaction_updated applied [tasks.announce] with state.updates screen")


async def pr4(ctx: Ctx):
    """PR4: GET /v1/tasks/events on one gateway process streams the task events of a session on another (the task Redis)."""
    sink = ctx.sink()
    first, second = ctx.stack.ensure_durable(sink.url)
    received: List[Tuple[float, str, dict]] = []
    others: List[Tuple[float, str, dict]] = []
    stop = threading.Event()
    session, start, created, _ = await slow_lookup(ctx, "PR4", port=first)
    # The task's later events, on the other process's stream from now on.
    base = f"http://127.0.0.1:{second}/v1/tasks/events?stream_id="
    for url, into in ((base + session.stream_id, received), (base + "somebody-else", others)):
        threading.Thread(target=read_task_events, args=(url, into, stop), daemon=True).start()
    subscribed = now()
    result = await session.wait(lambda e: e["type"] == "task_result", 90, start)
    await session.quiet(20, start)
    await asyncio.sleep(1.0)
    stop.set()
    await session.close()
    socket = [(at, e) for at, e in session.events[start:]
              if e["type"].startswith("task_") and at > subscribed + 0.2]
    far = [(at, name, data) for at, name, data in received if name.startswith("task_")]
    delays, missing = [], []
    for at, event in socket:
        match = [f_at for f_at, name, data in far
                 if name == event["type"] and data.get("task_id") == event.get("task_id")
                 and data.get("revision", event.get("revision")) == event.get("revision")]
        if match:
            delays.append(match[0] - at)
        else:
            missing.append(event["type"])
    checks = {
        "the task's events after the reader came all reach the other process's stream": bool(socket)
            and not missing and bool(result),
        "each within 1,000 ms of the session's socket": bool(delays) and max(delays) <= 1.0,
        "with the session's stream_id": bool(far) and all(data.get("stream_id") == session.stream_id
                                                           for _, _, data in far),
        "a reader of another session gets none of them": not [e for e in others if e[1].startswith("task_")],
    }
    report(ctx, "PR4", "task events of one gateway process on another's GET /v1/tasks/events", checks,
           f"session on :{first}, reader on :{second}; socket {[e['type'] for _, e in socket]}; stream "
           f"{[name for _, name, _ in far]}; missing {missing}; delays {[ms(d) for d in delays]} ms; "
           f"another session's reader got {[name for _, name, _ in others]}",
           "every task_* event the session's socket got after the reader subscribed is on the other process's "
           "stream (same type, task and revision), each <= 1,000 ms after the socket, with the session's stream_id; "
           "a reader of another stream_id gets no task event",
           "both task gateways share the run's task Redis (Redis pub/sub)")


async def pr5(ctx: Ctx):
    """PR5: a room participant's control and task_seen on the interaction data topic reach the session; control_applied comes back on the topic, other messages pass as before."""
    ctx.stack.ensure_livekit()
    stand_in = ctx.stack.ensure_backends()
    session = await Session.open(ctx, "PR5", MODEL_FRONT, mic=False, port=stand_in.port,
                                 livekit={"room_name": f"pr5-{ctx.run_id}"})
    caller = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "caller").join()
    await asyncio.sleep(3.0)
    publish = caller.room.local_participant.publish_data
    start, room_from = len(session.events), len(caller.events)
    sent = now()
    await publish(json.dumps({"type": "control", "kind": "volume", "value": "quieter"}),
                  reliable=True, topic=ROOM_TOPIC)
    applied = await session.wait(lambda e: e["type"] == "control_applied", 10, start)
    deadline = now() + 5
    while now() < deadline and not any(e.get("type") == "control_applied" for _, e in caller.events[room_from:]):
        await asyncio.sleep(0.05)
    room_applied = [e for _, e in caller.events[room_from:] if e.get("type") == "control_applied"]
    seen_from = len(session.events)
    await publish(json.dumps({"type": "task_seen", "task_id": "task_pr5"}), reliable=True, topic=ROOM_TOPIC)
    await publish(json.dumps({"type": "chat", "text": "hello from the room"}), reliable=True, topic=ROOM_TOPIC)
    passed = await session.wait(lambda e: e["type"] == "message", 10, seen_from)
    await asyncio.sleep(1.0)
    after = [e for _, e in session.events[seen_from:]]
    await caller.leave()
    await session.close()
    texts = [str((e.get("message") or {}).get("message")) for e in after if e["type"] == "message"]
    event = applied[2] if applied else {}
    checks = {
        "the room's control reaches the session's control executor": (event.get("kind"), event.get("value"),
            event.get("source"), event.get("outcome")) == ("volume", "quieter", "client", "applied"),
        "control_applied goes back on the room's topic": len(room_applied) == 1
            and room_applied[0].get("state_version") == event.get("state_version"),
        "task_seen is the session's, not a data message, and no error": not any("task_seen" in t for t in texts)
            and not [e for e in after if e["type"] == "error"],
        "anything else on the topic is a data message as before": bool(passed)
            and any("hello from the room" in t for t in texts),
    }
    report(ctx, "PR5", "conversation controls and task_seen on the LiveKit data topic", checks,
           f"control_applied {event} {ms(applied[1] - sent) if applied else None} ms after the publish; on the "
           f"room topic {len(room_applied)}; data messages to the socket {texts}; errors "
           f"{[e for e in after if e['type'] == 'error']}",
           "control_applied (volume quieter, source client, applied) on the socket and once on the room topic; "
           "task_seen taken silently; a chat payload on the topic still reaches the socket as `message`")


SCENARIOS: Dict[str, Callable] = {
    "L0": l0, "L2": l2, "L1": l1, "L3": l3, "L9": l9, "L4": l4, "L5": l5, "L7": l7,
    "L6": l6, "L8": l8, "L10": l10, "S1": s1, "S2": s2, "S3": s3, "S4": s4, "S5": s5, "BL": bl,
    "W1": w1, "T1": t1, "R1": r1, "C1": c1, "A1": a1, "UH1": uh1,
    "D1": d1, "H1": h1, "P1": p1, "G1": g1, "O1": o1, "B1": b1, "K1": k1,
    "X1": x1, "X2": x2, "DG1": dg1, "DG2": dg2, "RW": rw, "RA": ra, "RM": rm, "RR": rr,
    "DT1": dt1, "DT2": dt2, "DT3": dt3,
    "BC1": bc1, "TP1": tp1, "PB1": pb1, "F1": f1, "PS1": ps1, "RG1": rg1, "RG2": rg2, "RG3": rg3,
    "LC1": lc1, "KD1": kd1, "LP1": lp1, "SDK1": sdk1, "PF1": pf1, "TD1": td1,
    "PC1": pc1, "BC2": bc2, "CW1": cw1, "EC1": ec1, "FS1": fs1,
    "SF1": sf1, "SF2": sf2, "SF3": sf3, "SF4": sf4,
    "SF5": sf5, "SS1": ss1, "PF2": pf2, "EL1": el1, "RD1": rd1,
    "HC1": hc1, "SS2": ss2, "SP1": sp1, "RV2": rv2, "RL1": rl1,
    "MF1": mf1, "AU1": au1, "AF1": af1, "MEM1": mem1, "RV1": rv1, "CL1": cl1, "MA1": ma1, "CT1": ct1, "CT2": ct2, "CT3": ct3,
    "CT4": ct4, "CT5": ct5, "CT6": ct6, "CT7": ct7,
    "MA4": ma4, "MA2": ma2, "MA3": ma3, "MA5": ma5, "MA6": ma6, "MA7": ma7,
    # MA2 with an 8 s deadline: the same checks, where this stack's voice and recogniser are
    # too slow for 5 s (fin-followup); reported under its own name, MA2 keeps its 5 s.
    "MA2L": ma2l,
    "PR1": pr1, "PR2": pr2, "PR3": pr3, "PR4": pr4, "PR5": pr5,
}
# What a run does without `--only`: the design's scenarios on the request-response recogniser,
# the ones that need no upload timing plus the interim-result ones on the streaming recogniser.
# BL (several minutes), A1 and UH1 (they need `--tts-delay-ms`) only by name. The orchestrator
# scenarios (MA1-MA4, MA2L, MA7, CT3) load `--orchestrator-model`; CT2 needs DEEPGRAM_API_KEY (or
# `--deepgram-env`) and reports NOT RUN (a failure) without it.
DEFAULT = {
    "request": ["L0", "L2", "L1", "L3", "L9", "L4", "L5", "L7", "L6", "L8", "L10", "S4",
                "W1", "T1", "R1", "C1", "D1", "H1", "P1", "G1", "O1", "B1", "K1",
                "X1", "X2", "DG1", "RW", "RA", "RM", "RR", "BC1", "PB1", "F1", "PS1",
                "RG1", "RG2", "RG3", "RV1", "LC1", "KD1", "LP1", "SDK1", "PF1", "DG2", "TD1", "PC1", "BC2", "CW1", "EC1",
                "FS1", "MF1", "AU1", "AF1", "MEM1", "CL1", "PR4", "PR8", "DT1", "DT2", "DT3",
                "MA1", "MA2", "MA2L", "MA3", "MA4", "MA5", "MA6", "MA7", "CT1", "CT2", "CT3",
                "CT4", "CT5", "CT6", "CT7", "SF1", "SF2", "SF3", "SF4",
                "SF5", "SS1", "PF2", "EL1", "RD1", "PR1", "PR2", "PR3", "PR5",
                # r3-s2s (20 Oct).
                "HC1", "SS2", "SP1", "RV2", "RL1",
                # r3-proto (20 Oct). PR8 is with PR4, before DT3: DT3 kills the second task
                # gateway, which PR8's callbacks reach (the task gateways are started once).
                "PR6", "PR7", "PR9", "PR10", "PR10O", "PR11", "FS2", "FS3"],
    "streaming": ["L0", "L2", "L4", "L5", "L7", "L6", "L10", "S1", "S2", "S3", "S4", "S5",
                  "D1", "H1", "P1", "DG1", "RW", "RA", "RM", "RR", "BC1", "TP1", "PB1", "F1", "PS1",
                  "LC1", "LP1", "TD1", "PC1", "BC2", "CW1", "EC1", "MF1", "AU1", "AF1", "MEM1",
                  "CL1", "PR4", "DT1", "DT2", "DT3", "MA5", "CT1", "SF2", "SF3", "SF4",
                  # gap round (19 Oct): the ones whose sessions hear the caller through this
                  # recogniser (CT5, CT7 use Deepgram; SS1, PF2, EL1 their own stand-ins). PR4
                  # before DT3, which kills the second task gateway (as in the request list).
                  "CT4", "CT6", "SF5", "RD1", "PR1", "PR2", "PR3", "PR5"],
}

# ---- r3-proto (20 Oct): A2A bindings and OAuth from the card, pushes on any process, MCP's
# GET stream and control wire, a session's backends, a client's own push, the front's socket
# supervisor and an operator-run server's licence


async def pr6(ctx: Ctx):
    """PR6: a session's own A2A backend: its card is read at the start and says it is reached over HTTP+JSON with an OAuth token from the token endpoint the card names."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    session = await backend_session(ctx, "PR6", "restagent")
    await session.warm()
    start = len(session.events)
    await session.say("20_q_capital")
    result = await session.wait(lambda e: e["type"] == "task_result", 60, start)
    await session.quiet(30, start)
    await session.close()
    cards = stand_in.of("rest", "card", mark)
    tokens = [hit for hit in stand_in.of("token", None, mark) if hit["ok"]]
    sends = stand_in.of("rest", "POST /restapi/message:send", mark)
    reads = [hit for hit in stand_in.seen[mark:] if hit["wire"] == "rest" and str(hit["method"]).startswith("GET /restapi/tasks/")]
    job = stand_in.last("rest") or {}
    issued = {hit.get("token") for hit in tokens}
    checks = {
        "the card is read at the session's start (no push_base_url)": len(cards) >= 1,
        "the token comes from the card's token endpoint (client credentials)": bool(tokens),
        "the task goes to the HTTP+JSON binding with that token": len(sends) == 1 and sends[0]["ok"]
            and sends[0].get("token") in issued,
        "it is read there (GET /tasks/{id}), every request with the token": bool(reads)
            and all(hit["ok"] for hit in reads),
        "the agent's answer is the task's result": bool(result) and result[2].get("state") == "completed"
            and norm(result[2].get("summary", "")) == norm(job.get("result") or "-"),
    }
    report(ctx, "PR6", "A2A over HTTP+JSON, OAuth token endpoint from the card", checks,
           f"card reads {len(cards)}; tokens issued {len(tokens)}; message:send {[hit['ok'] for hit in sends]}; "
           f"task reads {len(reads)} ok {all(hit['ok'] for hit in reads) if reads else None}; result "
           f"{result[2].get('state') if result else None!r} {result[2].get('summary') if result else None!r}",
           "card GET before the task; POST /token (Basic, client_credentials, scope assist) issues the token; "
           "POST /restapi/message:send and GET /restapi/tasks/{id} with Bearer <that token> and A2A-Version 1.0; "
           "task_result completed with the agent's words")


async def pr7(ctx: Ctx, label: str = "PR7", entry: str = "grpcagent"):
    """PR7: an A2A agent on the gRPC binding (lf.a2a.v1.A2AService) whose card names a gRPC interface on the entry's host, streams and has an extended card: the extended card is read with GetExtendedAgentCard, the task is one SendStreamingMessage whose events are its progress and result, each call with the bearer token in its metadata. PR7P: the same agent with a card that does neither: SendMessage, then GetTask."""
    streams = entry == "grpcagent"
    stand_in = ctx.stack.ensure_backends()
    if stand_in.grpc_port is None:
        report(ctx, label, "A2A over gRPC", {"the gRPC stand-in runs (an interpreter with grpcio)": False},
               "NOT RUN: no grpc python (--grpc-python, or $WAAV_LIVE_SCRATCH/tools/grpc-venv)", "-")
        return
    mark = len(stand_in.seen)
    log = Path(stand_in.grpc_log)
    before = len(log.read_text().splitlines()) if log.exists() else 0
    gateway_log = ctx.stack.log_dir / "live-backend-gateway.log"
    log_from = gateway_log.stat().st_size if gateway_log.exists() else 0
    session = await backend_session(ctx, label, entry)
    await session.warm()
    start = len(session.events)
    asked_at = now()
    await session.say("20_q_capital")
    progress = await session.wait(lambda e: e["type"] == "task_progress", 30, start)
    result = await session.wait(lambda e: e["type"] == "task_result", 60, start)
    await session.quiet(30, start)
    await session.close()
    hits = [json.loads(line) for line in log.read_text().splitlines()[before:]] if log.exists() else []
    named = lambda method: [hit for hit in hits if hit["method"] == method]
    sends, streamed, reads, cards = (named(m) for m in ("SendMessage", "SendStreamingMessage", "GetTask",
                                                         "GetExtendedAgentCard"))
    with open(gateway_log, "rb") as handle:
        handle.seek(log_from)
        lines = handle.read().decode(errors="replace").splitlines()
    extended = [line for line in lines if "interaction agent's extended card read" in line]
    checks = {"the card is read at the session's start": bool(stand_in.of(entry, "card", mark))}
    if streams:
        checks.update({
            "the extended card is read with GetExtendedAgentCard, with the bearer token and A2A-Version":
                len(cards) >= 1 and all(hit["authorised"] and hit["a2a_version"] == "1.0" for hit in cards)
                and bool(extended),
            "one SendStreamingMessage over gRPC with the bearer token, no SendMessage and no GetTask":
                len(streamed) == 1 and streamed[0]["authorised"] and bool(streamed[0].get("text"))
                and not sends and not reads,
            "the streamed status update is its progress": progress is not None,
        })
    else:
        checks.update({
            "no extended card is asked for": not cards,
            "one SendMessage over gRPC, return_immediately, with the bearer token and A2A-Version": len(sends) == 1
                and sends[0]["authorised"] and sends[0]["return_immediately"] and sends[0]["a2a_version"] == "1.0"
                and bool(sends[0].get("text")) and not streamed,
            "the task is read with GetTask": bool(reads) and all(hit["authorised"] for hit in reads),
            "the working task's status message is its progress": progress is not None,
        })
    checks["the completed task's artifact is its result"] = (
        bool(result) and result[2].get("state") == "completed" and norm(result[2].get("summary", "")) == norm(GRPC_ANSWER))
    report(ctx, label, "A2A over gRPC" + (" (streamed, extended card)" if streams else " (polled)"), checks,
           f"gRPC calls {[(hit['method'], hit['authorised']) for hit in hits]}; extended card read in the log "
           f"{len(extended)}; progress {progress[2].get('text') if progress else None!r} "
           f"{ms(progress[1] - asked_at) if progress else None} ms; result "
           f"{result[2].get('state') if result else None!r} {result[2].get('summary') if result else None!r} "
           f"{ms(result[1] - asked_at) if result else None} ms",
           ("card read; GetExtendedAgentCard (Bearer, A2A-Version 1.0) and the log's 'extended card read'; one "
            "SendStreamingMessage, no SendMessage/GetTask; task_progress from the streamed status; task_result = "
            "the streamed artifact's text") if streams else
           ("card read; no GetExtendedAgentCard; one SendMessage (return_immediately, Bearer, A2A-Version 1.0); "
            "GetTask until completed; task_result = the artifact's text"))


async def pr7p(ctx: Ctx):
    """PR7P: the gRPC agent whose card neither streams nor has an extended card: SendMessage, then GetTask, each with the bearer token."""
    await pr7(ctx, "PR7P", "grpcpoll")


async def pr8(ctx: Ctx):
    """PR8: a webhook's callback that reaches the other gateway process of the task Redis is handed to the process that runs the task; a forged one is refused there, one for a task nobody runs is 404."""
    sink = ctx.sink()
    first, second = ctx.stack.ensure_durable(sink.url)
    hook = ctx.stack.relay_hook
    log = ctx.stack.log_dir / "live-task-gateway-a.log"
    log_from = log.stat().st_size if log.exists() else 0
    marks = (len(hook.tasks), len(hook.callbacks))
    session = await Session.open(ctx, "PR8", {**MODEL_FRONT, "tasks": {"default_backend": "relayhook"}},
                                 None, conversation=False, port=first)
    await session.warm()
    start = len(session.events)
    await session.say("73_parcel_status")
    created = await session.wait(lambda e: e["type"] == "task_created", 20, start)
    result = await session.wait(lambda e: e["type"] == "task_result", 40, start)
    await session.quiet(20, start)
    await session.close()
    callbacks = hook.callbacks[marks[1]:]
    status = {name: code for name, code, _ in callbacks}
    with open(log, "rb") as handle:
        handle.seek(log_from)
        lines = handle.read().decode(errors="replace").splitlines()
    taken = [line for line in lines if "interaction callback taken from another gateway process" in line]
    refused = [line for line in lines if "interaction callback from another gateway process refused" in line]
    task_id = created[2]["task_id"] if created else "?"
    checks = {
        "the task runs on the first process, its callbacks reach the second": bool(created)
            and len(hook.tasks[marks[0]:]) >= 1 and len(callbacks) == 3,
        "the second process hands the result on (202) and the session's task completes with it":
            status.get("result") == 202 and bool(result) and result[2].get("state") == "completed"
            and norm(result[2].get("summary", "")) == norm(RELAY_ANSWER),
        "the owner checks the signature: the forged one is refused there, never the answer":
            status.get("forged") == 202 and len(refused) >= 1 and task_id in refused[0]
            and "forged" not in norm(result[2].get("summary", "") if result else "forged"),
        "a callback for a task no process runs is 404": status.get("unknown") == 404,
        "the owner took the relayed result": len(taken) >= 1 and task_id in taken[0],
    }
    report(ctx, "PR8", "callbacks that land on the wrong gateway process", checks,
           f"session on :{first}, callbacks to :{second}: {[(name, code) for name, code, _ in callbacks]}; "
           f"owner log taken {len(taken)}, refused {len(refused)}; result "
           f"{result[2].get('state') if result else None!r} {result[2].get('summary') if result else None!r}",
           "result callback to the second process 202 and the session's task_result completed with its text; "
           "the forged one 202 at the second and `refused` in the owner's log (its text never the answer); "
           "an unknown task 404",
           "both task gateways share the run's task Redis; the owner checks the signature (no secret is shared)")


async def pr9(ctx: Ctx):
    """PR9: MCP: the server's GET stream is opened after initialize; an elicitation on it about the call's task is the caller's question; its status notification ends the wait for the next poll; the result's control is applied and its control_applied goes back as a notification."""
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    session = await backend_session(ctx, "PR9", "getstream")
    await session.warm()
    start = len(session.events)
    await session.say("20_q_capital")
    asked = await session.wait(lambda e: e["type"] == "task_question", 30, start)
    answered_at = now()
    if asked:
        await session.send({"type": "task_answer", "request_id": asked[2].get("request_id"),
                            "outcome": "accept", "value": "personal"})
    result = await session.wait(lambda e: e["type"] == "task_result", 40, start)
    applied = await session.wait(lambda e: e["type"] == "control_applied", 10, start)
    await session.quiet(20, start)
    await session.close()
    hits = stand_in.seen[mark:]
    gets = [hit for hit in hits if hit["wire"] == "mcpget" and hit["method"] == "GET"]
    inits = [hit for hit in hits if hit["wire"] == "mcpget" and hit["method"] == "initialize"]
    responses = [hit for hit in hits if hit["wire"] == "mcpget" and hit["method"] == "response"]
    notes = [hit for hit in hits if hit["wire"] == "mcpget" and hit["method"] == "notifications/io.waav/control_applied"]
    polls = [hit for hit in hits if hit["wire"] == "mcpget" and hit["method"] == "tasks/get"]
    result_after = result[1] - answered_at if result else None
    accept = (responses[0]["body"].get("result") or {}) if responses else {}
    params = notes[0]["body"].get("params", {}) if notes else {}
    checks = {
        "the GET stream is opened with the session and the bearer": bool(gets) and gets[0]["ok"]
            and bool(inits) and gets[0]["t"] >= inits[0]["t"],
        "its elicitation about the task is the caller's question": bool(asked)
            and asked[2].get("kind") == "choose" and asked[2].get("options") == ["personal", "work"],
        "the answer goes back as the JSON-RPC response of its id": bool(responses)
            and accept == {"action": "accept", "content": {"list": "personal"}},
        "the status notification on the stream ends the wait (result within 3 s, poll interval 5 s)":
            result_after is not None and result_after <= 3.0,
        "the result is the task's text": bool(result) and result[2].get("state") == "completed"
            and "personal" in norm(result[2].get("summary", "")),
        "the result's control is applied and its control_applied goes back as a notification": bool(applied)
            and applied[2].get("kind") == "verbosity" and applied[2].get("source") == "agent"
            and applied[2].get("outcome") == "applied" and params.get("request_id") == "mcp-brief-1",
    }
    report(ctx, "PR9", "MCP GET stream, elicitation by task, control wire", checks,
           f"GET opens {len(gets)} (ok {[hit['ok'] for hit in gets]}); question {asked[2].get('options') if asked else None}; "
           f"answer {accept}; tasks/get {len(polls)}; result {ms(result_after)} ms after the answer "
           f"{result[2].get('summary') if result else None!r}; control_applied {applied[2] if applied else None}; "
           f"notification params {params}",
           "GET with Mcp-Session-Id and Bearer after initialize; task_question choose [personal, work]; JSON-RPC "
           "result {action: accept, content: {list: personal}}; task_result <= 3,000 ms after the answer; "
           "control_applied verbosity brief agent applied, and notifications/io.waav/control_applied request_id mcp-brief-1")


async def pr10(ctx: Ctx):
    """PR10: tasks.backends lists two catalog entries and no orchestrator: the session's own LLM chooses between them for each request (the flight to the browser agent, the failing test to the coding agent)."""
    stand_in = ctx.stack.ensure_backends()
    prompt = (ORCHESTRATOR_PROMPT + " Every request about travel or a booking goes to the agent for "
              "websites and travel; every request about code goes to the coding agent. Never do "
              "either yourself.")
    session = await Session.open(ctx, "PR10", {**MODEL_FRONT, "hitl": {"channels": ["voice"]},
                                               "tasks": {"backends": ["browser", "coder"]}},
                                 prompt, port=stand_in.port)
    await session.warm()
    start = len(session.events)
    answers: List[float] = []
    caller = asyncio.create_task(answer_questions(session, start, answers))
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    for clip in ("63_book_flight_from", "64_fix_named_test"):
        said_from = len(session.events)
        await session.say(clip)
        await session.wait(is_final, 15, said_from)
        await asyncio.sleep(1.5)
    await settled(session, start, 180)
    caller.cancel()
    await session.close()
    echo = session.ready.get("interaction", {}).get("tasks", {})
    created = [e for _, e in session.all("task_created", start)]
    children = {str(e.get("agent")): e for e in created if e.get("parent_id") is not None}
    requests = [e for e in created if e.get("parent_id") is None]
    results = {e["task_id"]: e for _, e in session.all("task_result", start)}
    checks = {
        "the session's own LLM orchestrates the listed backends": echo.get("orchestrator") is True
            and sorted(echo.get("agents") or []) == ["browser", "coder"],
        "the flight goes to the browser agent": "browser" in children,
        "the failing test goes to the coding agent": "coder" in children,
        "both requests end": len(requests) == 2 and all(results.get(e["task_id"], {}).get("state") == "completed"
                                                        for e in requests),
    }
    report(ctx, "PR10", "tasks.backends chosen per request by the session's own LLM", checks,
           f"echo tasks {echo}; requests {[e.get('title') for e in requests]}; children "
           f"{[(agent, e.get('title')) for agent, e in children.items()]}; results "
           f"{[results.get(e['task_id'], {}).get('state') for e in requests]}",
           "ready echo orchestrator true with agents [browser, coder]; a child on browser for the flight and one on "
           "coder for the test; both requests completed",
           f"the orchestrator is the session's conversation model ({ctx.args.background_alias}), not gpt-oss-20b")


async def pr10o(ctx: Ctx):
    """PR10O: tasks.backends lists two catalog entries and the session names an orchestrator (gpt-oss-20b): they are its agents, and it chooses between them for each request."""
    stand_in = ctx.stack.ensure_backends()
    session = await orchestrated_session(ctx, "PR10O", block={"hitl": {"channels": ["voice"]}},
                                         tasks_extra={"agents": [], "backends": ["browser", "coder"]})
    await session.warm()
    start = len(session.events)
    answers: List[float] = []
    caller = asyncio.create_task(answer_questions(session, start, answers))
    is_final = lambda e: e["type"] == "stt_result" and e.get("is_final")
    for clip in ("63_book_flight_from", "64_fix_named_test"):
        said_from = len(session.events)
        await session.say(clip)
        await session.wait(is_final, 15, said_from)
        await asyncio.sleep(1.5)
    await settled(session, start, 240)
    await updates_said(session, start)
    await settled(session, start)
    caller.cancel()
    await session.close()
    echo = session.ready.get("interaction", {}).get("tasks", {})
    created = [e for _, e in session.all("task_created", start)]
    children = {str(e.get("agent")): e for e in created if e.get("parent_id") is not None}
    requests = [e for e in created if e.get("parent_id") is None]
    results = {e["task_id"]: e for _, e in session.all("task_result", start)}
    checks = {
        "the listed backends are the orchestrator's agents (tasks.agents empty)": echo.get("orchestrator") is True
            and sorted(echo.get("agents") or []) == ["browser", "coder"],
        "the flight goes to the browser agent": "browser" in children,
        "the failing test goes to the coding agent": "coder" in children,
        "both requests end": len(requests) == 2 and all(results.get(e["task_id"], {}).get("state") == "completed"
                                                        for e in requests),
    }
    report(ctx, "PR10O", "tasks.backends chosen per request by the named orchestrator", checks,
           f"echo agents {echo.get('agents')}, orchestrator {echo.get('orchestrator')}; requests "
           f"{[e.get('title') for e in requests]}; children {[(agent, e.get('title')) for agent, e in children.items()]}; "
           f"results {[results.get(e['task_id'], {}).get('state') for e in requests]}",
           "ready echo orchestrator true with agents [browser, coder] from tasks.backends; a browser child for the "
           "flight and a coder child for the test; both requests completed")


async def pr11(ctx: Ctx):
    """PR11: with allow_client_push a session names its own push address and secret; a result that comes after a clean close is pushed there, signed with the session's secret and never the server's."""
    stand_in = ctx.stack.ensure_backends()
    before = len(stand_in.own_pushes)
    tasks = {"default_backend": "hook", "push_url": f"{stand_in.url}/ownpush", "push_secret": CLIENT_PUSH_SECRET}
    session = await Session.open(ctx, "PR11", {**MODEL_FRONT, "tasks": tasks}, None, conversation=False,
                                 port=stand_in.port)
    await session.warm()
    start = len(session.events)
    await session.say("20_q_capital")
    created = await session.wait(lambda e: e["type"] == "task_created", 20, start)
    refused = [w for w in session.warnings if "push" in json.dumps(w)]
    await session.close()
    closed = now()
    while now() - closed < 30 and len(stand_in.own_pushes) == before:
        await asyncio.sleep(0.2)
    pushes = stand_in.own_pushes[before:]
    push = pushes[0] if pushes else {}
    body = push.get("body") or {}
    task_id = created[2]["task_id"] if created else "?"
    checks = {
        "the session's own push address and secret are taken": not refused
            and session.ready.get("interaction", {}).get("tasks", {}).get("push") is True,
        "the result after the close is pushed there": bool(pushes) and body.get("task_id") == task_id
            and body.get("type") == "task_result" and body.get("stream_id") == session.stream_id,
        "signed with the session's own secret, not the server's": push.get("own") is True and push.get("servers") is False,
        "once, keyed by the task": len(pushes) == 1 and push.get("key") == task_id,
    }
    report(ctx, "PR11", "a session's own push address and secret (allow_client_push)", checks,
           f"warnings about the push {refused}; pushes {len(pushes)}; own signature {push.get('own')}, server's "
           f"{push.get('servers')}; body type {body.get('type')!r} task {body.get('task_id')!r} state {body.get('state')!r}; "
           f"{round(push['t'] - closed, 1) if push else None} s after the close",
           "no warning; one push of task_result for the task with the session's stream_id, its X-WaaV-Signature the "
           "HMAC under the session's push_secret (not the server's tool_secret), Idempotency-Key the task id")


def tree_mode(path: Path) -> Optional[int]:
    try:
        return path.stat().st_mode & 0o777
    except OSError:
        return None


async def fs2(ctx: Ctx):
    """FS2: the gateway runs the front model on a private UNIX socket in a per-boot run directory (0700, key file 0600), stops an earlier gateway's left-over child at start, and calls the model ready only after a canary decision."""
    freed = ctx.stack.stop_front()
    # A short root: a UNIX socket's path has at most 107 bytes (the log directory is longer).
    run_dir = Path(f"/tmp/waav-fs2-{ctx.run_id}")
    run_dir.mkdir(parents=True, exist_ok=True)
    # An earlier gateway's left-over child: its gateway is gone, it still runs the front's
    # program (argv[0] is the llama-server's path).
    gone = subprocess.Popen(["true"])
    gone.wait()
    program = f"{ctx.args.llama_dir}/llama-server"
    stale = subprocess.Popen(["bash", "-c", f"exec -a {program} sleep 300"], start_new_session=True)
    stale_dir = run_dir / "front-leftover"
    stale_dir.mkdir(exist_ok=True)
    (stale_dir / "front.pid").write_text(f"{stale.pid} {gone.pid}\n")
    try:
        await fs2_run(ctx, run_dir, stale, stale_dir)
    finally:
        if stale.poll() is None:
            stale.kill()
            stale.wait()
        shutil.rmtree(run_dir, ignore_errors=True)
        if freed:
            ctx.stack.start_front()


async def fs2_run(ctx: Ctx, run_dir: Path, stale: subprocess.Popen, stale_dir: Path):
    started = now()
    port = ctx.stack.ensure_socket_front_gateway(run_dir)
    log = ctx.stack.log_dir / "live-socket-front-gateway.log"

    def lines(needle: str) -> List[str]:
        with open(log, "rb") as handle:
            handle.seek(ctx.stack.socket_log_start)
            return [line for line in handle.read().decode(errors="replace").splitlines() if needle in line]

    deadline = now() + 200
    while now() < deadline and not lines("front model ready"):
        await asyncio.sleep(0.5)
    ready = lines("front model ready")
    stale.poll()
    stale_gone = stale.returncode is not None
    dirs = [path for path in run_dir.glob("front-*") if path.name != "front-leftover"]
    own = dirs[0] if dirs else run_dir / "none"
    socket_path = own / "front.sock"
    children = subprocess.run(["pgrep", "-f", f"llama-server .*--host {socket_path}"], capture_output=True, text=True)
    pids = [int(pid) for pid in children.stdout.split()]
    listening = subprocess.run(["ss", "-ltnp"], capture_output=True, text=True).stdout
    tcp = [line for line in listening.splitlines() if any(f"pid={pid}," in line for pid in pids)]
    session = await Session.open(ctx, "FS2", MODEL_FRONT, PLAIN_PROMPT, port=port)
    await session.warm()
    turn = await session.turn("20_q_capital")
    settled_lines = [line for line in lines("interaction turn settled") if session.stream_id in line]
    completed = [e.get("state") for _, e in session.all("task_result")]
    await session.close()
    process = ctx.stack.started.pop("socket-front-gateway")
    process.terminate()
    process.wait(timeout=40)
    gone_deadline = now() + 10
    while now() < gone_deadline and any(Path(f"/proc/{pid}").exists() for pid in pids):
        await asyncio.sleep(0.2)
    left = [pid for pid in pids if Path(f"/proc/{pid}").exists()]
    canary = re.search(r"canary_ms=(\d+)", ready[0]) if ready else None
    checks = {
        "the earlier gateway's left-over child is stopped and its directory removed": stale_gone and not stale_dir.exists(),
        "this boot's directory is 0700 with a 0600 key file and the socket": tree_mode(own) == 0o700
            and tree_mode(own / "front.key") == 0o600 and socket_path.is_socket(),
        "the child listens on the socket only (no TCP port)": len(pids) == 1 and not tcp,
        "ready only after a canary decision": bool(canary),
        "a session's turn is decided by the model over the socket": bool(settled_lines)
            and "source=Model" in settled_lines[0] and completed.count("completed") >= 1,
        "the child stops with the gateway": not left,
    }
    report(ctx, "FS2", "the front model on the supervisor's private socket", checks,
           f"left-over child stopped {stale_gone}, its directory left {stale_dir.exists()}; run dir {own.name} mode "
           f"{oct(tree_mode(own) or 0)}, key {oct(tree_mode(own / 'front.key') or 0)}, socket {socket_path.is_socket()}; "
           f"child pids {pids}, TCP listeners {tcp}; ready {round(now() - started, 1)} s budget, canary "
           f"{canary.group(1) if canary else None} ms; settled {settled_lines[:1]}; tasks {completed}; left after "
           f"SIGTERM {left}",
           "the left-over `sleep` (argv[0] llama-server, its gateway pid dead) gone and front-leftover removed; "
           "front-<boot> 0700, front.key 0600, front.sock a socket; one llama-server child, no TCP LISTEN of it; "
           "`front model ready` with canary_ms; the turn settled source=Model and its task completed; no child "
           "10 s after the gateway's SIGTERM")


class PropsProxy:
    """FS3: an operator-run front server as the gateway sees it: `GET /props` names an LFM model
    file; every other request goes to the run's front llama-server as it is."""

    def __init__(self, port: int, front: str):
        self.hits: List[str] = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def forward(self, method: str):
                outer.hits.append(f"{method} {self.path}")
                if self.path == "/props":
                    body = json.dumps({"model_path": "/models/LFM2.5-1.2B-Instruct-Q4_K_M.gguf",
                                       "total_slots": 1}).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    return self.wfile.write(body)
                data = self.rfile.read(int(self.headers.get("Content-Length", "0"))) if method == "POST" else None
                request = urllib.request.Request(f"{front}{self.path}", data=data, method=method,
                                                 headers={"Content-Type": "application/json"})
                try:
                    with urllib.request.urlopen(request, timeout=30) as response:
                        status, body = response.status, response.read()
                except urllib.error.HTTPError as error:
                    status, body = error.code, error.read()
                self.send_response(status)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self):
                self.forward("GET")

            def do_POST(self):
                self.forward("POST")

        self.server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()


async def fs3(ctx: Ctx):
    """FS3: an operator-run front server whose /props names LFM weights is refused without the licence statement, and taken with it."""
    args = ctx.args
    proxy = PropsProxy(args.licence_proxy_port, f"http://127.0.0.1:{args.front_port}")
    outcomes = {}
    try:
        for licence in (None, "lfm_open_eligible"):
            port = ctx.stack.start_licence_gateway(f"http://127.0.0.1:{args.licence_proxy_port}", licence)
            session = await Session.open(ctx, f"FS3-{licence or 'none'}", MODEL_FRONT, PLAIN_PROMPT, port=port)
            refused = next((w for w in session.warnings if w.get("code") == "interaction_refused"), None)
            outcomes[licence] = (refused, session.ready.get("interaction", {}).get("enabled"))
            await session.close()
            process = ctx.stack.started.pop("licence-gateway")
            process.terminate()
            process.wait(timeout=40)
    finally:
        proxy.server.shutdown()
    log = (ctx.stack.log_dir / "live-licence-gateway.log").read_text(errors="replace")
    without, with_licence = outcomes.get(None), outcomes.get("lfm_open_eligible")
    checks = {
        "without the statement the server is refused (interaction_refused front_unavailable)": bool(without)
            and bool(without[0]) and (without[0].get("detail") or {}).get("reason") == "front_unavailable"
            and "interaction front model server refused" in log,
        "with lfm_open_eligible it is the session's front": bool(with_licence) and not with_licence[0]
            and with_licence[1] is True,
        "the gateway read the server's /props": "GET /props" in proxy.hits,
    }
    report(ctx, "FS3", "the licence check of an operator-run front server", checks,
           f"without: warning {without[0] if without else None}; with: refused {with_licence[0] if with_licence else None}, "
           f"enabled {with_licence[1] if with_licence else None}; proxy requests {sorted(set(proxy.hits))}",
           "no WAAV_FRONT_LICENCE: interaction_refused reason front_unavailable and the refusal logged; "
           "WAAV_FRONT_LICENCE=lfm_open_eligible: the session runs the mode; /props was read")



SCENARIOS.update({"PR6": pr6, "PR7": pr7, "PR7P": pr7p, "PR8": pr8, "PR9": pr9, "PR10": pr10, "PR10O": pr10o, "PR11": pr11,
                  "FS2": fs2, "FS3": fs3})
# r3-verify: the ones whose sessions hear the caller through the chosen recogniser run on the
# streaming one too (PR10 and PR10O are orchestrator runs, request only as MA1-MA4 are).
DEFAULT["streaming"] += ["PR6", "PR7", "PR9", "PR11", "FS2", "FS3"]
DEFAULT["streaming"].insert(DEFAULT["streaming"].index("DT1"), "PR8")  # before DT3, as on the request list


# ---- r4-left (20 Oct): the relay's half-cascade, adoption after an interruption, a person's
# callback on another gateway process

HC2_INSTRUCTIONS = "HC2: You answer callers of the Larkspur cafe. We open at 9 in the morning."
# SP2: the seconds between the first call's interruption and the second call.
SP2_GAP_S = 3.0
# SP2: adopted 300 ms after `interrupted` (plan chapter 6); the slack is the stand-in's and the
# gateway's own time, well under the 10 s `adopt_by` it took before.
SP2_ADOPT_LIMIT_S = 1.5
PR12_WAIT_S = 15


async def hc2(ctx: Ctx):
    """HC2: the half-cascade on the /v1/realtime relay (a Bud gateway whose relay block asks speech.voice_owner gateway; the in-script vendor writes text replies): every session.update the vendor gets asks for text, the claim nobody backs is never said, each line of the gateway's voice reaches the client as a gateway response (bud_kind voice) with its audio, the caller cutting in stops the voice."""
    vendor = await vendor_stand_in()
    port = ctx.stack.ensure_bud_half()
    log = ctx.stack.log_dir / "live-bud-half-gateway.log"
    since = log.stat().st_size
    mark = vendor.mark()
    vendor.marks.clear()
    session = await Realtime.open(ctx, "HC2", "vendor", model="rt-silent", mic=False, port=port, session={
        "instructions": HC2_INSTRUCTIONS, "output_modalities": ["audio"],
        "audio": {"input": {"turn_detection": None}}})
    await asyncio.sleep(HC1_SECOND_S + HC1_CUT_AFTER_S + 8.0)
    wire = list(session.wire)
    events = list(session.events)
    await session.close()
    echo = session.ready.get("interaction") or {}
    updates = [f for f in vendor.since(mark, "relay") if f.get("type") == "session.update"]
    modalities = [(u.get("session") or {}).get("output_modalities") for u in updates]
    reply, long_at, cut = (vendor.marks.get(k) for k in ("hc2_reply", "hc2_long", "hc2_cut"))
    voice = [(at, e) for at, e in wire if e.get("type") == "response.created"
             and ((e.get("response") or {}).get("metadata") or {}).get("bud_kind") == "voice"]
    voice_ids = {(e.get("response") or {}).get("id") for _, e in voice}
    deltas = [(at, e) for at, e in wire if e.get("type") == "response.output_audio.delta"
              and e.get("response_id") in voice_ids]
    vendor_audio = [e for _, e in wire if e.get("type") == "response.output_audio.delta"
                    and not str(e.get("response_id", "")).startswith("resp_waav_")]
    claims = [e for _, e in events if e.get("type") == "unverified_claim"]
    stopped = [(at, e) for at, e in events if e.get("type") == "agent_audio_stopped"]
    stopped_at = next((at for at, e in stopped if cut and at >= cut), None)
    late = [at for at, _ in deltas if stopped_at and at > stopped_at + HC1_CUT_SLACK_S]
    first_audio = next((at for at, _ in deltas if reply and at >= reply), None)
    said = gateway_voice_lines(log, since)
    segments = [e for _, e in wire if e.get("type") == "bud.audio.segment"]
    checks = {
        "the echo says the gateway's voice and enforced truth": echo.get("regime") == "provider_speech"
            and (echo.get("speech") or {}).get("voice_owner") == "gateway"
            and (echo.get("speech") or {}).get("truth") == "enforced",
        "every session.update the vendor got asks for text only (the client's asked audio)": len(modalities) >= 2
            and all(m == ["text"] for m in modalities),
        "the claim was caught before it was said": len([c for c in claims if "booked" in c.get("text", "")]) == 1
            and all(c.get("output_stopped") is True and c.get("corrected") is True for c in claims),
        "the first sentence and the correction were said, the claim not":
            any(w.startswith("We open at 9") for w in said) and any(w.startswith("Just to be clear") for w in said)
            and not any("booked" in w for w in said),
        f"the voice's lines are gateway responses (bud_kind voice) with audio, the first within "
        f"{HC1_FIRST_AUDIO_S:.0f} s of the reply": len(voice) >= 2 and len(deltas) >= len(voice)
            and first_audio is not None and first_audio - reply <= HC1_FIRST_AUDIO_S,
        "each voice line has its bud.audio.segment": len([e for e in segments if e.get("phase") == "start"
                                                          and str(e.get("utterance_id", "")).startswith("v_")]) >= 2,
        "no audio of the vendor's reaches the client": not vendor_audio,
        "the caller cutting in stops the voice (bud.audio.stopped barge_in)": stopped_at is not None
            and any(e.get("reason") == "barge_in" for at, e in stopped if at >= cut),
    }
    report(ctx, "HC2", "the half-cascade on the /v1/realtime relay", checks,
           f"echo regime {echo.get('regime')} speech voice_owner {(echo.get('speech') or {}).get('voice_owner')}; "
           f"vendor modalities {modalities}; claims {[(c.get('rule'), c.get('text'), c.get('output_stopped')) for c in claims]}; "
           f"said {said}; voice responses {len(voice)}, their deltas {len(deltas)}, first "
           f"{ms(first_audio - reply) if first_audio and reply else None} ms after the reply; vendor audio deltas "
           f"{len(vendor_audio)}; stopped {ms(stopped_at - cut) if stopped_at and cut else None} ms after the cut; "
           f"deltas later than {HC1_CUT_SLACK_S} s after it {len(late)} (sent whole per line before the cut)",
           "echo provider_speech, voice_owner gateway, truth enforced; output_modalities [text] on every vendor "
           "session.update; one unverified_claim (booked) with output_stopped and corrected; 'We open at 9...' and "
           "the correction said, never the claim; >= 2 gateway responses bud_kind voice with deltas, the first <= "
           f"{HC1_FIRST_AUDIO_S:.0f} s after the reply; a bud.audio.segment v_* start per line; no vendor audio; "
           "bud.audio.stopped barge_in after the caller cut in",
           "the second Bud-mode gateway (realtime_default with speech.voice_owner gateway, the voice is its "
           "failure_voice: the speech server's Piper) relaying to the in-script vendor (deployment rt-silent)")


async def sp2(ctx: Ctx):
    """SP2: a Gemini delegate call is provisional (fix gateway with the fixes); the provider stops its turn for the caller (interrupted, no turnComplete): the call is adopted 300 ms later, not at adopt_by (10 s); a second call withdrawn 150 ms after its interruption is never seen."""
    vendor = await vendor_stand_in()
    _, on_port = ctx.stack.ensure_fix_gateways(vendor.port)
    mark = vendor.mark()
    vendor.marks.clear()
    block = {"tasks": {"llm": {"base_url": f"http://127.0.0.1:{vendor.llm_port}/v1", "model": "stand-in",
                               "api_key": "local-dummy"}}}
    events = await rt_frames(on_port, {"type": "config", "provider": "gemini", "model": "gemini-3.8-live",
                                       "instructions": "SP2: you answer questions.", "interaction": block},
                             SP2_GAP_S + 5.0)
    first, second = vendor.marks.get("sp2_interrupted"), vendor.marks.get("sp2_interrupted_b")
    created = [(at, e) for at, e in events if e.get("type") == "task_created"]
    responses = [r.get("id") for f in vendor.since(mark, "gemini")
                 for r in (f.get("toolResponse") or {}).get("functionResponses", [])]
    checks = {
        "the stand-in interrupted both turns": first is not None and second is not None,
        f"the first call is adopted 0.25-{SP2_ADOPT_LIMIT_S} s after the interruption (task_created speculative)":
            len(created) >= 1 and first is not None and 0.25 <= created[0][0] - first <= SP2_ADOPT_LIMIT_S
            and created[0][1].get("speculative") is True,
        "its result goes back on the call": "sp2-a" in responses,
        "the call withdrawn 150 ms after its interruption is never seen": len(created) == 1
            and "sp2-b" not in responses,
    }
    report(ctx, "SP2", "adoption 300 ms after an interruption (Gemini)", checks,
           f"task_created {[(ms(at - first) if first else None, e.get('speculative')) for at, e in created]} ms after "
           f"the first interruption; function responses {responses}",
           f"one task_created, speculative, 250-{SP2_ADOPT_LIMIT_S * 1000:.0f} ms after the first `interrupted`; "
           "a functionResponse for sp2-a; none for sp2-b, which was withdrawn 150 ms after its `interrupted`",
           "fix-gateway-on (gemini_tool_call_cancellation listed), Gemini stand-in, in-script LLM stand-in")


async def pr12(ctx: Ctx):
    """PR12: a person hand-off (controls.human.url) on one task gateway whose operator answers on the other's callback route: a callback with no proof is refused 401 there before the task Redis is asked, the signed human_unavailable is handed to the waiting process (202), which takes it: human_answered unavailable."""
    sink = ctx.sink()
    first, second = ctx.stack.ensure_durable(sink.url)
    hook = ctx.stack.person_hook
    log = ctx.stack.log_dir / "live-task-gateway-a.log"
    log_from = log.stat().st_size if log.exists() else 0
    session = await Session.open(ctx, "PR12", MODEL_FRONT, PLAIN_PROMPT, port=first)
    await session.warm()
    start, mark = len(session.events), len(hook.calls)
    await control_turn(session, "90_a_person", 30)
    requested = await session.wait(lambda e: e["type"] == "human_requested", 15, start)
    deadline = now() + 10
    while now() < deadline and not [c for c in hook.calls[mark:]
                                    if (c.get("body") or {}).get("type") == "human_requested"]:
        await asyncio.sleep(0.1)
    pushes = [c for c in hook.calls[mark:] if (c.get("body") or {}).get("type") == "human_requested"]
    task_id = str((pushes[0]["body"] if pushes else {}).get("task_id"))
    await asyncio.sleep(2.0)

    def bare(port: int) -> int:
        request = urllib.request.Request(f"http://127.0.0.1:{port}/v1/tasks/{task_id}/callback",
                                         data=b'{"type": "human_connected"}',
                                         headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code
        except Exception:
            return 0

    unproved = await asyncio.to_thread(bare, second)
    answered_at = now()
    status = await asyncio.to_thread(signed_callback, second, task_id, {"type": "human_unavailable"},
                                     ctx.args.tool_secret)
    outcome = await session.wait(lambda e: e["type"] == "human_answered", 10, start)
    await session.quiet(3, start)
    await session.close()
    with open(log, "rb") as handle:
        handle.seek(log_from)
        lines = handle.read().decode(errors="replace").splitlines()
    taken = [line for line in lines if "interaction callback taken from another gateway process" in line
             and task_id in line]
    checks = {
        "the hand-off went to the operator with a human_ task id": requested is not None and len(pushes) == 1
            and task_id.startswith("human_"),
        "a callback with no proof is 401 at the other process": unproved == 401,
        "the signed answer at the other process is handed on (202)": status == 202,
        "the waiting process took it: human_answered unavailable, before the wait ran out": outcome is not None
            and outcome[2].get("outcome") == "unavailable" and outcome[1] - answered_at < PR12_WAIT_S - 5,
        "the owner's log says it took a callback from another process": len(taken) >= 1,
    }
    report(ctx, "PR12", "a person's callback on another gateway process", checks,
           f"session on :{first}, callbacks to :{second}: unproved {unproved}, signed {status}; human_answered "
           f"{outcome[2].get('outcome') if outcome else None} {ms(outcome[1] - answered_at) if outcome else None} ms "
           f"after the answer; owner log taken {len(taken)}; task {task_id}",
           "human_requested and one request with task_id human_*; the bare callback 401; the signed human_unavailable "
           "202; human_answered unavailable within a few seconds (not at the 15 s wait); 'taken from another gateway "
           "process' in the owner's log",
           "both task gateways share the run's task Redis; their operator block sets controls.human.url to a local "
           "stand-in (wait_s 15)")


SCENARIOS.update({"HC2": hc2, "SP2": sp2, "PR12": pr12})
DEFAULT["request"] += ["PR7P", "HC2", "SP2"]
DEFAULT["request"].insert(DEFAULT["request"].index("DT1"), "PR12")  # before DT3 (it kills gateway b)
DEFAULT["streaming"] += ["PR7P"]
DEFAULT["streaming"].insert(DEFAULT["streaming"].index("DT1"), "PR12")


def one_started(ctx: Ctx):
    """Every utterance of every session of the run made the pump report `Started` once."""
    total, repeated = 0, []
    for label, stream_id in ctx.sessions:
        counts = ctx.stack.started_counts(stream_id)
        total += len(counts)
        repeated += [f"{label} u_{utt} x{count}" for utt, count in counts.items() if count > 1]
    if not total:
        return  # no interaction session played anything
    ctx.add("PUMP", "one Started per utterance (no Idle/Started flapping)", not repeated,
            f"{total} utterances played in {len(ctx.sessions)} sessions; with more than one Started: "
            f"{repeated or 'none'}",
            "every utterance that played produced exactly one pump Started",
            "read from the gateway's debug log line `interaction pump event`")


SPEAKER_LABEL = re.compile(r"\s*(to\s*)?(front|backend|user|caller|assistant|agent)\s*:", re.I)


def no_speaker_label(ctx: Ctx):
    """No sentence relayed from the background opens with a speaker label."""
    if not ctx.relayed:
        return
    labelled = [text for text in ctx.relayed if SPEAKER_LABEL.match(text)]
    stripped, _ = ctx.stack.log_count("interaction speaker label stripped", ctx.log_start)
    ctx.add("LABEL", "no relayed sentence opens with a speaker label", not labelled,
            f"{len(ctx.relayed)} relayed sentences in {len(ctx.sessions)} sessions; opening with a label: "
            f"{labelled or 'none'}; labels the gateway stripped in this run: {stripped}",
            "no `assistant_transcript` of kind relay starts with `Front:`, `ToFront:` or another role label",
            "the count of stripped labels is the gateway's debug log line `interaction speaker label stripped`")


def verdict(result: Result) -> str:
    return "SKIP" if result.skipped else "PASS" if result.passed else "FAIL"


def table(results: List[Result]) -> str:
    lines = ["", f"{'id':<5}{'result':<8}measured  (threshold)"]
    for result in results:
        lines.append(f"{result.id:<5}{verdict(result):<8}{result.measured}")
        lines.append(f"{'':<13}threshold: {result.threshold}")
        if result.notes:
            lines.append(f"{'':<13}note: {result.notes}")
    return "\n".join(lines)


async def run(args) -> int:
    if args.tts_delay_ms:
        # The gateway this run starts synthesises through the delaying proxy.
        args.audio_url = SlowSpeech(args.audio_url, args.tts_delay_ms / 1000).url
    voices = None
    chosen = [name.strip() for name in args.only.upper().split(",")] if args.only else ["CL1"]
    if args.start_stack and {"CL1", "SF3"} & set(chosen):
        # CL1's second voice and SF3's voice whose Piper is killed: the gateway this run starts
        # synthesises through the router.
        models = {f"cl1{name}": str(Path(args.scratch) / model) for name, model in CL1_VOICES.items() if model}
        models[SF3_VOICE] = str(Path(args.scratch) / CL1_VOICES["alan"])
        voices = VoiceRouter(args.audio_url, models, args.piper, Path(args.out_dir) / "cl1-piper")
        args.audio_url = voices.url
    stack = Stack(args)
    stack.voices = voices
    ctx = Ctx(args, stack, Webhook(args.tool_secret))
    stack.booking_tool = ctx.webhook.tool("book_table")
    wanted = [name.strip().upper() for name in args.only.split(",")] if args.only else DEFAULT[args.stt]
    summaries: Dict[str, Dict[str, str]] = {}
    try:
        if {"RL1", "HC2"} & set(wanted):
            # RL1's deployment names the vendor stand-in's port: it exists before the Bud gateway.
            await vendor_stand_in()
        stack.ensure()
        ctx.log_start = stack.log_size()
        if {"L4", "L5", "L7"} & set(wanted):
            print("preflight: the answer marker on the background model")
            marked = preflight(ctx)
            ctx.marker_noncompliant = marked < 4
            print(f"  ANSWER: on {marked} of 5 final rounds"
                  + (" -> marker_noncompliant (the answer is released at the end of the round)"
                     if ctx.marker_noncompliant else ""))
        for name in wanted:
            print(f"{name}: {SCENARIOS[name].__doc__.strip().splitlines()[0]}")
            try:
                await SCENARIOS[name](ctx)
            except Exception as error:  # a scenario that cannot run has failed
                ctx.add(name, SCENARIOS[name].__doc__.strip().splitlines()[0], False,
                        f"did not run to the end: {error!r}", "see the design table")
            for caller in list(ctx.rooms):
                await caller.leave()
            for session in list(ctx.live):
                await session.close()
        await asyncio.sleep(0.5)
        one_started(ctx)
        no_speaker_label(ctx)
    finally:
        await asyncio.sleep(0.5)
        summaries = stack.summaries()
        stack.stop()
        if voices is not None:
            voices.close()
    print(table(ctx.results))
    print("\ngateway interaction_session_summary lines:")
    for label, stream_id in ctx.sessions:
        if stream_id in summaries:
            print(f"  {label}: {summaries[stream_id]['line']}")
    if ctx.marker_noncompliant:
        print("\nmarker_noncompliant: the background model wrote ANSWER: on fewer than 4 of 5 final rounds")
    failed = [result.id for result in ctx.results if not result.passed and not result.skipped]
    skipped = [result.id for result in ctx.results if result.skipped]
    passed = len(ctx.results) - len(failed) - len(skipped)
    print(f"\n{passed} passed, {len(failed)} failed" + (f": {', '.join(failed)}" if failed else "")
          + (f", {len(skipped)} skipped: {', '.join(skipped)}" if skipped else ""))
    return 1 if failed else 0


# ---- r3-misc (20 Oct): keypad answers (a pause ends a value, choices by number), a person on
# the webhook's callback route, the caller's remembered language wish, POST /speak features and
# the provider fixes behind the remaining escape flags

# CT4's human gateway: how long the caller waits for the operator's answer (`controls.human.wait_s`).
CT4_WAIT_S = 15
FINDING_LINE = "Of course, I'm finding a person for you. One moment."
NO_ANSWER_LINE = "Sorry, nobody can take your call right now, but I am still here to help."
CHOICE_HINT = "You can also press 1 for roses, or 2 for tulips."
# KD2, written before the first run: a typed value with no # is answered policy.hitl_key_gap_ms
# (5,000 ms) after its last key; the actor's timer and the room's DTMF delivery add little.
KEY_GAP_S = 5.0
KEY_GAP_SLACK_S = 2.0


def signed_callback(port: int, task_id: str, event: dict, secret: str) -> int:
    """POST /v1/tasks/<task_id>/callback on the gateway at `port`, signed as a webhook backend's."""
    body, stamp = json.dumps(event).encode(), str(int(time.time()))
    mac = hmac.new(secret.encode(), f"{stamp}.".encode() + body, hashlib.sha256)
    request = urllib.request.Request(f"http://127.0.0.1:{port}/v1/tasks/{task_id}/callback", data=body, headers={
        "Content-Type": "application/json", "X-WaaV-Timestamp": stamp,
        "X-WaaV-Signature": "sha256=" + mac.hexdigest()})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return response.status
    except urllib.error.HTTPError as error:
        return error.code
    except Exception:
        return 0


async def kd2_choice(ctx: Ctx) -> Tuple[Dict[str, bool], str]:
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    block = {**MODEL_FRONT, "tasks": {"default_backend": "hook"}, "hitl": {"channels": ["voice", "dtmf"]}}
    session = await Session.open(ctx, "KD2-choice", block, conversation=False, mic=False,
                                 port=stand_in.port, livekit={"room_name": f"kd2c-{ctx.run_id}"})
    phone = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "sip_caller", kind="sip").join()
    await session.warm()
    await asyncio.sleep(3.0)
    await settle(session, phone)
    start = len(session.events)
    await phone.say("50_order_flowers")
    asked = await session.wait(lambda e: e["type"] == "task_question", 40, start)
    hint = await said(session, start, CHOICE_HINT, 40)
    if hint:
        await heard(session, hint, 40)
    keys_from = len(session.events)
    pressed = now()
    await phone.room.local_participant.publish_dtmf(code=2, digit="2")
    answer = await answer_received(session, keys_from, 20)
    answered_s = (session.all("task_answer_received", keys_from)[0][0] - pressed if answer else None)
    result = await session.wait(lambda e: e["type"] in ("task_result", "task_cancelled"), 90, keys_from)
    await session.quiet(20, start)
    await phone.leave()
    await session.close()
    got = [hit["body"].get("value") for hit in stand_in.of("hook", "answer", mark)]
    checks = {
        "the choice is asked with a key per option": bool(asked) and asked[2].get("kind") == "choose"
                                                      and bool(hint),
        "key 2 picks the second option": bool(answer)
            and (answer.get("outcome"), answer.get("by")) == ("accept", "dtmf")
            and str(answer.get("value")).lower() == "tulips"
            and answered_s is not None and answered_s <= KEY_ANSWER_LIMIT_S,
        "the backend gets it and the task completes": [str(v).lower() for v in got] == ["tulips"]
            and bool(result) and result[2].get("state") == "completed",
    }
    measured = (f"choice: task_question {asked[2].get('kind') if asked else None!r} "
                f"{asked[2].get('options') if asked else None}; hint said {bool(hint)}; key 2 -> "
                f"task_answer_received {answer} {ms(answered_s) if answered_s is not None else None} ms after the "
                f"key; the webhook got {got}; then {result[2].get('state') if result else None!r}")
    return checks, measured


async def kd2_pause(ctx: Ctx) -> Tuple[Dict[str, bool], str]:
    stand_in = ctx.stack.ensure_backends()
    mark = len(stand_in.seen)
    block = {**MODEL_FRONT, "tasks": {"default_backend": "hook"}, "hitl": {"channels": ["voice", "dtmf"]}}
    session = await Session.open(ctx, "KD2-pause", block, conversation=False, mic=False,
                                 port=stand_in.port, livekit={"room_name": f"kd2p-{ctx.run_id}"})
    phone = await RoomCaller(ctx, session.ready.get("livekit_room_name", ""), "sip_caller", kind="sip").join()
    await session.warm()
    await asyncio.sleep(3.0)
    await settle(session, phone)
    start = len(session.events)
    await phone.say("59_pay_bill")
    asked = await session.wait(lambda e: e["type"] == "task_question", 40, start)
    prompt = await said(session, start, KEYPAD_CARD, 30)
    if prompt:
        await heard(session, prompt, 40)
    keys_from = len(session.events)
    for digit in "4242":
        await phone.room.local_participant.publish_dtmf(code=int(digit), digit=digit)
        last = now()
        await asyncio.sleep(0.2)
    typed = await answer_received(session, keys_from, KEY_GAP_S + KEY_GAP_SLACK_S + 5)
    gap_s = (session.all("task_answer_received", keys_from)[0][0] - last if typed else None)
    result = await session.wait(lambda e: e["type"] in ("task_result", "task_cancelled"), 60, keys_from)
    await session.quiet(20, start)
    await phone.leave()
    await session.close()
    got = [hit["body"].get("value") for hit in stand_in.of("hook", "answer", mark)]
    checks = {
        "the card number is asked for on the keypad": bool(asked) and bool(prompt),
        "with no #, the pause after the last key answers it, redacted": bool(typed)
            and (typed.get("outcome"), typed.get("by")) == ("accept", "dtmf") and typed.get("value") is None
            and typed.get("value_redacted") is True and gap_s is not None
            and KEY_GAP_S - 0.5 <= gap_s <= KEY_GAP_S + KEY_GAP_SLACK_S,
        "the backend gets the digits and the task completes": got == ["4242"] and bool(result)
            and result[2].get("state") == "completed",
    }
    measured = (f"pause: keys 4 2 4 2 and no # -> task_answer_received {typed} "
                f"{ms(gap_s) if gap_s is not None else None} ms after the last key; the webhook got {got}; then "
                f"{result[2].get('state') if result else None!r}")
    return checks, measured


async def kd2(ctx: Ctx):
    """KD2: a phone caller picks a choice's option by its number, and a value typed with no # is answered after a pause."""
    try:
        import livekit.rtc  # noqa: F401
    except ImportError:
        raise RuntimeError("KD2 needs the LiveKit client: pip install livekit livekit-api")
    ctx.stack.ensure_livekit()
    parts = []
    for part in (kd2_choice, kd2_pause):
        try:
            parts.append(await part(ctx))
        except Exception as error:
            parts.append(({f"{part.__name__} ran": False}, f"{part.__name__} did not run to the end: {error!r}"))
            for caller in list(ctx.rooms):
                await caller.leave()
    (choice_checks, choice_measured), (pause_checks, pause_measured) = parts
    report(ctx, "KD2", "keypad: an option by number, a value ended by a pause", {**choice_checks, **pause_checks},
           f"{choice_measured}; {pause_measured}",
           f"the flowers choice is followed by {CHOICE_HINT!r}; key 2 gives task_answer_received accept by dtmf with "
           f"value tulips within {KEY_ANSWER_LIMIT_S:.0f} s, the webhook gets tulips and the task completes; the card "
           f"number typed as 4 2 4 2 with no # is answered (accept, dtmf, value null, value_redacted) "
           f"{KEY_GAP_S - 0.5:.1f} to {KEY_GAP_S + KEY_GAP_SLACK_S:.1f} s after the last key, the webhook gets 4242",
           "the phone line is a participant with a token of kind sip that publishes SIP DTMF packets through "
           "the LiveKit server (no SIP trunk); both questions are the webhook stand-in's (backend gateway). "
           "The recording pause is not run: no LiveKit egress service here")


async def ct8(ctx: Ctx):
    """CT8: the caller's language wish is remembered: 'talk to me in Spanish' on one call, and the same verified caller's next call starts in Spanish; forgetting the caller forgets it."""
    port = ctx.stack.ensure_controls_gateway()
    if port is None:
        ctx.add("CT8", "the caller's language wish is remembered", False,
                "NOT RUN: DEEPGRAM_API_KEY is not in the environment nor in --deepgram-env",
                "the Spanish switch needs a multilingual recogniser and a voice list; neither is local")
        return None
    extra = {
        "stt_config": {"provider": "deepgram", "model": "nova-3", "language": "multi",
                       "sample_rate": CALLER_RATE, "channels": 1, "punctuation": True, "encoding": "linear16"},
        "tts_config": {"provider": "deepgram", "voice_id": "aura-2-thalia-en", "audio_format": "pcm",
                       "sample_rate": AGENT_RATE},
    }
    sub = f"ct8-caller-{ctx.run_id}"
    block = lambda: {**MODEL_FRONT, "front": {**MODEL_FRONT["front"], "caller": {"identity_token": mint_identity(sub)}}}
    first = await Session.open(ctx, "CT8-first", block(), PLAIN_PROMPT, extra=extra, port=port)
    await first.warm()
    refused = [w.get("code") for w in first.warnings
               if w.get("code") in ("caller_identity_rejected", "caller_memory_unavailable")]
    asked = await control_turn(first, "84_in_spanish", 30)
    await first.quiet(15)
    await first.close()
    await asyncio.sleep(1.5)
    nxt = await Session.open(ctx, "CT8-next", block(), PLAIN_PROMPT, extra=extra, port=port)
    remembered = await nxt.wait(lambda e: e["type"] == "control_applied" and e.get("source") == "memory", 40)
    await nxt.quiet(15)
    states = [e.get("state") for _, e in nxt.all("interaction_updated") if e.get("state")]
    state = states[-1] if states else {}
    await nxt.close()
    base = f"http://127.0.0.1:{port}/v1/interaction/memory"
    forgot, _ = put_status(f"{base}/{sub}", {"facts": []})
    again = await Session.open(ctx, "CT8-forgotten", block(), PLAIN_PROMPT, extra=extra, port=port)
    await again.quiet(10)
    after = [e for _, e in again.all("control_applied") if e.get("source") == "memory"]
    await again.close()
    checks = {
        "the caller's token is taken": not refused,
        "the first call's switch is applied (partial)": bool(asked)
            and (asked.get("kind"), asked.get("value"), asked.get("outcome")) == ("language", "es", "partial"),
        "the next call starts in Spanish from memory": bool(remembered)
            and (remembered[2].get("kind"), remembered[2].get("value")) == ("language", "es")
            and remembered[2].get("outcome") in ("applied", "partial") and state.get("language") == "es"
            and str(state.get("voice", "")).endswith("-es"),
        "forgotten with the caller's facts": forgot == 204 and not after,
    }
    report(ctx, "CT8", "the caller's language wish is remembered", checks,
           f"identity warnings {refused}; first call {asked}; next call {remembered[2] if remembered else None} "
           f"{(remembered[1] - nxt.opened) if remembered else -1:.1f} s after it opened, state {state}; forget PUT "
           f"{forgot}, then memory controls {after}",
           "control_applied language es partial on the first call; on the next call of the same verified caller "
           "control_applied language es with source memory and the state in es with an *-es voice; after PUT "
           "facts [] (204) a third call gets no memory control",
           "Deepgram nova-3 multi + Aura-2 on the controls gateway (`--controls-gateway-port`, caller memory "
           "key of the run); spend is recorded by the caller of this script")
    return None


def http_speak(port: int, tts: dict, text: str) -> Tuple[int, int]:
    """POST /speak on the gateway at `port`: the status and the bytes of audio."""
    body = json.dumps({"text": text, "tts_config": tts}).encode()
    request = urllib.request.Request(f"http://127.0.0.1:{port}/speak", data=body,
                                     headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return response.status, len(response.read())
    except urllib.error.HTTPError as error:
        return error.code, 0
    except Exception:
        return 0, 0


async def pf3_session(port: int, config: dict, steps: List[Any], seconds: float = 2.5) -> List[dict]:
    """A `/realtime` session's JSON frames: `steps` (JSON messages or audio bytes) are sent once
    `session_created` is in; a function call is answered at once."""
    out: List[dict] = []
    sent = False
    async with websockets.connect(f"ws://127.0.0.1:{port}/realtime", max_size=None, ping_interval=None) as ws:
        await ws.send(json.dumps(config))
        deadline = now() + seconds
        while now() < deadline:
            try:
                frame = await asyncio.wait_for(ws.recv(), max(0.05, deadline - now()))
            except Exception:
                break
            if not isinstance(frame, str):
                continue
            event = json.loads(frame)
            out.append(event)
            if event.get("type") == "session_created" and not sent:
                sent = True
                for step in steps:
                    await ws.send(step if isinstance(step, bytes) else json.dumps(step))
            if event.get("type") == "function_call":
                await ws.send(json.dumps({"type": "function_result", "call_id": event.get("call_id"),
                                          "result": json.dumps({"booked": True})}))
    return out


async def pf3(ctx: Ctx):
    """PF3: the provider fixes behind the escape flags gemini_media_chunks, gemini_response_id_only, allow_setup_resend and deepgram_settings_resend: today's frames without them, the fixed ones with a session's provider_fixes."""
    vendor = await vendor_stand_in()
    off_port, on_port = ctx.stack.ensure_fix_gateways(vendor.port)
    update = {"type": "update_session", "instructions": "PF3 answer in French."}
    audio = bytes(3200)
    gemini = {"type": "config", "provider": "gemini", "model": "gemini-3.8-live", "tools": [PF1_TOOL],
              "instructions": "PF3"}
    deepgram = {"type": "config", "provider": "deepgram", "instructions": "PF3"}
    gemini_fixes = {"gemini_realtime_audio": True, "gemini_response_name": True, "setup_once": True}
    out: Dict[str, Any] = {}
    for label, port, fixes in (("today", off_port, None), ("fixed", on_port, gemini_fixes)):
        mark = vendor.mark()
        config = {**gemini, **({"provider_fixes": fixes} if fixes else {})}
        frames = await pf3_session(port, config, [audio, update], 3.0)
        sent = vendor.since(mark, "gemini")
        out[f"gemini_{label}"] = {
            "setups": sum(1 for f in sent if "setup" in f),
            "audio": sorted({k for f in sent for k in (f.get("realtimeInput") or {})}),
            "responses": [r for f in sent for r in (f.get("toolResponse") or {}).get("functionResponses", [])],
            "client": [(e.get("type"), e.get("code")) for e in frames
                       if e.get("type") in ("session_updated", "error", "warning")],
        }
    # deepgram_update_prompt is on by default (r3-misc probe): today's wire is the session's own `false`.
    for label, port, fixes in (("today", off_port, {"deepgram_update_prompt": False}),
                               ("fixed", on_port, {"deepgram_update_prompt": True})):
        mark = vendor.mark()
        config = {**deepgram, **({"provider_fixes": fixes} if fixes else {})}
        frames = await pf3_session(port, config, [update], 2.5)
        sent = vendor.since(mark, "deepgram")
        out[f"deepgram_{label}"] = {
            "kinds": [f.get("type") for f in sent if f.get("type") in ("Settings", "UpdatePrompt", "UpdateSpeak")],
            "prompt": next((f.get("prompt") for f in sent if f.get("type") == "UpdatePrompt"), None),
            "client": [(e.get("type"), e.get("code")) for e in frames
                       if e.get("type") in ("session_updated", "error", "warning")],
        }
    g0, g1, d0, d1 = (out[k] for k in ("gemini_today", "gemini_fixed", "deepgram_today", "deepgram_fixed"))
    checks = {
        "Gemini today: mediaChunks, a response by id only, a second setup": g0["audio"] == ["mediaChunks"]
            and bool(g0["responses"]) and all("name" not in r for r in g0["responses"]) and g0["setups"] == 2
            and ("session_updated", None) in g0["client"],
        "Gemini fixed: realtimeInput.audio": g1["audio"] == ["audio"],
        "Gemini fixed: the function response names its function": bool(g1["responses"])
            and all(r.get("name") == "book_table" for r in g1["responses"]),
        "Gemini fixed: the update is refused, one setup": g1["setups"] == 1 and ("error", "update_error") in g1["client"],
        "Deepgram today: a second Settings": d0["kinds"] == ["Settings", "Settings"],
        "Deepgram fixed: UpdatePrompt, no second Settings": d1["kinds"] == ["Settings", "UpdatePrompt"]
            and d1["prompt"] == "PF3 answer in French." and ("session_updated", None) in d1["client"],
    }
    report(ctx, "PF3", "the provider fixes behind the remaining escape flags", checks, f"{out}",
           "without the fixes (fix-gateway-off): Gemini audio as realtimeInput.mediaChunks, the client's function "
           "response without name, update_session = a second setup (session_updated); Deepgram update_session = a "
           "second Settings. With the session's provider_fixes on fix-gateway-on: realtimeInput.audio, name "
           "book_table, the update refused (error update_error, one setup); Deepgram UpdatePrompt with the new "
           "instructions and no second Settings",
           "in-script vendor stand-in (its Gemini calls book_table once for a setup naming PF3); no vendor key")


SCENARIOS.update({"KD2": kd2, "CT8": ct8, "PF3": pf3})
DEFAULT["request"] += ["KD2", "CT8", "PF3"]
DEFAULT["streaming"] += ["KD2"]


# ---- r3-fix (20 Oct): the near-duplicate question of a repeated action

NEW_CLIPS["96_book_four_sunday"] = "Please book a table for four at the Olive Garden on Sunday at 8pm."


async def d2(ctx: Ctx):
    """A request like a booking just made is asked about ("change it, or a second one?"); a second one is booked."""
    mark = len(ctx.webhook.calls)
    session = await Session.open(ctx, "D2", with_tools(ctx, "book_table"), TOOLS_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("10_book_table")
    _, after = await asked_and_heard(session, start)
    await session.say("12_yes")
    first = await session.wait(lambda e: e["type"] == "task_result", 60, after)
    await session.quiet(30, start)
    # As D1: past the 5 s in which a turn made mostly of an agent line's words is its echo,
    # well inside the 30 s a finished task can be repeated.
    await asyncio.sleep(6.0)
    again = len(session.events)
    booked_before = len(ctx.webhook.since(mark))
    await session.say("96_book_four_sunday")
    which = await session.wait(lambda e: e["type"] == "assistant_transcript" and e.get("kind") == "question"
                               and e.get("delta") == WHICH, 30, again)
    calls_at_question = len(ctx.webhook.since(mark))
    second = None
    answered = None
    if which:
        await heard(session, which)
        await asyncio.sleep(0.45)
        answered = len(session.events)
        await session.say("49_second_one")
        try:
            _, after_second = await asked_and_heard(session, answered)
            await session.say("12_yes")
            second = await session.wait(lambda e: e["type"] == "task_result", 60, after_second)
        except RuntimeError:
            pass
    await session.quiet(30, again)
    await session.close()
    calls = ctx.webhook.since(mark)
    parties = [call["arguments"].get("party_size") for call in calls if isinstance(call["arguments"], dict)]
    days = [str(call["arguments"].get("day", "")).lower() for call in calls if isinstance(call["arguments"], dict)]
    asked = [e["delta"] for _, e in session.transcripts(again) if e.get("kind") == "question" and e.get("delta")]
    checks = {
        "the first booking is made once, for 2": bool(first) and first[2].get("state") == "completed"
            and booked_before == 1 and parties[:1] == [2],
        "the request like it, seconds later, is asked about": bool(which),
        "nothing is booked before the caller answers": calls_at_question == booked_before,
        "a second one: a booking of its own for 4 on Sunday": bool(second) and second[2].get("state") == "completed"
            and len(calls) == 2 and parties[1:] == [4] and "sunday" in days[1],
    }
    report(ctx, "D2", "a request like a booking just made is asked about; a second one is booked", checks,
           f"bookings {[(c['tool'], c['arguments']) for c in calls]}; questions after the second request {asked}; "
           f"first result {first[2].get('state') if first else None!r}, second {second[2].get('state') if second else None!r}",
           f"after a booking that completed, 'Please book a table for four ... Sunday at 8pm' 6 s later gets "
           f"{WHICH!r} before any call; 'A second one, please.' gives its own confirmation, and after the yes "
           "a second call (party 4, Sunday); two calls in all",
           "the repeat is read by the front (REPEAT_SYSTEM); the booking tool commits, so the finished task "
           "is one a request may repeat (policy `repeatable`)")


SCENARIOS.update({"D2": d2})
DEFAULT["request"] += ["D2"]
DEFAULT["streaming"] += ["D2"]


# ---- r10-grok (23 Oct): xAI Grok on /realtime with the Grok fixes on by default

# A male built-in voice: xAI's default (`ara`) is a female one, so the pitch shows the choice.
GK1_VOICE = "rex"
GK1_PROMPT = "You are a helpful voice assistant. Answer in one short sentence."
# The mid-session update: a reply rule whose word no answer has otherwise.
GK1_RULE_WORD = "banana"
GK1_RULE = (f"New rule, replacing all earlier instructions: reply to every message with exactly one word, "
            f"{GK1_RULE_WORD}, and nothing else.")
GK1_TOOL = {"type": "function", "function": {
    "name": "get_weather", "description": "Get the current weather for a city.",
    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}}


async def gk1(ctx: Ctx):
    """xAI Grok on /realtime with its default fixes (needs GROK_API_KEY): the voice, the caller's audio, typed text, a client function, an update, a cancel, manual turns."""
    port = ctx.stack.ensure_grok_gateway()
    if port is None:
        ctx.add("GK1", "xAI Grok on /realtime", False,
                "SKIPPED: GROK_API_KEY is not in the environment nor in --xai-env", "needs an xAI key",
                skipped=True)
        return
    config = {"provider": "grok", "voice": GK1_VOICE, "instructions": GK1_PROMPT, "tools": [GK1_TOOL],
              "transcribe_input": True, "language": "en", "turn_detection": {"mode": "server_vad"}}
    is_done = lambda e: e["type"] in ("response_done", "error")
    opened = now()

    def words(session: Native, start: int, role: str) -> str:
        return " ".join(e["text"] for _, e in session.events[start:] if e["type"] == "transcript"
                        and e.get("role") == role and e.get("is_final")).strip()

    def audio_s(session: Native, since: float, until: float = float("inf")) -> float:
        return sum(len(data) for at, data in session.audio if since <= at < until) / 2 / AGENT_RATE

    async def typed(session: Native, text: str) -> int:
        start = len(session.events)
        await session.send({"type": "text", "text": text})
        await session.send({"type": "create_response"})
        return start

    session = await Native.open_native(ctx, "GK1", port, config)
    await asyncio.sleep(1.5)
    # The caller's audio, answered by server VAD.
    start, at = len(session.events), now()
    await session.say("20_q_capital")
    spoken = await session.wait(is_done, 25, start)
    caller, answer, answer_s = words(session, start, "user"), words(session, start, "assistant"), audio_s(session, at)
    # Typed text is the user's.
    start = await typed(session, "What is two plus two? Answer with one word.")
    summed = await session.wait(is_done, 25, start)
    four = words(session, start, "assistant")
    # A client function: called, answered from its result.
    start = await typed(session, "What is the weather in Paris right now?")
    call = await session.wait(lambda e: e["type"] in ("function_call", "error"), 25, start)
    weather, called = "", call is not None and call[2]["type"] == "function_call"
    if called:
        await session.wait(is_done, 10, start)
        start = len(session.events)
        await session.send({"type": "function_result", "call_id": call[2]["call_id"],
                            "result": json.dumps({"city": "Paris", "forecast": "light rain, 14 C"})})
        await session.send({"type": "create_response"})
        await session.wait(is_done, 25, start)
        weather = words(session, start, "assistant")
    # A cancel once 1.5 s of a long answer has come.
    start, at = len(session.events), now()
    await typed(session, "Tell me a long story about a dragon and a lighthouse, at least twelve sentences.")
    deadline = now() + 25
    while now() < deadline and audio_s(session, at) < 1.5:
        await asyncio.sleep(0.05)
    cancelled_at = now()
    await session.send({"type": "cancel_response"})
    stopped = await session.wait(is_done, 5, start, after=cancelled_at)
    await asyncio.sleep(3.0)
    late_s = audio_s(session, cancelled_at + GK1_LATE_S)
    # New instructions mid-session (after the cancel's long answer): a one-word rule any model
    # keeps, unlike a switch to French (r10-verify: xAI answered "Blue." twice of four).
    start = len(session.events)
    await session.send({"type": "update_session", "instructions": GK1_RULE})
    updated = await session.wait(lambda e: e["type"] in ("session_updated", "error"), 10, start)
    await asyncio.sleep(1.0)
    start = await typed(session, "What colour is the sky on a clear day?")
    await session.wait(is_done, 25, start)
    ruled = words(session, start, "assistant")
    pitch = median_f0(b"".join(data for _, data in session.audio))
    errors = [e for _, e in session.events if e["type"] == "error"]
    await session.close()
    # Manual turns: nothing is answered before the commit.
    manual = await Native.open_native(ctx, "GK1-manual", port, {**config, "turn_detection": {"mode": "manual"}})
    await asyncio.sleep(1.5)
    start, at = len(manual.events), now()
    await manual.say("20_q_capital")
    await asyncio.sleep(2.0)
    early = [e["type"] for _, e in manual.events[start:] if e["type"] in ("response_done", "response_started")
             or (e["type"] == "transcript" and e.get("role") == "assistant")] + (
        ["audio"] if audio_s(manual, at) else [])
    committed = len(manual.events)
    await manual.send({"type": "commit_audio"})
    await asyncio.sleep(0.5)
    await manual.send({"type": "create_response"})
    answered = await manual.wait(is_done, 25, committed)
    manual_caller, manual_answer = words(manual, start, "user"), words(manual, committed, "assistant")
    errors += [e for _, e in manual.events if e["type"] == "error"]
    await manual.close()
    seconds = now() - opened
    checks = {
        "the caller's audio: one final caller line": "capital" in caller.lower() and caller.lower().count("capital") == 1,
        "answered in audio with its transcript and response_done": "paris" in answer.lower() and answer_s > 0.5
            and bool(spoken) and spoken[2]["type"] == "response_done",
        "typed text is the user's": bool(summed) and ("four" in four.lower() or "4" in four),
        "the client's function is called and its result answered": called
            and call[2].get("name") == "get_weather" and "paris" in call[2].get("arguments", "").lower()
            and any(w in weather.lower() for w in ("rain", "14", "fourteen")),
        "new instructions apply mid-session": bool(updated) and updated[2]["type"] == "session_updated"
            and re.findall(r"[a-z]+", ruled.lower()) == [GK1_RULE_WORD],
        "a cancel ends the response": bool(stopped) and stopped[2]["type"] == "response_done"
            and stopped[1] - cancelled_at <= GK1_DONE_S and late_s == 0,
        "the chosen voice speaks": 0 < pitch < GK1_MALE_HZ,
        "manual turns: nothing before the commit, then the answer": not early and bool(answered)
            and answered[2]["type"] == "response_done" and "paris" in manual_answer.lower(),
        "no error": not errors,
    }
    report(ctx, "GK1", "xAI Grok on /realtime with its default fixes", checks,
           f"caller {caller!r}, answer {answer!r} ({answer_s:.2f} s audio); typed {four!r}; call "
           f"{(call[2].get('name'), call[2].get('arguments')) if called else call}, then {weather!r}; after the "
           f"update {ruled!r}; cancel: response_done "
           f"{f'{stopped[1] - cancelled_at:.2f} s' if stopped else None} after it, audio later than "
           f"{GK1_LATE_S} s: {late_s:.2f} s; voice {GK1_VOICE} median pitch {pitch} Hz; manual: before the "
           f"commit {early or 'nothing'}, caller {manual_caller!r}, answer {manual_answer!r}; errors "
           f"{[(e.get('code'), e.get('message', '')[:80]) for e in errors]}; xAI sessions {seconds:.0f} s",
           f"one final caller line naming the capital and an answer with Paris and over 0.5 s of audio, then "
           f"response_done; 'four' for 2+2; get_weather called for Paris and the answer from its result; "
           f"session_updated and the reply '{GK1_RULE_WORD}' alone (any case, punctuation aside) after the one-word "
           f"rule; response_done within {GK1_DONE_S} s of "
           f"the cancel and no audio later than {GK1_LATE_S} s after it; median pitch under {GK1_MALE_HZ} Hz "
           f"(rex is male, xAI's default ara female: 214-235 Hz in the probe); manual: no response before the "
           f"commit, then an answer with Paris; no error",
           "no provider fix is listed on the gateway: grok_session, grok_events and grok_voice_default are on "
           "by default (probed 23 Oct). Spend: xAI bills US$0.08 per session minute")


# GK1: a response_done within 3 s of the cancel and no audio 1.5 s after it (probe: 0.29-0.33 s and
# 0.04-0.23 s, one run 1.1 s under load); a male voice's median pitch (probe: rex 103-115 Hz).
GK1_DONE_S = 3.0
GK1_LATE_S = 1.5
GK1_MALE_HZ = 160.0

SCENARIOS.update({"GK1": gk1})
DEFAULT["request"] += ["GK1"]


# ---- r18-fix (26 Oct): an ElevenLabs agent on /realtime (EA1; EL1 is ElevenLabs' TTS endpoint)

# The agent `waav-gateway-live-test` (made by the scratch tool el_create_agent.py: overrides for
# prompt, first_message, language and tts voice_id allowed, pcm_16000 both ways, a female
# default voice). The session overrides its prompt with a one-word reply rule and its voice
# with a male premade voice (Adam), so the answer's words and pitch show both overrides took.
EA1_VOICE = "pNInz6obpgDQGcFmaJgB"
EA1_RULE_WORD = "banana"
EA1_PROMPT = (f"You are a test line. Reply to every message with exactly one word, {EA1_RULE_WORD}, "
              "and nothing else.")
EA1_MALE_HZ = 160.0
EA1_SESSION_S = 20.0


class ElevenLabsNative(Native):
    """A native `/realtime` client of an ElevenLabs agent: `pcm_16000` both ways, so the
    caller's 16 kHz clips go as they are."""

    async def _send_audio(self, frame: bytes):
        await self.ws.send(frame)


async def ea1(ctx: Ctx):
    """EA1: an ElevenLabs agent on /realtime (needs ELEVENLABS_API_KEY and ELEVENLABS_AGENT_ID in --elevenlabs-env): session_created, the agent's own first message, the caller's question answered in audio and words under the session's prompt and voice overrides, closed within about 20 s."""
    agent = env_file_value(ctx.args.elevenlabs_env, "ELEVENLABS_AGENT_ID")
    port = ctx.stack.ensure_elevenlabs_gateway() if agent else None
    if port is None:
        reason = ("ELEVENLABS_AGENT_ID is not in --elevenlabs-env: the stored key lacks convai_read/convai_write, so "
                  "no agent can be made or listed (make one with el_create_agent.py once the key has them)"
                  if not agent else "ELEVENLABS_API_KEY is not in the environment nor in --elevenlabs-env")
        ctx.add("EA1", "an ElevenLabs agent on /realtime", False, f"BLOCKED: {reason}",
                "needs an ElevenLabs key with ConvAI access and an agent id", skipped=True)
        return
    config = {"provider": "elevenlabs", "model": agent, "instructions": EA1_PROMPT, "voice": EA1_VOICE}
    opened = now()
    session = await ElevenLabsNative.open_native(ctx, "EA1", port, config)

    def said(role: str, start: int = 0, until: int = 10 ** 9) -> str:
        return " ".join(e["text"] for _, e in session.events[start:until] if e["type"] == "transcript"
                        and e.get("role") == role and e.get("is_final")).strip()

    try:
        # The agent's own first message, before the caller says anything.
        await session.wait(lambda e: e["type"] in ("response_done", "error"), 8)
        await asyncio.sleep(0.5)
        first, start, at = said("assistant"), len(session.events), now()
        await session.say("20_q_capital")
        done = await session.wait(lambda e: e["type"] in ("response_done", "error"), 10, start)
        await asyncio.sleep(0.5)
        caller, answer = said("user", start), said("assistant", start)
        answer_s = sum(len(data) for t, data in session.audio if t >= at) / 2 / CALLER_RATE
        pitch = median_f0(b"".join(data for t, data in session.audio if t >= at), CALLER_RATE)
        errors = [(e.get("code"), str(e.get("message", ""))[:120]) for _, e in session.events
                  if e["type"] == "error"]
    finally:
        # The agent id is the run's own: never written to the record, also when the scenario
        # stops early (closing records the session; the run's clean-up would record it as is).
        for index, (t, event) in enumerate(session.events):
            session.events[index] = (t, {k: ("<agent id>" if v == agent else v) for k, v in event.items()})
        await session.close()
    seconds = now() - opened
    checks = {
        "session_created": bool(session.ready.get("session_id")),
        "the agent's first message (its own, before the caller)": bool(first),
        "the caller's words": "capital" in caller.lower(),
        "the prompt override: the one-word rule": re.findall(r"[a-z]+", answer.lower()) == [EA1_RULE_WORD]
            and bool(done) and done[2]["type"] == "response_done",
        "the answer's audio": answer_s > 0.3,
        "the voice override: a male voice": 0 < pitch < EA1_MALE_HZ,
        "no error": not errors,
        f"closed within about {EA1_SESSION_S:.0f} s": seconds <= EA1_SESSION_S + 5,
    }
    report(ctx, "EA1", "an ElevenLabs agent on /realtime", checks,
           f"first message {first!r}; caller {caller!r}; answer {answer!r} ({answer_s:.2f} s audio, median pitch "
           f"{pitch} Hz); errors {errors}; session {seconds:.1f} s",
           f"session_created; the agent's first message; the caller's question heard; the answer '{EA1_RULE_WORD}' "
           f"alone with response_done (the prompt override, conversation_config_override.agent.prompt); over 0.3 s "
           f"of answer audio with a median pitch under {EA1_MALE_HZ:.0f} Hz (the voice override "
           "conversation_config_override.tts.voice_id, Adam, over the agent's female default); no error (an "
           f"override the agent does not allow is a client_error or a close); about {EA1_SESSION_S:.0f} s",
           "the gateway sends no first message or language override (RealtimeConfig has no first message; "
           "`language` is Deepgram Voice Agent's): the agent's own first message is checked. "
           "Fixes elevenlabs_server_events (response_done, client_error, the audio formats) and "
           "elevenlabs_voice_override are on by default")


SCENARIOS.update({"EA1": ea1})
DEFAULT["request"] += ["EA1"]

# PE1: the first line of the operator's prompt, which the stand-in Responses background says
# before its answer; the answer is a fact of the same prompt, which is the caller's to hear.
PE1_LINE = "You are the help desk for customer accounts."
PE1_ANSWER = "The store opens at nine in the morning."
PE1_PROMPT = f"{PE1_LINE}\n- {PE1_ANSWER}\n- Answer the caller in one short sentence."


async def pe1(ctx: Ctx):
    """PE1: a background sentence that repeats the system's own prompt (live P1, r15-verify: "You are the help desk for customer accounts.") is never spoken and is not part of the result; the answer after it, a fact the prompt gives, is."""
    stand_in = ctx.stack.ensure_backends()
    mark = ctx.stack.log_size_of("live-backend-gateway.log")
    session = await backend_session(ctx, "PE1", "store", system_prompt=PE1_PROMPT)
    await session.warm()
    start = len(session.events)
    await session.say("87_store_open")
    result = await session.wait(lambda e: e["type"] == "task_result", 40, start)
    await session.quiet(30, start)
    await session.close()
    relayed = [e["delta"] for _, e in session.transcripts(start, "relay") if e.get("delta")]
    withheld = [line for line in ctx.stack.log_since_of("live-backend-gateway.log", "withheld by the speech policy", mark)
                if "prompt_echo" in line]
    sent = [hit for hit in stand_in.of("responses", "create") if PE1_LINE in str(hit.get("body", {}).get("instructions"))]
    checks = {
        "the background was sent the prompt": bool(sent),
        "the answer is spoken": PE1_ANSWER in relayed,
        "the prompt's line is never spoken": not any("help desk" in line for line in relayed),
        "the result holds the answer alone": bool(result) and result[2].get("summary") == PE1_ANSWER,
        "the gateway withheld it as prompt_echo": len(withheld) >= 1,
    }
    report(ctx, "PE1", "a sentence repeating the background's prompt is not spoken", checks,
           f"relayed {relayed}; result {result[2].get('summary') if result else None!r}; prompt_echo log lines "
           f"{len(withheld)}; creates with the prompt {len(sent)}",
           f"the stand-in Responses background answers {PE1_LINE!r} then {PE1_ANSWER!r}; only the second is "
           "relayed and is the task_result summary; the backend gateway logs `interaction sentence withheld by the "
           "speech policy` with rule prompt_echo",
           "a stand-in: the local 3B said such a line in about 1 of 6 live P1 sessions (r15-verify), too seldom "
           "to check against the model")
    return session


SCENARIOS.update({"PE1": pe1})
DEFAULT["request"] += ["PE1"]
DEFAULT["streaming"] += ["PE1"]


def main():
    scratch = os.environ.get("WAAV_LIVE_SCRATCH", "/tmp/waav-interaction-live")
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--only", help="comma-separated scenario ids, for example L0,L2")
    parser.add_argument("--start-stack", action="store_true",
                        help="start the model servers and the gateway when they are not running")
    parser.add_argument("--allow-stop-front", action="store_true",
                        help="L8 may stop (and restart) a front model server this script did not start")
    parser.add_argument("--stt", choices=("request", "streaming"), default="request",
                        help="the sessions' speech-to-text: request-response (openai) or the local "
                             "streaming recogniser (deepgram wire, interim results)")
    parser.add_argument("--stt-vendor", choices=("local", "deepgram"), default="local",
                        help="with --stt streaming: the local Whisper stand-in (deepgram wire), or Deepgram "
                             "nova-3 streaming itself for the sessions of the main gateway (its key from "
                             "DEEPGRAM_API_KEY or --deepgram-env, given to that gateway only)")
    parser.add_argument("--stt-stand-in", choices=("whisper", "parakeet"), default="whisper",
                        help="with --stt streaming and the local vendor: the stand-in's recogniser, Whisper "
                             "small.en behind --audio-url, or Parakeet TDT 0.6B v3 in the stand-in (CPU, int8, "
                             "the onnx-asr package and the model in the Hugging Face cache)")
    parser.add_argument("--stt-port", type=int, default=8126)
    parser.add_argument("--tts-vendor", default="",
                        help="provider:model:voice for the sessions' speech (e.g. elevenlabs:eleven_flash_v2_5:<voice id>; "
                             "the vendor key comes from this process's environment)")
    parser.add_argument("--tts-delay-ms", type=int, default=0,
                        help="hold back every synthesis of a sentence by this long (a proxy in front of "
                             "--audio-url; the gateway must be started by this run). A1 needs it")
    parser.add_argument("--gateway-port", type=int, default=3401)
    parser.add_argument("--bud-half-port", type=int, default=3419,
                        help="HC2: a second Bud-mode gateway whose relay block asks for the half-cascade")
    parser.add_argument("--bud-gateway-port", type=int, default=3402,
                        help="the port of the Bud-mode gateway scenario B1 starts")
    parser.add_argument("--bud-redis-port", type=int, default=6399,
                        help="the loopback port of the Redis container scenario B1 starts")
    parser.add_argument("--bud-redis-image", default="redis:7-alpine")
    parser.add_argument("--backend-gateway-port", type=int, default=3405,
                        help="the gateway of RW, RA, RM and RR, whose catalog lists the stand-in backends")
    parser.add_argument("--regime-gateway-port", type=int, default=3406,
                        help="the gateway of RG1 and RG2, whose /realtime OpenAI provider is the "
                             "Bud-mode gateway's GA cascade")
    parser.add_argument("--fix-gateway-ports", default="3407,3408",
                        help="PF1's two gateways (provider fixes off, then on), whose speech-to-speech "
                             "vendors are a loopback stand-in")
    parser.add_argument("--grpc-agent-port", type=int, default=18460,
                        help="PR7: the gRPC A2A stand-in (a2a_grpc_standin.py)")
    parser.add_argument("--grpc-python", default=None,
                        help="PR7: an interpreter with grpcio (default $WAAV_LIVE_SCRATCH/tools/grpc-venv/bin/python)")
    parser.add_argument("--socket-front-gateway-port", type=int, default=3417,
                        help="FS2: a gateway that runs its front model on a private UNIX socket")
    parser.add_argument("--licence-front-gateway-port", type=int, default=3418,
                        help="FS3: a gateway whose front is an operator-run server that names LFM weights")
    parser.add_argument("--licence-proxy-port", type=int, default=18462,
                        help="FS3: that server (its /props, the rest from the run's front llama-server)")
    parser.add_argument("--task-gateway-ports", default="3403,3404",
                        help="the two gateway processes DT1 to DT3 start on one task Redis")
    parser.add_argument("--task-redis-port", type=int, default=16399,
                        help="loopback port of the task Redis container DT1 to DT3 start")
    parser.add_argument("--front-gateway-port", type=int, default=3409,
                        help="FS1: a gateway that runs its own front model's llama-server")
    parser.add_argument("--supervised-front-port", type=int, default=18751,
                        help="FS1: where that gateway's llama-server listens")
    parser.add_argument("--livekit-port", type=int, default=17880,
                        help="the loopback port of the LiveKit server scenario K1 starts (it also takes "
                             "the two ports after it); the gateway is started with LIVEKIT_URL on it")
    parser.add_argument("--livekit-image", default="livekit/livekit-server:v1.12.0",
                        help="the LiveKit server image. The gateway's room API client (livekit-api 0.4.9) "
                             "posts to /%%2Ftwirp%%2F..., which server 1.12.0 serves and 1.13.9 answers "
                             "with 404")
    parser.add_argument("--gateway-bin", default=str(REPO_GATEWAY / "target/debug/waav-gateway"))
    parser.add_argument("--gateway-log", help="the running gateway's log (default: <log-dir>/live-gateway.log)")
    parser.add_argument("--audio-url", default="http://127.0.0.1:8124")
    parser.add_argument("--piper", default="/home/bud/tools/piper/piper",
                        help="CL1: the Piper program that speaks the second voice")
    parser.add_argument("--orchestrator-port", type=int, default=18790,
                        help="MA1: the llama-server of the orchestrator model")
    parser.add_argument("--orchestrator-model",
                        default=os.environ.get("WAAV_ORCHESTRATOR_MODEL",
                                               "/home/bud/models/audit-2026-09-26/gpt-oss-20b-MXFP4.gguf"),
                        help="MA1: the orchestrator's GGUF file (read only; env WAAV_ORCHESTRATOR_MODEL)")
    parser.add_argument("--front-port", type=int, default=18741)
    parser.add_argument("--controls-gateway-port", type=int, default=3415,
                        help="CT2: a gateway whose operator block allows language switches")
    parser.add_argument("--human-gateway-port", type=int, default=3416,
                        help="CT4: a gateway whose operator block puts a caller through to a person")
    parser.add_argument("--grok-gateway-port", type=int, default=3420,
                        help="GK1: a gateway whose /realtime Grok provider holds the xAI key")
    parser.add_argument("--xai-env", default="",
                        help="GK1: a KEY=VALUE file with GROK_API_KEY, given to GK1's gateway only "
                             "(when the variable is not in this process's environment); without a key GK1 is "
                             "skipped")
    parser.add_argument("--elevenlabs-gateway-port", type=int, default=3421,
                        help="EA1: a gateway whose /realtime ElevenLabs provider holds the ElevenLabs key")
    parser.add_argument("--elevenlabs-env", default="",
                        help="EA1: a KEY=VALUE file with ELEVENLABS_API_KEY and ELEVENLABS_AGENT_ID (the agent "
                             "id is read here; the key is given to EA1's gateway only); without the agent id EA1 "
                             "is reported BLOCKED, neither passed nor failed")
    parser.add_argument("--deepgram-env", default="",
                        help="CT2: a KEY=VALUE file with DEEPGRAM_API_KEY, given to CT2's gateway only "
                             "(when the variable is not in this process's environment)")
    parser.add_argument("--background-port", type=int, default=18742)
    parser.add_argument("--llama-dir", default=os.environ.get(
        "LLAMA_DIR", "/home/bud/ditto/llamacpp-build/b11200-cuda/llama-b11200"))
    parser.add_argument("--front-model", default=f"{scratch}/models/Qwen3.5-2B-Q4_K_M.gguf",
                        help="the front model's GGUF file (Qwen3.5-2B, Apache-2.0: "
                             "huggingface.co/unsloth/Qwen3.5-2B-GGUF, Q4_K_M)")
    parser.add_argument("--background-model", default="/home/bud/models/gguf/qwen3b-q4_k_m.gguf")
    parser.add_argument("--background-alias", default="qwen3b-q4_k_m")
    parser.add_argument("--ort", default=os.environ.get(
        "ORT_DYLIB_PATH", f"{scratch}/ort/onnxruntime-linux-aarch64-1.22.0/lib/libonnxruntime.so"))
    parser.add_argument("--tool-secret", default="live-e2e-secret",
                        help="must equal the gateway's WAAV_INTERACTION_TOOL_SECRET")
    parser.add_argument("--audio-dir", default=f"{scratch}/audio/caller")
    parser.add_argument("--out-dir", default=f"{scratch}/audio/live")
    parser.add_argument("--log-dir", default=f"{scratch}/logs")
    args = parser.parse_args()
    args.scratch = scratch
    args.gateway_ws = f"ws://127.0.0.1:{args.gateway_port}/ws"
    sys.exit(asyncio.run(run(args)))


if __name__ == "__main__":
    main()
