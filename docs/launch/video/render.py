"""Render the launch video: scenes.html frame by frame (headless Chromium), composited by ffmpeg
over the demo's screen recording, then muxed with the audio track.

usage: render.py --data data.json --out video.mp4 [--from S --to S] [--fps 30] [--screen demo_screen.mp4]
The demo screen video must already be time-mapped (its t=0 is the demo scene's start).
"""
import argparse
import json
import subprocess
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

HERE = Path(__file__).parent
CHROME = "/home/bud/.cache/ms-playwright/chromium-1234/chrome-linux/chrome"


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--data", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--fps", type=int, default=30)
    p.add_argument("--from", dest="t0", type=float, default=0.0)
    p.add_argument("--to", dest="t1", type=float, default=None)
    p.add_argument("--screen", default=None, help="time-mapped demo screen video")
    p.add_argument("--audio", default=None)
    p.add_argument("--crf", type=int, default=17)
    a = p.parse_args()
    data = json.loads(Path(a.data).read_text())
    total = max(s["end"] for s in data["scenes"])
    t1 = a.t1 if a.t1 is not None else total
    n = int(round((t1 - a.t0) * a.fps))
    demo = next((s for s in data["scenes"] if s["id"] == "demo"), None)
    box = data.get("screen_box", [48, 136, 1248, 702])
    cmd = ["ffmpeg", "-loglevel", "error", "-y",
           "-f", "image2pipe", "-framerate", str(a.fps), "-c:v", "png", "-i", "-"]
    filt = f"color=c=0x07050F:s=1920x1080:r={a.fps}:d={(t1 - a.t0):.3f}[bg];"
    if a.screen and demo:
        ds = demo["start"] - a.t0
        cmd += ["-i", a.screen]
        x, y, w, h = box
        filt += (f"[1:v]scale={w}:{h}:flags=lanczos,format=yuva420p,"
                 f"setpts=PTS-STARTPTS+({ds:.3f})/TB[scr];"
                 f"[bg][scr]overlay={x}:{y}:eof_action=pass:enable='between(t,{max(0, ds):.3f},{demo['end'] - a.t0:.3f})'[b1];"
                 f"[b1][0:v]overlay=0:0:format=auto[v]")
    else:
        filt += "[bg][0:v]overlay=0:0:format=auto[v]"
    if a.audio:
        cmd += ["-ss", f"{a.t0:.3f}", "-i", a.audio]
    cmd += ["-filter_complex", filt, "-map", "[v]"]
    if a.audio:
        cmd += ["-map", f"{2 if (a.screen and demo) else 1}:a", "-c:a", "aac", "-b:a", "256k", "-shortest"]
    cmd += ["-c:v", "libx264", "-preset", "medium", "-crf", str(a.crf), "-pix_fmt", "yuv420p", "-r", str(a.fps),
            "-movflags", "+faststart", a.out]
    ff = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    started = time.time()
    with sync_playwright() as pw:
        br = pw.chromium.launch(executable_path=CHROME, args=["--disable-gpu", "--font-render-hinting=none"])
        pg = br.new_page(viewport={"width": 1920, "height": 1080}, device_scale_factor=1)
        pg.add_init_script(f"window.DATA = {json.dumps(data)};")
        pg.goto((HERE / "scenes.html").as_uri())
        pg.wait_for_function("window.render !== undefined")
        pg.evaluate("document.fonts.ready")
        for i in range(n):
            t = a.t0 + i / a.fps
            pg.evaluate(f"window.render({t:.4f})")
            ff.stdin.write(pg.screenshot(type="png", omit_background=True))
            if i % 150 == 0:
                print(f"frame {i}/{n} t={t:.1f}s {time.time() - started:.0f}s", flush=True)
        br.close()
    ff.stdin.close()
    ff.wait()
    print("wrote", a.out, f"{time.time() - started:.0f}s")


if __name__ == "__main__":
    main()
