"""Turn a recorded take into the video's data: the demo overlay's timeline (from the real events),
the time-compression plan for the waits, the scene schedule, and the time-mapped screen/audio.

usage: build_data.py --take DIR --out data.json [--speed 4] [--min-gap 6]
"""
import argparse
import json
import subprocess
import wave
from pathlib import Path

import numpy as np

AGENTS = ("browser", "terminal", "writer")


def load(take: Path):
    events = [json.loads(l) for l in open(take / "events.jsonl")]
    lines = json.loads((take / "caller_lines.json").read_text())
    return events, lines


def timeline(events, lines):
    msgs, lanes, replies, running, floor = [], {a: [] for a in AGENTS}, [], [], []
    for l in lines:
        msgs.append({"t": l["start"], "who": "caller", "kind": "you", "text": l["text"]})
    started = {}
    kinds = {"ack": "acknowledges", "relay": "answer", "update": "update", "progress": "progress", "hold": "holding",
             "confirm": "confirm", "question": "confirm", "notice": "", "answer": "answer", "choice_offer": "asks",
             "control": "control", "social": ""}
    cur = {}
    for e in events:
        t, ty = e["t"], e["type"]
        if ty == "assistant_transcript" and e.get("delta"):
            uid = e.get("utterance_id")
            if uid in cur and cur[uid]["t"] > t - 6:
                cur[uid]["text"] += " " + e["delta"]
            else:
                m = {"t": t, "who": "agent", "kind": kinds.get(e.get("kind"), e.get("kind") or ""), "text": e["delta"]}
                msgs.append(m)
                cur[uid] = m
        elif ty in ("task_created", "task_progress", "task_question", "task_result", "task_cancelled") and e.get("agent") in AGENTS:
            a = e["agent"]
            state = {"task_created": "working", "task_progress": "working", "task_question": "asking",
                     "task_result": "done", "task_cancelled": "cancelled"}[ty]
            text = e.get("text") or e.get("title") or ""
            if ty == "task_result":
                text = e.get("summary") or "done"
            if ty == "task_question":
                text = "waiting for your yes"
            if ty == "task_progress" and not e.get("text"):
                continue
            lanes[a].append({"t": t, "state": state, "text": text})
        if ty == "task_created":
            started[e["task_id"]] = t
        if ty in ("task_result", "task_cancelled") and e["task_id"] in started:
            running.append({"from": started.pop(e["task_id"]), "to": t, "agent": e.get("agent")})
        if ty == "floor_state":
            floor.append({"t": t, "who": e.get("holder")})
    for tid, t0 in started.items():
        running.append({"from": t0, "to": 1e9})
    msgs.sort(key=lambda m: m["t"])
    return {"messages": msgs, "lanes": lanes, "replies": replies, "running": running, "floor": floor}


def first_audio_after(agent_pcm, rate, t0, t1):
    fr = int(rate * 0.02)
    i0, i1 = int(t0 * rate), min(len(agent_pcm), int(t1 * rate))
    for i in range(i0, i1 - fr, fr):
        if np.sqrt(np.mean(agent_pcm[i:i + fr].astype(float) ** 2)) > 300:
            return i / rate
    return None


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--take", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--speed", type=float, default=4.0)
    p.add_argument("--min-gap", type=float, default=7.0, help="quiet stretches longer than this are sped up")
    p.add_argument("--demo-start", type=float, default=24.0)
    a = p.parse_args()
    take = Path(a.take)
    events, lines = load(take)
    tl = timeline(events, lines)
    w = wave.open(str(take / "agent.wav"))
    rate = w.getframerate()
    agent = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    w = wave.open(str(take / "caller.wav"))
    caller = np.frombuffer(w.readframes(w.getnframes()), dtype="<i2")
    dur = len(agent) / rate
    # reply after you stop: first agent audio after each caller line's end (measured on the audio)
    fr20 = int(rate * 0.02)
    for l in lines:
        # the caller's last voiced 20 ms within the line
        i0, i1 = int(l["start"] * rate), min(len(caller), int(l["end"] * rate))
        voiced = [i for i in range(i0, i1 - fr20, fr20)
                  if np.sqrt(np.mean(caller[i:i + fr20].astype(float) ** 2)) > 300]
        if not voiced:
            continue
        vend = (voiced[-1] + fr20) / rate
        nxt = first_audio_after(agent, rate, vend, vend + 8)
        if nxt is not None and nxt - vend >= 0.1:  # an agent line already under way is not a reply
            tl["replies"].append({"t": nxt, "ms": round((nxt - vend) * 1000), "line": l["label"]})
    # quiet stretches (no speech on either side) longer than min-gap are sped up
    fr = int(rate * 0.1)
    loud = np.array([max(np.abs(agent[i:i + fr]).max(initial=0), np.abs(caller[i:i + fr]).max(initial=0)) > 600
                     for i in range(0, len(agent) - fr, fr)])
    segs, i, n = [], 0, len(loud)
    cursor = 0.0
    while i < n:
        if not loud[i]:
            j = i
            while j < n and not loud[j]:
                j += 1
            s, e = i * 0.1, j * 0.1
            if e - s >= a.min_gap:
                # keep 1.5 s of real time at each end of a long quiet
                if s + 1.5 > cursor:
                    segs.append({"call_start": cursor, "call_end": s + 1.5, "speed": 1})
                segs.append({"call_start": s + 1.5, "call_end": e - 1.5, "speed": a.speed})
                cursor = e - 1.5
            i = j
        else:
            i += 1
    if cursor < dur:
        segs.append({"call_start": cursor, "call_end": dur, "speed": 1})
    vdur = sum((s["call_end"] - s["call_start"]) / s["speed"] for s in segs)
    # zooms: the pane an agent is working in (desktop panes: Writer top-left, terminal bottom-left,
    # browser right), from the real task events
    PANES = {"browser": [962, 0, 958, 539], "writer": [0, 0, 958, 539], "terminal": [0, 541, 958, 539]}
    focus = []
    first_child = min((e["t"] for e in events if e["type"] == "task_created" and e.get("agent")), default=None)
    for e in events:
        a_ = e.get("agent")
        if a_ not in PANES:
            continue
        if e["type"] == "task_created" and a_ == "browser":
            focus.append({"from": e["t"] + 4.0, "to": e["t"] + 14.0, "crop": PANES[a_], "label": "Browser"})
        if e["type"] == "task_created" and a_ == "writer":
            # the draft is typed on screen: from the writer's start until its save question
            asked = min((q["t"] for q in events if q["type"] == "task_question" and q.get("agent") == "writer"
                         and q["t"] > e["t"]), default=e["t"] + 25.0)
            focus.append({"from": e["t"] + 3.0, "to": asked, "crop": PANES[a_], "label": "LibreOffice Writer"})
        if e["type"] == "task_result" and a_ == "writer":
            focus.append({"from": e["t"] - 1.0, "to": e["t"] + 5.0, "crop": PANES[a_], "label": "LibreOffice Writer"})
        if e["type"] == "task_result" and a_ == "terminal":
            focus.append({"from": e["t"] - 12.0, "to": e["t"] + 4.0, "crop": PANES[a_], "label": "Terminal"})
    # the terminal while it downloads, between the other zooms
    tstart = min((e["t"] for e in events if e["type"] == "task_created" and e.get("agent") == "terminal"), default=None)
    if tstart is not None:
        busy = sorted((f["from"], f["to"]) for f in focus)
        writer_end = max((f["to"] for f in focus if f["label"].startswith("LibreOffice")), default=tstart + 4.0)
        t = max(tstart + 4.0, writer_end)
        end = max((e["t"] for e in events if e["type"] == "task_result" and e.get("agent") == "terminal"), default=dur)
        for a0, b0 in busy + [(end, end)]:
            if a0 - t > 6.0:
                focus.append({"from": t, "to": a0, "crop": PANES["terminal"], "label": "Terminal"})
            t = max(t, b0)
    # no overlaps: an earlier zoom yields to a later one
    focus.sort(key=lambda f: f["from"])
    for f, g in zip(focus, focus[1:]):
        if f["to"] > g["from"]:
            f["to"] = g["from"]
    focus = [f for f in focus if f["to"] - f["from"] > 1.0]
    demo_start = a.demo_start
    demo_end = demo_start + vdur
    data = {"take": str(take), "call_duration": dur, "demo": {"segments": segs, "timeline": tl, "focus": focus},
            "demo_window": [demo_start, demo_end]}
    Path(a.out).write_text(json.dumps(data, indent=1))
    print(f"call {dur:.1f}s -> demo {vdur:.1f}s with {sum(1 for s in segs if s['speed'] > 1)} sped-up stretches; "
          f"replies {[r['ms'] for r in tl['replies']]}")


if __name__ == "__main__":
    main()
