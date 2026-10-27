# Interaction Mode Tutorial

Interaction mode turns a `/ws` voice session into an agent that keeps talking with the caller while slower work happens elsewhere. A small, fast language model, the **front model**, reads every caller turn and decides what it is: a request, a correction, a "stop talking", an answer to a question. It acknowledges the caller at once ("One moment."). The actual request becomes a **task** that a slower **background** runs: your own LLM, an orchestrator model that hands work to agents, or an agent of yours reached over a webhook, the Agent2Agent (A2A) protocol or the Model Context Protocol (MCP). When the task's answer comes back, the gateway reads it to the caller word for word, sentence by sentence. Throughout, the gateway keeps track of who is speaking (the **floor**), lets the caller interrupt (**barge-in**), and sends your client an event for each thing that happens. A session that does not ask for the mode runs exactly as before.

This tutorial takes you from an empty machine to a working session, then adds tasks on an agent, caller controls and questions to the caller. The full reference is [Interaction Mode](../interaction_mode.md); this page links into it where you need more detail.

## Before You Start

You need:

- **The gateway**, built with the turn-taking models, or the Docker image. Interaction mode runs its own voice activity detection and Smart Turn end-of-turn model, so a source build needs the `turn-ensemble` feature. The image built from `gateway/Dockerfile` already has it.
- **A speech-to-text and a text-to-speech provider key.** This tutorial uses Deepgram for both: set `DEEPGRAM_API_KEY` in the gateway's environment. The speech-to-text must stream (Deepgram `nova-3` does), and the session language must be English unless you add a language pack.
- **llama.cpp's `llama-server`** and the front model, **Qwen3.5-2B** as a GGUF file (for example `Qwen3.5-2B-Q4_K_M.gguf`). The mode does not start without a ready front model; there is no rule-based fallback.
- **A background model**: any OpenAI-compatible chat endpoint. It can be a second `llama-server`, a hosted model, or, for the orchestrator in Step 7, a larger model such as `gpt-oss-20b`.
- **Python 3.10 or newer** with the `bud_waav` SDK (`clients_sdk/python`) for Step 5, or any WebSocket client for Step 6.

## Step 1: Start the Front Model

Run `llama-server` with two slots (`-np 2`) and room for the front's prompt in each (`-c 8192`). The gateway uses one slot for the caller's turns and the other for words said over the agent.

```bash
llama-server -m /models/Qwen3.5-2B-Q4_K_M.gguf \
  --host 127.0.0.1 --port 18741 -np 2 -c 8192 --no-webui
```

Add llama.cpp's GPU offload option (`-ngl`) if you have a GPU. Check that it answers:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:18741/health   # 200 when ready
```

You can instead let the gateway start and restart this server itself with `front_model.command` (see Step 3).

## Step 2: Start a Background Model

Any OpenAI-compatible `/v1/chat/completions` endpoint works. For a local setup, run a second `llama-server` on another port with an instruct model of your choice:

```bash
llama-server -m /models/your-background-model.gguf --host 127.0.0.1 --port 18742 -c 8192
```

The session names this endpoint in its `conversation_config` (Step 5). You do not need it when every task goes to an agent of yours (Step 7).

## Step 3: Turn the Mode On in the Gateway

Interaction mode is off unless the server switch is on. Add this block to the YAML file you pass with `-c` (the keys come from `gateway/config.example.yaml`):

```yaml
interaction:
  enabled: true
  front_model:
    url: "http://127.0.0.1:18741"
    model: "qwen3.5-2b"
```

`model` is the name sent to the front server. To have the gateway run the front server itself, add `command` (the program, then its arguments; it must listen at `url`):

```yaml
interaction:
  enabled: true
  front_model:
    url: "http://127.0.0.1:18741"
    command: ["/opt/llama/llama-server", "-m", "/models/Qwen3.5-2B-Q4_K_M.gguf",
              "--host", "127.0.0.1", "--port", "18741", "-np", "2", "-c", "8192", "--no-webui"]
```

Every key also has an environment variable, which wins over the YAML key. The three above are:

```bash
export WAAV_INTERACTION_ENABLED=true
export WAAV_INTERACTION_FRONT_URL=http://127.0.0.1:18741
export WAAV_INTERACTION_FRONT_MODEL=qwen3.5-2b
```

A mistake in the block is logged at startup and never stops the server. A block that does not parse (for example one with an unknown key) is ignored as a whole, so read the startup log.

## Step 4: Start the Gateway

From source:

```bash
cd gateway
cargo build --release --features turn-ensemble
./target/release/waav-gateway init          # downloads the turn-taking models once
DEEPGRAM_API_KEY=${DEEPGRAM_API_KEY} ./target/release/waav-gateway -c config.yaml
```

The models are loaded through ONNX Runtime; if the gateway cannot find `libonnxruntime.so`, set `ORT_DYLIB_PATH` to its path.

With Docker (the image already holds the models; host networking lets it reach the front server on 127.0.0.1):

```bash
docker build -t waav-gateway gateway/
docker run --network host \
  -e DEEPGRAM_API_KEY=${DEEPGRAM_API_KEY} \
  -e WAAV_INTERACTION_ENABLED=true \
  -e WAAV_INTERACTION_FRONT_URL=http://127.0.0.1:18741 \
  -e WAAV_INTERACTION_FRONT_MODEL=qwen3.5-2b \
  waav-gateway
```

When the gateway runs the front server itself, wait for the log line `front model ready` before you open a session.

## Step 5: Open a Session with the Python SDK

Pass `interaction=True` (the defaults) or an object of settings. The SDK sends it as the `interaction` field of the `config` message.

```python
import asyncio
from bud_waav import BudClient, ConversationConfig, TaskQuestion, TaskResult

async def main():
    bud = BudClient(base_url="http://localhost:3001")
    call = bud.talk.create(
        stt={"provider": "deepgram", "model": "nova-3", "language": "en"},
        tts={"provider": "deepgram", "voice_id": "aura-asteria-en"},
        conversation_config=ConversationConfig(
            base_url="http://127.0.0.1:18742/v1", model="your-background-model"),
        stream_id="tutorial-1",
        interaction=True,
    )
    async with call:
        # The resolved settings from `ready`; None when the gateway refused the mode.
        print("interaction:", call.interaction_settings)
        async for ev in call:
            if ev.type == "warning":                    # a config_warning
                print("warning:", ev.warning.code, ev.warning.message)
            elif ev.type == "interaction":
                event = ev.interaction
                print(event.type, event.model_dump(exclude_none=True))
                if isinstance(event, TaskQuestion) and "client" in event.channels:
                    await call.answer_task(event.request_id, "accept")
                elif isinstance(event, TaskResult):
                    print(event.task_ref, event.state, event.summary)

asyncio.run(main())
```

Send the caller's audio with `await call.send_audio(pcm_bytes)`: 16-bit little-endian mono PCM at the speech-to-text sample rate. The agent's speech comes back as `audio` events. The TypeScript SDK works the same way: `interaction` on `SessionConfig`, the `interaction` event with the `InteractionEvent` union, and `answerTask`, `cancelTask`, `updateInteraction`, `reportPlayback`.

## Step 6: Open a Session with Plain WebSocket JSON

Connect to `ws://localhost:3001/ws` and send the `config` message first:

```json
{
  "type": "config",
  "stream_id": "tutorial-2",
  "stt_config": { "provider": "deepgram", "model": "nova-3", "language": "en",
                  "sample_rate": 16000, "channels": 1, "encoding": "linear16" },
  "tts_config": { "provider": "deepgram", "model": "aura-asteria-en", "voice_id": "aura-asteria-en",
                  "audio_format": "linear16", "sample_rate": 24000 },
  "conversation_config": { "base_url": "http://127.0.0.1:18742/v1", "model": "your-background-model" },
  "interaction": true
}
```

The gateway answers with `ready`. With the mode on, `protocol_version` is `"1.1"` and `ready.interaction` holds the resolved settings. If `interaction` is missing, the session runs without the mode, and a `config_warning` that came before it says why (see Troubleshooting).

Now stream the caller's audio as binary frames, or type a caller turn to try things without a microphone:

```json
{ "type": "agent_input", "text": "What time does the pharmacy on Main Street close today?" }
```

A typical sequence of events for that turn (fields shortened):

```json
{"type": "assistant_transcript", "source": "front", "kind": "ack", "delta": "Let me check.", ...}
{"type": "task_created", "task_id": "task_9f2c...", "task_ref": "t1", "title": "...", "kind": "lookup", "background": "llm", ...}
{"type": "task_progress", "task_id": "task_9f2c...", "state": "working", ...}
{"type": "task_output", "task_id": "task_9f2c...", "state": "completed", "text": "...", ...}
{"type": "assistant_transcript", "source": "background", "verbatim": true, "delta": "...", ...}
{"type": "task_result", "task_id": "task_9f2c...", "state": "completed", "summary": "...", ...}
```

The agent's audio arrives as binary frames between them. The `kind` values and the full field lists are in [Events (Server → Client)](../interaction_mode.md#events-server--client).

## Step 7: Hand Work to an Agent

So far the session's own LLM ran each task. You can run tasks on your own services instead. The operator lists them under `interaction.task_backends`; the address and the secret never leave the server. A session names one with `tasks.default_backend`, which the server accepts from a session only with `allow_client_backends: true` (the server's own `default` block may always name one).

### A webhook

```yaml
interaction:
  enabled: true
  front_model: { url: "http://127.0.0.1:18741", model: "qwen3.5-2b" }
  allow_client_backends: true
  task_backends:
    - name: "crm"
      type: "webhook"
      url: "https://crm.example.com/waav/tasks"
      secret_env: "CRM_WEBHOOK_SECRET"     # the environment variable that holds the signing secret
```

The session then needs no LLM of its own:

```json
{ "type": "config", "stt_config": { ... }, "tts_config": { ... },
  "interaction": { "tasks": { "default_backend": "crm" } } }
```

Each task is a signed `POST` to the webhook with `type: "task"`, the `task_id`, a `revision` and the caller's `request`. `X-WaaV-Signature` is `sha256=` and the hex HMAC-SHA256 of `"<X-WaaV-Timestamp>.<body>"`, keyed with the secret. The reply is one event or a list of events. A minimal handler:

```python
import hashlib, hmac, json, os
from http.server import BaseHTTPRequestHandler, HTTPServer

SECRET = os.environ["CRM_WEBHOOK_SECRET"].encode()

class Hook(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        signed = self.headers["X-WaaV-Timestamp"].encode() + b"." + body
        expected = "sha256=" + hmac.new(SECRET, signed, hashlib.sha256).hexdigest()
        if not hmac.compare_digest(expected, self.headers.get("X-WaaV-Signature", "")):
            self.send_response(401); self.end_headers(); return
        msg = json.loads(body)
        if msg["type"] == "task":
            reply = {"type": "result", "text": "Order A 17 ships on Friday."}
        elif msg["type"] == "cancel":
            reply = {"type": "cancelled"}
        else:                                   # an "answer" to a question of ours
            reply = []
        out = json.dumps(reply).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers(); self.wfile.write(out)

HTTPServer(("0.0.0.0", 8088), Hook).serve_forever()
```

A long job replies `[]` at once and posts its events later, signed the same way, to `POST /v1/tasks/<task_id>/callback`: `progress` (`text`), `question`, `result` (`text`) or `failed` (`error`). See [Webhook](../interaction_mode.md#webhook).

### A2A and MCP agents, and an orchestrator

An A2A agent and an MCP server are listed the same way, with `type: "a2a"` or `type: "mcp"`:

```yaml
  task_backends:
    - name: "trips"                       # an A2A agent: its Agent Card describes it
      type: "a2a"
      url: "https://trips.example.com/a2a"
      secret_env: "TRIPS_KEY"
    - name: "notes"                       # an MCP server: each tool it lists becomes a tool
      type: "mcp"
      url: "https://notes.example.com/mcp"
      tools: ["*"]
      secret_env: "NOTES_TOKEN"
```

With several agents, give the session an **orchestrator**: a larger model whose tools are those agents. The front keeps talking to the caller while the orchestrator plans, and each agent call becomes a child task whose events carry `parent_id`.

```json
"interaction": {
  "tasks": {
    "orchestrator": { "base_url": "http://127.0.0.1:18790/v1", "model": "gpt-oss-20b",
                      "reasoning_effort": "low",
                      "system_prompt": "You hand the caller's requests to agents; they know the caller's account and ask for anything else themselves." },
    "agents": ["trips", "notes"]
  }
}
```

Say in the `system_prompt` what the agents know and that they ask the caller themselves; otherwise the model tends to ask the caller for every detail first. See [Many Agents](../interaction_mode.md#many-agents).

### Reading tasks over HTTP

Tasks are also visible outside the socket. Without an authenticated caller id, name the session with `stream_id`:

```bash
curl "http://localhost:3001/v1/tasks?stream_id=tutorial-2"            # live and recent tasks
curl "http://localhost:3001/v1/tasks/task_9f2c...?stream_id=tutorial-2" # one task with its result
curl -N "http://localhost:3001/v1/tasks/events?stream_id=tutorial-2"    # task events as server-sent events
```

The Python SDK has `await bud.list_tasks(stream_id=...)` and `await bud.get_task(task_id, stream_id=...)`. A client can cancel a task with `{"type": "task_cancel", "task_id": "..."}` (`call.cancel_task(task_id)` in the SDK).

## Step 8: Caller Controls

The caller steers the conversation by voice: "slower please", "keep it short", "say that again", "what was the number?", "talk to me in Spanish". The front model reads these in context; no phrase list decides. Your app can send the same controls, for example from a "slower" button:

```json
{ "type": "control", "kind": "rate", "value": "slower" }
{ "type": "control", "kind": "verbosity", "value": "brief" }
{ "type": "control", "kind": "repeat", "value": "number" }
{ "type": "control", "kind": "language", "value": "es" }
```

Each gets a `control_applied` event with an `outcome` of `applied`, `partial`, `unsupported` or `refused`, and an `interaction_updated` event with the new conversation state. When a control is not applied in full, the agent says so; it never pretends. Some limits to know:

- **Rate** changes in steps of 0.15, only on voices that take a rate mid-session (OpenAI-compatible and Deepgram).
- **Language** switches only to a language the operator lists in `interaction.default.controls.languages`, that the recogniser hears, that the voice provider has a voice for, and that has a reviewed language pack. Only English ships reviewed; `controls.answers_only: true` allows a `partial` switch where the background answers in the new language. `controls` is set by the operator only, and setting `interaction.default` turns the mode on for every session that fits (a session can still send `"interaction": false`).
- **Repeat** says the last line the caller heard in full, or one part of it (`number`, `time`, `code`), or the line again `slower` or `louder`.

The SDKs have no helper for `control` yet: send it as plain JSON. For how long answers are and the caller's pace, you can also change the settings with `interaction_update`, which the SDKs wrap:

```python
await call.update_interaction({"front": {"caller": {"verbosity": "brief", "pace": "slower"}}})
```

The reply is an `interaction_updated` event listing what was `applied` and `refused`. The full list of kinds is in [Conversation Controls](../interaction_mode.md#conversation-controls).

## Step 9: Questions to the Caller (Human in the Loop)

Before a task changes something, the caller confirms it. Declare a tool with `side_effects: true`: the gateway holds the call before anything is sent, reads it back to the caller and waits for a "yes" said after the read-back was heard. A session may declare its own tools only when the server sets `allow_client_tools: true`; tools in the server's `default` block need no such switch and are signed with `tool_secret`:

```json
"interaction": {
  "tasks": {
    "tools": [{
      "name": "book_table",
      "description": "Book a table",
      "parameters": { "type": "object", "properties": { "party": { "type": "integer" } }, "required": ["party"] },
      "url": "https://tools.example.com/book",
      "side_effects": true,
      "confirm_phrase": "book a table",
      "progress_phrase": "Booking the table"
    }]
  },
  "policy": { "hitl_answer_timeout_ms": 30000 }
}
```

The client sees the question as a `task_question`:

```json
{"type": "task_question", "task_id": "task_5b1e...", "request_id": "...", "kind": "confirm",
 "prompt": "To confirm: book a table ... Do you want me to do that?", ...}
```

The caller answers by voice, or your app answers for them:

```json
{ "type": "task_answer", "request_id": "...", "outcome": "accept" }
{ "type": "task_answer", "request_id": "...", "outcome": "decline", "correction": "make it six people" }
```

Then `task_answer_received` reports the `outcome` and who answered (`by`: `voice`, `client`, `dtmf`, `timeout`, ...). A question nobody answers within `policy.hitl_answer_timeout_ms` closes with `timeout`. Agents ask questions the same way: a webhook's `question` event, an A2A task that needs input, or an MCP elicitation.

Some questions are never answered by voice: a form with several fields (`kind: "form"` with a JSON Schema in `schema`), a sign-in link (`kind: "auth_url"` with `url`) and a sensitive value such as a card number (`sensitive: true`). They carry `channels: ["client"]`; show them on screen and reply with `task_answer` (`value` holds the form's fields as an object). This needs `client` in `hitl.channels`, which is the default. See [Questions on the Client's Screen](../interaction_mode.md#questions-on-the-clients-screen).

## Step 10: Observe What Happens

Besides the task events, these help you follow a call:

| Event | What it tells you |
|-------|-------------------|
| `floor_state` | Who holds the floor: `holder` is `user`, `assistant`, `both` or `none`. Turn off with `events.floor: false`. |
| `agent_audio_stopped` | The agent's audio was cut; `reason` is `barge_in` when the caller interrupted, `client_clear` after your `clear` message. Stop playing those `utterance_ids`. |
| `assistant_transcript` | Each line the agent says, with `source` (`front`, `background`, `gateway`), `kind` and `heard`. |
| `audio_segment` | Where each line's audio `start`s and `end`s. Turn off with `events.segments: false`. |
| `interaction_updated` | Settings or state changed, or a capability was lost: `degraded` lists it (for example `front_unavailable`). |
| `session_failed` | A service the call needs stayed down; the gateway said its exit line and closes the socket. |
| `heartbeat` | Nothing else was sent for 5 seconds; the session is alive. |

A client that knows exactly how much of a line it played should report it with `playback_position` (`call.report_playback(utterance_id, played_ms)`), at most four times a second. The gateway then uses it to know what the caller heard, which matters after an interruption.

Barge-in is on by default. To stop the caller interrupting the agent, set `"duplex": {"barge_in": false}`, at the start or mid-session with `interaction_update`.

## Troubleshooting

A session that asked for the mode and cannot have it runs without it, and one `config_warning` says why. `detail.reason` narrows it down.

| `code` | Cause | What to do |
|--------|-------|------------|
| `interaction_disabled_by_server` | `interaction.enabled` is off. | Set it (or `WAAV_INTERACTION_ENABLED=true`) and restart. Check the startup log: a block with an unknown key is ignored whole. |
| `interaction_refused` (reason `front_unavailable`) | The front model is not ready: the server is down, still loading, or `front_model.url` is wrong. | `curl <url>/health` must answer 200. With `command`, wait for `front model ready` in the log. |
| `interaction_needs_background` | No `conversation_config`, no `tasks.llm` and no named backend. | Add one of them. |
| `interaction_surface_unavailable` | `reason` says which: `audio_disabled`, `stt_file_only` (the speech-to-text cannot stream; Deepgram `whisper-*` is file-only), `language_unsupported` (no pack for `stt_config.language`), `background_unavailable`, `agent`, `dag_config`. | Use a streaming recogniser and an English session, or add a language pack. |
| `interaction_override_refused` | The session set a key only the operator may set (`controls`, `tasks.default_backend` or `tasks.tools` without the server's leave). | Move the key to the server's block, or turn on `allow_client_backends` / `allow_client_tools`. |
| `interaction_value_ignored`, `unknown_config_keys` | A value of the wrong type or range, or a misspelt key. `detail.path` names it; the default is kept. | Fix the key. |

Other symptoms:

- **The caller hears "Sorry, I didn't catch that."** The front model did not decide within its budget (`front.total_budget_ms`, 1.5 seconds). The front server is overloaded or too slow: give it its own GPU or fewer competing jobs.
- **`interaction_updated` lists `front_unavailable` mid-call.** The front server stopped answering; nothing new is decided until it is back, and the mode ends after 150 seconds without it.
- **An `error` after `task_answer`, `task_cancel` or `control`.** These messages need a session that runs the mode. With the mode on, the `code` says what went wrong: `question_closed` (the question was already answered or timed out), `unknown_request_id` or `task_unknown` (no question or task has that id).

## Known Limits

Interaction mode speaks English unless you add a reviewed language pack, every turn waits for the front model, tasks live in memory unless you set `interaction.task_redis_url`, and a few live-test cases are recorded but not fixed. Read [Limits](../interaction_mode.md#limits) before you go to production.

## Where to Go Next

- [Interaction Mode](../interaction_mode.md): every setting, event and message.
- [Session Configuration Reference](../interaction_mode.md#session-configuration-reference): the keys of the `interaction` object with their ranges.
- [Operator Backends](../interaction_mode.md#operator-backends): webhooks, A2A, MCP and OpenAI Responses in depth.
- [LiveKit Rooms and Phone Calls](../interaction_mode.md#livekit-rooms-and-phone-calls): the mode on a room's audio or a SIP call.
