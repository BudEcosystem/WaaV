"""Three desktop agents behind Bud WaaV interaction mode, each a Claude Opus 5.5 tool loop.

The gateway's orchestrator (Claude Opus 5.5) starts child tasks on these webhook backends:
  browser   operates a Chromium window on the sandbox desktop
  terminal  types and runs commands in the sandbox desktop's terminal
  writer    drafts a document and, after the caller says yes, saves it and opens it in LibreOffice

Protocol (gateway docs, "Operator Backends" > "Webhook"): the gateway POSTs {type: task|answer|cancel}
to the entry's url; the reply "taken" is empty; events go back later, signed, to
POST /v1/tasks/<task_id>/callback with X-WaaV-Timestamp and X-WaaV-Signature (HMAC-SHA256 of
"<ts>.<body>" keyed with the entry's secret).

Every action is real: the sandbox is a Debian container (no host files, no host network services).
Writes a log of every step to --log (JSONL) for the video's overlays.
"""
import argparse
import hashlib
import hmac
import json
import os
import shlex
import subprocess
import threading
import time
import traceback
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import Request, urlopen

import anthropic

GATEWAY = os.environ.get("WAAV_HTTP", "http://127.0.0.1:4410")
SECRET = os.environ["DESK_SECRET"].encode()
DESK = os.environ.get("DESK_CONTAINER", "bud-desk-demo")
BROWSERD = os.environ.get("BROWSERD", "http://127.0.0.1:19333")
MODEL = os.environ.get("AGENT_MODEL", "claude-opus-5-5")
client = anthropic.Anthropic()  # ANTHROPIC_API_KEY from the environment
LOG_LOCK = threading.Lock()
LOG_PATH = None
TASKS = {}  # task_id -> {"answers": {request_id: answer}, "cancelled": bool, "revision": int}


def log(kind, **fields):
    rec = {"wall": round(time.time(), 3), "kind": kind, **fields}
    print(json.dumps(rec)[:400], flush=True)
    if LOG_PATH:
        with LOG_LOCK, open(LOG_PATH, "a") as f:
            f.write(json.dumps(rec) + "\n")


def post_events(task_id, events):
    body = json.dumps(events).encode()
    ts = str(int(time.time()))
    sig = hmac.new(SECRET, ts.encode() + b"." + body, hashlib.sha256).hexdigest()
    req = Request(f"{GATEWAY}/v1/tasks/{task_id}/callback", data=body, method="POST",
                  headers={"Content-Type": "application/json", "X-WaaV-Timestamp": ts,
                           "X-WaaV-Signature": f"sha256={sig}"})
    try:
        with urlopen(req, timeout=10) as r:
            return r.status
    except Exception as e:  # noqa: BLE001
        log("callback_error", task_id=task_id, error=str(e)[:200])
        return None


# ---- the desktop

def sh(args, timeout=120):
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout)


def browser(path, **body):
    req = Request(BROWSERD + path, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    try:
        with urlopen(req, timeout=60) as r:
            return json.loads(r.read())
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)[:300]}


def focus(title_part):
    sh(["docker", "exec", "-e", "DISPLAY=:1", DESK, "bash", "-lc",
        f"wmctrl -a {shlex.quote(title_part)} 2>/dev/null || true"], timeout=10)


TERM_LOCK = threading.Lock()

# Runs in the container: types stdin into the tmux session key by key, with a person's rhythm.
TYPIST = r"""
import random, subprocess, sys, time
for ch in sys.stdin.read():
    subprocess.run(["tmux", "send-keys", "-t", "work", "-l", "--", ch])
    time.sleep(random.uniform(0.025, 0.06) + (0.12 if ch in " |&;" and random.random() < 0.3 else 0))
"""

# Runs in the container: types a document into the open LibreOffice Writer window, as a person
# would, with Writer's own shortcuts for the styles (Ctrl+1 and Ctrl+2 headings, Shift+F12 bullets).
WRITER_TYPIST = r"""
import json, subprocess, sys, time
blocks = json.loads(sys.stdin.read())
def x(*a): subprocess.run(["xdotool", *a])
wid = subprocess.run(["xdotool", "search", "--name", "LibreOffice Writer"], capture_output=True, text=True).stdout.split()
if wid:
    x("windowactivate", "--sync", wid[-1])
time.sleep(0.4)
x("key", "ctrl+a"); x("key", "Delete"); time.sleep(0.3)
for kind, text in blocks:
    if kind == "h1": x("key", "ctrl+1")
    elif kind == "h2": x("key", "ctrl+2")
    elif kind == "bullets": x("key", "shift+F12")
    else: x("key", "ctrl+0")
    items = text if kind == "bullets" else [text]
    for item in items:
        x("type", "--delay", "16", "--", item)
        x("key", "Return")
        time.sleep(0.15)
    if kind == "bullets": x("key", "shift+F12")
    time.sleep(0.35)
"""

# Runs in the container: saves the Writer document under the given path (Save As dialog).
WRITER_SAVE = r"""
import subprocess, sys, time
path = sys.stdin.read().strip()
def x(*a): subprocess.run(["xdotool", *a])
wid = subprocess.run(["xdotool", "search", "--name", "LibreOffice Writer"], capture_output=True, text=True).stdout.split()
if wid:
    x("windowactivate", "--sync", wid[-1])
time.sleep(0.3)
x("key", "ctrl+shift+s"); time.sleep(2.0)
x("key", "ctrl+a"); x("type", "--delay", "20", "--", path); time.sleep(0.4)
x("key", "Return"); time.sleep(2.0)
"""


def terminal_run(command, timeout=600):
    """Type the command into the visible terminal (as a person would), run it, return its output."""
    with TERM_LOCK:
        marker = f"__done_{uuid.uuid4().hex[:6]}__"
        sh(["docker", "exec", DESK, "tmux", "send-keys", "-t", "work", "clear", "Enter"], timeout=10)
        time.sleep(0.3)
        # typed one key at a time at a person's pace (one docker exec for the whole command)
        subprocess.run(["docker", "exec", "-i", DESK, "python3", "-c", TYPIST], input=command, text=True,
                       capture_output=True, timeout=120)
        time.sleep(0.5)
        sh(["docker", "exec", DESK, "tmux", "send-keys", "-t", "work", "-l", "--", f"; echo {marker} $?"], timeout=10)
        sh(["docker", "exec", DESK, "tmux", "send-keys", "-t", "work", "Enter"], timeout=10)
        end = time.time() + timeout
        out = ""
        while time.time() < end:
            out = sh(["docker", "exec", DESK, "tmux", "capture-pane", "-p", "-J", "-t", "work", "-S", "-400"], timeout=10).stdout
            lines = out.splitlines()
            done = [i for i, l in enumerate(lines) if l.startswith(marker)]
            if done:
                end_i = done[-1]
                start_i = max([i for i, l in enumerate(lines[:end_i]) if f"echo {marker}" in l] or [-1]) + 1
                code = lines[end_i][len(marker):].strip()
                body = "\n".join(lines[start_i:end_i])
                return {"exit_code": int(code) if code.isdigit() else None, "output": body.strip()[-3500:]}
            time.sleep(0.4)
        return {"exit_code": None, "output": out.strip()[-3500:], "note": "timed out; still running"}


def blocks_of(title, body_md):
    """The document as typing steps, read line by line: a title, '## ' section headings, runs of '- '
    bullets, and paragraphs (consecutive plain lines). Markdown emphasis is dropped."""
    out, para, bullets = [("h1", title.strip().lstrip("# ").strip())], [], []

    def flush():
        if para:
            out.append(("p", " ".join(para)))
            para.clear()
        if bullets:
            out.append(("bullets", list(bullets)))
            bullets.clear()

    for raw in body_md.splitlines():
        line = raw.strip().replace("**", "").replace("__", "")
        if not line:
            flush()
        elif line.startswith("#"):
            flush()
            out.append(("h2", line.lstrip("#").strip()))
        elif line.startswith(("- ", "* ", "• ")):
            if para:
                out.append(("p", " ".join(para)))
                para.clear()
            bullets.append(line[2:].strip())
        else:
            if bullets:
                out.append(("bullets", list(bullets)))
                bullets.clear()
            para.append(line)
    flush()
    return out


def type_document(title, body_md):
    """Type the draft into the Writer window on the desktop, as a person would."""
    r = subprocess.run(["docker", "exec", "-i", "-e", "DISPLAY=:1", DESK, "python3", "-c", WRITER_TYPIST],
                       input=json.dumps(blocks_of(title, body_md)), capture_output=True, text=True, timeout=300)
    return {"typed": True} if r.returncode == 0 else {"error": r.stderr[-400:]}


def save_typed(title):
    safe = "".join(c if c.isalnum() or c in " -_" else "_" for c in title).strip().replace(" ", "_")[:60] or "draft"
    path = f"/home/ava/Documents/{safe}.odt"
    subprocess.run(["docker", "exec", "-i", "-e", "DISPLAY=:1", DESK, "python3", "-c", WRITER_SAVE], input=path,
                   capture_output=True, text=True, timeout=60)
    ok = sh(["docker", "exec", DESK, "test", "-s", path], timeout=10).returncode == 0
    return {"saved": path} if ok else {"error": "the document was not saved"}


def open_document(title, body_md):
    """Write a .docx in ~/Documents of the sandbox and open it in LibreOffice Writer."""
    safe = "".join(c if c.isalnum() or c in " -_" else "_" for c in title).strip().replace(" ", "_")[:60] or "draft"
    path = f"/home/ava/Documents/{safe}.docx"
    script = r'''
import sys, json, docx
from docx.shared import Pt
title, body, path = json.loads(sys.stdin.read())
d = docx.Document()
st = d.styles["Normal"]; st.font.name = "Inter"; st.font.size = Pt(12)
d.add_heading(title, level=1)
for block in body.split("\n\n"):
    b = block.strip()
    if not b: continue
    if b.startswith("## "): d.add_heading(b[3:], level=2); continue
    if all(l.strip().startswith(("- ", "* ")) for l in b.splitlines()):
        for l in b.splitlines(): d.add_paragraph(l.strip()[2:], style="List Bullet")
        continue
    d.add_paragraph(b.replace("\n", " "))
d.save(path)
print(path)
'''
    r = subprocess.run(["docker", "exec", "-i", DESK, "python3", "-c", script], input=json.dumps([title, body_md, path]),
                       capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        return {"error": r.stderr[-400:]}
    sh(["docker", "exec", "-d", "-e", "DISPLAY=:1", DESK, "soffice", "--writer", "--nologo", path], timeout=10)
    time.sleep(3.0)
    sh(["docker", "exec", "-e", "DISPLAY=:1", DESK, "bash", "-lc",
        "wmctrl -r LibreOffice -e 0,30,30,940,1000 2>/dev/null; wmctrl -a LibreOffice 2>/dev/null; true"], timeout=10)
    return {"saved": path}


# ---- the agents

SAY = {"type": "string", "description": "What the caller hears while this runs: a short phrase of 3 to 8 words, "
                                         "e.g. 'Opening the releases page.' Leave empty for a minor step."}
AGENTS = {
    "browser": {
        "system": ("You operate a real Chromium browser on a Linux desktop to do the request you are given, while the "
                   "person watches the screen. Work as a person would: open the project's main page (e.g. "
                   "https://github.com/<org>/<repo>), then follow links with click_text (e.g. 'Releases', then the release), "
                   "read_page to read a page, and highlight to point at the two or three facts you report (e.g. the "
                   "'Latest' label, a file name). At most 8 steps. "
                   "Your final message is spoken to a person on a call: ONE short sentence (at most 25 words) with the concrete "
                   "facts they asked for (names, numbers, dates). Write versions and numbers as people say them ('version one point one'), never digits with dots. No markdown, no URLs, no file names."),
        "tools": [
            {"name": "open_url", "description": "Open a URL in the browser.", "input_schema": {
                "type": "object", "properties": {"url": {"type": "string"}, "say": SAY}, "required": ["url"]}},
            {"name": "read_page", "description": "Read the visible text of the current page.", "input_schema": {
                "type": "object", "properties": {"say": SAY}}},
            {"name": "click_text", "description": "Click the link or button with this text.", "input_schema": {
                "type": "object", "properties": {"text": {"type": "string"}, "say": SAY}, "required": ["text"]}},
            {"name": "scroll", "description": "Scroll the page down.", "input_schema": {
                "type": "object", "properties": {"pixels": {"type": "integer"}, "say": SAY}}},
            {"name": "highlight", "description": "Scroll to a short piece of text on the page and mark it, to show the person what you found.",
             "input_schema": {"type": "object", "properties": {"text": {"type": "string", "description": "Exact text on the page, a few words."}, "say": SAY},
                              "required": ["text"]}},
        ],
    },
    "terminal": {
        "system": ("You operate a real terminal on a sandboxed Debian Linux machine (arm64, user 'ava', no sudo, "
                   "internet access; curl, tar, sha256sum, grep, jq and ls are available, `file` is not). Do the request you are given by running shell "
                   "commands with run. Keep each command short and readable (it is typed on screen); chain with && "
                   "when natural. Never run anything destructive outside your home directory. A server you start "
                   "must run in the background (&) and be stopped before you finish. Use at most 8 commands. Download with "
                   "`curl -L -# -O <url>` so the progress shows on screen. To check one archive against a checksum list: "
                   "`grep <archive-name> SHA256SUMS | sha256sum -c -`. "
                   "Your final message is spoken to a person on a call: ONE short sentence (at most 25 words) with the outcome "
                   "and the key facts (versions, checks passed, timings). Write versions and numbers as people say them ('version one point one'), never digits with dots. No markdown, no file names."),
        "tools": [
            {"name": "run", "description": "Run a shell command in the visible terminal and get its output.",
             "input_schema": {"type": "object", "properties": {"command": {"type": "string"}, "say": SAY},
                              "required": ["command"]}},
        ],
    },
    "writer": {
        "system": ("You write documents for a person on a call and save them in LibreOffice Writer on their desktop. "
                   "Draft the document the request asks for: clear, specific and ready to send, with a short title and "
                   "plain paragraphs or '- ' bullet lists ('## ' for a section heading); keep it under 180 words. Never "
                   "leave blanks or placeholders: use the facts in the request and in the company notes below, and leave "
                   "out what you do not know. Include every fact the request asks you to include, numbers above all, "
                   "exactly as given (an early launch with few downloads is still news). First call write_draft once: it types the draft into Writer on the screen "
                   "while the person watches. Then call save_document once: the person is asked to approve before it is "
                   "saved; if they decline, say so. Your final message is spoken: one short sentence (at most 15 words) on what you saved, without the title and without digits.\n\n"
                   "Company notes (Bud Ecosystem):\n"
                   "- Bud WaaV is an open-source (Apache-2.0), self-hosted real-time voice gateway written in Rust: one "
                   "API for 44 speech vendors (31 speech-to-text, 34 text-to-speech, 11 speech-to-speech integrations) "
                   "and any LLM.\n"
                   "- Release 1.1 adds interaction mode: a small, fast front model decides every turn of the caller and "
                   "answers at once, while the real work runs as background tasks on any LLM or agent (A2A, MCP, "
                   "webhooks); barge-in, backchannels, confirmations before actions, caller controls, LiveKit and phone "
                   "calls.\n- Binaries for Linux x86-64 and arm64, with SHA256 checksums, on GitHub "
                   "(github.com/BudEcosystem/WaaV)."),
        "tools": [
            {"name": "write_draft", "description": "Type the draft into the open Writer document on the screen.",
             "input_schema": {"type": "object", "properties": {
                 "title": {"type": "string"}, "body": {"type": "string", "description": "The document, '## ' headings, '- ' bullets, blank lines between paragraphs."},
                 "say": SAY}, "required": ["title", "body"]}},
            {"name": "save_document", "description": "Save the typed document. The caller approves first.",
             "input_schema": {"type": "object", "properties": {"title": {"type": "string"}, "say": SAY},
                              "required": ["title"]}},
        ],
    },
}


def run_tool(agent, task_id, name, args):
    if agent == "browser":
        if name == "open_url":
            return browser("/open", url=args["url"])
        if name == "read_page":
            return browser("/read")
        if name == "click_text":
            return browser("/click", text=args["text"])
        if name == "scroll":
            return browser("/scroll", pixels=args.get("pixels", 600))
        if name == "highlight":
            return browser("/highlight", text=args["text"])
    if agent == "terminal" and name == "run":
        return terminal_run(args["command"])
    if agent == "writer" and name == "write_draft":
        return type_document(args["title"], args["body"])
    if agent == "writer" and name == "save_document":
        rid = f"save-{uuid.uuid4().hex[:8]}"
        post_events(task_id, [{"type": "question", "request_id": rid, "kind": "confirm",
                               "action": "save the document", "arguments": {"title": args["title"]}}])
        log("question", agent=agent, task_id=task_id, request_id=rid, title=args["title"])
        end = time.time() + 120
        while time.time() < end:
            ans = TASKS.get(task_id, {}).get("answers", {}).get(rid)
            if ans is not None:
                break
            if TASKS.get(task_id, {}).get("cancelled"):
                return {"error": "the task was cancelled"}
            time.sleep(0.1)
        else:
            return {"declined": True, "reason": "no answer"}
        if ans.get("outcome") != "accept":
            return {"declined": True, "outcome": ans.get("outcome")}
        out = save_typed(args["title"])
        if "saved" in out:
            post_events(task_id, [{"type": "effect", "label": f"saved {args['title']}"}])
        return out
    return {"error": f"unknown tool {name}"}


def agent_loop(agent, task):
    task_id = task["task_id"]
    spec = AGENTS[agent]
    request = task.get("request", "")
    messages = [{"role": "user", "content": request}]
    log("task", agent=agent, task_id=task_id, request=request)
    started = time.time()
    try:
        for step in range(12):
            if TASKS.get(task_id, {}).get("cancelled"):
                post_events(task_id, [{"type": "cancelled"}])
                log("cancelled", agent=agent, task_id=task_id)
                return
            t = time.time()
            resp = client.beta.messages.create(
                model=MODEL, max_tokens=16000, system=spec["system"], tools=spec["tools"], messages=messages,
                output_config={"effort": "low"},
                betas=["server-side-fallback-2026-07-01"], fallbacks="default",
            )
            log("model", agent=agent, task_id=task_id, step=step, seconds=round(time.time() - t, 2),
                stop=resp.stop_reason, usage={"in": resp.usage.input_tokens, "out": resp.usage.output_tokens})
            if resp.stop_reason == "refusal":
                post_events(task_id, [{"type": "failed", "error": "declined by the model"}])
                return
            uses = [b for b in resp.content if b.type == "tool_use"]
            if not uses:
                text = " ".join(b.text for b in resp.content if b.type == "text").strip()
                post_events(task_id, [{"type": "result", "text": text or "Done."}])
                log("result", agent=agent, task_id=task_id, text=text, seconds=round(time.time() - started, 2))
                return
            messages.append({"role": "assistant", "content": resp.content})
            results = []
            for u in uses:
                say = (u.input or {}).get("say")
                if say and u.name != "save_document":  # the gateway's read-back asks for the save
                    post_events(task_id, [{"type": "progress", "text": say}])
                t = time.time()
                out = run_tool(agent, task_id, u.name, u.input or {})
                log("tool", agent=agent, task_id=task_id, tool=u.name, input={k: v for k, v in (u.input or {}).items() if k != "body"},
                    seconds=round(time.time() - t, 2), output=json.dumps(out)[:600])
                results.append({"type": "tool_result", "tool_use_id": u.id, "content": json.dumps(out)[:8000],
                                **({"is_error": True} if isinstance(out, dict) and out.get("error") else {})})
            messages.append({"role": "user", "content": results})
        post_events(task_id, [{"type": "result", "text": "I've done what I could in the steps I had."}])
    except Exception as e:  # noqa: BLE001
        log("error", agent=agent, task_id=task_id, error=traceback.format_exc()[-800:])
        post_events(task_id, [{"type": "failed", "error": str(e)[:200]}])


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        agent = self.path.strip("/")
        try:
            msg = json.loads(raw or b"{}")
        except ValueError:
            msg = {}
        kind, task_id = msg.get("type"), msg.get("task_id")
        reply = b""
        if agent in AGENTS and kind == "task":
            TASKS[task_id] = {"answers": {}, "cancelled": False, "revision": msg.get("revision")}
            threading.Thread(target=agent_loop, args=(agent, msg), daemon=True).start()
        elif kind == "answer":
            TASKS.setdefault(task_id, {"answers": {}})["answers"][msg.get("request_id")] = msg
            log("answer", agent=agent, task_id=task_id, outcome=msg.get("outcome"))
        elif kind == "cancel":
            TASKS.setdefault(task_id, {"answers": {}})["cancelled"] = True
            reply = json.dumps([{"type": "cancelled"}]).encode()
            log("cancel", agent=agent, task_id=task_id)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(reply)))
        self.end_headers()
        self.wfile.write(reply)


def main():
    global LOG_PATH
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=19800)
    p.add_argument("--log", default="/home/bud/launch/demo/desk_agents.jsonl")
    a = p.parse_args()
    LOG_PATH = a.log
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), H)
    srv.daemon_threads = True
    log("listening", port=a.port)
    srv.serve_forever()


if __name__ == "__main__":
    main()
