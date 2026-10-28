"""Draw Figure 1 (architecture of Bud WaaV interaction mode) as a vector PDF.

Run: /home/bud/launch/venv/bin/python -I figures/architecture.py   (from /home/bud/launch/paper)
TikZ and rsvg-convert are not available on the build machine, so the figure is drawn with matplotlib.
Every label is a configuration default or a component named in the WaaV 1.1.0 documentation.
"""

import os

import matplotlib

matplotlib.use("pdf")
import matplotlib.pyplot as plt
from matplotlib.patches import FancyArrowPatch, FancyBboxPatch

plt.rcParams.update({
    "font.family": "DejaVu Sans",
    "font.size": 5.5,
    "pdf.fonttype": 42,
})

W, H = 170.0, 105.0
fig = plt.figure(figsize=(W / 25.4, H / 25.4))
ax = fig.add_axes([0, 0, 1, 1])
ax.set_xlim(0, W)
ax.set_ylim(0, H)
ax.axis("off")

INK = "#1d1d1f"
MUTED = "#5b5b66"
C_CLIENT = "#eef2f7"
C_GW = "#f7f5fc"
C_AUDIO = "#e6f0fa"
C_FRONT = "#ece4fb"
C_TASK = "#e8f5ec"
C_OUT = "#fdf2e3"
EDGE_GW = "#8b5cf6"


def box(x, y, w, h, text, fc, ec=INK, lw=0.6, bold=False, size=None, align="center", r=1.2):
    p = FancyBboxPatch((x, y), w, h, boxstyle=f"round,pad=0,rounding_size={r}",
                       fc=fc, ec=ec, lw=lw)
    ax.add_patch(p)
    tx = x + w / 2 if align == "center" else x + 1.6
    ax.text(tx, y + h / 2, text, ha=align, va="center", color=INK,
            fontsize=size or plt.rcParams["font.size"],
            fontweight="bold" if bold else "normal", linespacing=1.25)
    return p


def arrow(x1, y1, x2, y2, style="-|>", color=INK, lw=0.6, ls="-", rad=0.0):
    a = FancyArrowPatch((x1, y1), (x2, y2), arrowstyle=style, mutation_scale=6,
                        color=color, lw=lw, linestyle=ls,
                        connectionstyle=f"arc3,rad={rad}")
    ax.add_patch(a)


def label(x, y, text, size=5.6, color=MUTED, ha="center", style="italic"):
    ax.text(x, y, text, ha=ha, va="center", fontsize=size, color=color, style=style)


# ---------------------------------------------------------------- left: clients and surfaces
ax.text(15, 102, "Clients and surfaces", ha="center", fontsize=6.6, fontweight="bold", color=INK)
clients = [
    ("/ws\n(apps, Python and\nTypeScript SDKs, widget)", 86),
    ("/v1/realtime\n(OpenAI Realtime\nclients)", 72),
    ("/realtime\n(speech-to-speech\nsessions)", 58),
    ("LiveKit rooms\nand SIP calls", 44),
    ("/v1/tasks and\n/v1/tasks/events\n(list, server-sent events)", 30),
]
for t, y in clients:
    box(1, y - 5.5, 28, 11, t, C_CLIENT)
    arrow(29, y, 33, y, style="<|-|>")

# ---------------------------------------------------------------- centre: gateway
gx, gy, gw, gh = 33, 16, 100, 83
box(gx, gy, gw, gh, "", C_GW, ec=EDGE_GW, lw=0.9, r=2.0)
ax.text(gx + gw / 2, gy + gh - 3.0, "WaaV gateway (Rust): one interaction actor per call",
        ha="center", fontsize=6.6, fontweight="bold", color=INK)

ax.text(51, 92.0, "Caller audio path", ha="center", fontsize=5.6, fontweight="bold", color=MUTED)
ax_chain = [
    ("Echo control\ncorrelation gate,\nWebRTC AEC3", 81, 9),
    ("Noise suppression\nDeepFilterNet (optional)", 72, 7),
    ("Voice activity\nSilero VAD v5, 32 ms chunks;\nonset = 2 voiced chunks", 60.5, 9.5),
    ("End of turn\nSmart Turn v3.2 (by ear),\ntext detector (by words),\nSTT finals, 1.5 s silence cap;\neager end of turn", 41.5, 15),
    ("Floor and barge-in\nduck -15 dB at onset,\npause at 400 ms,\nclassify by 900 ms;\nlistening sounds", 20, 15),
]
for t, y, h in ax_chain:
    box(35, y, 32, h, t, C_AUDIO)
arrow(51, 81, 51, 79)
arrow(51, 72, 51, 70)
arrow(51, 60.5, 51, 56.5)
arrow(51, 41.5, 51, 35)

box(71, 74, 60, 16,
    "Front model: Qwen3.5-2B (Q4_K_M) on llama.cpp\n"
    "one grammar-constrained act per caller turn:\n"
    "delegate | update | abort | follow_up | progress |\n"
    "control (kind) | social | refuse | wait (why)\n"
    "reply budget 1.5 s; two server slots per session",
    C_FRONT, ec=EDGE_GW, lw=0.8)
box(71, 50, 60, 18,
    "Policy and speech lanes\n"
    "front lane: acknowledgements, progress, holds\n"
    "main lane: verbatim relay, read-backs, questions\n"
    "backchannel lane: listening sounds\n"
    "metered 20 ms playout; timeline of what was heard",
    "#f1edf9")
box(71, 20, 60, 24,
    "Task registry\n"
    "A2A 1.0 states, revisions, committed effects\n"
    "speculative start; ANSWER: marker; sentence checks\n"
    "confirmation before side effects (heard margin)\n"
    "update inbox (ask, tell, screen); follow-ups\n"
    "tasks after the call; optional shared Redis",
    C_TASK)

arrow(67, 50, 71, 80, rad=-0.25)
arrow(101, 74, 101, 68)
label(110.5, 71.0, "act + short line", size=5.0)
arrow(98, 50, 98, 44)
arrow(104, 44, 104, 50)
label(117, 47.0, "task events, answers", size=5.0)
arrow(67, 30, 71, 56, rad=0.25)

# ---------------------------------------------------------------- right: backgrounds
ax.text(153, 102, "Backgrounds (tasks)", ha="center", fontsize=6.6, fontweight="bold", color=INK)
bgs = [
    ("Session LLM (OpenAI-\ncompatible, Anthropic,\nGemini; local or hosted)", 87, 11),
    ("Orchestrator LLM\n(e.g. gpt-oss-20b); agents\nas child tasks", 74, 11),
    ("Signed webhook\n(HMAC, idempotency)", 63, 8),
    ("A2A 1.0 agent (JSON-RPC,\nHTTP+JSON, gRPC)", 53, 8),
    ("MCP task tool\n(2026-07-28, Tasks ext.)", 43, 8),
    ("OpenAI Responses model", 34.5, 6),
    ("Bud voice agent;\ntext-output DAG", 26, 8),
]
for t, y, h in bgs:
    box(137, y - h / 2, 32, h, t, C_TASK)
arrow(131, 32, 137, 53, style="<|-|>")
arrow(131, 32, 137, 74, style="<|-|>")

# ---------------------------------------------------------------- bottom: speech providers
box(33, 1.5, 44, 10, "Speech-to-text: 31 vendors\n(streaming; file-only models\nvia segmented STT)", C_OUT)
box(81, 1.5, 52, 10, "Text-to-speech: 34 vendors + self-hosted\n(every line in the session's voice)", C_OUT)
box(137, 1.5, 32, 10, "Speech-to-speech:\n11 integrations\n(regimes B and C)", C_OUT)
arrow(51, 20, 51, 11.5)
arrow(101, 20, 101, 11.5)
arrow(133, 19, 137, 8, style="<|-|>", ls=(0, (2, 1.5)), rad=0.0)

out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "architecture.pdf")
fig.savefig(out)
print(out)
