"""Cut a take's screen recording and call audio into the demo's time-mapped media.

Segments at speed 1 play as recorded; faster ones are sped up (the screen) and silent (the call
audio: sped-up speech is not real speech), and the overlay labels them. Writes demo_screen.mp4 and
demo_audio.wav (stereo: the caller a little left, the agent a little right).

usage: make_demo_media.py --data data.json --outdir DIR
"""
import argparse
import json
import subprocess
from pathlib import Path


def run(cmd):
    subprocess.run(cmd, check=True)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--data", required=True)
    p.add_argument("--outdir", required=True)
    a = p.parse_args()
    data = json.loads(Path(a.data).read_text())
    take = Path(data["take"])
    meta = json.loads((take / "meta.json").read_text())
    offset = meta["timeline_start_wall"] - meta["screen_start_wall"]  # screen time of call t=0
    out = Path(a.outdir)
    out.mkdir(parents=True, exist_ok=True)
    # split the speed segments at the focus changes (a zoom on the pane the agents are using)
    focus = data["demo"].get("focus", [])
    def crop_at(t):
        for f in focus:
            if f["from"] <= t < f["to"]:
                return f["crop"]
        return None
    cuts = sorted({f["from"] for f in focus} | {f["to"] for f in focus})
    pieces = []
    for s in data["demo"]["segments"]:
        bounds = [s["call_start"]] + [c for c in cuts if s["call_start"] < c < s["call_end"]] + [s["call_end"]]
        for x, y in zip(bounds, bounds[1:]):
            if y - x > 0.02:
                pieces.append({"call_start": x, "call_end": y, "speed": s["speed"], "crop": crop_at((x + y) / 2)})
    vparts, aparts = [], []
    for i, s in enumerate(pieces):
        cs, ce, sp = s["call_start"], s["call_end"], s["speed"]
        crop = s["crop"]
        vf = f"setpts=(PTS-STARTPTS)/{sp},fps=30"
        if crop:
            vf = f"crop={crop[2]}:{crop[3]}:{crop[0]}:{crop[1]},scale=1920:1080:flags=lanczos," + vf
        v = out / f"seg{i:02d}.mp4"
        run(["ffmpeg", "-loglevel", "error", "-y", "-ss", f"{cs + offset:.3f}", "-to", f"{ce + offset:.3f}",
             "-i", str(take / "screen.mp4"), "-an", "-vf", vf,
             "-c:v", "libx264", "-preset", "veryfast", "-crf", "16", "-pix_fmt", "yuv420p", str(v)])
        vparts.append(v)
        aw = out / f"seg{i:02d}.wav"
        dur = (ce - cs) / sp
        if sp == 1:
            run(["ffmpeg", "-loglevel", "error", "-y", "-ss", f"{cs:.3f}", "-to", f"{ce:.3f}", "-i", str(take / "call.wav"),
                 "-af", "pan=stereo|c0=0.85*c0+0.35*c1|c1=0.35*c0+0.85*c1", "-ar", "48000", str(aw)])
        else:
            run(["ffmpeg", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo",
                 "-t", f"{dur:.3f}", str(aw)])
        aparts.append(aw)
    lst = out / "v.txt"
    lst.write_text("".join(f"file '{v.name}'\n" for v in vparts))
    run(["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(lst), "-c", "copy",
         str(out / "demo_screen.mp4")])
    lst = out / "a.txt"
    lst.write_text("".join(f"file '{w.name}'\n" for w in aparts))
    run(["ffmpeg", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(lst), "-c:a", "pcm_s16le",
         str(out / "demo_audio.wav")])
    # the screen recording can stop a little before the call does: hold its last frame to the end
    def length(f):
        return float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0",
                                     str(f)], check=True, capture_output=True, text=True).stdout)
    short = length(out / "demo_audio.wav") - length(out / "demo_screen.mp4")
    if short > 0.05:
        padded = out / "demo_screen_pad.mp4"
        run(["ffmpeg", "-loglevel", "error", "-y", "-i", str(out / "demo_screen.mp4"),
             "-vf", f"tpad=stop_mode=clone:stop_duration={short:.3f}",
             "-c:v", "libx264", "-preset", "veryfast", "-crf", "16", "-pix_fmt", "yuv420p", str(padded)])
        padded.replace(out / "demo_screen.mp4")
    for f in vparts + aparts:
        f.unlink()
    print("wrote", out / "demo_screen.mp4", out / "demo_audio.wav")


if __name__ == "__main__":
    main()
