#!/usr/bin/env python3
"""FRD-022 end-to-end on the audio plane: two WaaV replicas sharing one Redis, Bud voice
deployments published into `voice_table:*` the way budapp publishes them, and two controllable
OpenAI-compatible mock vendors (the `self_hosted` vendor) standing in for real ones.

Checks (bud-runtime specs/022 TEST_CASES ids):
  rate limit across replicas, 429 JSON + headers, live change      [TC-LF-*, TC-RG-03]
  max_concurrent across replicas                                   [TC-WR-10]
  retry on 503, no retry / no fallback on 400                      [TC-WR-01, TC-WR-02]
  fallback on 401, served-by headers                               [TC-WR-03]
  vendor 429 surfaced as 429 + Retry-After                         [TC-WR-07]
  429 Retry-After opens the deployment breaker -> fallback         [TC-WR-14, TC-WR-04]
  one tenant's bad key leaves another tenant alone                 [TC-WR-09]
  vendor outage across deployments opens the vendor tier           [TC-WR-13]
  fallback held back by its own limit -> 429 smallest Retry-After  [TC-WR-12]
  transcription retry + fallback                                   [FRD 6.3/6.4]
  realtime sessions release their connection slot                  [TC-RG-05, TC-RG-06]

Stdlib only. Usage:
  WAAV_BIN=/path/to/waav-gateway WAAV_REDIS_URL=redis://127.0.0.1:6379 \
      python3 gateway/scripts/deployment_policy_e2e.py
"""

from __future__ import annotations

import base64
import concurrent.futures
import hashlib
import http.client
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

WAAV_BIN = os.environ.get("WAAV_BIN", "waav-gateway")
# Run each replica in this image (binary mounted over /app/waav-gateway, host network), so the
# image's ONNX runtime and assets are what the binary starts with. Unset: run WAAV_BIN directly.
WAAV_IMAGE = os.environ.get("WAAV_DOCKER_IMAGE")
REDIS_URL = os.environ.get("WAAV_REDIS_URL", "redis://127.0.0.1:6379")
PORTS = [3201, 3202]
VENDOR_PORTS = {"a": 3401, "b": 3402}
KEY = "bud-e2e-frd022-key"
BAD_KEY = "bud-e2e-not-a-key"
FAKE_MP3 = b"ID3\x03\x00\x00\x00\x00\x00\x00" + b"\xff\xfb\x90\x64" + bytes(400)

FAILED: list[str] = []


def check(cond: bool, what: str) -> None:
    print(("  ok   " if cond else "  FAIL ") + what)
    if not cond:
        FAILED.append(what)


# ---------------------------------------------------------------- tiny RESP client
class Redis:
    def __init__(self, url: str):
        u = urllib.parse.urlparse(url)
        self.addr = (u.hostname or "127.0.0.1", u.port or 6379)

    def cmd(self, *args):
        with socket.create_connection(self.addr, timeout=5) as s:
            f = s.makefile("rwb")
            f.write(f"*{len(args)}\r\n".encode())
            for x in args:
                b = x if isinstance(x, bytes) else str(x).encode()
                f.write(b"$%d\r\n%s\r\n" % (len(b), b))
            f.flush()
            return self._read(f)

    def _read(self, f):
        line = f.readline().rstrip(b"\r\n")
        t, rest = line[:1], line[1:]
        if t == b"+":
            return rest.decode()
        if t == b"-":
            raise RuntimeError(rest.decode())
        if t == b":":
            return int(rest)
        if t == b"$":
            n = int(rest)
            return None if n < 0 else f.read(n + 2)[:-2].decode()
        if t == b"*":
            n = int(rest)
            return None if n < 0 else [self._read(f) for _ in range(n)]
        raise RuntimeError(f"bad reply {line!r}")


# ---------------------------------------------------------------- mock vendors
class Vendor:
    """An OpenAI-compatible speech/transcription server whose next answers are scripted."""

    def __init__(self, port: int):
        self.port = port
        self.lock = threading.Lock()
        self.script: dict[str, list[dict]] = {"speech": [], "transcriptions": []}
        self.calls = {"speech": 0, "transcriptions": 0}
        self.delay = 0.0
        vendor = self

        class H(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *a):  # quiet
                pass

            def do_POST(self):
                n = int(self.headers.get("content-length") or 0)
                self.rfile.read(n)
                kind = (
                    "speech"
                    if self.path.endswith("/audio/speech")
                    else "transcriptions"
                )
                with vendor.lock:
                    vendor.calls[kind] += 1
                    step = vendor.script[kind].pop(0) if vendor.script[kind] else None
                    delay = vendor.delay
                if delay:
                    time.sleep(delay)
                status = (step or {}).get("status", 200)
                if status == 200:
                    if kind == "speech":
                        body, ctype = FAKE_MP3, "audio/mpeg"
                    else:
                        body, ctype = (
                            json.dumps({"text": f"hello from {vendor.port}"}).encode(),
                            "application/json",
                        )
                else:
                    body = json.dumps(
                        {
                            "error": {
                                "message": (step or {}).get(
                                    "message", f"scripted {status}"
                                )
                            }
                        }
                    ).encode()
                    ctype = "application/json"
                self.send_response(status)
                self.send_header("content-type", ctype)
                self.send_header("content-length", str(len(body)))
                if (step or {}).get("retry_after") is not None:
                    self.send_header("retry-after", str(step["retry_after"]))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", port), H)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def queue(self, kind: str, *steps: dict):
        with self.lock:
            self.script[kind] = list(steps)

    def reset(self):
        with self.lock:
            self.script = {"speech": [], "transcriptions": []}
            self.calls = {"speech": 0, "transcriptions": 0}
            self.delay = 0.0

    def count(self, kind: str) -> int:
        with self.lock:
            return self.calls[kind]


# ---------------------------------------------------------------- helpers
def voice_entry(vendor: str, *, endpoints=("text_to_speech",), **policy) -> str:
    entry = {
        "vendor": "self_hosted",
        "api_base": f"http://127.0.0.1:{VENDOR_PORTS[vendor]}/v1",
        "endpoints": list(endpoints),
        "model": f"model-{vendor}",
        "voice": "alloy",
    }
    entry.update(policy)
    return entry


def limits(**w) -> dict:
    """The rate_limits block budapp publishes."""
    return {
        "algorithm": w.pop("algorithm", "fixed_window"),
        "requests_per_second": w.pop("rps", None),
        "requests_per_minute": w.pop("rpm", None),
        "requests_per_hour": w.pop("rph", None),
        "burst_size": None,
        "enabled": w.pop("enabled", True),
        "cache_ttl_ms": 500,
        "local_allowance": 0.8,
        "sync_interval_ms": 100,
        "redis_timeout_ms": 10,
    }


def publish(redis: Redis, endpoint_id: str, entry: dict) -> None:
    redis.cmd("SET", f"voice_table:{endpoint_id}", json.dumps({endpoint_id: entry}))


def speech(port: int, model: str, key: str = KEY):
    body = json.dumps(
        {"model": model, "input": "hello there", "response_format": "mp3"}
    ).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/audio/speech",
        data=body,
        headers={"content-type": "application/json", "authorization": f"Bearer {key}"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
    except urllib.error.HTTPError as e:
        return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read()


def transcription(port: int, model: str):
    boundary = "frd022boundary"
    wav = (
        b"RIFF"
        + (36 + 3200).to_bytes(4, "little")
        + b"WAVEfmt "
        + (16).to_bytes(4, "little")
    )
    wav += (
        (1).to_bytes(2, "little")
        + (1).to_bytes(2, "little")
        + (16000).to_bytes(4, "little")
    )
    wav += (
        (32000).to_bytes(4, "little")
        + (2).to_bytes(2, "little")
        + (16).to_bytes(2, "little")
    )
    wav += b"data" + (3200).to_bytes(4, "little") + bytes(3200)
    parts = [
        f'--{boundary}\r\nContent-Disposition: form-data; name="model"\r\n\r\n{model}\r\n'.encode(),
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="a.wav"\r\n'
        f"Content-Type: audio/wav\r\n\r\n".encode()
        + wav
        + b"\r\n",
        f"--{boundary}--\r\n".encode(),
    ]
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/audio/transcriptions",
        data=b"".join(parts),
        headers={
            "content-type": f"multipart/form-data; boundary={boundary}",
            "authorization": f"Bearer {KEY}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, {k.lower(): v for k, v in r.headers.items()}, r.read()
    except urllib.error.HTTPError as e:
        return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read()


def ws_upgrade(port: int, path: str, key: str) -> int:
    """Open a WebSocket upgrade, read the status, close. Returns the HTTP status."""
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    conn.putrequest("GET", path)
    conn.putheader("Upgrade", "websocket")
    conn.putheader("Connection", "Upgrade")
    conn.putheader("Sec-WebSocket-Version", "13")
    conn.putheader("Sec-WebSocket-Key", base64.b64encode(os.urandom(16)).decode())
    conn.putheader("Authorization", f"Bearer {key}")
    conn.endheaders()
    resp = conn.getresponse()
    status = resp.status
    conn.close()
    return status


def wait_port(port: int, path: str = "/", timeout: float = 90.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=2):
                return
        except urllib.error.HTTPError:
            return
        except Exception:
            time.sleep(0.5)
    raise RuntimeError(f"port {port} never came up")


# ---------------------------------------------------------------- scenario
def main() -> int:
    redis = Redis(REDIS_URL)
    redis.cmd("FLUSHDB")
    redis.cmd("CONFIG", "SET", "notify-keyspace-events", "KEA")

    vendors = {name: Vendor(port) for name, port in VENDOR_PORTS.items()}
    deployments = {
        "ep-rl": voice_entry("a", rate_limits=limits(rph=6)),
        "ep-cc": voice_entry("a", max_concurrent=2, rate_limits=limits()),
        "ep-retry": voice_entry(
            "a", retry_config={"num_retries": 2, "max_delay_s": 0.2}
        ),
        "ep-400": voice_entry("a", fallback_models=["ep-fb"]),
        "ep-401": voice_entry("a", fallback_models=["ep-fb"]),
        "ep-429": voice_entry("a", retry_config={"num_retries": 0, "max_delay_s": 1.0}),
        "ep-429fb": voice_entry("a", fallback_models=["ep-fb"]),
        "ep-fb": voice_entry("b"),
        "ep-fb-limited": voice_entry("b", rate_limits=limits(rph=1)),
        "ep-limited-chain": voice_entry("a", fallback_models=["ep-fb-limited"]),
        "ep-tenant-good": voice_entry("a"),
        "ep-v1": voice_entry("a"),
        "ep-v2": voice_entry("a"),
        "ep-v3": voice_entry("a", fallback_models=["ep-fb"]),
        "ep-stt": voice_entry(
            "a",
            endpoints=("audio_transcription",),
            retry_config={"num_retries": 1, "max_delay_s": 0.2},
            fallback_models=["ep-stt-fb"],
        ),
        "ep-stt-fb": voice_entry("b", endpoints=("audio_transcription",)),
    }
    for eid, entry in deployments.items():
        publish(redis, eid, entry)
    key_hash = hashlib.sha256(b"bud-" + KEY.encode()).hexdigest()
    aliases = {eid: {"endpoint_id": eid, "project_id": "p-e2e"} for eid in deployments}
    aliases["__metadata__"] = {
        "api_key_id": "ak-e2e",
        "user_id": "u-e2e",
        "api_key_project_id": "p-e2e",
    }
    redis.cmd("SET", f"api_key:{key_hash}", json.dumps(aliases))

    tmp = Path(tempfile.mkdtemp(prefix="frd022-waav-"))
    procs = []
    env = dict(
        os.environ,
        WAAV_REDIS_URL=REDIS_URL,
        AUTH_REQUIRED="true",
        WAAV_ALLOW_LOOPBACK_ENDPOINTS="1",
        RUST_LOG=os.environ.get("RUST_LOG", "warn"),
        RATE_LIMIT_REQUESTS_PER_SECOND="1000",
        RATE_LIMIT_BURST_SIZE="2000",
        MAX_CONNECTIONS_PER_IP="100",
    )
    try:
        for port in PORTS:
            replica_env = dict(env, PORT=str(port))
            if WAAV_IMAGE:
                keys = [
                    "WAAV_REDIS_URL",
                    "AUTH_REQUIRED",
                    "WAAV_ALLOW_LOOPBACK_ENDPOINTS",
                    "RUST_LOG",
                    "RATE_LIMIT_REQUESTS_PER_SECOND",
                    "RATE_LIMIT_BURST_SIZE",
                    "MAX_CONNECTIONS_PER_IP",
                    "PORT",
                ]
                cmd = ["docker", "run", "--rm", "--name", f"frd022-waav-{port}", "--network", "host"]
                cmd += ["-v", f"{WAAV_BIN}:/app/waav-gateway:ro"]
                for k in keys:
                    cmd += ["-e", f"{k}={replica_env[k]}"]
                cmd.append(WAAV_IMAGE)
            else:
                cmd = [WAAV_BIN]
            procs.append(
                subprocess.Popen(
                    cmd,
                    env=replica_env,
                    stdout=open(tmp / f"waav-{port}.log", "w"),
                    stderr=subprocess.STDOUT,
                )
            )
        for port in PORTS:
            wait_port(port, "/ready")
        time.sleep(2.5)  # limiter heartbeat: both replicas registered

        print("replica set")
        pods = redis.cmd("ZRANGE", "rl2:{waav}:pods", "0", "-1") or []
        check(len(pods) == 2, f"both replicas registered ({pods})")

        print("rate limit across replicas: 6/hour on 2 replicas, 14 requests")
        res = [speech(PORTS[i % 2], "ep-rl") for i in range(14)]
        ok = [r for r in res if r[0] == 200]
        denied = [r for r in res if r[0] == 429]
        check(len(ok) == 6, f"admitted {len(ok)} == 6 (cluster-wide, not 12)")
        if ok:
            check(
                ok[0][1].get("x-ratelimit-limit") == "6",
                f"X-RateLimit-Limit {ok[0][1].get('x-ratelimit-limit')}",
            )
            check(
                ok[0][1].get("x-bud-endpoint-id") == "ep-rl",
                "x-bud-endpoint-id on the served response",
            )
        if denied:
            h = denied[0][1]
            check(
                h.get("content-type", "").startswith("application/json"), "429 is JSON"
            )
            check(
                json.loads(denied[0][2])["error"]["code"] == "rate_limit_exceeded",
                "429 code",
            )
            check(
                int(h.get("retry-after", "0")) >= 1,
                f"Retry-After {h.get('retry-after')}",
            )

        print("live change: 6/hour -> 10/hour, no restart")
        publish(redis, "ep-rl", voice_entry("a", rate_limits=limits(rph=10)))
        time.sleep(1.5)
        res = [speech(PORTS[i % 2], "ep-rl") for i in range(14)]
        check(
            sum(1 for r in res if r[0] == 200) == 10,
            f"new limit: admitted {sum(1 for r in res if r[0] == 200)} == 10",
        )

        print("max_concurrent 2 across replicas (slow vendor)")
        vendors["a"].delay = 1.5
        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as ex:
            res = list(ex.map(lambda i: speech(PORTS[i % 2], "ep-cc"), range(6)))
        vendors["a"].delay = 0.0
        ok = sum(1 for r in res if r[0] == 200)
        conc = [r for r in res if r[0] == 429]
        check(ok <= 2 and ok >= 1, f"at most 2 concurrent served ({ok})")
        check(
            all(
                json.loads(r[2])["error"]["code"] == "concurrency_limit_exceeded"
                for r in conc
            )
            and len(conc) >= 4,
            f"the rest are 429 concurrency_limit_exceeded ({len(conc)})",
        )

        print("retry: 503, 503, then 200 with num_retries 2")
        vendors["a"].reset()
        vendors["a"].queue("speech", {"status": 503}, {"status": 503})
        status, h, _ = speech(PORTS[0], "ep-retry")
        check(status == 200, f"status {status}")
        check(
            vendors["a"].count("speech") == 3,
            f"3 vendor calls ({vendors['a'].count('speech')})",
        )

        print("caller error: 400 is not retried and not failed over")
        vendors["a"].reset()
        vendors["b"].reset()
        vendors["a"].queue("speech", {"status": 400, "message": "voice_not_found"})
        status, h, body = speech(PORTS[0], "ep-400")
        check(status == 400, f"status {status}")
        check(
            vendors["a"].count("speech") == 1 and vendors["b"].count("speech") == 0,
            "1 call, no fallback",
        )

        print("our credential: 401 fails over to the fallback deployment")
        vendors["a"].reset()
        vendors["b"].reset()
        vendors["a"].queue("speech", {"status": 401, "message": "invalid api key"})
        status, h, _ = speech(PORTS[0], "ep-401")
        check(status == 200, f"status {status}")
        check(h.get("x-bud-fallback") == "true", "x-bud-fallback: true")
        check(
            h.get("x-bud-endpoint-id") == "ep-fb",
            f"served by ep-fb ({h.get('x-bud-endpoint-id')})",
        )
        check("x-bud-voice-substituted" in h, "x-bud-voice-substituted present")

        print("vendor 429 without a fallback is a 429 with the vendor's Retry-After")
        vendors["a"].reset()
        vendors["a"].queue("speech", {"status": 429, "retry_after": 30})
        status, h, _ = speech(PORTS[0], "ep-429")
        check(status == 429, f"status {status} (was 502)")
        check(h.get("retry-after") == "30", f"Retry-After {h.get('retry-after')}")

        print(
            "vendor 429 with Retry-After opens the breaker for that long -> straight to fallback"
        )
        vendors["a"].reset()
        vendors["b"].reset()
        vendors["a"].queue("speech", {"status": 429, "retry_after": 20})
        status, h, _ = speech(PORTS[0], "ep-429fb")
        check(
            status == 200 and h.get("x-bud-fallback") == "true",
            f"first request falls back ({status})",
        )
        calls_before = vendors["a"].count("speech")
        status, h, _ = speech(PORTS[0], "ep-429fb")
        check(
            status == 200 and h.get("x-bud-endpoint-id") == "ep-fb",
            "second request served by the fallback",
        )
        check(
            vendors["a"].count("speech") == calls_before,
            "primary not attempted while its breaker is open",
        )

        print("tenant isolation: a bad key on one deployment leaves another alone")
        vendors["a"].reset()
        vendors["a"].queue("speech", *[{"status": 401}] * 8)
        for _ in range(8):
            speech(PORTS[0], "ep-401")
        vendors["a"].reset()
        status, _, _ = speech(PORTS[0], "ep-tenant-good")
        check(
            status == 200,
            f"other deployment on the same vendor still served ({status})",
        )

        print("vendor outage across deployments opens the vendor tier")
        vendors["a"].reset()
        vendors["b"].reset()
        vendors["a"].queue("speech", *[{"status": 503}] * 6)
        for i in range(6):
            speech(PORTS[0], "ep-v1" if i % 2 == 0 else "ep-v2")
        calls_before = vendors["a"].count("speech")
        status, h, _ = speech(PORTS[0], "ep-v3")
        check(status == 200 and h.get("x-bud-endpoint-id") == "ep-fb", f"third deployment failed over ({status})")
        check(vendors["a"].count("speech") == calls_before, "without an attempt on the failing vendor")

        print("fallback held back by its own limit")
        vendors["a"].reset()
        vendors["b"].reset()
        speech(
            PORTS[0], "ep-fb-limited"
        )  # spend ep-fb-limited's only request this hour
        vendors["a"].queue("speech", {"status": 503})
        status, h, body = speech(PORTS[0], "ep-limited-chain")
        check(status == 429, f"status {status} (every fallback held back by its own limit)")
        check(int(h.get("retry-after", "0")) >= 1, f"Retry-After {h.get('retry-after')}")
        check(
            vendors["b"].count("speech") == 1,
            "the limited fallback was not called again",
        )

        print("transcription: retry, then fallback")
        vendors["a"].reset()
        vendors["b"].reset()
        vendors["a"].queue("transcriptions", {"status": 503})
        status, h, body = transcription(PORTS[1], "ep-stt")
        check(status == 200, f"retried to success ({status}, {body[:120]!r})")
        check(
            vendors["a"].count("transcriptions") == 2,
            f"2 calls ({vendors['a'].count('transcriptions')})",
        )
        vendors["a"].queue("transcriptions", {"status": 503}, {"status": 503})
        status, h, body = transcription(PORTS[1], "ep-stt")
        check(
            status == 200 and h.get("x-bud-fallback") == "true", f"fell back ({status})"
        )
        check(b"3402" in body, "the fallback vendor answered")

        print("realtime sessions release their connection slot (per-IP cap 100)")
        statuses = [ws_upgrade(PORTS[0], "/v1/realtime", KEY) for _ in range(101)]
        check(
            statuses[-1] == 101 and statuses.count(101) == 101,
            f"101st realtime session admitted ({sorted(set(statuses))})",
        )
        rejected = [ws_upgrade(PORTS[0], "/ws", BAD_KEY) for _ in range(150)]
        check(
            all(s == 401 for s in rejected),
            f"bad key refused ({sorted(set(rejected))})",
        )
        check(
            ws_upgrade(PORTS[0], "/ws", KEY) == 101,
            "a valid /ws upgrade after 150 rejections",
        )
    finally:
        if WAAV_IMAGE:
            # SIGTERM to the gateway itself: graceful shutdown hands the limiter's reservations back.
            subprocess.run(
                ["docker", "stop", "-t", "20"] + [f"frd022-waav-{p}" for p in PORTS],
                capture_output=True,
            )
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(timeout=20)
            except subprocess.TimeoutExpired:
                p.kill()
        if FAILED:
            for f in sorted(tmp.glob("*.log")):
                print(f"--- {f.name} (tail)")
                print("".join(f.read_text(errors="replace").splitlines(True)[-30:]))
        for v in vendors.values():
            v.server.shutdown()

    pods = redis.cmd("ZRANGE", "rl2:{waav}:pods", "0", "-1") or []
    check(pods == [], f"replicas deregistered on shutdown ({pods})")
    print()
    if FAILED:
        print(f"{len(FAILED)} check(s) failed")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
