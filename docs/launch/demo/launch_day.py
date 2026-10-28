"""The launch demo: one real call with Bud WaaV interaction mode running three desktop agents.

The caller works with the agent like with a coworker: one request at a time, with conversation in
between. Claude Opus 5.5 (the orchestrator, on the gateway) starts a child task on a desk agent for
each request (browser, terminal, writer: desk_agents.py, each a Claude Opus 5.5 tool loop driving the
sandboxed desktop where the work is seen being done); the Qwen3.5-2B front model keeps talking with
the caller. The caller asks a question while the work runs, adds the next request when the last one
is under way, and approves the save.

The desktop's screen is recorded (ffmpeg x11grab in the sandbox container) for the whole call.
Outputs under --out: caller.wav agent.wav call.wav events.jsonl caller_lines.json screen.mp4 meta.json
"""
import argparse
import asyncio
import json
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import demo_run  # noqa: E402  (Call, synth, wav writing)

DESK = "bud-desk-demo"
ORCH_PROMPT = """You are Ava, an assistant on a voice call who gets real work done on the caller's computer.
You have three agents: browser (operates a web browser), terminal (runs commands on a sandboxed Linux machine with internet access), writer (drafts documents and saves them in the word processor after the caller approves).
- Start an agent for each thing the caller asks for, at once, in the same round; never start work the caller has not asked for yet. Begin each agent request with a short title in plain words with no digits, ending with a colon (for example "Check the GitHub release:"), then the details. Give each agent every detail it needs (exact URLs, product names, versions, what to check). The caller's product is Bud WaaV, an open-source real-time voice gateway; its repository is https://github.com/BudEcosystem/WaaV and release 1.1.0 is tagged 1.1.0 with Linux binaries waav-gateway-1.1.0-linux-x86_64.tar.gz and waav-gateway-1.1.0-linux-aarch64.tar.gz plus SHA256SUMS.
- For download numbers, the terminal agent can query the GitHub API with curl and jq (https://api.github.com/repos/BudEcosystem/WaaV/releases/tags/1.1.0 lists each asset with its download_count).
- When asked to write an announcement, give the writer every fact found earlier in the call (the release's status and files, the download numbers); a later correction from the caller goes to the writer.
- Facts you know: Bud WaaV supports forty-four speech vendors (thirty-one speech-to-text, thirty-four text-to-speech, eleven speech-to-speech) and any LLM; it is open source under Apache 2.0 and written in Rust; release one point one adds interaction mode.
- Answer general questions yourself in one or two short spoken sentences.
- Everything you and the agents write as an answer is read aloud: write numbers and versions as people say them ("version one point one", "forty-four"), no URLs, no file names, no markdown."""


CALLER_CACHE = {}


def synth_eleven(text: str, voice: str) -> "demo_run.np.ndarray":
    """The caller's line in an ElevenLabs voice, through the gateway's own /speak. Lines are
    synthesised before the call (prepare_lines): a synthesis during the call would block the event
    loop and the microphone would then send its audio in a burst instead of in real time."""
    if (text, voice) in CALLER_CACHE:
        return CALLER_CACHE[(text, voice)]
    import json as _j
    from urllib.request import Request, urlopen
    np = demo_run.np
    body = _j.dumps({"text": text, "tts_config": {"provider": "elevenlabs", "model": "eleven_multilingual_v2",
                                                  "voice_id": voice, "audio_format": "linear16",
                                                  "sample_rate": demo_run.CALLER_RATE}}).encode()
    req = Request(f"http://{demo_run.GW}/speak", data=body, headers={"content-type": "application/json"})
    pcm = np.frombuffer(urlopen(req, timeout=60).read(), dtype="<i2")
    loud = np.where(np.abs(pcm) > 300)[0]
    pcm = pcm[max(0, loud[0] - 800): loud[-1] + 1600] if len(loud) else pcm
    CALLER_CACHE[(text, voice)] = pcm
    return pcm


LINES = {
    "open": "Morning, Ava! Can you pull up the GitHub release for WaaV one point one and check it all looks right?",
    "chat": "Thanks. Remind me, how many speech vendors do we support now?",
    "numbers": "Great. Can you also get the download numbers in the terminal?",
    "status": "How's it looking?",
    "draft": "Nice. Can you draft the launch announcement now, with those numbers in it?",
    "yes": "Yes, go ahead.",
    "keep": "Keep going, thanks.",
    "second": "It's a new one.",
    "thanks": "Perfect, that's everything. Thanks, Ava!",
}


LINES_DIR = Path(__file__).parent / "caller_lines"


def inner_gaps(pcm, rate, skip_s=0.0):
    """Pauses of 200 ms or more inside a line (after its first `skip_s` seconds), in ms."""
    np = demo_run.np
    a, fr = pcm.astype(float), int(rate * 0.02)
    loud = np.array([np.sqrt(np.mean(a[i:i + fr] ** 2)) > 350 for i in range(0, len(a) - fr, fr)])
    if not loud.any():
        return []
    first, last = int(np.argmax(loud)), len(loud) - int(np.argmax(loud[::-1]))
    gaps, i = [], max(first, int(skip_s / 0.02))
    while i < last:
        if loud[i]:
            i += 1
            continue
        j = i
        while j < last and not loud[j]:
            j += 1
        if (j - i) * 20 >= 200:
            gaps.append((j - i) * 20)
        i = j
    return gaps


# A line may pause after its lead-in ("Great.", "Thanks.", "Nice.") but not inside the request: a pause
# there ends the caller's turn and splits the request in two.
LEAD_IN_S = {"chat": 0.8, "numbers": 0.7, "draft": 0.8, "thanks": 1.4, "open": 1.0}


def prepare_lines(voice: str):
    """Each line once, from disk when kept: a take uses the same caller audio as the last one. A new
    synthesis is kept only without a pause inside the request (best of a few tries)."""
    np = demo_run.np
    LINES_DIR.mkdir(exist_ok=True)
    for label, text in LINES.items():
        f = LINES_DIR / f"{label}.npy"
        if f.exists() and (LINES_DIR / f"{label}.txt").read_text() == text:
            CALLER_CACHE[(text, voice)] = np.load(f)
            continue
        best = None
        for _ in range(4):
            CALLER_CACHE.pop((text, voice), None)
            pcm = synth_eleven(text, voice)
            gaps = inner_gaps(pcm, demo_run.CALLER_RATE, LEAD_IN_S.get(label, 0.0))
            if best is None or max(gaps, default=0) < best[0]:
                best = (max(gaps, default=0), pcm)
            if not gaps:
                break
        CALLER_CACHE[(text, voice)] = best[1]
        np.save(f, best[1])
        (LINES_DIR / f"{label}.txt").write_text(text)
        print(f"caller line {label}: longest inner pause {best[0]} ms", flush=True)


class LaunchCall(demo_run.Call):
    async def open(self):
        cfg = {
            "type": "config", "stream_id": f"launch-day-{int(time.time())}",
            "stt_config": {"provider": "deepgram", "model": "nova-3", "language": "en", "sample_rate": demo_run.CALLER_RATE,
                           "channels": 1, "punctuation": True, "encoding": "linear16",
                           "features": {"keyterms": ["WaaV", "Bud WaaV", "GitHub"]}},
            "tts_config": {"provider": "deepgram", "model": self.args.agent_voice, "voice_id": self.args.agent_voice,
                           "audio_format": "linear16", "sample_rate": demo_run.AGENT_RATE, "features": {"streaming": True}},
            "conversation_config": {"base_url": "https://api.anthropic.com/v1", "model": "claude-opus-5-5",
                                    "provider_kind": "anthropic", "system_prompt": ORCH_PROMPT, "max_tokens": 4096},
            "interaction": {"front": {"profile": "model"},
                            "tasks": {"orchestrator": "claude-opus-5-5", "agents": ["browser", "terminal", "writer"],
                                      "announce": "tell"},
                            # a cloud planner over Wi-Fi: Claude plans for a few seconds before its first token,
                            # which the 2.5 s default reads as a stalled service
                            "failure": {"stall_after_ms": 8000}},
        }
        import websockets
        self.ws = await websockets.connect(f"ws://{demo_run.GW}/ws", max_size=None, ping_interval=None)
        self.t0 = time.monotonic()
        self.t0_wall = time.time()
        await self.ws.send(json.dumps(cfg))
        self.reader = asyncio.create_task(self._read())
        self.micloop = asyncio.create_task(self._mic())
        ready = await self.wait(lambda e: e["type"] in ("ready", "error"), 60)
        warns = [e for _, e in self.events if e["type"] == "config_warning"]
        if ready is None or ready[1]["type"] != "ready" or "interaction" not in ready[1]:
            raise RuntimeError(f"no interaction session: {ready} {warns}")
        if warns:
            print("warnings:", warns, flush=True)

    async def say_eleven(self, label, text, voice):
        audio = synth_eleven(text, voice)
        while self.mic:
            await asyncio.sleep(0.005)
        start = self.t()
        self.mic.extend(audio.tobytes())
        dur = len(audio) / demo_run.CALLER_RATE
        self.lines.append({"label": label, "text": text, "start": round(start, 3), "end": round(start + dur, 3)})
        print(f"{start:6.2f}s caller: {text}", flush=True)
        await asyncio.sleep(dur)
        return start + dur


async def responder(call: LaunchCall, voice: str):
    """The caller answers what the agent asks once it has been heard in full: a confirmation with a
    yes, an offer to stop a long task with "keep going". A question that was cut off is asked again."""
    seen = 0
    pending = {}  # utterance_id -> kind
    confirms = set()  # tasks with an open confirmation
    while True:
        while seen < len(call.events):
            at, e = call.events[seen]
            seen += 1
            if e["type"] == "task_question" and e.get("kind") == "confirm":
                confirms.add(e["task_id"])
            elif e["type"] in ("task_answer_received", "task_result", "task_cancelled"):
                confirms.discard(e.get("task_id"))
            if e["type"] != "assistant_transcript":
                continue
            uid, kind = e.get("utterance_id"), e.get("kind")
            if e.get("delta") and kind == "question" and "second one" in e["delta"]:
                pending[uid] = "second"
                call.question_open = True
            elif e.get("delta") and (kind == "choice_offer" or (kind == "question" and "want me to" in e["delta"])):
                pending[uid] = kind
                call.question_open = True
            elif not e.get("delta") and uid in pending:
                kind = pending.pop(uid)
                if e.get("heard") == "full":
                    # as a person would: answer once the agent has stopped talking
                    await asyncio.sleep(0.45)
                    await call.quiet(hold=0.4, timeout=8)
                    if kind == "second":
                        await call.say_eleven("second", LINES["second"], voice)
                    elif kind == "question" and confirms:
                        await call.say_eleven("yes", LINES["yes"], voice)
                    else:
                        await call.say_eleven("keep", LINES["keep"], voice)
                call.question_open = bool(pending)
        await asyncio.sleep(0.03)


def requests_done(call: LaunchCall) -> bool:
    tops = {e["task_id"] for _, e in call.events if e["type"] == "task_created" and not e.get("parent_id")}
    done = {e["task_id"] for _, e in call.events if e["type"] in ("task_result", "task_cancelled") and e["task_id"] in tops}
    return bool(tops) and tops <= done


async def free_floor(call: LaunchCall, hold=0.9, timeout=30):
    """Wait until nothing is being said and no question is waiting for the caller."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        await call.quiet(hold=hold, timeout=max(1, end - time.monotonic()))
        if not call.question_open and not call.mic:
            return
        await asyncio.sleep(0.2)


def results_of(call: LaunchCall, agent: str):
    return [at for at, e in call.events if e["type"] == "task_result" and e.get("agent") == agent]


def spoken_since(call: LaunchCall, at: float, kinds=("update", "answer", "relay")):
    return [e for t, e in call.events if t > at and e["type"] == "assistant_transcript" and e.get("delta")
            and e.get("kind") in kinds]


async def until(pred, timeout):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        await asyncio.sleep(0.3)
    return False


async def say_when_free(call: LaunchCall, label: str, voice: str, hold=1.0, timeout=60):
    await free_floor(call, hold=hold, timeout=timeout)
    await call.say_eleven(label, LINES[label], voice)


async def scene(call: LaunchCall, voice: str):
    """A coworker's morning: one request at a time, conversation in between."""
    call.question_open = False
    answer = asyncio.create_task(responder(call, voice))
    # keep the front model's server awake (an idle server answers its first request slowly)
    import urllib.request
    await asyncio.get_running_loop().run_in_executor(None, lambda: urllib.request.urlopen(urllib.request.Request(
        "http://127.0.0.1:19741/completion", data=b'{"prompt":"Hi","n_predict":1}',
        headers={"content-type": "application/json"}), timeout=15).read())
    await asyncio.sleep(1.0)
    # 1. the release, in the browser
    await call.say_eleven("open", LINES["open"], voice)
    await call.wait(lambda e: e["type"] == "task_created" and e.get("agent") == "browser", 60)
    # 2. a question while it works
    await asyncio.sleep(2.0)
    asked = call.t()
    await say_when_free(call, "chat", voice, hold=1.2)
    await until(lambda: spoken_since(call, asked + 1.0), 30)
    # 3. the next request, while the browser is still at it
    await asyncio.sleep(1.5)
    await say_when_free(call, "numbers", voice, hold=1.2)
    await call.wait(lambda e: e["type"] == "task_created" and e.get("agent") == "terminal", 60)
    # 4. both results told (or ask how it is going once)
    asked_status = False
    end = time.monotonic() + 240
    while time.monotonic() < end:
        b, t = results_of(call, "browser"), results_of(call, "terminal")
        if b and t and spoken_since(call, max(b[0], t[0]), kinds=("update", "answer")):
            break
        if not asked_status and time.monotonic() > end - 200 and not (b and t):
            await asyncio.sleep(10.0)
            if not (results_of(call, "browser") and results_of(call, "terminal")):
                await say_when_free(call, "status", voice, hold=1.5)
            asked_status = True
        await asyncio.sleep(0.5)
    # 5. the announcement, with what was found; the responder says yes to the save
    await asyncio.sleep(1.0)
    await say_when_free(call, "draft", voice, hold=2.0)
    await until(lambda: results_of(call, "writer"), 300)
    w = results_of(call, "writer")
    if w:
        await until(lambda: spoken_since(call, w[0], kinds=("update", "answer")), 30)
    await asyncio.sleep(1.0)
    await say_when_free(call, "thanks", voice, hold=2.0, timeout=60)
    await call.quiet(hold=3.0, timeout=30)
    answer.cancel()


async def main():
    p = argparse.ArgumentParser()
    p.add_argument("--out", required=True)
    p.add_argument("--agent-voice", default="aura-2-juno-en")
    p.add_argument("--caller-voice", default="JBFqnCBsd6RMkjVDRZzb")
    p.add_argument("--warm", type=float, default=12.0)
    args = p.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    prepare_lines(args.caller_voice)  # before the call: no synthesis while the microphone streams
    call = LaunchCall(args, demo_run.Tools(0.0))
    await call.open()
    await asyncio.sleep(args.warm)  # warm-up lines, as a deployed agent is warm before the call
    # start the screen recording, then the call's timeline
    subprocess.run(["docker", "exec", DESK, "rm", "-f", "/tmp/screen.mp4"])
    rec = subprocess.Popen(["docker", "exec", DESK, "ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab",
                            "-video_size", "1920x1080", "-framerate", "30", "-i", ":1", "-c:v", "libx264",
                            "-preset", "ultrafast", "-crf", "18", "-pix_fmt", "yuv420p", "/tmp/screen.mp4"],
                           stdin=subprocess.PIPE)
    screen_start_wall = time.time()
    call.frames.clear()
    shift = call.t()
    shift_wall = call.t0_wall + shift
    try:
        await scene(call, args.caller_voice)
    finally:
        subprocess.run(["docker", "exec", DESK, "pkill", "-INT", "ffmpeg"])
        rec.wait(timeout=30)
        call.micloop.cancel()
        await call.ws.close()
        call.reader.cancel()
    subprocess.run(["docker", "cp", f"{DESK}:/tmp/screen.mp4", str(out / "screen.mp4")])
    demo_run.write_outputs(call, out, shift)
    (out / "meta.json").write_text(json.dumps({"timeline_start_wall": shift_wall, "screen_start_wall": screen_start_wall},
                                              indent=1))
    print("wrote", out)


if __name__ == "__main__":
    asyncio.run(main())
