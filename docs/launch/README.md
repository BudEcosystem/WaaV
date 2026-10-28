# Interaction mode launch: paper, video and demo

The launch material for WaaV interaction mode (1.1), made on 27–28 October 2026, with the scripts that produced
it. The demo call it shows is take 27, recorded on the build that became
[PR #49](https://github.com/BudEcosystem/WaaV/pull/49).

| Path | What is in it |
|---|---|
| [`paper/main.pdf`](paper/main.pdf) | The paper (21 pages): design, Full-Duplex-Bench v1 and v3 results, comparisons, the case study. Sources are `main.tex`, `sections/`, `refs.bib`; `figures/architecture.py` draws the architecture figure |
| [`video/launch_v4.mp4`](video/launch_v4.mp4) | The launch video (4:11, 1080p) |
| [`video/`](video/) | How the video is built: `build_data.py` turns a take into timeline data (`d27.json`, `d27_demo.json`), `make_demo_media.py` cuts the screen recording, `render.py` renders `scenes.html` frame by frame with Playwright, `assemble.py` mixes narration (`vo/`, Deepgram Aura-2 athena) and music. `bench_v1.json` and `bench_v3.json` hold the benchmark cards |
| [`demo/`](demo/) | The demo call: `launch_day.py` plays the caller (pre-synthesised lines in `caller_lines/`, said at the moment each is written for), `desk_agents.py` runs the browser, terminal and writer agents (Claude Opus 5.5 tool loops) behind the gateway's signed webhooks, `take_retry.sh` and `reset_take.sh` reset the desktop and record a take. `take27/` keeps the take's event log and tool calls (the recordings are not committed) |
| [`desk/`](desk/) | The sandboxed desktop the agents use: a container with Xvfb, an xterm, LibreOffice and Chromium driven by `browserd.py` (visible pointer, scrolling, highlights) |
| [`bench/`](bench/) | The Full-Duplex-Bench harness for WaaV (`fdb_waav.py`), the v1 run and scoring scripts, and `stack.sh`, which starts the front model and gateway |
| [`voices/`](voices/) | How the voices were chosen: candidate Aura-2 voices synthesised (`synth.py`) and scored with UTMOSv2 (`score.py`, `scores.json`) |
| [`research/`](research/) | The related-work survey behind the paper's comparisons |
| [`to_be_fixed.md`](to_be_fixed.md) | Every defect the benchmarks and takes found, with status |

The scripts were written for one machine and keep its absolute paths (`/home/bud/launch`, `/home/bud/bench`).
Keys are read from environment files at start-up (`$IX/secrets/*.env`) and never appear in these files.
