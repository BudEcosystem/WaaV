# Interaction wire events: the task-event mapping and the one wire-names table

This is the readable copy of the wire-names table that `INTEGRATION_DECISIONS.md` section 4 asks workstream 6 to
publish and every design to cite. W1 to W7 below name the seven workstreams, one per plan chapter: W1 runtime, W2 front
model, W3 background tasks, W4 policy and human in the loop, W5 duplex audio, W6 providers and endpoints, W7
evaluation and rollout. It restates `W6-providers-endpoints.md` sections 3.12.1 to 3.12.4 (bodies, the surfaces, the
task-event mapping and the wire-names table) with field types and one example per event, using the names the decisions
and their Addenda A to D fix: workstream 6's event names, workstream 3's task states with a `cancelling`
flag, `effects` on results and cancellations, workstream 4's question kinds and outcomes, only workstream 2's ten act
names in `act`, and workstream 3's `AnswerSentence` for the streamed answer. The golden test
`wire_names_match_contract_golden` (Release 0, in `core/interaction/contracts.rs`) pins this table and the mapping.

Every JSON block below is checked by `python3 regime_resolve.py --validate`: it must parse, every event in the table
must have an example, and every state, effects, `by`, `outcome`, question kind, act, utterance kind, transcript
source, floor reason, answer-stop reason and overlap class must come from the shared vocabularies. The checker also
enforces two rules of the mapping: `task_cancelled` only with `state: "cancelled"` and `cancelling: false`, and
`cancelling: true` only with a live state.

## How to read it

- **Protocol version 1.1.** `/ws` `ready.protocol_version` moves from `"1.0"` (`handlers/ws/messages.rs:38`) to
  `"1.1"` for every session in Release 1, and `/realtime` `session_created` gains the same field. This is an
  owner-approved interaction-off change (W6 section 9.2, sign-off 9); older TypeScript SDKs log one drift warning.
- **Sent only when interaction is on.** With no `interaction` block nothing in this file is sent, except the
  Lifecycle-track `config_warning` codes `provider_sunset` and `model_retiring` (and `/realtime` `warning`), which apply
  to every session that names a retiring provider or model.
- **Same body on every surface.** `/ws` (conversation, agent, raw and DAG sessions) and `/realtime` use the names in
  the second and third columns. LiveKit rooms receive the same JSON on the data topic `waav.interaction` through the
  existing send-message operation (`livekit/client/messaging.rs:17`). SIP calls have no screen: nothing is sent to the
  caller, questions are voice-only, and from Release 4 DTMF answers appear as `by: "dtmf"`.
- **`/v1/realtime`.** The same bodies with `type` renamed to the `bud.*` name; like the GA cascade's existing
  `bud.*` extras they also carry `event_id` (the cascade's `event()` helper adds `type` and `event_id`,
  `handlers/openai_realtime/cascade.rs:200-208`). Client `bud.*` events are intercepted before the relay policy so
  they never reach the vendor.
- **Times and ids.** Every event carries `t_ms`, milliseconds on the session timeline (workstream 1). `task_id` is
  workstream 3's gateway-minted `TaskId` ("task_" plus 32 hex characters); `task_ref` is the session alias ("t1", "t2",
  never reused) that the caller and a provider hear; `request_id` is a workstream 4 `HitlId`; `utterance_id` is a
  workstream 4 `Utterance` id. Examples shorten ids.
- **Provisional runs** (a background run started at E, at an eager end of turn, or on a provider's speculative tool
  call, before anyone has decided the turn is a task; workstream 3's `start_provisional` with origin `FrontAtE`,
  `EagerEndOfTurn` or `ProviderCall { adopt_by }`, Addendum C2.1) emit nothing. `task_created` is sent at adoption,
  with `speculative: true`; a discarded run only increments the metric `waav_provisional_discarded_total`.
- **Streamed answers.** Each sentence of the background's final answer (workstream 3's `AnswerSentence`) is spoken
  through workstream 1's one open relay utterance and appears as one `assistant_transcript` with
  `sentence: {attempt, seq}` when it is played; there is no task event per sentence. A Bud agent streams only when
  `background.answer_marker` is declared; otherwise its answer is relayed whole at completion, about 0.4 to 1 s later
  than today's agent path (Addenda C1.4 and D5). The marker comes from budapp's `VoiceAgentEntry.background` (field
  requested for 2026-11-02) or, for an agent the operator knows emits it, from `background.answer_marker` in the
  server's `interaction.defaults.agent` or in an agent alias's `interaction`; budapp's field wins. The echo reports
  the resolved rule and the marker's source (`tasks.answer_stream`, `tasks.answer_marker_source`), so workstream 7
  applies the time-to-answer gate only to agents with a marker and reports the others against the agent-path baseline.
- **Errors.** `/realtime` `error` already has an optional `code` (`handlers/realtime/messages.rs:313-319`). The `/ws`
  `error` has only `message` today (`handlers/ws/messages.rs:372-376`); Release 1 adds the same optional, additive
  `code` (Addendum A2 item 9), set only on the new interaction errors, so existing errors serialise unchanged.
- **Delivery class** on `/ws` egress (`handlers/ws/messages.rs` send classes): *critical* events are never dropped;
  *coalescing* events keep only the latest value per key under back-pressure.

## The table

| Concept | `/ws` and LiveKit (`waav.interaction`) | `/realtime` | `/v1/realtime` | Direction | Payload from | Replaces (never use) | Delivery | First release |
|---|---|---|---|---|---|---|---|---|
| task created (at adoption) | `task_created` | same | `bud.task.created` | out | W3 `TaskEventKind::Created` | `task_status`, `task_event` | critical | 1 on `/ws`; 2 on the GA cascade; 3 on `/realtime`, relay and facade |
| task state or progress | `task_progress` | same | `bud.task.progress` | out | W3 `Queued`, `StillQueued`, `Working`, `Progress`, `InputAdded`, `Revised`, `Retried`, `Superseded`, `EffectsUpdated`, `AnswerStopped`, `DeadlineReached`, `Orphaned`, `Resumed`, and `CancelRequested` and `CancelUnconfirmed` (with `cancelling: true`, state unchanged; Addendum C4) | `task_status` | coalescing per task | 1 |
| question asked | `task_question` | same | `bud.task.question` | out | W4 `Signal(QuestionQueued)` for W3 `InputRequired`, `AuthRequired`, `CancelNeedsConfirmation`; `heard` updated by `Signal(QuestionHeard)` | `hitl_request` | critical | 1 |
| answer received | `task_answer_received` | same | `bud.task.answer_received` | out | W4 `Signal(QuestionResolved { by })` (W3 `InputResolved`) | `hitl_resolved` | critical | 1 |
| result | `task_result` | same | `bud.task.result` | out | W3 `Completed`, `Failed`, `Rejected`, `Unknown`, and `StaleOutcome` (with `superseded: true`; Addendum C4) | `task_status` | critical | 1 |
| cancelled (confirmed) | `task_cancelled` | same | `bud.task.cancelled` | out | W3 `Cancelled` only | `task_status` | critical | 1 |
| floor | `floor_state` | same | `bud.floor.state` | out | W1 `FloorChanged` (reasons include `stream_gap`) | `interaction_state` | coalescing per session | 1 |
| provider claim | `unverified_claim` | same | `bud.claim.unverified` | out | W6 `ClaimMonitor` | none | critical | 3 |
| agent audio stopped | `agent_audio_stopped` | same | `bud.audio.stopped` | out | W1 `PlaybackEvent::Stopped` | none | critical | 1 |
| segment marker | `audio_segment` | not sent | consumed by the GA cascade as W1's response opener | out | W1 lane executor | none | normal | 1 |
| playback control (opt-in `events.playback_control`) | `playback_control` | same | `bud.playback.control` | out | W5 gate commands | none | normal | 1 |
| overlap (opt-in `events.overlap`) | `overlap` | same | `bud.overlap` | out | W5 overlap notes | none | normal | 1 |
| interaction echo | `ready.interaction` | `session_created.interaction` | `bud.interaction.state` (after `session.created`) | out | W6, with W2 `FrontEcho`, W4 `policy_hash` and effective `PolicyConfig`, W5 barge-in values | none | critical | 1 |
| update applied | `interaction_updated` | same | `bud.interaction.updated` | out | W6 | none | critical | 1 |
| warning | `config_warning` (existing) | `warning` (new) | `bud.warning` | out | W6 codes (table below) | none | critical | Lifecycle track (lifecycle codes); 1 |
| error with a code | `error` gains optional `code` (for example `question_closed`) | `error` (already has `code`) | GA `error` event with `error.code` | out | W6 | none | critical | 1 |
| agent speech text | `assistant_transcript` with `source`, `kind`, `verbatim`, `act`, `task_id`, `utterance_id`, `sentence`, `heard` | `transcript` (assistant) with the same optional fields | GA transcript events; `metadata.bud_*` on gateway-initiated responses | out | W4 `Utterance` (a streamed answer is one open relay item, one transcript per relayed sentence), W2 act, W3 `AnswerSentence` | W4's three-field draft; `act: "ack"`; `chunk` | normal | 1 |
| provider call withdrawn | not sent | `function_call_cancelled` | not sent | out | Gemini `toolCallCancellation`, Deepgram `FunctionCallCancelled` | none | normal | Lifecycle track (wire-fix patch) |
| answer a question (client) | `task_answer` | same | `bud.task.answer` | in | to W4 `HitlClientAnswer` | `hitl_response` | n/a | 1 |
| cancel a task (client) | `task_cancel` | same | `bud.task.cancel` | in | to W1 `ClientCancelTask` | none | n/a | 1 |
| change settings (client) | `interaction_update` | same | `bud.interaction.update` | in | W6 | `persona.preferences` patches | n/a | 1 |
| playback heartbeat (client, at most 4 per second) | `playback_position` | not accepted | `conversation.item.truncate` as today | in | to W1 heard time | none | n/a | 2 |
| confirmation mirror (opt-in `events.ga_mcp_approvals`) | not sent | not sent | GA `mcp_approval_request` item (`server_label: "bud"`); the client's `mcp_approval_response` item is read as `task_answer` | both | W4 `confirm` question | none | critical | 2 |

## The task-event mapping

W6 section 3.12.4 maps every workstream 3 `TaskEventKind` (and workstream 4's question signals) onto the client
events above; this is its short form. `revision` is the task's current revision unless noted. Provisional runs
publish no task events at all.

| Workstream 3 event | Client event | `state` on the wire | Other fields |
|---|---|---|---|
| `Created { speculative }` | `task_created` | `submitted` or `working` | `speculative`, `origin`, `side_effects`, `background` |
| `Queued`, `StillQueued` | `task_progress` | `submitted` | `text` names what it waits behind |
| `Working`, `Progress`, `Retried`, `Resumed` | `task_progress` | `working` | `text` only after the outbound speech policy; `fraction`; `spoken` from W4's `Delivered` signal |
| `InputAdded`, `Revised`, `Superseded`, `Orphaned`, `DeadlineReached` | `task_progress` | unchanged | `Revised` raises `revision`; a superseded result of a run with no side effects is not shown |
| `EffectsUpdated` | `task_progress` | unchanged | `effects` refreshed |
| `AnswerSentence` | none; one `assistant_transcript` per relayed sentence when it is played | n/a | `source: background`, `kind: relay`, `verbatim: true`, `sentence: {attempt, seq}` |
| `AnswerStopped` | `task_progress` | unchanged | `answer_stopped: {reason, released_sentences}` |
| `InputRequired`, `AuthRequired`, `CancelNeedsConfirmation` | `task_question` (from W4's `QuestionQueued`, with the rendered words) | `input_required`, `auth_required`, or unchanged for a cancel confirmation | `heard` follows W4's `QuestionHeard` |
| `InputResolved` | `task_answer_received` (from W4's `QuestionResolved { by }`) | the state after resolution | `outcome`, `by`, `corroborated`, `value_redacted` |
| `CancelRequested` | `task_progress` | unchanged | `cancelling: true` |
| `CancelUnconfirmed` | `task_progress` | unchanged | `cancelling: true`, `effects`; never `task_cancelled`, never `unknown` (Addendum C4) |
| `Cancelled` | `task_cancelled` | `cancelled` | `cancelling: false`, `effects`, `by`, `reason` |
| `Completed` | `task_result` | `completed` | `effects`, `streamed`, `spoken` |
| `Failed` | `task_result` | `failed` | `effects` (a failed run may have committed something) |
| `Rejected` | `task_result` | `rejected` | `effects: "none_committed"` |
| `Unknown` | `task_result` | `unknown` | `effects: "unknown"` |
| `StaleOutcome` | `task_result` | the stale run's outcome | `superseded: true`, `revision` of the stale run, its `effects`; the task itself goes on |
| `Partial` | not sent (timeline only) | | |

## Shared value sets

| Field | Values | Owner |
|---|---|---|
| `state` (every task event) | `submitted`, `working`, `input_required`, `auth_required`, `completed`, `failed`, `cancelled`, `rejected`, `unknown` | W3 |
| `cancelling` (every task event) | `true` while a cancel is in flight, else `false` | W3 |
| `effects` (`task_result`, `task_cancelled`, `task_progress`) | `"none_committed"`, a list of committed-effect labels such as `["table hold at Rosa's"]`, or `"unknown"`. A task that committed anything, or may have, is never shown only as cancelled: a screen shows "stopped, but ..." for any value other than `none_committed`. An unconfirmed cancel is `task_progress` with `cancelling: true` and the state unchanged until a `task_cancelled` or a `task_result` follows | W3 |
| `outcome` | `accept`, `decline` (optional `correction`), `cancel`, `timeout`, `superseded`, `unanswerable` (with `reason`, for example `no_verbatim_channel`); a client may send only `accept`, `decline`, `cancel` | W4 |
| `by` (`task_answer_received`) | `voice`, `dtmf` (Release 4), `client`, `timeout`, `session_end` | W4 |
| `kind` (`task_question`) | `confirm`, `choose`, `value`, `form`, `auth_url`, `offer` | W4 |
| `act` (`assistant_transcript`, only on lines a front model wrote) | `wait`, `backchannel`, `delegate`, `update`, `abort`, `social`, `echo`, `progress`, `question`, `refuse` | W2 |
| `kind` (`assistant_transcript`) | `ack`, `answer`, `progress`, `hold`, `choice_offer`, `bridge`, `relay`, `question`, `confirm`, `notice`, `offer`, `checkin`, `followup`, `greeting` | W4 |
| `source` (`assistant_transcript`) | `front`, `background`, `gateway`, `provider`, `gateway_clip` | W6 |
| `heard` | on transcripts `full`, `partial`, `none`; on questions `verified`, `unverified`, `pending` | W1, W6 |
| `floor_state.holder` / `.lane` / `.reason` | `user`, `assistant`, `both`, `none` / `front`, `main`, `backchannel`, `provider`, `null` / `user_speech`, `front_ack`, `relay`, `hold`, `idle`, `reconnect`, `stream_gap` (a streamed relay parked after `runtime.stream_gap_hold_ms`) | W1 |
| `answer_stopped.reason` | `revised`, `superseded`, `cancelling`, `speech_policy`, `marker_then_tool_call`, `regenerating`, `speakable_cap`, `run_failed` | W3 |
| `sentence` (`assistant_transcript`) | `{attempt, seq}`: W3's `AnswerSentence` attempt and sentence number (from 0) | W3 |
| `unverified_claim.kind` | `completion`, `entity`, `outcome`, `nonce_leak` | W6 (detector), W4 (correction words) |
| `agent_audio_stopped.reason` | `barge_in`, `client_clear`, `policy`, `provider` | W1 |
| `overlap.class` | `interrupt`, `backchannel`, `talking_to_others`, `background`, `noise`, `echo`, `null` | W5 |
| `task_created.origin` | `front`, `provider_tool`, `delegation`, `client` | W6 |
| `task_cancelled.by` | `user`, `client`, `system`, `provider` | W3 |

## Server to client

### `task_created` / `bud.task.created`

| Field | Type | Meaning |
|---|---|---|
| `task_id` | string | W3 `TaskId` |
| `task_ref` | string | session alias ("t1") |
| `revision` | integer | W3 `Revision`, starts at 1 |
| `state` | string | task state (usually `working` or `submitted`) |
| `cancelling` | boolean | cancel in flight |
| `title` | string | short task title |
| `kind` | string | W3 task kind (`lookup`, `research`, `action`) |
| `side_effects` | boolean | the task may commit something |
| `speculative` | boolean | true when the task was adopted from a provisional run |
| `origin` | string | `front`, `provider_tool`, `delegation`, `client` |
| `background` | string | the backend that runs it, by name (`agent:support`, `llm`, `crm`) |
| `t_ms` | integer | timeline offset |

```json
{"type": "task_created", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "working", "cancelling": false, "title": "your table booking", "kind": "action", "side_effects": true, "speculative": true, "origin": "front", "background": "agent:support", "t_ms": 4180}
```

```json
{"type": "bud.task.created", "event_id": "evt_9001", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "working", "cancelling": false, "title": "your table booking", "kind": "action", "side_effects": true, "speculative": false, "origin": "delegation", "background": "llm", "t_ms": 4180}
```

### `task_progress` / `bud.task.progress`

| Field | Type | Meaning |
|---|---|---|
| `task_id`, `task_ref`, `revision`, `state`, `cancelling`, `t_ms` | as above | `cancelling: true` with the state unchanged while a cancel is requested or unconfirmed |
| `seq` | integer | increases per task |
| `text` | string or null | progress text (data, never instructions) |
| `fraction` | number or null | 0 to 1 when the backend reports one |
| `spoken` | boolean | whether W4 also said it |
| `effects` | effects or null | set when committed effects change (`EffectsUpdated`) |
| `answer_stopped` | object or null | `{reason, released_sentences}` when a streamed answer stopped at a sentence boundary (W3 `AnswerStopped`) |

```json
{"type": "task_progress", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "working", "cancelling": false, "seq": 3, "text": "Checking availability at Rosa's", "fraction": null, "spoken": true, "effects": null, "answer_stopped": null, "t_ms": 7020}
```

```json
{"type": "bud.task.progress", "event_id": "evt_9002", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "working", "cancelling": false, "seq": 4, "text": null, "fraction": null, "spoken": false, "effects": null, "answer_stopped": {"reason": "revised", "released_sentences": 1}, "t_ms": 8350}
```

An unconfirmed cancel (W3 `CancelUnconfirmed`): the state stays `working`, `cancelling` stays `true`, and `effects`
says what may already have happened. W4 says the honest line ("I've asked it to stop; I'll let you know when it's
confirmed").

```json
{"type": "task_progress", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "working", "cancelling": true, "seq": 6, "text": null, "fraction": null, "spoken": true, "effects": "unknown", "answer_stopped": null, "t_ms": 25200}
```

### `task_question` / `bud.task.question`

| Field | Type | Meaning |
|---|---|---|
| `task_id`, `task_ref`, `state` (`input_required` or `auth_required`), `cancelling`, `t_ms` | as above | |
| `request_id` | string | W4 `HitlId` |
| `kind` | string | `confirm`, `choose`, `value`, `form`, `auth_url`, `offer` |
| `prompt` | string | the exact rendered words |
| `options` | array or null | for `choose` |
| `schema` | object or null | for `value` and `form` |
| `sensitive` | boolean | the answer must not be spoken back or sent (`value_redacted`) |
| `irreversible` | boolean | binds only from a high-attribution turn and a strong yes |
| `channels` | array of strings | `voice`, `client`, `dtmf` |
| `expires_at_ms` | integer | timeline offset |
| `spoken` | boolean | asked aloud |
| `heard` | string | `verified`, `unverified`, `pending` |

```json
{"type": "task_question", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "input_required", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "kind": "confirm", "prompt": "Shall I book a table for 4 at Rosa's on Saturday at 7 pm?", "options": null, "schema": null, "sensitive": false, "irreversible": false, "channels": ["voice", "client"], "expires_at_ms": 39500, "spoken": true, "heard": "pending", "t_ms": 9500}
```

```json
{"type": "bud.task.question", "event_id": "evt_9003", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "input_required", "cancelling": false, "request_id": "q_task_5b1e9c0d_2", "kind": "choose", "prompt": "Would you like 7 pm or 8 pm?", "options": ["7 pm", "8 pm"], "schema": null, "sensitive": false, "irreversible": false, "channels": ["voice", "client"], "expires_at_ms": 61000, "spoken": true, "heard": "verified", "t_ms": 31000}
```

### `task_answer_received` / `bud.task.answer_received`

| Field | Type | Meaning |
|---|---|---|
| `task_id`, `task_ref`, `state`, `cancelling`, `t_ms` | as above | |
| `request_id` | string | the question answered |
| `outcome` | string | W4 outcome |
| `reason` | string or null | with `unanswerable` (for example `no_verbatim_channel`) |
| `correction` | string or null | with `decline` |
| `by` | string | `voice`, `dtmf`, `client`, `timeout`, `session_end` |
| `corroborated` | boolean | in regimes B and C, a provider answer signal agreed with the transcript |
| `value` | any or null | never sent for a sensitive question |
| `value_redacted` | boolean | true when `value` was withheld |

```json
{"type": "task_answer_received", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "working", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "outcome": "accept", "reason": null, "correction": null, "by": "voice", "corroborated": true, "value": null, "value_redacted": false, "t_ms": 11240}
```

```json
{"type": "bud.task.answer_received", "event_id": "evt_9004", "task_id": "task_5b1e9c0d", "task_ref": "t1", "state": "working", "cancelling": false, "request_id": "q_task_5b1e9c0d_1", "outcome": "decline", "reason": null, "correction": "make it Tuesday", "by": "voice", "corroborated": false, "value": null, "value_redacted": false, "t_ms": 12400}
```

### `task_result` / `bud.task.result`

| Field | Type | Meaning |
|---|---|---|
| `task_id`, `task_ref`, `revision`, `cancelling`, `t_ms` | as above | |
| `state` | string | `completed`, `failed`, `rejected`, `unknown` |
| `status` | string | same value as `state`, kept for readers of the earlier draft |
| `summary` | string | display summary (the verbatim relay goes through `assistant_transcript`) |
| `effects` | effects | what was committed |
| `spoken` | string | `full`, `partial`, `none` |
| `streamed` | boolean | relayed sentence by sentence (decisions section 3.2) |
| `superseded` | boolean | a newer revision replaced this result; true for W3's `StaleOutcome`, which reports what a superseded run with side effects did (`revision` is then the stale run's) |

```json
{"type": "task_result", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "completed", "status": "completed", "cancelling": false, "summary": "Booked: table for 4, Saturday 19:00, confirmation R-5512", "effects": ["table booking at Rosa's"], "spoken": "full", "streamed": true, "superseded": false, "t_ms": 13900}
```

```json
{"type": "bud.task.result", "event_id": "evt_9005", "task_id": "task_77aa01fe", "task_ref": "t2", "revision": 1, "state": "unknown", "status": "unknown", "cancelling": false, "summary": "The booking service stopped answering; the outcome is not known", "effects": "unknown", "spoken": "full", "streamed": false, "superseded": false, "t_ms": 64000}
```

A stale outcome (W3 `StaleOutcome`): revision 1 booked a table for four before the caller changed it to five; the
task goes on with revision 2.

```json
{"type": "task_result", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 1, "state": "completed", "status": "completed", "cancelling": false, "summary": "The earlier booking for 4 had already gone through", "effects": ["table booking for 4 at Rosa's"], "spoken": "full", "streamed": false, "superseded": true, "t_ms": 14800}
```

### `task_cancelled` / `bud.task.cancelled`

| Field | Type | Meaning |
|---|---|---|
| `task_id`, `task_ref`, `revision`, `t_ms` | as above | |
| `cancelling` | boolean | always `false`: this event is sent only for W3's confirmed `Cancelled` |
| `state` | string | always `cancelled`; an unconfirmed cancel is never this event (it is `task_progress` with `cancelling: true`, Addendum C4) |
| `by` | string | `user`, `client`, `system`, `provider` |
| `reason` | string | for example `user_cancelled` |
| `effects` | effects | what had already been committed (`none_committed`, a list, or `unknown`); anything but `none_committed` is shown as "stopped, but ..." |

```json
{"type": "task_cancelled", "task_id": "task_5b1e9c0d", "task_ref": "t1", "revision": 2, "state": "cancelled", "cancelling": false, "by": "user", "reason": "user_cancelled", "effects": ["table hold at Rosa's"], "t_ms": 15000}
```

```json
{"type": "bud.task.cancelled", "event_id": "evt_9006", "task_id": "task_77aa01fe", "task_ref": "t2", "revision": 1, "state": "cancelled", "cancelling": false, "by": "client", "reason": "client_cancelled", "effects": "none_committed", "t_ms": 15200}
```

### `floor_state` / `bud.floor.state`

| Field | Type | Meaning |
|---|---|---|
| `holder` | string | `user`, `assistant`, `both`, `none` |
| `lane` | string or null | `front`, `main`, `backchannel`, `provider` |
| `reason` | string | `user_speech`, `front_ack`, `relay`, `hold`, `idle`, `reconnect`, `stream_gap` |
| `t_ms` | integer | |

```json
{"type": "floor_state", "holder": "assistant", "lane": "front", "reason": "front_ack", "t_ms": 11010}
```

A streamed answer whose next sentence is late: after `runtime.stream_gap_hold_ms` (600 ms) workstream 1 parks the
relay and the floor is free again until the next sentence arrives.

```json
{"type": "floor_state", "holder": "none", "lane": null, "reason": "stream_gap", "t_ms": 13240}
```

```json
{"type": "bud.floor.state", "event_id": "evt_9007", "holder": "user", "lane": null, "reason": "user_speech", "t_ms": 11420}
```

### `unverified_claim` / `bud.claim.unverified` (Release 3, regimes B and C)

| Field | Type | Meaning |
|---|---|---|
| `kind` | string | `completion`, `entity`, `outcome`, `nonce_leak` |
| `rule` | string | the detector rule that fired (for example `CompletionClaim`) |
| `text` | string | the provider's words |
| `corrected` | boolean | W4 scheduled a correction |
| `output_stopped` | boolean | the claim monitor stopped provider output before the words played |
| `t_ms` | integer | |

```json
{"type": "unverified_claim", "kind": "completion", "rule": "CompletionClaim", "text": "your refund was processed", "corrected": true, "output_stopped": true, "t_ms": 12500}
```

```json
{"type": "bud.claim.unverified", "event_id": "evt_9008", "kind": "entity", "rule": "NewMoney", "text": "40 dollars", "corrected": false, "output_stopped": false, "t_ms": 12510}
```

### `agent_audio_stopped` / `bud.audio.stopped`

| Field | Type | Meaning |
|---|---|---|
| `utterance_ids` | array of strings | the utterances stopped (stops are by utterance id, never "all queued") |
| `reason` | string | `barge_in`, `client_clear`, `policy`, `provider` |
| `delivered_ms` | array of integers | audio delivered per utterance before the stop |
| `t_ms` | integer | |

```json
{"type": "agent_audio_stopped", "utterance_ids": ["u_41"], "reason": "barge_in", "delivered_ms": [1840], "t_ms": 12620}
```

```json
{"type": "bud.audio.stopped", "event_id": "evt_9009", "utterance_ids": ["u_44", "u_45"], "reason": "policy", "delivered_ms": [600, 0], "t_ms": 20400}
```

### `audio_segment` (not on `/realtime`; consumed by the GA cascade)

| Field | Type | Meaning |
|---|---|---|
| `phase` | string | `start`, `end` |
| `utterance_id` | string | |
| `lane` | string | `front`, `main`, `backchannel` |
| `kind` | string | W4 utterance kind |
| `task_id` | string or null | |
| `t_ms` | integer | |

```json
{"type": "audio_segment", "phase": "start", "utterance_id": "u_41", "lane": "front", "kind": "ack", "task_id": null, "t_ms": 12100}
```

### `playback_control` / `bud.playback.control` (opt-in)

| Field | Type | Meaning |
|---|---|---|
| `action` | string | `duck`, `pause`, `resume`, `clear` |
| `lane` | string | |
| `gain_db` | number or null | for `duck` (default depth −15 dB, sign-off 19) |
| `ramp_ms` | integer or null | |
| `utterance_id` | string or null | |
| `t_ms` | integer | |

```json
{"type": "playback_control", "action": "duck", "lane": "main", "gain_db": -15, "ramp_ms": 60, "utterance_id": "u_41", "t_ms": 12230}
```

```json
{"type": "bud.playback.control", "event_id": "evt_9010", "action": "resume", "lane": "main", "gain_db": null, "ramp_ms": 80, "utterance_id": "u_41", "t_ms": 12900}
```

### `overlap` / `bud.overlap` (opt-in)

| Field | Type | Meaning |
|---|---|---|
| `onset_id` | integer | W5 onset id |
| `phase` | string | `start`, `end` |
| `class` | string or null | W5 interruption class; null until decided |
| `t_ms` | integer | |

```json
{"type": "overlap", "onset_id": 17, "phase": "end", "class": "backchannel", "t_ms": 12210}
```

```json
{"type": "bud.overlap", "event_id": "evt_9011", "onset_id": 18, "phase": "start", "class": null, "t_ms": 13010}
```

### Interaction echo: `ready.interaction`, `session_created.interaction`, `bud.interaction.state`

The `/ws` `ready` message, the `/realtime` `session_created` message and, on `/v1/realtime`, a
`bud.interaction.state` event sent after `session.created` carry the resolved interaction settings (W6 section 3.10.5),
including, by Addendum C4, the hash and the effective values of workstream 4's `PolicyConfig` and workstream 5's
effective barge-in values. In Release 1 the front resolves to `scripted` on every deployment (Addendum B1), so a
cascade echo then shows `profile: "scripted"`, `placement: "none"` and `front_model_scripted` in `degraded`. An SDK
that sent `interaction` and gets `ready` without the echo is talking to an older gateway.

| Field | Type | Meaning |
|---|---|---|
| `enabled` | boolean | interaction mode is running |
| `protocol_version` | string | `"1.1"` |
| `regime` | string | `cascade`, `provider_speech`, `delegating`, `native_duplex` |
| `source` | array of strings | server sources merged, in order (`server_default`, `alias:<name>`, `agent_entry`, `deployment`) |
| `front` | object or string | W2's `FrontEcho` (`requested`, `profile` (`scripted`, `lfm-standard`, `lfm-large` or `apache-standard`), `licence` attestation kind only, `qualified`, `placement`, `say_mode`, `persona`) in the cascade and the half-cascade; `"narrator_templates"`, `"narrator_front_model"` or `"provider"` otherwise |
| `filler_owner`, `backchannel_owner`, `idle_owner`, `voice_owner` | string | resolved owners |
| `confirmations` | string | resolved confirmation path |
| `truth` | string | `enforced` or `monitored` |
| `tasks` | object | `{backend, speculative_start, answer_stream, answer_marker_source}` resolved for the session's backend. `answer_stream` is the resolved release rule (`marker`, `round_end`, `direct` or `whole`; W3 section 3.17.2). `answer_marker_source` is, for a Bud agent, where its `background.answer_marker` came from (`agent_entry`, `alias:<name>` or `server_default`; Addendum D5), else null |
| `verbs`, `activity` | object | provider sessions only: verb support levels and activity sources |
| `policy_hash` | string | hash of W4's effective `PolicyConfig` (W7 reads it) |
| `policy` | object | W4's effective `PolicyConfig` values after clamps, including `ack_due_ms` (0 allowed), `fast_answer_ms`, `ack_target_ms` (800), `ack_hard_deadline_ms`, `answer_window_ms` (4,000) and `agreed_wait_max_ms`; the examples abridge it |
| `barge_in` | object | W5's effective `duplex.barge_in` values (mode, duck depth and ramp, pause, decide and maximum pause, `duck_on_bare_onset`); the examples abridge it |
| `memory` | string | `off` or `on` |
| `degraded` | array of strings | warning codes that changed behaviour |

```json
{"type": "ready", "protocol_version": "1.1", "stream_id": "550e8400-e29b-41d4-a716-446655440000", "interaction": {"enabled": true, "protocol_version": "1.1", "regime": "cascade", "source": ["server_default", "alias:support-interactive"], "front": {"requested": "auto", "profile": "scripted", "licence": "lfm_open_eligible", "qualified": false, "placement": "none", "say_mode": "compose", "persona": "acme-support@3"}, "filler_owner": "gateway", "backchannel_owner": "gateway", "idle_owner": "gateway", "voice_owner": "gateway", "confirmations": "exact", "truth": "enforced", "tasks": {"backend": "agent:support", "speculative_start": "off", "answer_stream": "marker", "answer_marker_source": "server_default"}, "policy_hash": "sha256:9c1e", "policy": {"ack_due_ms": 300, "fast_answer_ms": 700, "ack_target_ms": 800, "ack_hard_deadline_ms": 2000, "answer_window_ms": 4000, "agreed_wait_max_ms": 600000}, "barge_in": {"mode": "two_stage", "duck_db": -15.0, "duck_ramp_ms": 60, "pause_after_ms": 400, "decide_by_ms": 900, "max_pause_ms": 2500, "duck_on_bare_onset": "auto"}, "memory": "off", "degraded": ["front_model_scripted"]}}
```

```json
{"type": "bud.interaction.state", "event_id": "evt_9012", "interaction": {"enabled": true, "protocol_version": "1.1", "regime": "delegating", "source": ["server_default", "deployment"], "front": "provider", "filler_owner": "provider", "backchannel_owner": "provider", "idle_owner": "provider", "voice_owner": "provider", "confirmations": "client_ui", "truth": "monitored", "tasks": {"backend": "llm", "speculative_start": "on", "answer_stream": "whole", "answer_marker_source": null}, "verbs": {"say_exact": "absent", "say_guided": "native", "add_context": "native", "stop_output": "emulated"}, "activity": {"user_speaking": "gateway_vad", "responding_end": "response_done"}, "policy_hash": "sha256:41ad", "policy": {"ack_due_ms": 300, "fast_answer_ms": 700, "ack_target_ms": 800, "ack_hard_deadline_ms": 2000, "answer_window_ms": 4000, "agreed_wait_max_ms": 600000}, "barge_in": {"mode": "two_stage", "duck_db": -15.0, "pause_after_ms": 400, "max_pause_ms": 2500}, "memory": "off", "degraded": []}}
```

### `interaction_updated` / `bud.interaction.updated`

| Field | Type | Meaning |
|---|---|---|
| `applied` | array of strings | dotted paths applied |
| `refused` | array of objects | `{path, reason}`; never mid-session: `regime`, owners, `speech`, `front.profile`, `front.persona`, `front.caller.language`, `front.caller.identity_token`, `tasks`; in Bud mode also any path the agent's `session_overrides` does not list |

```json
{"type": "interaction_updated", "applied": ["front.caller.verbosity"], "refused": [{"path": "regime", "reason": "not_mid_session"}]}
```

```json
{"type": "bud.interaction.updated", "event_id": "evt_9013", "applied": ["events.floor"], "refused": []}
```

### `config_warning` / `warning` (on `/realtime`) / `bud.warning`

The existing `/ws` shape `{code, message, detail}` (`handlers/ws/messages.rs:383-393`). Codes and whether the session
still runs are in the last section.

```json
{"type": "config_warning", "code": "truth_ack_required", "message": "Interaction mode needs the operator to accept monitored truth for gemini; the session runs without interaction mode.", "detail": {"provider": "gemini", "model": "gemini-3.8-live", "path": "interaction.speech.truth_ack", "alternative": null}}
```

```json
{"type": "warning", "code": "model_retiring", "message": "gpt-realtime shuts down on 2027-01-20; use gpt-realtime-2.1.", "detail": {"shutdown": "2027-01-20", "replacement": "gpt-realtime-2.1"}}
```

```json
{"type": "bud.warning", "event_id": "evt_9014", "code": "filler_owner_overridden", "message": "Gemini 3.8 Live's proactive audio cannot be switched off; the provider keeps fillers.", "detail": {"requested": "gateway", "effective": "provider"}}
```

### `assistant_transcript` (extended) / `/realtime` `transcript`

The mode-off shape `{type, turn_index, delta}` (`handlers/ws/messages.rs:438-439`) is unchanged; the new fields are
optional and omitted when absent.

| Field | Type | Meaning |
|---|---|---|
| `turn_index` | integer | existing |
| `delta` | string | existing: the words |
| `source` | string | `front`, `background`, `gateway`, `provider`, `gateway_clip` |
| `kind` | string | W4 utterance kind |
| `verbatim` | boolean | true when the words are the background's own (relay) |
| `act` | string | W2 act; only when `source` is `front` |
| `task_id` | string | |
| `utterance_id` | string | |
| `sentence` | object | `{attempt, seq}` for each relayed sentence of a streamed answer (W3 `AnswerSentence`; replaces the earlier `chunk {attempt, index}`) |
| `heard` | string | `full`, `partial`, `none` |

```json
{"type": "assistant_transcript", "turn_index": 3, "delta": "Booked: a table for 4 at Rosa's on Saturday at 7 pm.", "source": "background", "kind": "relay", "verbatim": true, "task_id": "task_5b1e9c0d", "utterance_id": "u_42", "sentence": {"attempt": 0, "seq": 0}, "heard": "full"}
```

```json
{"type": "assistant_transcript", "turn_index": 4, "delta": "Still on it.", "source": "front", "kind": "progress", "verbatim": false, "act": "progress", "task_id": "task_5b1e9c0d", "utterance_id": "u_43", "heard": "full"}
```

```json
{"type": "transcript", "role": "assistant", "text": "Let me look up order 4411.", "source": "gateway", "kind": "ack", "verbatim": false, "utterance_id": "u_7", "heard": "full"}
```

### `function_call_cancelled` (`/realtime` only)

```json
{"type": "function_call_cancelled", "call_ids": ["c1"]}
```

## Client to gateway

A late answer to a closed question gets `error` with `code: "question_closed"`; request ids are checked against the
session's open questions (an unknown one gets `unknown_request_id`, an unknown task in `task_cancel` gets
`task_unknown`) and the first answer wins whatever its channel. The `/ws` `error` gains the optional `code` in Release 1
(Addendum A2 item 9); on `/v1/realtime` the same code travels in the GA `error` event's `error.code`.

### `error` (existing; optional `code` on `/ws` from Release 1)

| Field | Type | Meaning |
|---|---|---|
| `message` | string | existing |
| `code` | string, optional | set only on the new interaction errors (`question_closed`, `unknown_request_id`, `task_unknown`); omitted elsewhere, so today's errors serialise unchanged |

```json
{"type": "error", "message": "That question was already answered.", "code": "question_closed"}
```

### `task_answer` / `bud.task.answer`

| Field | Type | Meaning |
|---|---|---|
| `request_id` | string | an open question |
| `outcome` | string | `accept`, `decline`, `cancel` |
| `value` | any or null | for `value`, `choose` and `form` questions |
| `correction` | string or null | with `decline` |

```json
{"type": "task_answer", "request_id": "q_task_5b1e9c0d_1", "outcome": "accept", "value": null, "correction": null}
```

```json
{"type": "bud.task.answer", "event_id": "evt_c01", "request_id": "q_task_5b1e9c0d_2", "outcome": "accept", "value": "8 pm", "correction": null}
```

### `task_cancel` / `bud.task.cancel`

```json
{"type": "task_cancel", "task_id": "task_5b1e9c0d"}
```

```json
{"type": "bud.task.cancel", "event_id": "evt_c02", "task_id": "task_5b1e9c0d"}
```

### `interaction_update` / `bud.interaction.update`

Allowed mid-session paths: `front.caller.verbosity`, `front.caller.pace`, `policy.*` within W4's clamps, `events.*`,
`hitl.channels`; in Bud mode only paths the agent's `session_overrides` list, and on the relay and facade only paths in
the server's `allow_client_override`. `/ws` allows one `config` per session, which is why this is a separate message.

```json
{"type": "interaction_update", "patch": {"front": {"caller": {"verbosity": "detailed"}}}}
```

```json
{"type": "bud.interaction.update", "event_id": "evt_c03", "interaction": {"events": {"floor": false}}}
```

### `playback_position` (Release 2; `/ws` and LiveKit)

```json
{"type": "playback_position", "utterance_id": "u_41", "played_ms": 1320}
```

### GA confirmation mirror (opt-in, `/v1/realtime`, Release 2)

With `events.ga_mcp_approvals: true` a `confirm` question also appears as a GA `mcp_approval_request` item with
`server_label: "bud"`; its exact item shape follows OpenAI's Realtime MCP guide and is to be re-verified before
coding (W6 section 3.12.2). The client's answer is intercepted as a `task_answer`:

```json
{"type": "conversation.item.create", "item": {"type": "mcp_approval_response", "approval_request_id": "q_task_5b1e9c0d_1", "approve": true}}
```

## Warning codes

`config_warning` on `/ws`, `warning` on `/realtime`, `bud.warning` on `/v1/realtime` (W6 section 3.10.4). Workstream 6
owns this list (Addendum C4); every design uses these codes. "Off" means the session runs with interaction off, exactly
as today; "fatal" means the session is refused.

| Code | Result | When |
|---|---|---|
| `unknown_config_keys` (existing) | key ignored | unknown key inside the block, path prefixed `interaction.` |
| `interaction_value_ignored` | field keeps its default | unknown or ill-typed value |
| `interaction_override_refused` | key ignored | a server-only key from a session: any `licence`, `speech.truth_ack`, `speech.fallback` from a client, `defaults`, `allow_client_override`, `realtime_legacy_wire`, `duplex.compute` (Addendum A2 item 5), the server task catalog under `tasks` (`task_backends`, `allow_client_backends`, `allow_client_tools`, `allow_client_push`, `phone_continuation`, `speech_policy`; A2 item 4), backend URLs or keys, and `background` (a Bud agent's answer marker: budapp's entry, the server's `defaults.agent` or an agent alias only; Addendum D5). Memory and tenant-persona settings are not interaction keys at all: they live under `front_model` (A2 item 3) |
| `interaction_disabled_by_server` | off | server switch off |
| `interaction_missing_build_features` | off | build without `silero-vad` and `smart-turn`, or the VAD model fails to load |
| `interaction_surface_unavailable` | off | the surface's interaction release has not shipped (`{surface, release}`); in Release 1 also a `/ws` session whose speech-to-text model is file-only (segmented), with `release: 2` (Addendum D2) |
| `interaction_needs_background` | off | raw `/ws` or a provider session with no task backend |
| `interaction_dag_audio_output` | off | DAG with an `AudioOutput`, `TtsProvider` or `RealtimeProvider` node |
| `interaction_not_applicable` | off | DAG template with no LLM-like node |
| `interaction_locale_unsupported` | off | session language without a policy pack |
| `interaction_language_step_down` | stepped down to the plain conversation path | caller keeps speaking an unsupported language on a `/ws` conversation or agent session (sign-off 18). Raw `/ws`, the GA cascade and regimes B and C cannot step down: they play a repeat-request template and continue in English (Addendum C7.1) |
| `interaction_unsupported` | off | the regime rule finds no regime (Infer without an injection channel; Hume before its shutdown) |
| `regime_downgraded` | automatic regime used | forced regime the provider cannot run, or a cascade surface asked for another regime (Addendum A2 item 7) |
| `model_family_unknown` | fillers resolve to `gateway` | an Azure realtime session without the optional `model_family` field (Release 3; Addendum A2 item 10) |
| `voice_owner_unavailable` | provider keeps the voice | `speech.voice_owner: gateway` without a half-cascade |
| `truth_ack_required` | off | regime B or C for a provider the server's `speech.truth_ack` does not list (`{provider, model, path, alternative}`) |
| `truth_enforcement_unavailable` | fatal | `speech.truth: enforced` with no cascade or half-cascade |
| `unsupported_vendor` / `provider_not_turn_based` | fatal | GPT-Live on `/v1/realtime` / on a DAG realtime node; a vendor the relay or facade does not serve |
| `interaction_forces_input_transcription` | transcription on | `transcribe_input: false` with interaction on |
| `filler_owner_overridden`, `backchannel_owner_overridden`, `idle_owner_overridden` | resolved per provider | owner value the provider cannot honour |
| `front_profile_substituted` | W2's `auto` resolution | requested profile not enabled, qualified or licensed (`{requested, effective, reason}`) |
| `front_model_scripted` | rule router and phrase banks only | front resolves to `scripted` (every Release 1 deployment, Addendum B1), or a call runs scripted |
| `speculative_start_unsafe_backend` | speculation off for that backend | `tasks.speculative_start: on` for a backend that executes its own tools (budprompt, webhooks, A2A, MCP) unless it is declared free of side effects (Addendum C2.2) |
| `caller_identity_rejected` | caller memory off | invalid, expired or wrong-tenant `interaction.front.caller.identity_token` (Release 2; token format pinned by W2) |
| `caller_memory_unavailable` | caller memory off for this boot | memory enabled but the task store runs in its in-memory fallback (Release 2; W2 section 3.16) |
| `persona_unknown` | the source's default persona | persona id not registered |
| `capacity_fallback` / `capacity_exceeded` | fallback voice / fatal | GPT-Live admission refused |
| `provider_sunset` / `model_retiring` | session runs | retiring provider or model (`{shutdown, replacement}`) |
| `provider_retired` / `model_retired` | fatal | retired provider or model |
| `tool_behavior_unsupported` | tool declared non-blocking | blocking tool on Gemini 3.8 Live Extended Thinking |
