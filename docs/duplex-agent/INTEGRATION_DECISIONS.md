# Integration decisions for the full-duplex interaction agent

Written 2026-10-04 by the orchestrator of the planning effort, after the seven workstream designs were revised and
cross-checked. **This file is binding.** Where it disagrees with a design (`W1-...md` to `W7-...md`), with
`DESIGN_FRAME.md` or with a cross-check report, this file wins. Where this file is silent, the resolution recommended by
`cross-check-consistency.md` (its findings 1 to 24 and its section 4 tables) applies, then the resolution recommended by
`cross-check-coverage.md`. A design that cannot follow a decision against the code must say so with evidence; it must
not silently deviate.

Terms used throughout: **E** is the confirmed end of the caller's turn (the moment the gateway decides the caller has
finished). The **front** is the small local model that answers at once; the **background** is the larger LLM or agent
that does the task. **Regime A** is the cascade (WaaV owns the voice), **B** a speech-to-speech provider speaking under
gateway control, **C** a delegating speech-to-speech provider that is itself the front with WaaV as its background
brain, **D** native duplex on WaaV Infer.

---

## 1. One release list

The seven designs numbered their releases independently. From now on every document uses these names and numbers.
Nothing in a release may depend on something in a later one.

| Release | Name | Purpose |
|---|---|---|
| Lifecycle track | Dated vendor changes | Ships to production as each item is ready, independent of interaction mode |
| Release 0 | Measure and lay groundwork | Harness, baselines, contracts, dark groundwork; interaction mode does not exist yet |
| Release 1 | Cascade interaction on `/ws` | Regime A on `/ws` conversation, `/ws` agent, raw `/ws` with a background target; LiveKit and SIP only if the echo gates pass |
| Release 2 | Full duplex audio, the GA cascade and durable tasks | Echo cancellation, backchannels, `/v1/realtime` cascade, Bud background runs and approvals, webhook backend, task persistence, trained front, GPU front pool, caller memory |
| Release 3 | Speech-to-speech providers, open task protocols and text-output DAGs | Regimes B and C on `/realtime` and `/v1/realtime`; A2A, MCP Tasks, OpenAI Responses backends; DAG task backend |
| Release 4 | Realtime voices on phone and rooms, Hindi and DTMF | Regimes B and C on LiveKit and SIP through `/ws`; Hindi and code-switched packs; DTMF |
| Release 5 | Native duplex on WaaV Infer (conditional) | Regime D, only if W7's four criteria hold |

The content of each release is section 5 of `cross-check-consistency.md` ("merged release list"), as refined by section 5
of `cross-check-coverage.md`, with the changes made by the decisions below. The mapping from each design's own numbers
is the table at the end of `cross-check-consistency.md` section 5. Each design rewrites its section 8 with these names.

**Lifecycle track, with hard dates.**

| Deployed by | Item | Owner |
|---|---|---|
| 2026-11-13 | Hume EVI: retiring warning now, refusal from 2026-11-13 (W6). Ships as a standalone first patch, ahead of the rest of Release 0 | W6 |
| 2026-11-13 (target, same patch where the live probe passes) | Gemini explicit tool `behavior`, `toolCallCancellation`, `interactionStatus`; Deepgram, Nova 2 Sonic and Ultravox wire fixes, each behind an escape flag and a live probe | W6 |
| Release 0 | LiveKit text turn detector removed from the image and the turn ensemble; model files under `CACHE_PATH` hashed on every load (after legal sign-off) | W5 |
| 2027-01-06 | OpenAI text-to-speech default moved off the `tts-1` family | W6 |
| 2027-01-20 | OpenAI realtime default moved off the `gpt-realtime` family | W6 |
| 2027-02-26 | OpenAI transcription default moved off `whisper-1` and the `gpt-4o-transcribe` family (shared with the segmented speech-to-text plan) | W6 |

Durations (W7's estimates, from a Release 0 start on about 2026-10-05): Release 0 about 3 to 4 weeks; Release 1 about 6
to 8 weeks from the start of Release 0; Release 2 about 6 weeks after Release 1; Release 3 about 8 weeks after Release 2;
Release 4 and Release 5 are scheduled at the Release 3 exit review.

---

## 2. The front model: which model, how it is packaged, what it does

### 2.1 Liquid is the primary front model

The request names a local Liquid AI model. The designs had made a no-model `scripted` profile the out-of-the-box
default because of the LFM Open License v1.0 (commercial use is licensed only while the user's legal entity, with its
affiliates, has under US$10M annual revenue; section 5 of the licence). That is a legal constraint on *who may run* the
weights, not a reason to design around another model. Decision:

- **`lfm-standard` (LFM2.5-1.2B-Instruct, Q4_K_M) is the primary, reference front model.** The prompt, the act grammar,
  the phrase banks, the guard tables, the distillation pipeline and the release gates are tuned and measured on it
  first. Every other profile is measured against it.
- **Profile resolution.** `interaction.front.profile` defaults to `auto`, resolved per server in this order:
  1. `lfm-standard` when the server's front-model configuration carries a licence attestation
     `licence: "lfm_open_eligible"` (the operator attests its legal entity is under the threshold) or
     `licence: "liquid_commercial"` (the operator holds a Liquid commercial licence). The attestation is server-side only
     (operator YAML or environment), never client-settable, and it is echoed in the session's `interaction` echo.
  2. `apache-standard` when no attestation is present and an Apache-2.0 profile has passed the W2 release gates. The
     candidates are chosen by the measured bake-off (section 8 of this file): Gemma 4 E2B, Qwen3.5-2B and Qwen3.5-0.8B,
     with thinking disabled.
  3. `scripted` otherwise, with `config_warning` code `front_model_scripted`.
  A missing attestation therefore never silently runs LFM weights, and never silently drops to no model when an Apache
  profile qualifies. This replaces W6 section 3.10.4's fallback wording and W2 decision 2.1's default.
- **Release 1 ships `lfm-standard` prompted** (compose grammar, rule layer, guard) if it passes the Release 1 front gates
  (W2 section 3.10); otherwise Release 1 runs `scripted` by default and `lfm-standard` arrives fine-tuned in Release 2.
  The gate result, not a date, decides.
- **Packaging.** The gateway image always contains the pinned `llama-server` build, the front configuration parser and
  supervisor, the persona and prompt assets, the grammars and the scripted banks. Weights: the LFM licence (section 4)
  permits redistribution with a copy of the licence, so Bud publishes an image variant that bakes the LFM GGUF with its
  `LICENSE` and `NOTICE` (tag suffix `-front-lfm`), and an Apache variant (`-front-apache`) once a profile qualifies. The
  plain image downloads the configured profile's weights at `init`, pinned by hash. Whether Bud publishes the
  `-front-lfm` variant publicly or only to its own and attested customers' registries is a sign-off (section 11).
- **Placement.** Release 1: a managed CPU sidecar per gateway pod (single supervised `llama-server`, UNIX socket, per-call
  pinned slot) and an external-endpoint mode for an operator-run `llama-server`. Release 2: a **shared front pool**
  deliverable owned by W2: a GPU (and CPU) `llama-server` pool image, deployment manifest and Helm chart, slot ranges per
  gateway, admission, a capacity test and an owner. Sizing for both comes from the Poisson load test in the bake-off
  (section 8), not from a single-stream number; decisions are sparse (about one per caller turn), so concurrency per
  instance is a duty-cycle question.

### 2.2 What the front model does, and what the gateway guarantees

The request says the small model handles human-in-the-loop questions and updates. The evidence (frame R4 and R5: the
1.2B model routes and confirms unreliably on its own, and fast voices that state content contradict the reasoner in 14
to 19% of cases) forbids letting it word anything that binds. The binding interpretation, to be approved at sign-off:

> **The small model runs the conversation; the gateway guarantees the words that bind.**

- **The front decides, for every caller turn, what the turn is**: a new request (`delegate`), a correction or addition
  to a running task (`update`), a cancel (`abort`), an answer to a pending question (routed to W4's parser), a status
  question, small talk, a backchannel, or a turn that is not finished yet (`wait`). This is the human-in-the-loop
  *dialogue control*: noticing that "actually make it four people" amends the booking, that "yes" answers the read-back,
  that "never mind" cancels.
- **The front words** acknowledgements, progress and status lines, small talk, clarification requests on unclear audio
  and refusals, in the persona's voice, from the persona's phrase banks and copy spans of verified text (W2's compose
  grammar). Free wording is opt-in with an acknowledgement.
- **The gateway words** everything that binds or reports a result: confirmation read-backs, choices and value requests
  (W4 templates, rendered from structured `ActionProposal` data, part of the persona pack and therefore personalisable),
  and background results (relayed verbatim, never through the front).
- The ten acts are W2's: `wait`, `backchannel`, `delegate`, `update`, `abort`, `social`, `echo`, `progress`, `question`,
  `refuse`. `confirm` and `handover` no longer exist anywhere.

### 2.3 Front-call lifecycle (consistency finding 2, coverage finding 2)

W2 owns `FrontModel` and `FrontCall` internals: supersede, cancel by closing the stream, the record of what was acted on,
admission and the per-instance error breaker (3 errors, 30 s) with per-call latency hysteresis (3 budget breaches, 20 s).
The default is **cancel-and-rewrite**; W2's **detach-and-append** fallback is selected by the Release 0 live test
`live_front_cancel_then_rewrite_reuses_checkpoint`. W1 keeps its priority classes and its single outer backstop
`TurnDecisionMax` = `act_ms` + 200 ms = 500 ms, drops its pod-level lateness breaker and the "turn decision waits for the
phrasing call" rule, and reads W2's sizing.

---

## 3. Time to the first word, and time to the actual answer

### 3.1 The background starts at E, in parallel with the front (new)

Coverage finding 3 showed that, as designed, every answer would wait for the front's decision and then for the whole
background run, so answers would arrive later than on today's conversation path (which streams sentences,
`core/conversation/mod.rs:1366-1409`). Decision:

- **Speculative start.** At E (or at an eager end of turn where W5's endpoint policy emits one), W1 starts the
  background on the caller's words *in parallel* with the front's turn decision, as a provisional run in W3 with no
  task id visible to anyone. When the front's act is `delegate` (new task), the provisional run is **adopted** as the
  task (same run, no restart). On any other act it is cancelled. Until adoption, W3's executor gate **refuses every
  side-effecting tool call** (it may wait for adoption up to the act budget plus the backstop, then the call proceeds or
  the run is cancelled); read-only tools and generation proceed. A provisional run never speaks and never emits client
  events.
- **Default:** `interaction.tasks.speculative_start` = `auto`: on for the LLM tier; off for Bud agent runs, webhooks,
  A2A and MCP backends (run creation there has cost, latency of its own and server-side records) unless the operator
  enables it. W7 measures the wasted-token rate (provisional runs cancelled per 100 turns).
- Owners: W3 (provisional runs, adoption, gate), W1 (fires at E, routes the act), W2 (the act decides adoption), W7
  (metrics). Release 1.

### 3.2 The answer streams sentence by sentence (new)

- Where the backend is the author of the final answer, its text is relayed **sentence by sentence as it arrives**: the
  LLM tier's final generation round (the tool rounds may stay non-streaming; the final round streams), and a Bud run's
  `output_text` after its last tool item. Each sentence is checked before release: the task's revision is unchanged, the
  task is not superseded or cancelled, and the sentence passes the outbound speech policy. A failed check stops the
  stream at the sentence boundary and W4 decides what follows (a withdrawal line if anything was spoken).
- This is still verbatim relay (the background's own words, on the main lane, never through the front). W4's
  pagination applies to long answers. Text deltas that are not the final answer (tool rounds, Bud drafts before the last
  tool item) are never relayed. Release 1 for the LLM tier and Bud foreground runs; Release 2 for Bud background runs.

### 3.3 Acknowledgement timing

- The acknowledgement is due at **E + `ack_due_ms`**. Default 300 ms. When the session's rolling median time from E to
  the background's first answer sentence for this backend is under `fast_answer_ms` (default 700 ms, after at least 3
  turns), `ack_due_ms` becomes 700 ms and the acknowledgement is **skipped if the first answer sentence is ready by
  then**. Hard limit E + 2,000 ms unchanged; amendment window E + 1,200 ms unchanged. The cached-clip versus generated
  choice stays W4's rule 5.
- W7 measures the arms 0, 150 and 300 ms for the default floor in Release 1 and the default is set from the data
  (coverage finding 7).

### 3.4 Experience targets (provisional, published now, confirmed at the Release 0 exit review)

Measured from the caller's acoustic end of speech at the client edge (W7's scoring anchor), regime A, streaming
speech-to-text, `/ws` transport, unloaded:

| Metric | Provisional target | Basis |
|---|---|---|
| First audible response (acknowledgement or answer), cached clip | p50 ≤ 0.9 s, p95 ≤ 1.3 s | W7 section 3.9.3 budget arithmetic |
| First audible response, generated acknowledgement | p50 ≤ 1.3 s, p95 ≤ 2.0 s | W7 section 3.9.3 |
| Time to answer (first word of the real answer), turns that need no tool | no worse than the Release 0 baseline + 150 ms at p50 on the same stack | coverage finding 3 |
| Unexplained silence | target 2 s, hard 3 s | W7 |
| Silence while holding after an expectation was set | W4's limits (holding line at 8 s, hard 10 s) | W4 |
| Barge-in: agent audio ducked at the ear | about 250-300 ms after speech onset on `/ws` | W5 section 3.4.3 |
| Barge-in: agent stopped | p50 0.3 s, p95 0.6 s (direction; measurement point stated by W7) | W7, W5 |

**Gap to the reference experience, stated honestly.** Thinking Machines Lab's interaction model reaches 0.40 s on
Full-Duplex-Bench v1 with 200 ms micro-turns (fact-checked). A cascade cannot reach that: endpointing, text-to-speech
first audio and transport alone exceed it. Regime C providers (GPT-Live, Gemini 3.8 Live) and regime D (native duplex
on Infer) are the routes to that number; regime A buys provider and model freedom, truthfulness and control at a cost
of roughly 0.5 s.

---

## 4. Contracts, vocabularies, numbers and configuration

- **One contract file.** Release 0 creates `gateway/src/core/interaction/contracts.rs` holding every cross-workstream
  type and enum, with a golden test per workstream (W2's `front_act_contract.json` is the model). The plan carries a
  readable copy, `contracts.md`. Designs cite sections, never sibling line numbers.
- **Vocabularies, numbers and configuration paths:** `cross-check-consistency.md` section 4 tables 4.1, 4.2 and 4.3 are
  adopted as written, with these changes: the acknowledgement rule in section 3.3 of this file; `answer_window_ms` is
  one value (4,000 ms, owned by W4, to be measured) read by W1's interlock and W5's rule 5; `duck_on_bare_onset` moves to
  W5's `duplex.barge_in`; the front breaker is W2's only.
- **Names** (consistency finding 10): `TaskRef` is W3's string alias ("t1", "t2", never reused); W2's struct becomes
  `TaskBinding`; caller words are `UserUtterance`, agent speech is W4's `Utterance`; W2's enum is `DelegationKind`;
  `Revision(u32)` everywhere with W3's struct renamed `RevisionRecord`; question ids are `HitlId`; front decision ids are
  `FrontDecisionId`; regimes are W6's `Cascade`, `ProviderSpeech`, `Delegating`, `NativeDuplex`.
- **HITL types** are W4's (`HitlKind`: `confirm`, `choose`, `value`, `form`, `auth_url`, `offer`; outcomes `accept`,
  `decline` with optional correction, `cancel`, `timeout`, `superseded`, `unanswerable` with reason). W3 deletes its own
  definitions and adds an `unanswerable` column to its answer table.
- **Task states on the wire** are W3's: `submitted`, `working`, `input_required`, `auth_required`, `completed`, `failed`,
  `cancelled`, `rejected`, `unknown`, plus a `cancelling` flag. Every client task event carries `state`; `task_result`
  and `task_cancelled` carry `effects` (`none_committed`, a list of labels, or `unknown`). A screen must never show
  "cancelled" for a task that committed something (consistency finding 13).
- **Client events** are W6's names (`task_created`, `task_progress`, `task_question`, `task_answer_received`,
  `task_result`, `task_cancelled`, `floor_state`, `unverified_claim`; client messages `task_answer`, `task_cancel`,
  `interaction_update`), with W4's additions mapped onto them (`by` gains `dtmf` and `session_end`;
  `assistant_transcript` gains `kind` and `verbatim`; `act` carries only W2 act names). On `/v1/realtime` they are
  `bud.*`. Protocol version 1.1. W6 publishes one wire-names table that every design cites.
- **Configuration homes:** `interaction` (`true` or an object; absent means off) in each existing home (the `/ws`
  envelope, `RealtimeSessionConfig`, deployment `RealtimeSettings`, `AliasDefinition`, `VoiceAgentEntry`, a DAG
  definition). Persona selection and caller fields live under `interaction.front` (`persona`, `caller`); W6's separate
  `interaction.persona` block is removed. Server-side: one `interaction` YAML key parsed outside `ServerConfig` and stored
  in `AppState`, mirrored by `WAAV_INTERACTION_ENABLED`; W2's `front_model` key stays separate and carries the licence
  attestation of section 2.1.
- **Missing build features:** a build without the VAD and Smart Turn features refuses interaction mode with a
  `config_warning` (W1's rule; W5's degraded path becomes test-only).

---

## 5. Seams between workstreams

Each resolution below is the one recommended by `cross-check-consistency.md` for the finding named, adopted as binding.

| Seam | Binding resolution |
|---|---|
| Executor between W4 and W1 (finding 4) | W4 adds W1's inputs (`Lane(LaneEvent)`, `ClientSpeak`, `SessionReady { greeting }`, `Restarted { open_tasks }`), `bypass_interlocks` and `reply_to` on `Utterance`, the `Greeting` kind, `late` on front results; drops `replaceable`; accepts W1's `StopAtBoundary` for an audible relay of an amended or cancelled task |
| Early "yes" over a confirmation (finding 5) | While W4's `answer_expected()` is true, W5 classes a short answer as `interrupt` whether or not the last clause has played; W4's heard check decides whether it binds; W4's answer-parser row runs before its "not an interrupt" row |
| Turn detection (finding 6) | W1 installs W5's `ClassifiedStart` and `EndpointStop`; the analyser's Silero is the only VAD in interaction mode; `RuntimeMsg::Duplex(DuplexEvent)` on a bounded channel with a drop counter; W1's bot-speaking probe is `agent_holds_floor()` |
| Yield backstop (finding 7) | W1 computes `yield_backstop_ms` at startup as the maximum of W5's bounds plus 1,000 ms (about 7,700 ms today) and validates it against W5's configuration |
| Stops (finding 8) | `PlayoutControl::stop` takes utterance ids (or `AudibleOnly`), never `AllQueued`; queued items return to W4 as `Returned`; one stop event; a client `clear` uses W1's `ClientClear` path |
| Heard time (finding 9) | One function owned by W1: `heard_at = delivered_at + lead(transport) + transport_lag_ms(transport)`, carried on playback events; W4 keeps `heard_margin_ms` (150) as a separate margin |
| `ProviderVoice` and `SpeechPort` (finding 14) | W6 implements W1's current `SpeechPort`; W1 accepts `SpeakTag.purpose`, `SpeakTag.task` and `PlaybackEvent.estimated` |
| Provider-speech truth (finding 15) | One pipeline: W6's `ClaimMonitor` detects (calling W2's guard and W4's lexicons), emits one `PolicyInput::ProviderSpeech`, W4 alone words and schedules the correction, W6 alone emits `unverified_claim`. Monitored-truth regimes run only for providers listed in the server-side `speech.truth_ack` (default empty, so regimes B and C are refused with a warning until an operator lists a provider). Input mute only where W5's echo defence is not running |
| Front prompt content (finding 16) | W2 owns all prompt text; W3 supplies `TaskView` data (handle, state, title, queued-behind, cancel unconfirmed, effects) through W1; W2 renders it in coarse time buckets |
| W5's safety outputs (finding 17) | W4 consumes `attribution` and `suspected_echo` on `EndOfTurn`, `Held { expired | released }`, `SideTalkHeld`, `EchoWithdrawn`; an irreversible accept binds only from a `High`-attribution turn; a `suspected_echo` turn never binds |
| Idle hang-up (finding 18) | W1's suppression reads W4's `holds_session_open()`; "silence" means caller silence; the cap on an agreed wait is W4's `policy.agreed_wait_max_ms`, default 600,000 ms (10 minutes), a sign-off item |
| Clips (finding 19) | One `ClipBank` type owned by W5 (content, groups, cache key, loudness); rendering owned by W1 (capture tags on the session's own provider); W6's `ClipRenderer` is the regime B and C implementation of the same call; bank and renderer in Release 1, backchannel emitter in Release 2; W5's drop rule ("if the caller resumed speaking") |
| Playback pump file (coverage finding 10) | W1 owns `core/audio/playback_pump.rs`; W5's pacing requirements apply to `spawn_metered` only; the legacy pump is untouched |
| Conversation sessions (finding 22) | In interaction mode the orchestrator is not constructed and the STT callback only does `try_send` (W1 decision 3), so `/ws` conversation sessions are in Release 1; the legacy inline-turn fix stays a separate mode-off flag `spawn_turns`, default off |
| Server switch (finding 21) | One `interaction` server key parsed outside `ServerConfig` (section 4) |
| Smaller numbers (finding 23) | As the finding's table says; W3 adds `Continuation` to `RevisionKind` and a `retry(task)` operation |

---

## 6. Endpoints, DAGs and Bud agents

- **DAG task backend (coverage finding 5).** W3 adds an eighth backend, `DagBackend`: wraps `executor.execute_from`,
  carries a cancel token, reports progress per node, and takes `speakable` from the graph's text output node. Release 3,
  with W1's text-output DAG support.
- **Audio-output DAGs** (for example speech-to-text, translate, text-to-speech) are **refused in interaction mode** with
  `config_warning` code `interaction_dag_audio_output`, and W5's DAG drain leaves Release 1. A duplex DAG output (pump
  plus barge-in on graph audio) is listed under "Later" with a named owner (W5).
- **Bud agent sessions in Release 1 (coverage finding 6).** W6 adds an operator-side default so Bud sessions are not
  blocked on budapp: `interaction` may be set in an `AliasDefinition` and in the server `interaction.defaults.agent`
  block. A budapp-published `VoiceAgentEntry.interaction` wins when present. Client paths remain limited by the agent's
  `session_overrides`.
- **Cross-team dependencies** become one list in the plan, each with the owning team, the field or behaviour, the release
  that needs it and a requested date: budapp (`VoiceAgentEntry.interaction`, `front`, `background`, per-tool
  confirmation templates), budprompt (`durable_engine_enabled` on target deployments, a per-run context field for voice
  rules, push instead of resume tails), budgateway (proxying the status `GET` and the SSE resume `GET`; 429 before run
  creation).
- **Coverage statement.** Tables 1 and 2 of `cross-check-coverage.md` are the plan's coverage statement. Every exception
  in its finding 9 is listed in the plan with its release or its refusal, and each is a sign-off item.

---

## 7. Personalisation

- **Persona.** An operator-registered, typed `PersonaSpec` (W2) compiled into the front prompt, the background
  preamble, text-to-speech settings, provider instructions for regimes B and C, the phrase banks, and W4's templates.
  W2 adds `interruptibility` (`low`, `normal`, `high`), `utc_offset` and W5's four-level `backchannel_intensity` (`off`,
  `low`, `medium`, `high`) to `PersonaSpec`.
- **Who can register a persona.** Release 1: the operator's server YAML and, in Bud mode, budapp. Release 2: a
  tenant-scoped persona registration API (`PUT /v1/interaction/personas/{id}`, validated by the same compiler and guard,
  versioned), a sign-off item. Clients select a registered persona and set only whitelisted typed caller fields
  (`interaction.front.caller.*`: name to use, verbosity, pace, language); clients never supply persona text.
- **Caller memory (new, Release 2).** W2 designs a `CallerMemory` port: read at call start (at most 3 typed facts, as W2
  decision 2.5 already limits), written after the call by the background or the agent platform, keyed by an
  operator-supplied opaque caller identity (hashed), stored in the Redis instance W3 already uses for tasks, retention
  configurable, **off by default**. In Bud mode budapp may supply facts instead. The identity key and retention are
  sign-off items.

---

## 8. Measurements this integration phase runs now

The plan rests on measurement, not modelling (spine S9). Before the plan is written, a measured bake-off runs on the
GB10 with the packaged prompt and grammar (W2's final assets):

- Models: LFM2.5-1.2B-Instruct (reference), Gemma 4 E2B-it, Qwen3.5-2B and Qwen3.5-0.8B (thinking off), Q4_K_M, with
  the pinned `llama-server` build used by the earlier benchmark.
- What is measured: act correctness and line quality on the earlier six scenarios plus new ones for every act, every
  human-in-the-loop routing case, prompt injection and invented facts; latency to the act and to the first guarded
  chunk, warm and cold, CPU (6 threads) and GPU; the cost of the grammar with assistant prefill; and a Poisson load test
  (calls each making a decision about every 6 s) giving p95 act latency against concurrent calls, which sets the sizing
  in section 2.1.
- The results decide: whether `apache-standard` exists in Release 1 and which model it is; the CPU sidecar's calls per
  instance; whether the prompted `lfm-standard` is likely to pass the Release 1 gates. Models are deleted after the run
  (the disk is nearly full).

---

## 9. Scope statements

- **Proactive speech.** Out of scope for Releases 1 to 4 beyond the policies already designed: timers, progress, holding
  lines, offers, check-ins and results delivered when the line is idle. The gateway never starts speaking over the
  caller (W4 rule 1). Interjecting on its own initiative (for example, to warn that the caller's statement contradicts a
  verified result) is listed under "Later" as a research item.
- **Technology register.** The plan carries a technology register: every model, protocol and technology considered for
  full duplex, turn-taking, echo, task management and orchestration, each marked chosen, candidate, rejected or watch,
  with the reason and the licence. Thinking Machines Lab's Apache-2.0 background model, Inkling, is listed as a
  candidate background model reachable through the LLM tier. W7 maintains the register after the plan.
- **Languages.** English in Releases 1 to 3. A caller speaking another language in an English-only session gets the
  behaviour W2 and W4 define for unsupported languages (interaction mode steps down to the plain conversation path with
  a `config_warning`; a sign-off item). Hindi and code-switched packs in Release 4.

---

## 10. Changes to the frame

`DESIGN_FRAME.md` is amended by this file: spine S8's default (section 2.1 here replaces "Apache-2.0 default"); S2 and S5
are read with section 2.2 here; the goals "never an unexplained silence", "monitored truth in regimes B and C" and
"verified paraphrase in regime C" replace any stronger wording; the parallel speculative start and streamed relay (section
3) are added to the spine as S10 ("the background starts at E and its answer streams").

---

## 11. Decisions reserved for a person

The plan proceeds on the default shown. Each item names where it was raised.

| # | Decision | Default the plan assumes | Raised in |
|---|---|---|---|
| 1 | Adopt the interpretation "the small model runs the conversation; the gateway guarantees the words that bind" | Adopted | Section 2.2; coverage finding 4 |
| 2 | Whether Bud's own legal entity is under the LFM threshold, and whether to seek a Liquid commercial or OEM licence | Ask Liquid for commercial terms now; Bud's hosted fleet sets the attestation only after legal confirms | Section 2.1; W2 section 10; W7 section 10 item 1 |
| 3 | Publish the `-front-lfm` image variant publicly, or only to Bud's and attested customers' registries | Bud's and attested customers' registries only | Section 2.1 |
| 4 | Whether CI and nightly runs on LFM weights count as commercial use | CI uses the Apache profile; LFM jobs opt-in until legal answers | W7 section 10 item 1 |
| 5 | Fleet front placement: shared GPU front pool versus CPU sidecars | CPU sidecar in Release 1, shared pool in Release 2 | Section 2.1; coverage finding 4 |
| 6 | Speculative background start default | On for the LLM tier, off for Bud and external backends | Section 3.1 |
| 7 | Reworded goals: "never an unexplained silence"; monitored truth in regimes B and C; verified paraphrase in regime C; a spoken approval is passed to the agent platform and does not execute by itself | Adopted | Coverage section 6 item 1 |
| 8 | The merged release list, and shipping the Hume and wire fixes as an early standalone patch | Adopted | Section 1 |
| 9 | Interaction-off changes: each row of W6 section 9.2, the LiveKit detector removal and the remediation of past images, the `spawn_turns` default, the legacy pump's pacing | Approve with escape flags; `spawn_turns` off; legacy pump untouched | W6, W5, W1 |
| 10 | Monitored-truth regimes: per-provider operator acknowledgement (`speech.truth_ack`) versus on by default | Per-provider acknowledgement | Section 5 |
| 11 | The cap on an agreed wait before the idle hang-up | 10 minutes with check-ins | Section 5 |
| 12 | SIP and LiveKit in Release 1 when the echo gates pass, or wait for echo cancellation in Release 2 | Ship in Release 1 only if the gates pass | W7 section 10 item 10 |
| 13 | Bud agents in Release 1 through the operator-side default | Yes | Section 6 |
| 14 | Spoken approvals of budprompt governance escalations | Off unless the agent author allows voice approvals | W3 section 10 item 1 |
| 15 | Session end: which tasks run on after hang-up, and the reconnect grace | W3's proposal (cancel tasks with no possible side effects and no push target; 60 s same-pod grace) | W3 section 10 item 2 |
| 16 | Privacy: caller words in Redis, timeline and front-log retention, caller-memory identity key and retention | Off (`store_content: false`); memory off by default | W3, W1, section 7 |
| 17 | Tenant persona registration API | Release 2 | Section 7 |
| 18 | Behaviour for a caller in an unsupported language | Step down to the plain conversation path with a warning | Section 9 |
| 19 | Backchannel defaults per transport and the duck depth | Web on (medium), phone off; −15 dB | W5 section 10 item 5 |
| 20 | Bud side runs (parallel Bud tasks on one conversation) | Off unless the agent author opts in | W3 section 10 item 7 |
| 21 | Use of production transcripts for evaluation and training; two-channel recording; training on a teacher model's outputs | None until consent, retention and terms are approved | W2, W5 |
| 22 | OpenAI text-to-speech strategy after 2027-01-06 | W6 section 10 item 3 proposal | W6 |
| 23 | Whether `/realtime` clients may call the verbs directly | Gateway runtime only in Release 3; client verbs later | W6 section 10 item 4 |
| 24 | Phone-line continuation by caller number | Off | W3 section 10 item 6 |
| 25 | Licences and terms: Krisp, MaAI VAP provenance, MoshiRAG, Full-Duplex-Bench data (CC BY-NC), PersonaPlex hosting | Not used commercially until cleared | W5, W7 |
| 26 | Native-speaker authoring of the Hindi packs and lexicons | Required before Release 4 | W4, W5 |

---

## Addendum A (2026-10-05): problems found while building the chapters and artefacts

Written after the seven chapters, the front-model package and the provider artefacts were produced. Where this addendum
disagrees with anything above, with a chapter or with an artefact, this addendum wins.

### A1. The adoption deadline is anchored at E

- A provisional run's adoption deadline is **E + `act_ms` + `turn_decision_slack_ms` + 100 ms hop grace = E + 600 ms**,
  anchored at E, never at the provisional start.
- A run started at the eager end of turn has no adoption deadline until E. At E, W3's new
  `confirm_provisional(id, e_at)` (requested by W1) keeps the run when W5 reports the same words and anchors the
  deadline at E; different words or `EagerWithdrawn` cancel it. A ceiling of eager start + 1,500 ms (the silence
  ceiling) cancels a run whose E never comes.
- Chapter 2's "provisional start + 300 + 200 = 500 ms" is wrong and is corrected to this rule; chapters 1, 3 and 5
  already agree with it.

### A2. Provider and configuration artefact problems (from `plan/providers/PROVIDER_COVERAGE.md`)

| # | Problem | Binding answer |
|---|---|---|
| 1 | Ultravox would be offered a half-cascade with "enforced" truth, but nothing stops its model wording its own replies | `select_regime` tests `half_cascade_possible` (text output **and** a documented reply switch). Only the OpenAI family qualifies today. W6 renames the field and narrows the rule |
| 2 | Grok's default model: W6 says `grok-voice-think-fast-2.0`, the code sends `grok-realtime` (`core/realtime/grok/protocol.rs:49-53`) | The descriptor follows the code. Moving the default is a lifecycle-track item behind a live probe |
| 3 | Caller memory and personalisation switches are named differently by W2 and W6 | Session path `interaction.front.caller.identity_token` with `config_warning` `caller_identity_rejected` (W6's names). Server settings for memory and tenant personas live under W2's `front_model` key: `front_model.memory.*` (`identity_key_file`, `retention_days` 90, `enabled` false) and `front_model.tenant_personas.enabled`. W6's schema adopts these server paths |
| 4 | W3's operator catalog sits at the top of the server key, W6 puts it under `interaction.tasks.*` | `interaction.tasks.*` (W6). W3 renames `task_backends`, `allow_client_backends`, `allow_client_tools`, `allow_client_push`, `phone_continuation` and `speech_policy` under it |
| 5 | W5's compute pool path differs in W1, W5 and W6 | Server-only `interaction.duplex.compute.*` (W5); refused from sessions; W1 and W6 correct their tables |
| 6 | Bud deployments cannot reach Inworld, Ultravox or Yandex before Release 4 (not served by the `/v1/realtime` relay or facade; native `/realtime` refuses Bud mode) | Added to the coverage exceptions as sign-off 27; Release 4's `interaction.speech.realtime` closes it only if budapp can name such a deployment |
| 7 | "An operator may only narrow the regime" was never defined | Adopt the reference resolver's reading: a forced regime is honoured only when the provider can run it; the arms that force delegation are exclusive; `auto` owners resolve to `provider` in a forced delegating regime; a cascade surface asked for another regime gets `config_warning` `regime_downgraded`. W6 writes this into its regime rule |
| 8 | Half-cascade wording: "provider as speech-to-text and voice" against `voice_owner: gateway` | The half-cascade uses WaaV's text-to-speech as the voice (`voice_owner: gateway`); owners resolve to filler `gateway`, backchannel `none`, idle `gateway`. W6 corrects section 3.3 |
| 9 | `question_closed` cannot be sent: the `/ws` `error` message has only `message` (`handlers/ws/messages.rs:372-376`) | Add an optional, additive `code` field to the `/ws` `error` message in Release 1 |
| 10 | Azure realtime deployments hide the model name (`core/realtime/azure/protocol.rs:36-38`), so lifecycle and preamble rules cannot be evaluated | Add an optional `model_family` field to the Azure realtime configuration (Release 3); until it is set, fillers resolve to `gateway` and the session gets `config_warning` `model_family_unknown` |

### A3. Sign-off added

| # | Group | Decision | Default the plan assumes |
|---|---|---|---|
| 27 | C | Bud deployments reach Inworld, Ultravox and Yandex speech-to-speech only from Release 4, and only if budapp can name such a deployment | Accept; revisit at the Release 3 exit review |

---

## Addendum B (2026-10-05): what the measured front-model bake-off changes

Source: `plan/front-model/FRONT_MODEL_BAKEOFF.md` (four models on the GB10 with the packaged prompt and grammar, 68
labelled decisions, cancel-and-rewrite, grammar cost and a Poisson load test). The bake-off's verdicts are forecasts on
a small seed set, but the gaps are large. This addendum wins over sections 2 and 8 above and over chapter 2.

### B1. Release 1 front: the deterministic router with persona lines, by default

- Measured on the packaged prompt, **no prompted small model passes the Release 1 front gates**: acceptable acts
  LFM2.5-1.2B 0.50 to 0.56, Qwen3.5-0.8B lower, Gemma 4 E2B and Qwen3.5-2B 0.79, against the deterministic rule router's
  0.87 on the same set (optimistic: same author). Delegate recall at best 0.92 against a 0.98 gate; 3 to 4 of 5
  injections followed.
- So the `auto` resolution of section 2.1 yields **`scripted` in Release 1 on every deployment**, attested or not: the
  rule router decides the act at once, the persona's phrase banks and the clip bank speak, and W4's templates and the
  verbatim relay do everything that binds. The immediate-response experience of Release 1 comes from this path, not
  from a model. The model stack still ships in Release 1, dark and opt-in (`front.profile` forced by the operator for
  evaluation only), so the gate runs, the x86 run and the pool capacity test use shipped code.
- **Liquid stays the primary model, and the route to it is fine-tuning.** LFM2.5-1.2B is the only model that met a
  latency target (GPU act p95 70 ms warm; 95 ms at one call under load) and the cheapest to run; what it lacks is act
  choice, which is what distillation trains. `lfm-standard` becomes the attested default the moment its fine-tuned
  version passes the gates; until then attested deployments run `scripted` too. If the fine-tune fails, `scripted`
  stays the default, because a front model that chooses acts worse than the rules makes the experience worse.

### B2. The distillation track starts in Release 0, not Release 2

W2's distillation pipeline (`tools/front-distill/`: teacher-labelled timelines validated by the Rust guard and
renderer, LoRA fine-tuning on about 20,000 decisions, preference tuning for the never-rules, 1,500 injection cases) has
no gateway dependency, so it runs as a parallel track from Release 0. The bake-off's evaluation set and its verbatim
failure list seed the frozen sets and the preference pairs. A fine-tuned `lfm-standard` that passes the gates may ship
in a Release 1 point release; the Apache candidate carried forward is Qwen3.5-2B, with Gemma 4 E2B as the alternate;
Qwen3.5-0.8B is dropped. Other Liquid candidates measured by the orchestrator after the bake-off are recorded in
section B7.

### B3. Placement and sizing, as measured

- **CPU sidecar: no model.** On 6 ARM threads (with other inference engines on the same cores) no model met the CPU act
  target even at one call (LFM act p95 1,496 ms). The Release 1 CPU sidecar runs `scripted`. A CPU device class is
  qualified for a model only if the Release 0 x86 run on the gateway pods' CPU measures act p95 within 200 ms at one
  call; then `max_calls_per_instance` is 1.
- **GPU front pool: floor of 4 calls per LFM instance and 2 per Qwen3.5-2B instance on a shared GPU** (act p95 within
  300 ms up to 4 simultaneous LFM calls deciding about every 6 s; saturated at 16). The real number comes from the
  Release 2 pool's own capacity test on its hardware. Admission permits equal that number, so excess decisions go to the
  rule router, never into a queue.
- The first spoken chunk missed its target on every device (decode is time-sliced on the shared GPU). Acknowledgements
  therefore default to clips, and a generated line is used only when W4's rule 5 predicts it will be heard in time.

### B4. The gateway renders the front's prompt and calls `/completion`

On the chat endpoint `llama-server` inserts `<think></think>` before LFM's prefill, which puts the model out of
distribution and defeats cache reuse after every cancel-and-rewrite (tokens processed grow each decision; CPU act p95
doubles). With a client-rendered prompt on `POST /completion`, cancel-and-rewrite reuses the cached history (16 extra
tokens at the median) on all three model families, so **cancel-and-rewrite stays the default** and detach-and-append is
not needed. Each model profile defines one assistant-turn opener (none for LFM2.5; the empty think block for Qwen3.5;
the empty thought channel for Gemma 4), used identically in history and in the generation prompt.

### B5. Package and design corrections the bake-off proved

1. The grammar is mandatory (16 to 75% of outputs are valid without it). W2's "99% valid without grammar" row becomes a
   monitored health signal, not a gate.
2. Copy spans may not read back a sensitive value when the agent, not the caller, named the category (fixed in
   `front_reference.py`; W2 section 3.2.5 states the rule this way).
3. On a yes/no question only inquiry-framed spans are allowed ("let me check whether ..."), never a bare restating span;
   when the caller's words hit a persona's forbidden topic, copy spans are disabled and only `refuse` and `delegate` may
   speak.
4. An `injection_cue` lexicon in the rule layer commits `refuse` for instruction-override turns, and the background
   preamble and W3's executor gate treat the caller's words as data (injections otherwise become background tasks).
5. The repeat-request exemption in the rule layer (`request_not_wait` no longer turns "sorry, what did you say?" into a
   task).
6. Rebase at `max_context_tokens` 6,144 is required, not rare: a call's history grows about 100 to 170 tokens per
   decision and reaches an 8,192-token slot near decision 38.
7. Call-start priming of the 1,700 to 1,800-token persona prefix costs 0.23 to 1.14 s on the shared GPU (6.4 to 6.7 s on
   CPU), so the first decisions of a call are rule-routed unless cross-call prefix reuse works (a Release 0 measurement).

### B6. Sign-off changes

- Sign-off 1 now reads: "the small model runs the conversation once it is fine-tuned and gated; until then the
  deterministic router does, and the gateway always guarantees the words that bind."
- Sign-off 5 default: scripted CPU sidecar in Release 1; shared GPU front pool in Release 2, sized from its own capacity
  test (floor 4 calls per LFM instance).
- New sign-off 28 (group A): start the distillation track in Release 0, including the teacher model and its terms
  (sign-off 21 covers training on a teacher's outputs).

---

## Addendum C (2026-10-05): seams left open by the aligners and the contract check

Sources: each aligner's "left open" list and the contract reference's check (`plan/contracts.md`). This addendum wins
over everything above it, over the chapters and over the artefacts.

### C1. Streamed answers

1. The event is W3's `TaskEventKind::AnswerSentence { seq }` (not `AnswerChunk`). W4 renames `index`/`chunks` to
   `seq`/`sentences`, its `ToolRound` stop reason to `MarkerThenToolCall`, and adds `RunFailed`; W4 maps every
   `AnswerStopped` reason onto its stream-stop rows.
2. The relay is W1's one open main-lane item: W4 sends `StreamSpan { task, attempt, seq_from, seq_to, last }`; spans of
   one answer share one `ItemId`; an explicit close is a `Speak` with no clauses and `last = true`.
3. Draft text is held back by the `ANSWER:` marker rule (W3), not by a preamble lexicon. W7's draft-leak gate counts
   `no_marker` and `draft_dropped`; a marker followed by a tool call is stopped at the clause boundary, withdrawn, and
   gated as a rate (proposal at most 2%). The LLM tier cuts sentences with the conversation path's sentence aggregator.
4. **Bud agents stream only if the agent declares `background.answer_marker`** (a budapp field); otherwise the answer
   is relayed whole at completion, because a draft and a final message are distinguishable only at
   `response.completed` and WaaV cannot prompt a Bud agent. A budprompt final-answer marker joins the cross-team list.

### C2. Provisional runs

1. W3's `TaskRegistry` gains `start_provisional(origin, words, adopt_by) -> ProvisionalKey`,
   `confirm_provisional(key, e_at)`, `discard_provisional(key, DiscardReason)` and `DelegateRequest.provisional:
   Option<ProvisionalKey>`. `origin` is `FrontAtE`, `EagerEndOfTurn` or `ProviderCall { adopt_by }` (W6's Deepgram and
   Gemini speculative calls). `DiscardReason` includes `StepDown`. W1 uses these names. The deadline is Addendum A1.
2. Speculative start on a backend that executes its own tools (budprompt, webhooks, A2A, MCP) is allowed only when the
   backend is declared free of side effects; otherwise the operator's `on` is refused with `config_warning`
   `speculative_start_unsafe_backend`. This narrows section 3.1's "unless the operator enables it".

### C3. Missing design sections, to be written in the final phase

W3 3.16 (provisional runs) and 3.17 (streamed answers); W5 3.8.7 (`EndOfTurnReport`: `attribution`, `suspected_echo`,
`source`, `eager: EagerMatch { Same, Different, None }`); W6 3.12.4 (task-event mapping); W2 3.16 (`CallerMemory`) and
3.17 (persona registration).

### C4. Smaller contract names

- W1 builds `PolicyInput::VadOnset` from W5's `DuplexEvent::VoiceOnset`; `on_vad_edge` is withdrawn everywhere.
- `ProviderVoice` implements `mark_stopped -> PumpStopReport` and `set_stream_open`; W1 and W6 share
  `SpeakPurpose { Ack, Progress, Question, Confirmation, Relay, Correction, Filler, Clip, Client, Other }`.
- `TaskRef(String)` is a newtype in `contracts.rs`, one definition.
- `config_warning` codes (W6 owns the list): `interaction_missing_build_features`, `interaction_language_step_down`,
  `interaction_surface_unavailable`, `caller_memory_unavailable`, `model_family_unknown`, `regime_downgraded`,
  `truth_ack_required`, `interaction_dag_audio_output`, `front_model_scripted`, `caller_identity_rejected`,
  `speculative_start_unsafe_backend`; `floor_state` reason `stream_gap`.
- `CancelUnconfirmed` becomes `task_progress` with `cancelling: true` and the state unchanged (W3 3.4.2); it is never
  `task_cancelled` or `unknown`. `StaleOutcome` is a `task_result` source.
- W6's session echo carries `policy_hash`, the effective `PolicyConfig` and the barge-in values.
- W4's `PolicyConfig` publishes `ack_due_ms` (0 allowed), `fast_answer_ms`, `ack_target_ms` (800, the latest a
  generated line may be heard), `ack_hard_deadline_ms`, `answer_window_ms` (4,000) and `agreed_wait_max_ms`.

### C5. Caller memory and tenant personas

Identity travels as an operator-signed token in `interaction.front.caller.identity_token` (outside the four
client-settable caller fields; sign-off 16), with `config_warning` `caller_identity_rejected`. Server settings are
`front_model.memory.*` and `front_model.tenant_personas.*` (at most 20 personas with 5 versions each per tenant). The
caller field for a name is `preferred_name`. One token format, pinned by W2.

### C6. Experience targets, corrected by the measurements

1. **First audible response** targets apply to turns where an acknowledgement plays. In fast mode the first sound is
   the answer itself; its target is time to answer (baseline + 150 ms at p50), and the first sound must still come by
   1.6 s p50 from the acoustic end. `fast_answer_ms` is configurable; W7 runs both arms in Release 1.
2. Acknowledgement clips are at most 24 characters (`ack_clip_max_chars`), so a playing clip delays a ready answer by
   at most about 1.2 s; W7 measures the effect on time to answer in the first three turns of a session.
3. **Generated acknowledgements**: the measured first spoken chunk (LFM 575 to 656 ms p95 on a shared GPU) makes the
   1.3 s generated target unreachable on shared hardware. Acknowledgements default to clips; the generated target is
   re-derived at the Release 0 exit on dedicated front hardware.
4. **Front qualification gates** are the quality rows and the act-latency rows. The first-chunk row is not a
   qualification gate (fine-tuning cannot change decode speed); it only enables generated acknowledgements under W4's
   rule 5.
5. **Barge-in**: the experience metric is split. "The agent yields" means the duck is audible at the ear, p50 at most
   0.3 s from the caller's onset. "The agent is silent" (paused or cut) has a direction of p50 at most 0.7 s at a `/ws`
   ear, from W5's 400 ms pause plus onset detection and the 150 ms lead. A pause at 250 ms is a Release 1 listening
   test.

### C7. Session behaviour

1. **Language step-down** is possible only on `/ws` conversation and agent sessions (the plain path starts with an
   empty history). Raw `/ws`, the GA cascade and regimes B and C play a repeat-request template and continue in
   English. Sign-off 18's default is restated this way.
2. A clean close (the client closes or the call hangs up normally) skips the 60 s reconnect grace; only an abnormal
   disconnect gets it.
3. Bud realtime deployments get an operator default `interaction.defaults.realtime` in Release 3, like
   `interaction.defaults.agent`.
4. The server `interaction` key is loaded from the same YAML file as `ServerConfig`; when `ServerConfig` came from the
   environment only, the key comes from `WAAV_INTERACTION_*` variables. Verified in Release 0.

### C8. Sign-off wording

- Sign-off 4: CI runs `scripted` and the Qwen3.5-2B candidate forced with `allow_unqualified`; LFM jobs are opt-in
  until legal answers.
- Sign-off 9 adds the AssemblyAI default move to Universal-3.6 Pro (it changes interaction-off sessions).
- Section 2.1's placement text and the consistency table's front CPU sizing are superseded by Addendum B3.

### B7. Other Liquid models, measured by the orchestrator after the bake-off

Same package, harness, sampling and shared GPU as the bake-off (`bench/bakeoff/chain_extra_liquid.sh`,
`quality_extra.jsonl`, `poisson_extra.jsonl`; files `LiquidAI/LFM2.5-2.6B-GGUF` Q4_K_M sha256 `02a8b7e1...`,
`LiquidAI/LFM2.5-VL-3B-GGUF` Q4_K_M sha256 `2436cf4b...`, the vision model used text-only; GPU only).

| Model | Acceptable act | Macro-F1 | Delegate recall / precision | Status question sent as a new task | Injections followed | Invented entities | Warm act p95 (one call) | Calls within the 300 ms act budget |
|---|---|---|---|---|---|---|---|---|
| LFM2.5-1.2B-Instruct (bake-off) | 0.50 to 0.56 | 0.49 to 0.56 | 0.73 to 0.85 / 0.62 to 0.68 | 0 | 4 of 5 | 0 | 70 ms | 4 |
| **LFM2.5-2.6B** (reasoning suppressed by the prefill) | **0.72 to 0.74** | 0.68 to 0.72 | **0.96** / 0.74 to 0.76 | 0 | 3 of 5 | 0 | 144 ms | 4 (act p95 278 ms at 4, 394 ms at 8) |
| LFM2.5-VL-3B, text only | 0.71 | 0.62 to 0.63 | 0.92 / 0.77 to 0.79 | 0.17 | 2 to 3 of 5 | 0 | 108 to 111 ms | 4 (act p95 262 ms at 4, 401 ms at 8) |

Decision: **LFM2.5-2.6B joins the plan as `lfm-large`**, the stronger Liquid base for the Release 0 distillation track
(same licence and attestation as `lfm-standard`; reasoning is suppressed by the prefill and grammar, and no reasoning
text was produced). It has the highest delegate recall of any front measured, including the rule router (0.92), at
twice the 1.2B's act latency and the same calls per GPU instance. The distillation track fine-tunes both LFM2.5-1.2B
and LFM2.5-2.6B and qualifies whichever passes the gates first, preferring the 1.2B when both pass (faster act).
LFM2.5-VL-3B is the Liquid candidate for the Hindi profile in Release 4 (it lists Hindi; no Hindi set was run). All
three still fail the Release 1 gates when prompted, so B1 stands.

---

## Addendum D (2026-10-05): answers to the final independent review

Source: `research/duplex-agent/review/final-review-*.md` (five reviewers; 33 serious findings, every one confirmed by a
separate verifier). This addendum wins over everything above it.

### D1. The rule router, Release 1's front, is fixed and gated

1. **Questions are never met with silence.** A router step before the final `wait` rule: an addressed turn with no
   side-talk cue, no open question and no answer window, that the polar-question check flags and that ends in "?" or
   starts with a wh-word or a yes/no starter, and is not a status, social or repeat turn, routes to `delegate`.
   Fragments that end on a dangling word keep `wait` (`incomplete`). Simulated on the 68 seed decisions by the reviewer:
   delegate recall 0.92 to 1.00, acceptable acts 0.87 to 0.90, precision 0.81 to 0.82, no new misses. Decisions T11.d4
   and T16.d3 become regression cases.
2. **The router is gated too**, at the Release 0 exit, on a frozen held-out set: delegate recall at least 0.95 with the
   lower confidence bound published. It is tuned only on a separate development split.
3. **It is disclosed for what it is.** The router also misses the model gates (precision 0.81 against 0.95, `refuse` 1
   of 5, 3 of 5 injections followed before the injection-cue rule); its seed-set score is optimistic because one author
   wrote its lexicons and the set, and its lead over the best Apache models is within the ±0.1 intervals. It is chosen
   because it is the least-bad front, costs no model time and behaves the same every time, not because it passes.
4. **Front qualification gates, in full**: act macro-F1 at least 0.85 with every act at least 0.75; delegate recall at
   least 0.98 and precision at least 0.95; status questions sent as new tasks at most 1%; injection success at most 1%;
   single-call act p95 at most 100 ms on GPU or 200 ms on CPU on the device class the profile will run on; act p95 within
   `act_ms` (300 ms) under Poisson load at the published calls per instance. On the measured shared GPU only
   LFM2.5-1.2B meets the single-call row; LFM2.5-2.6B (144 ms) and Qwen3.5-2B (112 to 162 ms) qualify only on a device
   class that measures 100 ms or less, which fine-tuning cannot change.
5. **Gate sets are not self-referential.** The 68 seed decisions keep their human labels; about 500 more decisions are
   written outside the synthetic generator and labelled by two people who wrote neither the generator nor the router.
   Teacher labels may qualify a profile only after the teacher agrees with the human labels at kappa 0.7 or more, and
   the act-choice rows must pass on both the teacher-labelled and the human sets. Every checkpoint report scores the
   router on the same sets.

### D2. Release 1 is scoped to streaming speech-to-text

Interaction mode on segmented speech-to-text sessions (file-only models) ships in Release 2, after the segmented plan's
Release 1 (18 December 2026) for agent sessions and its Release 2 (29 January 2027) for conversation sessions. W5's
segmented gate rows, the onset tap (owned by W5, reviewed by the segmented team) and the segmented tests move to
Release 2; W5 no longer needs the segmented plan's `TurnEvent::Aborted`, `DetectorSpeechStart` or test kit in Release
1. The two plans touch the same files (voice manager, Smart Turn, speech-to-text result processing); a shared weekly
merge review is a cross-team item.

### D3. Team, sizing and target dates (replaces section 1's durations)

Indicative sizing by the plan author from the chapters' change lists (engineer-weeks; not a commitment):

| Workstream | Lifecycle | R0 | R1 | R2 | R3 | R4 |
|---|---|---|---|---|---|---|
| Runtime (chapter 1) | 0 | 4 | 8 | 3 | 4 | 1 |
| Front model (chapter 2), gateway side | 0 | 5 | 2 | 6 | 2 | 3 |
| Distillation track (machine-learning engineer) | 0 | 3 | 5 | 4 | 0 | 2 |
| Tasks (chapter 3) | 0 | 1 | 7 | 6 | 6 | 1 |
| Policy and human in the loop (chapter 4) | 0 | 2 | 6 | 3 | 3 | 3 |
| Duplex audio (chapter 5) | 0 | 2 | 6 | 5 | 3 | 3 |
| Providers and endpoints (chapter 6) | 5 | 2 | 8 | 3 | 8 | 4 |
| Evaluation, CI and infrastructure (chapter 7) | 1 | 7 | 3 | 3 | 4 | 2 |
| **Total** | **6** | **26** | **45** | **33** | **30** | **19** |

Team assumption: **six gateway engineers and one machine-learning engineer dedicated to this plan**, separate from the
three engineers the segmented plan assumes, plus part-time Bud, legal and operations support. Target dates for that
team (including the year-end break):

| Release | Target window |
|---|---|
| Lifecycle track first patch (Hume) | by 2026-10-23 |
| Release 0 | 2026-10-05 to 2026-11-06 |
| Release 1 | 2026-11-09 to 2027-01-29 |
| Release 2 | 2027-02-01 to 2027-03-12 |
| Release 3 | 2027-03-15 to 2027-04-23 |
| Releases 4 and 5 | scheduled at the Release 3 exit |

Critical path to Release 1: the runtime actor and executor, then the policy core and the task layer, then duplex audio
and the echo gates. With fewer engineers the dates move in proportion; the lifecycle track does not.

### D4. Sign-offs by the date the work needs them

Section 11's groups are replaced by "needed by" dates: **week 1** (before the first lifecycle patch and the distillation
track): 2, 4, 8, the first-patch and detector-removal parts of 9, the teacher-output part of 21, and 28; **before
2026-11-13**: the wire-fix parts of 9; **Release 0 exit review (2026-11-06)**: 1, 3, 5, 6, 7, 11, 12, 13, 14, 15, 16,
18; **before 2026-12-15** (after the 2026-11-30 probe): 22; **before Release 2 (2027-01-29)**: 17, 19, 20, the
transcript and recording parts of 21, 24; **before Release 3 (2027-03-12)**: 10, 23, 25, 27; **before Release 4**: 26.
An unnumbered decision is added: the live provider matrix's monthly budget cap, owned by the product owner, needed
before nightly runs start.

### D5. Bud agents without an answer marker

A Bud agent whose author has not declared `background.answer_marker` gets its answer relayed whole at completion, about
0.4 to 1 s later than today's agent path (W3 estimate). Sign-off 13 is amended so that a person accepts this
regression explicitly. The time-to-answer gate on the `/ws` agent surface applies only to agents that declare the
marker; the others are reported against the agent-path baseline. An operator or alias may declare `answer_marker` for
an agent it knows emits the marker (W3 section 3.17.2). budapp's `answer_marker` field is requested for 2026-11-02.

### D6. Infrastructure, operations and owners

1. A dedicated quiet x86 runner by Release 0 week 2 (absolute timing, CPU-sidecar probes, the x86 CPU run); dedicated
   GPU time before the Release 0 exit (a separate runner or a reserved GB10 window). The local-front CI job stays
   advisory until a model profile ships. Owner: the evaluation lead, with operations.
2. Sign-off 5's default gains an operations owner, a GPU class and a budget for the shared front pool, named at the
   Release 0 exit; GPU provisioning waits for a profile to qualify, while the image, chart and client ship dark. A
   deployment with no qualified device class (including GPU-less on-premises) runs `scripted`.
3. An operations section joins the plan: rollout by canary pods (`WAAV_INTERACTION_ENABLED`), then one pilot Bud
   agent, then agent by agent, with the environment switch as the rollback; on shutdown, no new tasks or speculative
   runs, in-flight side-effecting tool calls finish within the drain window, and unfinished tasks are recorded as
   `unknown` (in-memory tasks in Release 1 do not survive a deploy); alerts and runbooks for the `llama-server`
   supervisor, the post-call task supervisor and the analyser compute pool; sessions per node published from the
   Release 2 load test.
4. Smart Turn never runs on live audio today (the mel buffer is trimmed to 400 samples while 50 frames are needed), so
   today's turns end on the LiveKit text detector, speech-to-text finals or the silence timer. Interaction mode fixes
   this for its own sessions in Release 1; the Release 0 baseline records it; mode-off sessions are unchanged.

### D7. Experience targets, final form

The clip target is split into a gateway-owned part measured from E (at most `ack_due_ms` + transport lead + playout +
jitter) and a per-stack number from the acoustic end, set at the Release 0 exit from the measured median E for each
`ack_due_ms` arm; p50 0.9 s holds only if the median E is at most about 390 ms at `ack_due_ms` 300. Early-turn time to
answer (before fast mode) is reported separately from later turns.

---

## Addendum E (2026-10-05): last items from applying Addendum D

1. **Wh-questions route too.** The router's question step (D1.1) fires on an addressed polar question **or** an
   addressed wh-question ("Where's my refund?"), with the same exclusions (status, social and repeat turns, an open
   question or answer window, fragments ending on a dangling word). Scored on the 68 seed decisions it gives the same
   numbers as the polar-only reading (acceptable acts 0.90, delegate recall 1.00, precision 0.82) and stops a wh-question
   being met with silence. `front_reference.py` has it on by default (`ROUTER_WH_QUESTIONS = True`).
2. **Provider-call provisional runs** are discarded at `adopt_by` + 500 ms. `adopt_by` is per provider (Deepgram 3,000
   ms, Gemini 10,000 ms; estimates to be measured). There is no separate 10,000 ms constant.
3. **The identity token format** is W2's `v1.<payload>.<mac>`; the configuration schema already matches it.
4. **A Release 1 session on a file-only speech-to-text model** runs with interaction off and `config_warning`
   `interaction_surface_unavailable` (`{surface, release: 2}`).
5. **Still to add to the reference router before the Release 0 router gate:** the `injection_cue` step of Addendum B5
   item 4 (the router follows 3 of 5 injections without it).
