#!/usr/bin/env python3
"""
Regression guard for the QUICK-FAILURE-CUTOFF client-surface fix (commit 185f0cb),
run END-TO-END through the full waav-gateway binary, credential-free.

THE BUG (found by adversarial bug-hunt wf_fb932e8d, now fixed): after the realtime
scaffold hit the quick-failure cutoff (dead session, is_ready()==false), a client
that KEPT STREAMING audio had every frame SILENTLY DROPPED, and because the handler
reset its idle timer on EVERY received frame the 5-min idle timeout NEVER fired ->
the dead session was held open INDEFINITELY with zero feedback (empirically: 110
frames / 28s, no error, no close).

THE FIX: the /realtime handler registers on_reconnection; on a terminal give-up
(success==false) it forwards a `connection_lost` error + closes the client WS.

This probe points OPENAI_REALTIME_URL at a mock that instantly drops every dial
(forcing the cutoff), then streams audio and asserts the client IS surfaced an
error and/or close. Exits 0 on PASS, non-zero if the indefinite-hold bug regresses.
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
GW_PORT = 3037
MOCK_PORT = 19037
HOLD_SECS = 22.0  # well past the 3 quick-failure dials (~1s+2s+4s); a session
                  # that's going to error/close on its own would have by now.


async def quickfail_handler(ws):
    # Accept the upgrade, send nothing, close instantly (quick failure).
    await ws.close(code=1011, reason="instant drop")


def select_subprotocol(connection, subprotocols):
    if not subprotocols:
        return None
    return "realtime" if "realtime" in subprotocols else subprotocols[0]


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


def start_gateway():
    env = os.environ.copy()
    env.update({
        "PORT": str(GW_PORT), "HOST": "127.0.0.1", "AUTH_REQUIRED": "false",
        "RATE_LIMIT_REQUESTS_PER_SECOND": "1000", "RATE_LIMIT_BURST_SIZE": "1000",
        "MAX_CONNECTIONS_PER_IP": "1000", "CUDA_HOME": "/tmp/nocuda",
        "ORT_DYLIB_PATH": "/home/bud/.local/ortlib/libonnxruntime.so",
        "OPENAI_REALTIME_URL": f"ws://127.0.0.1:{MOCK_PORT}/upstream",
        "OPENAI_API_KEY": "dummy-openai-key",
    })
    log = open(os.path.join(LOGDIR, "gw_quickfail_hold.log"), "w")
    p = subprocess.Popen([GATEWAY_BIN], cwd=GATEWAY_DIR, env=env,
                         stdout=log, stderr=subprocess.STDOUT)
    wait_port("127.0.0.1", GW_PORT, timeout=90.0)
    wait_livez(GW_PORT, timeout=90.0)
    return p


async def amain():
    server = await websockets.serve(quickfail_handler, "127.0.0.1", MOCK_PORT,
                                    max_size=None,
                                    select_subprotocol=select_subprotocol)
    gw = start_gateway()
    time.sleep(1.5)

    errors = []
    closed = False
    audio_frames_sent = 0
    frame = bytes(int(24000 * 0.1) * 2)
    t0 = time.monotonic()
    try:
        async with websockets.connect(f"ws://127.0.0.1:{GW_PORT}/realtime",
                                      max_size=None, open_timeout=10) as ws:
            await ws.send(json.dumps({"type": "config", "provider": "openai",
                                      "model": "gpt-realtime", "voice": "alloy",
                                      "transcribe_input": True}))
            # Let the 3 quick failures burn down (~1s+2s backoff + dials).
            await asyncio.sleep(6.0)

            async def streamer():
                nonlocal audio_frames_sent
                while True:
                    try:
                        await ws.send(frame)
                        audio_frames_sent += 1
                    except websockets.ConnectionClosed:
                        return
                    await asyncio.sleep(0.2)

            st = asyncio.create_task(streamer())
            deadline = time.time() + HOLD_SECS
            while time.time() < deadline:
                try:
                    msg = await asyncio.wait_for(ws.recv(),
                                                 timeout=deadline - time.time())
                except asyncio.TimeoutError:
                    break
                except websockets.ConnectionClosed:
                    closed = True
                    break
                if isinstance(msg, (bytes, bytearray)):
                    continue
                try:
                    o = json.loads(msg)
                    if o.get("type") == "error":
                        errors.append(o.get("message", "?"))
                except (json.JSONDecodeError, ValueError):
                    pass
            st.cancel()
            try:
                await st
            except (asyncio.CancelledError, Exception):
                pass
    except Exception as e:  # noqa: BLE001
        errors.append(f"client-exc: {type(e).__name__}: {e}")
    finally:
        elapsed = round(time.monotonic() - t0, 1)
        gw.send_signal(signal.SIGINT)
        try:
            gw.wait(timeout=8)
        except subprocess.TimeoutExpired:
            gw.kill()
        server.close()
        await server.wait_closed()

    # Confirm the cutoff actually fired (authoritative log line).
    gave_up = False
    try:
        with open(os.path.join(LOGDIR, "gw_quickfail_hold.log")) as f:
            gave_up = "too many quick failures; giving up" in f.read()
    except OSError:
        pass

    # Regression guard (handler on_reconnection fix, commit 185f0cb): once the
    # supervisor gives up, the client MUST be told (error and/or close) — never
    # left streaming into a silently-dead session. PASS = surfaced; FAIL = the
    # old indefinite-hold bug regressed.
    surfaced = bool(errors) or closed
    fixed = gave_up and surfaced
    regressed = gave_up and not surfaced
    print(json.dumps({
        "supervisor_gave_up": gave_up,
        "held_open_secs_while_streaming": elapsed,
        "audio_frames_streamed_post_cutoff": audio_frames_sent,
        "client_got_error": len(errors) > 0,
        "errors": errors[:5],
        "gateway_closed_socket": closed,
        "VERDICT": (
            "PASS: dead session surfaced to client (error/close)" if fixed
            else "FAIL/REGRESSION: DEAD SESSION HELD OPEN (no error, no close) "
                 "while client streamed" if regressed
            else "INCONCLUSIVE: cutoff never fired (check gw_quickfail_hold.log)"
        ),
    }, indent=2))
    # Exit non-zero on regression OR an inconclusive run so CI/ops catches it.
    return 0 if fixed else 1


if __name__ == "__main__":
    sys.exit(asyncio.run(amain()))
