"""Assemble the video's data.json: the scene schedule around the demo, narration placement and the
benchmark scene. usage: assemble.py --demo demo_data.json --bench bench.json --out data.json"""
import argparse, json, wave
from pathlib import Path

def dur(path):
    with wave.open(str(path)) as w:
        return w.getnframes() / w.getframerate()

p = argparse.ArgumentParser()
p.add_argument("--demo", required=True)
p.add_argument("--bench", required=True)
p.add_argument("--bench2", default=None, help="a second benchmark card, narrated by vo/pause.wav")
p.add_argument("--vo", default="vo")
p.add_argument("--out", required=True)
a = p.parse_args()
d = json.loads(Path(a.demo).read_text())
vo = Path(a.vo)
demo_len = d["demo_window"][1] - d["demo_window"][0]
t = 0.0
sched, narr = [], []
def scene(sid, length, **kw):
    global t
    sched.append({"id": sid, "start": round(t, 3), "end": round(t + length, 3), **kw}); t += length
scene("open", 7.0)
narr.append({"file": "problem.wav", "at": t + 0.6}); scene("problem", max(12.0, dur(vo / "problem.wav") + 3.0))
narr.append({"file": "thesis.wav", "at": t + 0.4}); scene("thesis", max(8.5, dur(vo / "thesis.wav") + 2.2))
scene("demo", demo_len, bg=False, fade=0.4)
narr.append({"file": "how.wav", "at": t + 0.6}); scene("how", dur(vo / "how.wav") + 2.0)
bench = json.loads(Path(a.bench).read_text())
if (vo / "bench.wav").exists():
    narr.append({"file": "bench.wav", "at": t + 0.6}); scene("bench", max(14.0, dur(vo / "bench.wav") + 3.0))
else:
    scene("bench", 14.0)
bench2 = json.loads(Path(a.bench2).read_text()) if a.bench2 else None
if bench2:
    narr.append({"file": "pause.wav", "at": t + 0.6}); scene("bench2", max(14.0, dur(vo / "pause.wav") + 3.0))
narr.append({"file": "benefits.wav", "at": t + 0.5}); scene("benefits", dur(vo / "benefits.wav") + 3.0)
scene("future", 6.0)
narr.append({"file": "end.wav", "at": t + 0.8}); scene("end", 8.0)
demo_scene = next(s for s in sched if s["id"] == "demo")
d.update({"scenes": sched, "narration": narr, "bench": bench, "bench2": bench2, "demo_window": [demo_scene["start"], demo_scene["end"]]})
Path(a.out).write_text(json.dumps(d, indent=1))
print("total", round(t, 1), "s; demo", demo_scene["start"], "-", demo_scene["end"])
