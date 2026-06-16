#!/usr/bin/env python3
"""
WaaV S2S RECONNECT + REPLAY end-to-end validation (credential-free).

Validates the scaffold's resilience machinery (RealtimeSession<P> supervisor in
gateway/src/core/realtime/scaffold/session.rs) through the REAL gateway driver,
with NO vendor keys — by pointing <PROVIDER>_REALTIME_URL at a mock upstream that
DROPS mid-session and counting/capturing what the gateway re-sends on reconnect.

Two scenarios, each with its own gateway instance + mock on a spare port:

  SCENARIO 1 — RECONNECT + REPLAY (provider configurable: openai | deepgram)
    Mock: conn#1 = session-ready + 1 NORMAL turn (so the gateway logs a turn) +
          hold open >MIN_STABLE_CONNECTION(5s) (a STABLE conn) then CLOSE the WS.
          conn#2 (the reconnect) = CAPTURE all gateway->mock frames (must include
          the re-sent session config AND, for openai, replayed conversation.item
          .create items), then serve a FRESH turn on the next user audio.
    Asserts:
      (a) mock saw a 2nd connection (RECONNECTED) within the backoff window,
      (b) on reconnect the gateway RE-SENT the session config,
      (c) it REPLAYED the prior conversation turn(s) (openai: the item.create
          frames carrying the logged user+assistant text; deepgram: Settings
          re-send is the replay surface),
      (d) the session is USABLE after reconnect (a new turn round-trips: client
          gets agent audio + assistant transcript again post-drop).

  SCENARIO 2 — QUICK-FAILURE CUTOFF (bounded, no storm)
    Mock: DROP IMMEDIATELY on every connect (well within MIN_STABLE_CONNECTION).
    Asserts: the scaffold STOPS after MAX_CONSECUTIVE_QUICK_FAILURES(3) rather
    than storming — connection count is BOUNDED (~<=5, not dozens), and an ERROR
    surfaces to the client / the socket ends (no infinite loop).

Run:  python3 /tmp/waav_live/reconnect_replay_test.py [openai|deepgram|both]
"""

import asyncio
import json
import os
import signal
import socket
import subprocess
import sys
import tempfile
import time

import websockets

HERE = os.path.dirname(os.path.abspath(__file__))
# Gateway logs go to a temp dir, never into the repo (this script is committed).
LOGDIR = tempfile.gettempdir()
GATEWAY_DIR = "/home/bud/ditto/waav/WaaV/gateway"
GATEWAY_BIN = os.path.join(GATEWAY_DIR, "target/debug/waav-gateway")

# ── tunables ────────────────────────────────────────────────────────────────
# MIN_STABLE_CONNECTION in the scaffold is 5s; hold the 1st conn open a bit past
# that so the drop is a STABLE-connection drop (clean reconnect, quick_failures
# reset to 0) and NOT counted toward the quick-failure cutoff.
STABLE_HOLD_SECS = 6.5
# Default reconnect backoff: initial_delay 1000ms * 2^0 = ~1000ms (+<=25% jitter)
# for the FIRST reconnect attempt; allow generous slack for it to land.
RECONNECT_WAIT_SECS = 8.0

API_KEY_ENVS = {
    "openai": "OPENAI_API_KEY",
    "deepgram": "DEEPGRAM_API_KEY",
}
URL_ENVS = {
    "openai": "OPENAI_REALTIME_URL",
    "deepgram": "DEEPGRAM_REALTIME_URL",
}
MODELS = {
    "openai": "gpt-realtime",
    "deepgram": "",
}
VOICES = {
    "openai": "alloy",
    "deepgram": None,
}
SAMPLE_RATE = {
    "openai": 24000,
    "deepgram": 24000,
}

SILENCE_PCM = bytes(32000)
import base64
SILENCE_B64 = base64.b64encode(SILENCE_PCM).decode("ascii")
USER_TEXT = "hello mock turn one"
ASSISTANT_TEXT = "Mock heard you turn one."


# =============================================================================
# Mock upstream servers (GA / deepgram wire), with connection counting + a
# reconnect-drop behaviour and a capture of the 2nd-connection frames.
# =============================================================================

def ga_session_ready():
    return [json.dumps({
        "type": "session.created",
        "session": {"id": "sess_mock_123", "object": "realtime.session",
                    "model": "mock-realtime", "expires_at": 0,
                    "modalities": ["audio", "text"]},
    })]


def ga_turn(user_text, asst_text):
    return [
        json.dumps({"type": "conversation.item.input_audio_transcription.completed",
                    "item_id": "item_user_1", "content_index": 0,
                    "transcript": user_text}),
        json.dumps({"type": "response.output_audio_transcript.done",
                    "response_id": "resp_1", "item_id": "item_asst_1",
                    "output_index": 0, "content_index": 0, "transcript": asst_text}),
        json.dumps({"type": "response.output_audio.delta",
                    "response_id": "resp_1", "item_id": "item_asst_1",
                    "output_index": 0, "content_index": 0, "delta": SILENCE_B64}),
        json.dumps({"type": "response.done",
                    "response": {"id": "resp_1", "object": "realtime.response",
                                 "status": "completed"}}),
    ]


def dg_session_ready():
    return [json.dumps({"type": "Welcome", "request_id": "req_mock_123"}),
            json.dumps({"type": "SettingsApplied"})]


def dg_turn(user_text, asst_text):
    return [
        json.dumps({"type": "ConversationText", "role": "user", "content": user_text}),
        json.dumps({"type": "ConversationText", "role": "assistant", "content": asst_text}),
        SILENCE_PCM,
        json.dumps({"type": "AgentAudioDone"}),
    ]


def is_user_audio(provider, msg):
    if isinstance(msg, (bytes, bytearray)):
        # deepgram sends RAW binary user audio.
        return provider == "deepgram"
    try:
        o = json.loads(msg)
    except (json.JSONDecodeError, ValueError):
        return False
    if not isinstance(o, dict):
        return False
    if provider == "openai":
        return o.get("type") == "input_audio_buffer.append" and "audio" in o
    return False


def classify_frame(provider, msg):
    """Tag a gateway->mock frame for the reconnect-capture assertion."""
    if isinstance(msg, (bytes, bytearray)):
        return ("binary", len(msg))
    try:
        o = json.loads(msg)
    except (json.JSONDecodeError, ValueError):
        return ("nonjson", msg[:40])
    if not isinstance(o, dict):
        return ("nondict", None)
    if provider == "openai":
        t = o.get("type")
        if t == "session.update":
            return ("session_config", t)
        if t == "conversation.item.create":
            item = o.get("item", {})
            role = item.get("role")
            content = item.get("content") or []
            text = None
            if content and isinstance(content, list):
                text = content[0].get("text")
            return ("replay_item", {"role": role, "text": text})
        if t == "input_audio_buffer.append":
            return ("user_audio", None)
        return ("other", t)
    if provider == "deepgram":
        t = o.get("type")
        if t == "Settings":
            return ("session_config", t)
        if t == "KeepAlive":
            return ("keepalive", t)
        return ("other", t)
    return ("other", None)


class ReconnectMock:
    """Counts connections; conn#1 fires a normal turn then drops after a stable
    hold; conn#2+ CAPTURE the gateway's re-sent frames and serve a fresh turn."""

    def __init__(self, provider):
        self.provider = provider
        self.conn_count = 0
        self.captured_conn2 = []        # classified frames seen on conn#2 BEFORE
                                        # the post-reconnect user audio.
        self.conn2_roundtrip_fired = False
        self._lock = asyncio.Lock()

    def session_ready(self):
        return ga_session_ready() if self.provider == "openai" else dg_session_ready()

    def turn(self, u, a):
        return ga_turn(u, a) if self.provider == "openai" else dg_turn(u, a)

    async def handler(self, ws):
        async with self._lock:
            self.conn_count += 1
            my_conn = self.conn_count
        print(f"  [mock:{self.provider}] CONNECTION #{my_conn}", flush=True)

        for f in self.session_ready():
            await ws.send(f)

        fired = {"turn": False}
        post_reconnect_audio_seen = {"v": False}

        async def fire_turn(u, a):
            if fired["turn"]:
                return
            fired["turn"] = True
            for f in self.turn(u, a):
                await ws.send(f)

        try:
            if my_conn == 1:
                # conn#1: fire ONE turn on first user audio, hold open past
                # MIN_STABLE_CONNECTION, then CLOSE to trigger a reconnect.
                async def conn1_loop():
                    async for msg in ws:
                        if is_user_audio(self.provider, msg):
                            await fire_turn(USER_TEXT, ASSISTANT_TEXT)
                try:
                    await asyncio.wait_for(conn1_loop(), timeout=STABLE_HOLD_SECS)
                except asyncio.TimeoutError:
                    pass
                print(f"  [mock:{self.provider}] conn#1 DROPPING (stable hold "
                      f"{STABLE_HOLD_SECS}s elapsed)", flush=True)
                await ws.close(code=1011, reason="mock drop mid-session")
                return

            # conn#2+ (the reconnect): CAPTURE every frame the gateway sends
            # until we see the post-reconnect user audio, then serve a fresh turn.
            async for msg in ws:
                if is_user_audio(self.provider, msg):
                    post_reconnect_audio_seen["v"] = True
                    self.conn2_roundtrip_fired = True
                    await fire_turn("hello mock turn two",
                                    "Mock heard you turn two.")
                    continue
                if not post_reconnect_audio_seen["v"]:
                    self.captured_conn2.append(classify_frame(self.provider, msg))
        except websockets.ConnectionClosed:
            pass


class QuickFailMock:
    """Drops IMMEDIATELY on every connect (within MIN_STABLE_CONNECTION) to drive
    the quick-failure cutoff. Counts connections to prove boundedness."""

    def __init__(self, provider):
        self.provider = provider
        self.conn_count = 0
        self.first_conn_at = None
        self.last_conn_at = None
        self._lock = asyncio.Lock()

    async def handler(self, ws):
        async with self._lock:
            self.conn_count += 1
            n = self.conn_count
            now = time.monotonic()
            if self.first_conn_at is None:
                self.first_conn_at = now
            self.last_conn_at = now
        print(f"  [quickfail:{self.provider}] CONNECTION #{n} -> immediate drop",
              flush=True)
        # Send NOTHING and close instantly (well within 5s stable window). The
        # handshake succeeded (TCP+WS upgrade) so this is the "connected then
        # immediate close" quick-failure the scaffold guards against.
        await ws.close(code=1011, reason="instant drop")


def select_subprotocol(connection, subprotocols):
    if not subprotocols:
        return None
    if "realtime" in subprotocols:
        return "realtime"
    return subprotocols[0]


async def serve_mock(handler, host, port):
    return await websockets.serve(handler, host, port, max_size=None,
                                  select_subprotocol=select_subprotocol)


# =============================================================================
# Gateway lifecycle
# =============================================================================

def wait_port(host, port, timeout=20.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with socket.create_connection((host, port), timeout=0.5):
                return True
        except OSError:
            time.sleep(0.2)
    return False


def wait_livez(port, timeout=90.0):
    import urllib.request
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/livez",
                                        timeout=1.0) as r:
                if r.status == 200:
                    return True
        except Exception:  # noqa: BLE001
            time.sleep(0.3)
    return False


def gateway_env(provider, gw_port, mock_port):
    env = os.environ.copy()
    env["PORT"] = str(gw_port)
    env["HOST"] = "127.0.0.1"
    env["AUTH_REQUIRED"] = "false"
    env["RATE_LIMIT_REQUESTS_PER_SECOND"] = "1000"
    env["RATE_LIMIT_BURST_SIZE"] = "1000"
    env["MAX_CONNECTIONS_PER_IP"] = "1000"
    env["CUDA_HOME"] = "/tmp/nocuda"
    env["ORT_DYLIB_PATH"] = "/home/bud/.local/ortlib/libonnxruntime.so"
    env[URL_ENVS[provider]] = f"ws://127.0.0.1:{mock_port}/upstream"
    env[API_KEY_ENVS[provider]] = f"dummy-{provider}-key"
    return env


def start_gateway(provider, gw_port, mock_port, logname):
    log = open(os.path.join(LOGDIR, logname), "w")
    p = subprocess.Popen([GATEWAY_BIN], cwd=GATEWAY_DIR,
                         env=gateway_env(provider, gw_port, mock_port),
                         stdout=log, stderr=subprocess.STDOUT)
    if not wait_port("127.0.0.1", gw_port, timeout=90.0):
        print(f"[FATAL] gateway did not open port {gw_port}", flush=True)
    elif not wait_livez(gw_port, timeout=90.0):
        print("[WARN] gateway /livez did not return 200 in time", flush=True)
    return p


def base_config(provider):
    cfg = {
        "type": "config",
        "provider": provider,
        "transcribe_input": True,
        "turn_detection": {"mode": "server_vad", "threshold": 0.5,
                           "silence_duration_ms": 500, "prefix_padding_ms": 300},
    }
    if MODELS[provider]:
        cfg["model"] = MODELS[provider]
    if VOICES[provider]:
        cfg["voice"] = VOICES[provider]
    return cfg


async def drain(ws, seconds, sink):
    """Collect client<-gateway messages for `seconds`, recording into sink dict."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        rem = deadline - time.time()
        if rem <= 0:
            break
        try:
            msg = await asyncio.wait_for(ws.recv(), timeout=rem)
        except asyncio.TimeoutError:
            break
        except websockets.ConnectionClosed:
            sink["closed"] = True
            break
        if isinstance(msg, (bytes, bytearray)):
            sink["audio_bytes"] += len(msg)
            sink["events"].append(f"AUDIO:{len(msg)}B")
            continue
        try:
            o = json.loads(msg)
        except (json.JSONDecodeError, ValueError):
            continue
        t = o.get("type")
        sink["events"].append(t)
        if t == "session_created":
            sink["session_created"] = True
        elif t == "transcript" and o.get("role") == "assistant":
            sink["asst_transcript"] = True
            sink["asst_text"] = o.get("text") or o.get("content")
        elif t == "transcript" and o.get("role") == "user":
            sink["user_transcript"] = True
        elif t == "error":
            sink["errors"].append(o.get("message", "?"))


def new_sink():
    return {"session_created": False, "asst_transcript": False, "asst_text": None,
            "user_transcript": False, "audio_bytes": 0, "errors": [],
            "events": [], "closed": False}


# =============================================================================
# SCENARIO 1 — reconnect + replay
# =============================================================================

async def scenario_reconnect_replay(provider, gw_port, mock_port):
    print(f"\n========== SCENARIO 1: RECONNECT+REPLAY [{provider}] ==========",
          flush=True)
    mock = ReconnectMock(provider)
    server = await serve_mock(mock.handler, "127.0.0.1", mock_port)
    gw = start_gateway(provider, gw_port, mock_port,
                       f"gw_reconnect_{provider}.log")
    time.sleep(1.5)

    gw_ws = f"ws://127.0.0.1:{gw_port}/realtime"
    sr = SAMPLE_RATE[provider]
    frame = bytes(int(sr * 0.1) * 2)

    result = {
        "reconnected": False,
        "conn_count_final": 0,
        "resent_config": False,
        "replayed_items": [],
        "replay_observed": False,
        "post_reconnect_roundtrip": False,
        "pre_drop_roundtrip": False,
        "reconnect_latency_s": None,
        "errors": [],
        "captured_conn2": [],
    }

    try:
        async with websockets.connect(gw_ws, max_size=None, open_timeout=10) as ws:
            await ws.send(json.dumps(base_config(provider)))

            # ── TURN 1 (pre-drop): drive a turn so the gateway logs a conv item.
            pre = new_sink()
            await asyncio.sleep(1.2)  # let session_created + upstream connect land
            for _ in range(6):
                try:
                    await ws.send(frame)
                except websockets.ConnectionClosed:
                    break
                await asyncio.sleep(0.12)
            await drain(ws, 4.0, pre)
            result["pre_drop_roundtrip"] = pre["asst_transcript"] and pre["audio_bytes"] > 0
            print(f"  pre-drop turn: asst_transcript={pre['asst_transcript']} "
                  f"audio={pre['audio_bytes']}B sess={pre['session_created']} "
                  f"events={pre['events'][:8]}", flush=True)

            # ── Wait for the mock's stable-hold to elapse + the DROP, then for
            #    the scaffold to RECONNECT (mock conn#2). The mock drops at
            #    ~STABLE_HOLD_SECS from conn#1; reconnect lands ~1s after.
            drop_wait_start = time.monotonic()
            # Keep the client socket alive + drain anything during the gap.
            gap = new_sink()
            # Wait until the mock reports >=2 connections (reconnected) or timeout.
            reconnect_deadline = time.time() + STABLE_HOLD_SECS + RECONNECT_WAIT_SECS
            while time.time() < reconnect_deadline:
                if mock.conn_count >= 2:
                    result["reconnect_latency_s"] = round(
                        time.monotonic() - drop_wait_start, 2)
                    break
                # opportunistically drain (non-blocking-ish) to keep ws healthy
                try:
                    msg = await asyncio.wait_for(ws.recv(), timeout=0.25)
                    if isinstance(msg, (bytes, bytearray)):
                        gap["audio_bytes"] += len(msg)
                    else:
                        try:
                            o = json.loads(msg)
                            gap["events"].append(o.get("type"))
                            if o.get("type") == "error":
                                gap["errors"].append(o.get("message", "?"))
                        except (json.JSONDecodeError, ValueError):
                            pass
                except asyncio.TimeoutError:
                    pass
                except websockets.ConnectionClosed:
                    gap["closed"] = True
                    break

            result["reconnected"] = mock.conn_count >= 2
            result["conn_count_final"] = mock.conn_count
            result["errors"] = gap["errors"]
            print(f"  after drop: mock conn_count={mock.conn_count} "
                  f"reconnected={result['reconnected']} "
                  f"latency={result['reconnect_latency_s']}s "
                  f"client_closed={gap['closed']} gap_events={gap['events'][:6]}",
                  flush=True)

            # Give the gateway a beat to finish re-sending config + replay on conn#2.
            await asyncio.sleep(0.8)
            result["captured_conn2"] = list(mock.captured_conn2)
            tags = [c[0] for c in mock.captured_conn2]
            result["resent_config"] = "session_config" in tags
            replayed = [c[1] for c in mock.captured_conn2 if c[0] == "replay_item"]
            result["replayed_items"] = replayed
            # "replay observed": openai => >=1 item.create; deepgram => the
            # Settings re-send IS the replay surface (no per-item API).
            if provider == "openai":
                result["replay_observed"] = len(replayed) >= 1
            else:
                result["replay_observed"] = result["resent_config"]
            print(f"  conn#2 capture: resent_config={result['resent_config']} "
                  f"replayed_items={replayed}", flush=True)
            print(f"  conn#2 raw frames: {mock.captured_conn2}", flush=True)

            # ── TURN 2 (post-reconnect): a NEW turn must round-trip (usability).
            if result["reconnected"]:
                post = new_sink()
                for _ in range(6):
                    try:
                        await ws.send(frame)
                    except websockets.ConnectionClosed:
                        break
                    await asyncio.sleep(0.12)
                await drain(ws, 4.0, post)
                result["post_reconnect_roundtrip"] = (
                    post["asst_transcript"] and post["audio_bytes"] > 0)
                print(f"  post-reconnect turn: asst_transcript="
                      f"{post['asst_transcript']} audio={post['audio_bytes']}B "
                      f"text={post['asst_text']!r} events={post['events'][:8]}",
                      flush=True)
    except Exception as e:  # noqa: BLE001
        result["errors"].append(f"client-exc: {type(e).__name__}: {e}")
        print(f"  [client-exc] {type(e).__name__}: {e}", flush=True)
    finally:
        gw.send_signal(signal.SIGINT)
        try:
            gw.wait(timeout=8)
        except subprocess.TimeoutExpired:
            gw.kill()
        server.close()
        await server.wait_closed()

    return result


# =============================================================================
# SCENARIO 2 — quick-failure cutoff
# =============================================================================

async def scenario_quick_failure(provider, gw_port, mock_port):
    print(f"\n========== SCENARIO 2: QUICK-FAILURE CUTOFF [{provider}] ==========",
          flush=True)
    mock = QuickFailMock(provider)
    server = await serve_mock(mock.handler, "127.0.0.1", mock_port)
    gw = start_gateway(provider, gw_port, mock_port,
                       f"gw_quickfail_{provider}.log")
    time.sleep(1.5)

    gw_ws = f"ws://127.0.0.1:{gw_port}/realtime"
    sr = SAMPLE_RATE[provider]
    frame = bytes(int(sr * 0.1) * 2)
    result = {
        "conn_count": 0,
        "bounded": False,
        "error_surfaced": False,        # did the gateway PROACTIVELY push an error
                                        # to the client when the cutoff fired?
        "client_closed": False,         # did the gateway PROACTIVELY close the WS?
        "post_cutoff_send_error": False,  # mitigation: a client send AFTER the
                                        # cutoff surfaces a "Failed to send" error
                                        # (is_ready()==false path).
        "errors": [],
        "events": [],
        "span_secs": None,
        "conn_count_at_cutoff_window": None,
    }

    try:
        async with websockets.connect(gw_ws, max_size=None, open_timeout=10) as ws:
            await ws.send(json.dumps(base_config(provider)))
            sink = new_sink()
            # The cutoff: 3 quick failures. The FIRST connect is the initial dial
            # (handshake succeeds → session_created fires), then it drops within
            # MIN_STABLE_CONNECTION → quick_failures climbs 1,2,3 across the
            # initial + 2 reconnect dials (~1s,2s backoff) → "giving up". Observe
            # ~8s: long enough to hit the cutoff AND confirm no 4th/5th dial
            # (a storm bug would show DOZENS climbing without bound).
            await drain(ws, 8.0, sink)
            result["conn_count_at_cutoff_window"] = mock.conn_count
            # PROBE the post-cutoff mitigation: after the supervisor gave up,
            # is_ready() should be false, so a client audio send should surface a
            # "Failed to send audio" error rather than be silently swallowed.
            try:
                await ws.send(frame)
            except websockets.ConnectionClosed:
                sink["closed"] = True
            # Drain a bit more to catch any post-cutoff error/close.
            await drain(ws, 4.0, sink)
            result["client_closed"] = sink["closed"]
            errs = sink["errors"]
            result["errors"] = errs[:5]
            result["events"] = sink["events"][:14]
            # An error that arrived BEFORE the post-cutoff probe is a proactive
            # surface; "Failed to send" is the mitigation surface.
            result["post_cutoff_send_error"] = any(
                "Failed to send" in e or "send audio" in e for e in errs)
            result["error_surfaced"] = any(
                "Failed to send" not in e and "send audio" not in e for e in errs)
    except Exception as e:  # noqa: BLE001
        result["errors"].append(f"client-exc: {type(e).__name__}: {e}")
    finally:
        # Let the mock settle, then snapshot the count.
        await asyncio.sleep(0.5)
        result["conn_count"] = mock.conn_count
        if mock.first_conn_at and mock.last_conn_at:
            result["span_secs"] = round(mock.last_conn_at - mock.first_conn_at, 2)
        gw.send_signal(signal.SIGINT)
        try:
            gw.wait(timeout=8)
        except subprocess.TimeoutExpired:
            gw.kill()
        server.close()
        await server.wait_closed()

    # Bounded := total connects small. Cutoff is 3 quick failures; the very first
    # connect is the initial (non-reconnect) dial, so total connects should be
    # ~1 initial + up to 3 reconnect attempts. We assert <=6 as a generous
    # storm-guard ceiling; a storming bug would show DOZENS.
    result["bounded"] = 0 < result["conn_count"] <= 6
    # Authoritative "supervisor gave up" signal: the scaffold logs this exactly
    # once when MAX_CONSECUTIVE_QUICK_FAILURES is hit. Its presence proves the
    # storm was stopped by the cutoff (not merely by the observation window).
    try:
        with open(os.path.join(LOGDIR, f"gw_quickfail_{provider}.log")) as f:
            log = f.read()
        result["supervisor_gave_up"] = "too many quick failures; giving up" in log
    except OSError:
        result["supervisor_gave_up"] = None
    print(f"  quickfail: conn_count={result['conn_count']} "
          f"bounded={result['bounded']} span={result['span_secs']}s "
          f"supervisor_gave_up={result['supervisor_gave_up']} "
          f"proactive_error={result['error_surfaced']} "
          f"client_closed={result['client_closed']} "
          f"post_cutoff_send_error={result['post_cutoff_send_error']} "
          f"errors={result['errors']} events={result['events']}", flush=True)
    return result


# =============================================================================
# Main
# =============================================================================

async def amain():
    which = sys.argv[1] if len(sys.argv) > 1 else "openai"
    providers = ["openai", "deepgram"] if which == "both" else [which]

    # Distinct gw/mock ports per provider to avoid any collision with a running
    # gateway on :3009 or :3019.
    ports = {
        "openai":   {"gw": 3031, "rmock": 19031, "qmock": 19032},
        "deepgram": {"gw": 3033, "rmock": 19033, "qmock": 19034},
    }

    report = {}
    for prov in providers:
        p = ports[prov]
        r1 = await scenario_reconnect_replay(prov, p["gw"], p["rmock"])
        r2 = await scenario_quick_failure(prov, p["gw"], p["qmock"])
        report[prov] = {"reconnect_replay": r1, "quick_failure": r2}

    # ── verdicts ──
    print("\n\n################## FINAL VERDICT ##################", flush=True)
    overall_ok = True
    for prov, r in report.items():
        rr = r["reconnect_replay"]
        qf = r["quick_failure"]
        a = rr["reconnected"]
        b = rr["resent_config"]
        c = rr["replay_observed"]
        d = rr["post_reconnect_roundtrip"]
        # REQUIRED safety property: bounded AND the supervisor actively gave up
        # (storm stopped by the cutoff, not the window).
        e = qf["bounded"] and bool(qf.get("supervisor_gave_up"))
        # SEPARATE sub-property (client UX, not safety): is the dead session
        # surfaced to the client at all — proactively (error/close) OR via the
        # post-cutoff send-error mitigation?
        e2_client_surface = (qf["error_surfaced"] or qf["client_closed"]
                             or qf["post_cutoff_send_error"])
        passed = a and b and c and d and e
        overall_ok = overall_ok and passed
        print(f"\n=== {prov.upper()} ===", flush=True)
        print(f"  (a) RECONNECTED (conn>=2 after drop)      : "
              f"{a}  [conn_count={rr['conn_count_final']}, "
              f"latency={rr['reconnect_latency_s']}s]", flush=True)
        print(f"  (b) RE-SENT session config on reconnect   : {b}", flush=True)
        print(f"  (c) REPLAYED prior conversation turn(s)   : {c}  "
              f"[items={rr['replayed_items']}]", flush=True)
        print(f"  (d) USABLE after reconnect (new turn RT)  : {d}", flush=True)
        print(f"  (e) QUICK-FAILURE CUTOFF bounded (no storm): {e}  "
              f"[conn_count={qf['conn_count']}, bounded={qf['bounded']}, "
              f"supervisor_gave_up={qf.get('supervisor_gave_up')}, "
              f"span={qf['span_secs']}s]", flush=True)
        print(f"      ↳ client-surface on cutoff (UX)       : "
              f"{e2_client_surface}  [proactive_error="
              f"{qf['error_surfaced']}, proactive_close={qf['client_closed']}, "
              f"post_cutoff_send_error={qf['post_cutoff_send_error']}]", flush=True)
        print(f"  pre-drop turn round-tripped (sanity)      : "
              f"{rr['pre_drop_roundtrip']}", flush=True)
        print(f"  >>> {prov}: {'PASS' if passed else 'FAIL'}", flush=True)

    print(f"\n################## {'ALL PASS' if overall_ok else 'FAILURES PRESENT'} "
          f"##################", flush=True)
    # machine-readable dump for the report
    print("\nJSON_REPORT_BEGIN")
    print(json.dumps(report, indent=2, default=str))
    print("JSON_REPORT_END")
    return 0 if overall_ok else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(amain()))
