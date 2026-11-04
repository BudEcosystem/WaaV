# Voice agent realtime sessions: Bud events and `bud_context`

A Bud voice agent on the OpenAI-Realtime-compatible WebSocket
(`GET /v1/realtime?model=prompt:<agent>`) runs the agent cascade: speech-to-text, a budprompt run
per turn, text-to-speech. Besides the standard GA events, the session sends a few `bud.*` events
that report what the cascade measured, and it accepts one Bud field on `session.update`.

All `bud.*` events have an `event_id` that starts with `evt_bud_`. Clients that do not know them can
ignore them.

## Switching the events off

`WAAV_REALTIME_BUD_SESSION_EVENTS` (default on; `off`, `false` or `0` turns it off) controls
`bud.tool.started`, `bud.tool.done` and `bud.turn.metrics`, together with the existing
`bud.session.stt` and `bud.session.warning`. When it is off, the GA events are unchanged and no
event below is sent.

## `bud.tool.started` / `bud.tool.done`

These are sent for each server-side tool the agent runs (MCP, web search, code interpreter, a
delegated agent, a function that belongs to the agent). They come right after the GA events for the
same tool (`response.output_item.added` + `response.mcp_call.in_progress`, then
`response.mcp_call.completed|failed` + `response.output_item.done`), and those GA events do not
change. A function the client declared in `session.tools` is reported through the GA
`function_call` events only.

```json
{"type": "bud.tool.started", "event_id": "evt_bud_…",
 "response_id": "resp_bud_…", "item_id": "mcp_1", "name": "lookup_order"}
```

```json
{"type": "bud.tool.done", "event_id": "evt_bud_…",
 "response_id": "resp_bud_…", "item_id": "mcp_1", "name": "lookup_order",
 "duration_ms": 1250, "ok": false, "error": "upstream 503"}
```

- `response_id` is the GA response id (the one in `response.created`), not budprompt's.
- `duration_ms` runs from the tool's start item to its done item, as the gateway received them
  from budprompt. It is `null` when the done item came with no start item.
- `error` is the tool item's own error text: its `error` string, an error object's `message`, or
  the object as JSON. It is `null` when the tool succeeded, and also when it failed without saying
  why.

## `bud.turn.metrics`

One per agent turn that the engine ran (completed, cancelled, failed or incomplete). It is sent just
before that turn's `response.done`. When the next turn cut the response, `response.done` with
`cancelled` has already been sent, and this event follows when the engine ends the cut turn.
Greetings get none, and so do alias sessions (`model=<alias>`), whose conversation loop does not
measure these.

```json
{"type": "bud.turn.metrics", "event_id": "evt_bud_…",
 "response_id": "resp_bud_…", "bud_response_id": "resp_0f3…", "item_id": "item_bud_…",
 "stt_ms": 240, "ttft_ms": 610, "ttfa_ms": 1180, "endpoint_to_audio_ms": 1490,
 "total_ms": 5200,
 "tools": [{"name": "lookup_order", "duration_ms": 1250, "ok": true}],
 "status": "completed"}
```

| Field | Meaning | `null` when |
|---|---|---|
| `response_id` | GA response id | — |
| `bud_response_id` | budprompt's response id (`/v1/responses/{id}`, traces) | the run never got one |
| `item_id` | the assistant message item of the response | the response produced no message |
| `stt_ms` | gateway saw the end of speech → the transcript that came after it | see below |
| `ttft_ms` | the turn's request start → first token of the agent's text | no token arrived |
| `ttfa_ms` | request start → first answer chunk handed to text-to-speech (the trace's `response_latency_ms`) | text mode, or cut before speaking |
| `endpoint_to_audio_ms` | end of speech → first `response.output_audio.delta` of this response sent to the client (the latency the caller hears) | typed turns, text mode, no audio |
| `total_ms` | request start → the turn's end (audio drained, or the cut) | — |
| `tools` | tools that finished in the turn, in finishing order | — (empty list) |
| `status` | `completed`, `cancelled`, `failed` or `incomplete`, as in `response.done` | — |

Nothing is estimated. A value the session did not observe is `null`.

- **End of speech** is when the gateway sent `input_audio_buffer.speech_stopped` for the
  caller's item, measured on the gateway clock. On a gateway-endpointed (segmented) session that is
  the detector's `turn_end`. On a streaming speech-to-text session it is the vendor's
  `speech_final`, which already includes the vendor's own endpointing silence. That silence is not
  in `endpoint_to_audio_ms`.
- **`stt_ms`** is measured only on segmented sessions, where the transcript arrives after the end
  of speech. A streaming vendor reports the end of speech and the final transcript in one message,
  so `stt_ms` is `null` there.
- A turn the agent started without an end of speech (typed text, `response.create`, a smart-turn
  decision before the caller item closed) has `null` for both end-of-speech measures.

## Interaction mode (`interaction: true`)

The same three events are sent, with the same gate. They are built from what the gateway sees of
the interaction actor's lines and of its Bud agent's tools.

**`bud.tool.started` / `bud.tool.done`** are sent for each tool that the session's Bud agent runs
while it works on a task. They have the same fields as above, plus `task_id`:

```json
{"type": "bud.tool.done", "event_id": "evt_bud_…", "response_id": null, "task_id": "task_5b1e…",
 "item_id": "mcp_1", "name": "lookup_order", "duration_ms": 1250, "ok": false, "error": "upstream 503"}
```

`response_id` is the realtime response of a line of the **same task** that is playing when the
event is sent. Otherwise it is `null`, which is the usual case: the front's acknowledgement has
ended, and the agent's answer has not started. Use `task_id` to group the events. A speculative
run's tools are not reported until its task is adopted.

**`bud.turn.metrics`** is sent once per line of the agent (each line is one realtime response),
just before its `response.done`. It has the same fields as above, plus `act`, `source`, `kind` and
`task_id`:

```json
{"type": "bud.turn.metrics", "event_id": "evt_bud_…", "response_id": "resp_bud_…",
 "bud_response_id": null, "item_id": "item_bud_…", "stt_ms": null, "ttft_ms": null,
 "ttfa_ms": null, "endpoint_to_audio_ms": 640, "total_ms": 2310,
 "tools": [{"name": "lookup_order", "duration_ms": 1250, "ok": false}],
 "status": "completed", "act": "delegate", "source": "front", "kind": "ack",
 "task_id": "task_5b1e…"}
```

| Field | In interaction mode |
|---|---|
| `endpoint_to_audio_ms` | from the end of the caller turn the line belongs to (the floor leaving the caller, on the gateway clock) to the line's first audio sent to the client. Line audio that arrived early is sent when the line starts, so it counts from then. `null` for a line with no caller turn behind it, such as a typed turn or a greeting, and for a line of a turn older than the last 16 |
| `total_ms` | from the same end of turn to the line's end (heard in full, or cut). `null` when the end of turn is not known |
| `stt_ms` | as above: only on segmented speech-to-text, otherwise `null` |
| `ttft_ms`, `ttfa_ms`, `bud_response_id` | always `null`. The front's and the agent's requests are made by the actor, and their start times do not reach the realtime layer. The trace (`voice.turn`) has the actor's own latencies |
| `tools` | the tools of the line's task that finished since the task's previous line reported them, so each tool is listed once. Empty for a line of the front's own (no `task_id`) |
| `status` | `completed` when the line was heard in full, otherwise `cancelled` |
| `act` | the front's act for the caller turn (`delegate`, `social`, `wait`, …), when the line carries one. Otherwise `null` |
| `source`, `kind` | who said the line (`front`, `background`, …) and what it is (`ack`, `relay`, …), as in the response's `metadata.bud_source`/`bud_kind` |

## `session.bud_context`: start with the earlier conversation

A client that moves a text chat into a voice call can send the earlier turns, so the agent continues
the same conversation:

```json
{"type": "session.update", "session": {
  "bud_context": [
    {"role": "user", "text": "My order number is 4417."},
    {"role": "assistant", "text": "Thanks. What would you like to know about it?"}
  ]}}
```

- Entries are oldest first, and each has a `role` of `user` or `assistant` and a non-empty `text`.
- The gateway keeps at most the newest **40** entries and **24,000** characters in total. The
  oldest entries are dropped first. If the newest entry alone is longer than that, only its last
  24,000 characters are kept.
- The entries go in front of the `input` of the session's **first** agent run, as message items
  (`{"type": "message", "role": …, "content": …}`). budprompt stores them in the voice conversation
  (`conv_voice_<session>`), so later runs already have them. If the first run fails before it
  reaches the agent, the next run carries them.
- A later `bud_context` before the first turn replaces the earlier one. `null` changes nothing.

Errors are normal realtime `error` events (`type: invalid_request_error`,
`param: session.bud_context`). The rest of the `session.update` still applies, and the session goes
on:

| `code` | When |
|---|---|
| `invalid_value` | not a list, a role other than `user`/`assistant`, or a missing, empty or non-string `text`. Nothing of the list is used. |
| `bud_context_too_late` | the first agent turn has already started (interaction mode: the agent has answered the caller's first turn) |
| `bud_context_unsupported` | the session has no Bud agent conversation to seed: an alias session (`model=<alias>`), or interaction mode whose tasks run on something other than the Bud agent (LLM tier, a catalog backend) |

There is one race. If the client's `bud_context` is sent while the first turn is already starting,
the GA layer cannot refuse it in time, and the engine does. The client then gets an `error` with
code `bud_context_too_late` and no `param`.

**Interaction mode (`interaction: true`).** The Bud agent that does the session's tasks takes the
context. It goes in front of the `input` of the agent's first budprompt run, with the same
validation and limits, so budprompt stores it in the session's conversation. Later runs continue
that conversation. A run that never reaches budprompt passes the context on to the next run. The
agent's tasks run side by side (`background.max_parallel`), so while the context is on its way the
other runs wait until budprompt has accepted the run that carries it (`response.created`), or until
that run failed before it got there; with no context pending, runs start at once.
"Too late" here means one of two things. Either the agent has said the first line that answers a
caller turn (the GA layer refuses with `param`), or an agent run has already been sent (the agent
refuses, with no `param`).

The front model, which picks each turn's act and says the quick lines, is **not** seeded. It
keeps no conversation history. Each decision is built from scratch: the persona's prompt, the
caller's words, the agent lines said since the front was last asked, and the live tasks. So it has
no history to put the context into. The context reaches the caller's ears only through the agent's
answers.

On a session with a vendor realtime model (not `prompt:`), `bud_context` is removed from
`session.update` before the update goes to the vendor.

## Client secrets for a voice agent (`ek_bud_…`)

A browser should not hold a Bud credential. Instead, its server mints a short-lived client secret
for the agent, and the browser connects with that secret.

**Mint** (server side, with the Bud credential in `Authorization: Bearer`):

```http
POST /v1/realtime/client_secrets
Authorization: Bearer <Bud API key | widget bearer | access token>

{"expires_after": {"anchor": "created_at", "seconds": 600},
 "session": {"model": "prompt:voice-support"}}
```

```json
{"value": "ek_bud_k1.…", "expires_at": 1767312000, "session": {"model": "prompt:voice-support"}}
```

- `session.model` is `prompt:<name>` or `prompt:<name>:v<n>`. The mint checks it exactly as the
  agent handshake does: the credential must reach the agent's alias, and the agent must have a
  voice. Otherwise the answer is `403 model_not_allowed` (`param: session.model`).
- `seconds` is 10 to 7200 (default 600). `expires_at` is capped by the minting credential's own
  expiry: a JWT's `exp`. A JWT with under 10 s left gets `401 credential_expiring`. An API key or
  a widget bearer reports no expiry to the gateway, so its secret is not capped. When a widget
  bearer lapses, its Redis entry expires, and the gateway treats that as a revocation (see below).
- **What is sealed.** The claims are `{v, iat, exp, ep, alias, parent, pid, uid, akid, cred}`,
  encrypted with XChaCha20-Poly1305 under `WAAV_CLIENT_SECRET_KEYS`:
  - `alias` is the requested model, and `ep` is the agent's alias.
  - `parent` is the snapshot hash of an API key or widget bearer, or the JWT's `sub`.
  - `cred` is the minting credential itself. Only an agent's secret carries `cred`; a
    deployment's secret never does.
  - The holder cannot read any of it, and the gateway stores nothing.
- **Widget bearers.** budapp's widget bearer (`secrets.token_urlsafe(32)`, registered as
  `api_key:{sha256("bud-"+bearer)}` with a Redis TTL) authenticates as an API-key parent.
  `bud-auth` recognizes it from the hydrated snapshot (it has no `bud_` prefix, so a snapshot
  miss is not looked up in Redis). Renewing the widget extends the same bearer, so a sealed
  bearer stays valid as long as the widget session does.

**Connect** with the secret as the subprotocol (`Sec-WebSocket-Protocol: realtime,
openai-insecure-api-key.ek_bud_…`), with `?model=` set to the same model it was minted for.

- A different model (another agent, or a deployment) gets `403 model_mismatch`. A deployment's
  secret used on `prompt:` also gets `403 model_mismatch`.
- The sealed parent is authenticated again when the client connects. A revoked key, a removed
  user, or an expired token gets `401 invalid_client_secret`, and an expired secret gets
  `401 client_secret_expired`.
- The session then runs exactly as if the parent had connected: every budgateway/budprompt call
  of the agent carries the parent credential, and usage is attributed to it.

**Refresh during a call.** A secret is checked only when the client connects. The credential it
carries, though, must stay valid for the whole call. A JWT parent expires after minutes, and a
widget bearer lapses if it is not renewed. To keep the call going, the browser gets a fresh secret
from its server (same agent, same parent) and sends:

```json
{"type": "bud.session.auth", "token": "ek_bud_k1.…"}
```

- Success: `{"type": "bud.session.authenticated"}`. From the next request on, the agent's turns
  carry the parent credential sealed in the new secret.
- Failure: a realtime `error` event. The session goes on with its current credential:
  - `code: "auth_refresh_refused"` means the secret was minted for another model, or by another
    parent (a refresh can never change who the session acts as).
  - `code: "auth_refresh_failed"` means the secret is expired or invalid, or its parent is no
    longer valid.
- A session that a raw credential opened can be refreshed the same way, with the same `{token}`
  holding an API key or JWT of the same identity.
