# Chapter 3. Background tasks: the task model, backends, speculative start and streamed answers

W1 to W7 name the seven workstreams, one per chapter: workstream 1 runtime, 2 front model, 3
background tasks (this part), 4 policy and human in the loop (HITL), 5 duplex audio, 6 providers and
endpoints, 7 evaluation and rollout.

## What this part is and why it exists

This part owns everything between "the front decided to hand something to the background" and "a
result, question, progress fact or failure is ready to be said". A **task** is one piece of work the
caller asked for, with a lifetime of its own: it survives barge-in, a new speech turn, a correction,
a dropped connection and, when it may have side effects, the end of the call. Tasks run on one of
eight **backends**: a background large language model (the "LLM tier"), a Bud agent through
budprompt, in-process tools, a customer webhook, an Agent2Agent (A2A) agent, a Model Context
Protocol (MCP) server with the Tasks extension, OpenAI Responses with asynchronous tools, and a
text-output DAG (a WaaV processing graph). Two mechanisms make the agent fast: the background
starts at **E** (the confirmed end of the caller's turn), or earlier at W5's **eager end of turn**
(a withdrawable guess that the caller has finished), in parallel with the front's decision (the
**speculative start**); its final answer is relayed **sentence by sentence** as it is written.

Without this part, a barge-in would kill the work (today a text-to-speech clear cancels a Bud run,
`core/agent/engine.rs:914-916`); the agent would say "cancelled" when a booking had gone through; a
correction could commit the old arguments; the front could say nothing true about progress; a
hang-up during a booking would lose the outcome; and answers would arrive later than on today's
conversation path, which speaks each sentence as it is confirmed
(`core/conversation/mod.rs:1366-1409`).

## How it works

**The task model.** States follow A2A 1.0: `submitted`, `working`, `input_required`,
`auth_required`, and terminal `completed`, `failed`, `cancelled`, `rejected` and `unknown` (the
gateway lost track of a task that may have had side effects), plus a `cancelling` flag; these nine
strings are the wire states of every client event. Each task has a session alias (`TaskRef`: "t1",
"t2", never reused), a revision number that rises when the caller changes the request, idempotency
ids for side-effecting operations, and a list of **committed effects** (things known to have
happened). Stopping a run never undoes one, so every cancel and failure carries `effects`: none, a
list, or unknown. An unconfirmed cancel is `task_progress` with `cancelling: true` and the state
unchanged, never `task_cancelled`. A stale result that may have done something is never hidden
("the booking for four had already gone through"): it is a `task_result` with `superseded: true`.

**The registry.** A pure state machine (`TaskRegistryCore`: commands in, events and effects out, no
I/O, testable with a fake clock) sits inside a per-session `TaskHost` that implements W1's
`TaskRegistry` port, runs one driver per backend run, and publishes `TaskEvent`s. It lives outside
W1's actor because tasks outlive it; after the call a process-wide supervisor keeps live tasks.

**From a caller's turn to a task.** W4 turns the front's act into the turn's settled act; W1 calls
the registry: `delegate` creates a task, `update` revises one, `abort` cancels one, answers come from
W4's parser (W4's HITL types, with an `unanswerable` outcome), and W4 adds `retry` and continuation
merges. One utterance causes at most one task operation. A repeat becomes a status request; a
near-duplicate of a live action becomes "change it or a second one?"; a continuer ("thanks",
"hmm") is never a revision; a costly cancel without a cancel phrase is confirmed first. The front
only proposes; the registry checks.

**Corrections and the gate.** Wherever WaaV executes a side-effecting tool (LLM tier, provider
function calls, OpenAI Responses), a gate reads the live revision just before the handler runs,
refuses a stale call, asks the caller from the call's exact arguments, and reserves the operation
first. Where a remote system executes, a correction restarts the run in two phases (the new run
starts only after the old one stopped, with its outcome in hand) or waits for it to end.

**Speculative start.** When W4 asks the front for a decision with `delegate` allowed, W1 calls
`start_provisional(origin, words, adopt_by)`, origin `FrontAtE` at E or `EagerEndOfTurn` at the
eager end of turn (W6 uses `ProviderCall { adopt_by }` for a provider's withdrawable call). The
registry returns a key and, when speculation is on, starts a real run that nobody sees: no task
reference, event, client message or speech. If the settled act is `delegate`, W1's `delegate` names
the key and the run is **adopted**: it becomes the task and its buffered output is applied in order.
Any other act calls `discard_provisional(key, reason)`. Until adoption the gate answers every
side-effecting call and every question to the caller with "wait for adoption"; reads and generation
continue. The **adoption deadline** is E + 600 ms (300 ms act budget, W1's 200 ms slack, 100 ms hop
grace). An eager run has none until E, when W1's `confirm_provisional(key, e_at)` anchors it if W5
reports the same words; different words or a withdrawal discard it, and so does eager start +
1,500 ms if E never comes. On by default for the LLM tier only; for a backend that runs its own
tools (budprompt, webhooks, A2A, MCP, DAG endpoint nodes) an operator's `on` is refused
(`speculative_start_unsafe_backend`) unless it is declared free of side effects.

**Streamed answer.** While a round streams, the gateway cannot see whether it will end in a tool
call (the token callback carries text only, `core/llm/mod.rs:1297-1301`; tool calls are collected
silently, `:1313-1331`). So the background prompt asks the model to begin its final answer with the
line `ANSWER:` (the **answer marker**). Text after it streams; text before it, and any round that
ends in a tool call, is never relayed. Each sentence passes a **release check** (task adopted;
revision current; task not superseded, cancelling or cancelled; outbound speech policy passed), is
cut by the backend's mode-off text pipeline (for the LLM tier, the conversation path's sentence
aggregator) and leaves as `AnswerSentence { seq }`; W4 carries it to W1's one open main-lane relay
item as `StreamSpan { task, attempt, seq_from, seq_to, last }`. A marker followed by a tool call
stops the stream; W4 cuts the audible sentence at its clause boundary and withdraws it; W7 gates
that breach as a rate (proposal at most 2%). A Bud agent streams only if `background.answer_marker`
is declared, by budapp (field requested for 2026-11-02) or, for an agent known to emit the marker,
by the server's `interaction.defaults.agent` or an alias (`core/alias/mod.rs:153`). Otherwise its
answer is relayed whole at `response.completed`, the first point where a draft and a final message
differ (WaaV cannot prompt a Bud agent): an estimated 0.4 to 1 s later than today's agent path,
which speaks each sentence as it arrives (`core/agent/engine.rs:3-4`, `:910-914`). Sign-off 13
accepts this regression (Addendum D5).

| Backend | Release | Answer | Speculative start | Effects visible |
|---|---|---|---|---|
| LLM tier | Release 1 | streams after the marker | on by default | yes (WaaV executes) |
| Bud agent (budprompt) | Release 1 foreground, Release 2 background runs | streams only with a declared marker, else whole at completion | off; only tool-free agents | tool items |
| In-process tools | Release 1 | whole | through the LLM tier | yes |
| Webhook with callback | Release 2 | whole | off; only read-only | reported |
| A2A, MCP Tasks | Release 3 | whole | off (MCP: listed read-only tools) | no |
| OpenAI Responses | Release 3 | streams after the marker | off (billed per run) | yes |
| DAG, text output | Release 3 | streams the text node | off; only without endpoint nodes | no |

**Worked example** (no-tool question, LLM tier, `/ws`, first turn; Release 1's default front, the
rule router, decides at once; times from E):

```text
E+0      "What time does the Boston office open on Saturday?"; acknowledgement due at E+300
E+1      start_provisional(FrontAtE, adopt_by E+600) -> key p7; run started, invisible
E+2      rule router: delegate; W4 settles; W1 delegate(provisional p7) -> adopted as t1; task_created
E+5      streaming request on the wire
E+300    no answer yet -> cached clip "Let me check."
E+420    model writes ANSWER:
E+520    "The Boston office opens at nine on Saturdays." passes the release check -> AnswerSentence 0
E+700    clip ends; W4 sends StreamSpan seq 0..0; W1 plays it on the main lane
E+900    sentence 1 released (same item); E+1,000 stream ends, no tool call -> Completed; span closed
```

With the rule router a start at E gains little; the gain is the eager end of turn (a run started at
E-300 and confirmed at E has sentence 0 ready about E+220). A model front also saves its act time
(warm p95 on a shared GPU: 70 ms LFM2.5-1.2B, 144 ms LFM2.5-2.6B). From the fourth such turn the
acknowledgement moves to E+700 and is skipped.

**Also in this part.** Truthful cancellation ("cancelled" only when the backend confirmed and
nothing was committed; otherwise "I stopped it, but it had already booked..."); progress from
material facts only, at most one a second per task, suggested silent; same-pod reconnect for 60 s
after an abnormal disconnect only; cross-pod continuation, signed push after the call and
`GET /v1/tasks` with an optional task Redis (`WAAV_TASK_REDIS_URL`; `WAAV_REDIS_URL` would switch
Bud mode on); process-wide admission that queues on a 429; an outbound speech policy that blocks
requests for secrets, and numbers, links and "call this" instructions the caller did not give;
refusal of caller words that override the background's rules; the operator's backend catalog and
switches under `interaction.tasks.*`.

**Shutdown and deploys** (Addendum D6.3). A stop signal today cancels the app-wide token
`AppState.shutdown` before the HTTP drain (`state/mod.rs:100-108`, `main.rs:466-470`; the TLS path
then waits up to 30 s, `:536-538`). The task host and post-call supervisor watch it: from then on no
task or speculative run starts, provisional runs are discarded, side-effecting tool calls already in
flight finish within the drain window, and each unfinished task is recorded as `unknown` with its
known effects. Release 1 tasks live in memory only and do not survive a deploy; a durable backend's
task continues on another pod only with the task Redis (Release 2).

**Key numbers.**

- Front act budget 300 ms; W1's backstop 500 ms; the Release 1 rule router decides at once.
- Adoption deadline E + 600 ms; eager ceiling start + 1,500 ms (Addendum A1); provider calls W6's
  `adopt_by` + 500 ms.
- Acknowledgement due at E + 300 ms, or at E + 700 ms and skipped if an answer sentence is ready,
  once the rolling median from E to the first sentence is under 700 ms; hard limit E + 2,000 ms.
- Time to answer on no-tool turns: no worse than the Release 0 baseline + 150 ms at p50 (target);
  on the `/ws` agent surface it gates only Bud agents that declare the marker, the others are
  reported against the agent-path baseline (Addendum D5).
- Speculation stays on if it gains at least 150 ms at p50 and wastes at most 25% of background
  tokens (W7's proposal, sign-off 6); eager end of turn about 200-450 ms before E (W5 estimate).
- Marker followed by a tool call: at most 2% of streamed answers (proposed gate).
- budprompt spends 0.95-1.57 s before the model call (Bud spec 025); a Bud resume tail costs about
  20 database reads a second per live run (from its code; to be measured).
- Per session: 3 active and 2 queued tasks, 50 per session, 200,000 background tokens; cancel
  confirmation waited for 10 s. Gateway overhead under 20 ms per delegate (estimate).

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| Task model | A2A states, revisions, operation ids, committed effects | every backend speaks a close variant; truthful cancel needs effects | more states than today's turn status |
| Where the registry lives | pure core in a per-session host, outside W1's actor | tasks outlive the actor; every safety rule is unit-testable | one short lock per call; a supervisor per process |
| Speculative start | LLM tier on; effects and questions held until adoption; deadline at E; remote executors refused unless side-effect free (Addendum C2.2) | the request leaves before the act, or 200-450 ms before E when eager; remote tools cannot be held | tokens of discarded runs; little gain at E with the rule router; remote backends gain nothing |
| Provisional-run port | a registry-minted key, always returned; `confirm_provisional` at E | one code path for W1 and W6; a late discard never hits a newer run | a small record per key |
| Which text is the answer | the `ANSWER:` marker; drafts never relayed | the stream reveals a tool call only at its end | 2-3 tokens per answer; a model that omits the marker answers at round end |
| Bud agents without a marker | relayed whole at `response.completed` (Addenda C1.4, D5) | WaaV cannot prompt a Bud agent; budprompt marks no final item | 0.4-1 s slower than today's agent path; sign-off 13 accepts it |
| Relay text pipeline | the backend's mode-off pipeline | byte-identical speech | two pipelines in step |
| Corrections | revision gate where WaaV executes; two-phase restart elsewhere | no stale commit; a restart never runs two bookings | a restart waits for the old run (seconds) |
| Cancellation wording | "cancelled" only if confirmed and nothing committed | the agent never says something untrue | more wording cases for W4 |
| Bud concurrency | one main run per conversation; side runs opt-in | budprompt serialises a conversation | a quick Bud question may queue behind a long one |
| Task persistence | memory always; task Redis optional, its own setting | `WAAV_REDIS_URL` turns Bud mode on | one more setting; cross-pod continuation only with Redis |
| Backend text | attributed, never "verified"; outbound speech policy | blocks injected numbers and links | some false blocks, measured by W7 |
| DAGs | text-output graphs only; audio-output graphs refused | node calls bypass the gate; duplex DAG audio is later work (W5) | speech-to-text, translate, text-to-speech DAGs get no interaction mode |

## What changes in the code

All new files are under `gateway/src/core/interaction/tasks/` unless stated.

| File | Function or type | Change | New or modified | Release |
|---|---|---|---|---|
| `core/interaction/contracts.rs` | `TaskRef`, `TaskState`, `TaskEvent`, `ProvisionalKey`, `TaskRegistry`, other task types | golden file `task_contract.json` | new (shared) | 0 |
| `core.rs`, `model.rs`, `host.rs`, `view.rs` | `TaskRegistryCore`, `TaskHost`, `TaskGate` | state machine, limits, duplicates, revisions, retry, events, views | new | 1 |
| `provisional.rs` | `start_provisional`, `confirm_provisional`, `discard_provisional` | speculative runs, adoption, deadline, eager ceiling, unsafe-backend refusal | new | 1 (provider origin 3) |
| `answer.rs` | `AnswerFilter`, `ReleaseRule` | marker detection, release rules | new | 1 |
| `backend.rs`, `driver.rs`, `admission.rs`, `progress.rs`, `input.rs`, `speech_policy.rs`, `prompt.rs`, `config.rs` | `TaskBackend`, `BackendAdmission`, `OutboundSpeechPolicy`, `TaskConfig`, `TaskServerConfig` | backends, admission, progress, input, prompt; `interaction.tasks.*` | new | 1 |
| `backends/llm_tier.rs`, `tool.rs`, `bud_agent.rs` | LLM tier, single-tool tasks, Bud foreground runs | own loop around `execute_batch_in_task` | new | 1 (Bud background runs 2) |
| `store.rs`, `supervisor.rs`, `push.rs` | `TaskStore`, `TaskRedis`, `TaskSupervisor` | store, relay, push, sweeper; drain on `AppState.shutdown` | new | 1 (memory, drain), 2 (Redis) |
| `backends/webhook.rs`; `handlers/tasks.rs` | webhook backend; callback and `GET /v1/tasks` routes | | new | 2 |
| `backends/a2a.rs`, `mcp.rs`, `openai_responses.rs`, `dag.rs` | protocol clients; `DagBackend` | | new | 3 |
| `core/llm/functions.rs` | `FunctionCallParams`, `RegistryItem`, `execute_one_with`, `ToolGate`, `GateDecision` | gate with `AwaitAdoption`; cooperative async cancel; mode-off paths unchanged | modified | 1 |
| `core/agent/brain.rs` | `task_events()`, `open_task`, `resume`, `submit_approval` | task-path decoder; `AgentEvent` unchanged | modified | 1, 2 |
| `core/conversation/mod.rs` | `background_client_config`, `validate_llm_url` | background settings; visibility | modified | 1 |
| `dag/context.rs`, `dag/executor.rs` | `DAGContext.progress_tx` | optional per-node progress hook | modified | 3 |
| `state/mod.rs`; `main.rs` | `AppState.tasks`, `.task_redis`; task router | lazy supervisor; routes only when the mode is on | modified | 1, 2 |
| `bud-auth/src/voice_agent.rs` | `background` block | `answer_marker` (Release 1); `voice_approvals`, `side_runs` | modified | 1, 2 |

`core/llm/mod.rs`, `ServerConfig` and its literals are not touched. Server settings join the one
`interaction` key parsed outside `ServerConfig`.

## What this part gives to and needs from the other parts

**Gives.** The `TaskRegistry` port (the three provisional-run calls, `delegate` with `provisional`,
`revise`, `cancel`, `retry`, `answer`, `provider_input`, `heard`, `board`, `subscribe`,
`events_since`, `session_ended`, `reattach`); `TaskEvent` with `AnswerSentence { seq }` and
`AnswerStopped`; the nine states, `cancelling`, `EffectsKnowledge`; `TaskRef`, `TaskId`,
`Revision(u32)`, `RevisionRecord`; `TaskView`, `TaskBoard`; `interaction.tasks.*` (session and
server); the background prompt; `TaskRedis` for caller memory.

**Needs.**

- W1: the provisional-run calls, `confirm_provisional` on matching words; adoption or discard from
  the settled act; `UserUtterance`, `UserUttId`, the heard snapshot and `HeardReport`;
  `TurnDecisionMax`; a session-end reason separating a clean close from an abnormal disconnect
  (today one exit, `SessionLoopExit::Closed`, `handlers/ws/handler.rs:496-498`).
- W2: `act_ms`; `ActDecision`, `DelegationKind`, `TaskBinding`, `FrontDecisionId`; rendering of
  `TaskView`s; the persona's background preamble; the `CallerMemory` port on `TaskRedis`.
- W4: `HitlRequest`, `HitlAnswer`, `HitlKind`, `HitlOutcome`, `Binding`, `ActionProposal`;
  `DeliveryPolicy`; settled acts and `TaskCommand` (with `Continuation` and `Retry`); the lexicons;
  `auth_wait_timeout_ms`; `holds_session_open()`; its own check of each sentence, `StreamSpan`s,
  and a stream-stop row per `AnswerStopped` reason (`MarkerThenToolCall`, `RunFailed`, others).
- W5: E, the eager end of turn, its withdrawal and whether E's words match; the class `interrupt`;
  `suspected_echo`; the silence ceiling.
- W6: the `InteractionConfig` envelope and server key; client event names (`task_created`,
  `task_progress`, `task_question`, `task_answer_received`, `task_result`, `task_cancelled`;
  `bud.*` on `/v1/realtime`); codes `interaction_dag_audio_output`,
  `speculative_start_unsafe_backend` and one for a DAG with no text output (requested); provider
  calls as `ProviderCall { adopt_by }`.
- W7: gates for side effects before adoption, no draft leak (`no_marker`, `draft_dropped`), the
  marker-then-tool rate, time to answer per Addendum D5; wasted work per start arm.
- Other teams (requested dates): budapp publishes the agent's `background` block, `answer_marker`
  by 2026-11-02 (the server default or an alias covers the gap); budprompt confirms
  `durable_engine_enabled`, a per-run voice-rules field and a final-answer marker, and budgateway
  proxies the status and resume `GET`s and returns 429 before creating a run, by 2026-11-30.

## Tests to write first

| Test name | Level | What it proves | Release |
|---|---|---|---|
| `task_contract_golden_file_matches` | unit | wire states and event names are stable | 0 |
| `cancel_unconfirmed_maps_to_task_progress_with_cancelling_and_unchanged_state` | unit | no early "cancelled" on a screen | 0 |
| `interaction_off_session_constructs_no_task_host` | in-process | mode off changes nothing | 0 |
| `cancel_after_done_operation_never_reports_plain_cancelled` | unit | committed effects are always reported | 1 |
| `stale_revision_tool_call_is_refused_before_execution` | unit | no stale arguments where WaaV executes | 1 |
| `one_utterance_produces_at_most_one_task_operation` | unit | front and parser cannot both act | 1 |
| `barge_in_never_reaches_task_registry` | in-process | speech interruption never cancels work | 1 |
| `delegate_naming_provisional_adopts_the_same_run_without_restart` | unit | adoption reuses the run | 1 |
| `side_effect_call_of_a_discarded_run_never_runs` | in-process paused time | the gate holds effects before adoption | 1 |
| `adoption_deadline_is_anchored_at_e_not_at_the_provisional_start` | in-process paused time | an eager run confirmed at E lives to E+600 | 1 |
| `unconfirmed_eager_run_is_discarded_at_its_start_plus_1500_ms` | in-process paused time | no orphan speculation | 1 |
| `speculative_on_for_a_remote_executor_is_refused_with_warning` | unit | no unholdable side effect | 1 |
| `speculative_commit_before_task_created_is_zero` | scripted audio | W7's zero-tolerance gate | 1 |
| `a_round_ending_in_a_tool_call_releases_nothing` | unit | drafts never relayed | 1 |
| `sentences_after_the_marker_release_before_the_round_ends` | unit | the answer streams | 1 |
| `llm_answer_sentences_are_byte_identical_to_the_conversation_path` | unit | relay equals today's speech | 1 |
| `bud_without_marker_releases_only_after_response_completed` | integration, mock budprompt | Bud drafts never relayed | 1 |
| `answer_marker_from_server_default_or_alias_streams_a_bud_answer` | integration, mock budprompt | the marker works before budapp ships it | 1 |
| `clean_close_skips_the_reconnect_grace_and_orphans_at_once` | in-process | only abnormal disconnects wait | 1 |
| `shutdown_starts_nothing_new_and_records_unfinished_tasks_as_unknown` | in-process paused time | a deploy never hides an effect | 1 |
| `reconnect_on_other_pod_resumes_durable_backends_only` | integration, Redis | truthful cross-pod continuation | 2 |
| `gemini_withdrawn_call_cancels_side_effect_free_task_but_not_running_operation` | integration | provider withdrawals never abandon a write | 3 |

## What ships in which release

Target windows from Addendum D3 (six gateway engineers, one machine-learning engineer). This part:
about 1, 7, 6, 6 and 1 engineer-weeks in Releases 0 to 4, on Release 1's critical path.

- **Release 0, measure and lay groundwork (2026-10-05 to 2026-11-06).** The task part of the
  contract file with its golden file; W4's HITL types imported; the budprompt and marker
  measurements below.
- **Release 1, cascade interaction on `/ws` (2026-11-09 to 2027-01-29).** Sessions on streaming
  speech-to-text only (Addendum D2). Task model, registry, host, drivers, admission, in-memory
  store, shutdown drain and same-pod reconnect; corrections, truthful cancel, duplicate policy,
  retry; speculative start for the LLM tier at E and at the eager end of turn; streamed answers;
  Bud foreground runs; LLM tier and in-process tools with the gate; outbound speech policy;
  `TaskView`s; the server catalog.
- **Release 2, full duplex audio, the GA cascade and durable tasks (2027-02-01 to 2027-03-12).**
  Interaction mode, and so tasks, on segmented (file-only) speech-to-text sessions (Addendum D2);
  Bud background runs with resume, streaming from the tail, approvals by voice where the author
  allows, side runs where allowed; webhook backend; task Redis, cross-pod relay, post-call push,
  `GET /v1/tasks`, phone-line continuation (opt-in); `TaskRedis` for caller memory.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs (2027-03-15 to
  2027-04-23).** Provider-originated tasks and provider speculative calls; A2A, MCP Tasks, OpenAI
  Responses; `DagBackend`; orphan sweeper.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF (scheduled at the Release 3
  exit).** Hindi lists for the utterance lexicon and the speech policy, after native-speaker review.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| Speculation wastes more tokens than it saves | medium | cost with little gain | counters per outcome and start arm; W7's paired runs; `auto` off or a delegate-share threshold |
| A remote executor commits before adoption | low (refused by design) | an action nobody asked for | `on` refused unless declared free of side effects; W7 zero-tolerance gate |
| Model omits the marker | medium | answer waits for its round end | counted; reported against the baseline |
| Model writes the marker, then calls a tool | low | a draft-like sentence heard | stopped at a clause boundary; W4 withdraws; gated at 2% |
| Bud agents never get a marker | medium | answers 0.4-1 s slower than today | accepted (sign-off 13); server default or alias; ask budprompt |
| Deploy during a task (Release 1) | high | in-memory tasks lost | drain; in-flight writes finish, the rest `unknown` |
| budprompt durable engine off | unknown | Bud tasks foreground only | detected per run; measured in Release 0 |
| Resume tails load budprompt's database | medium at scale | about 4,000 reads a second at 200 runs | poll only when idle; load test; push asked of budprompt |
| Credential expires after the call | high for long Bud runs | outcome unreadable | `unknown` with the response id, pushed |
| Speech policy blocks a real answer | medium | caller hears a template | false blocks measured; patterns tunable |

## What is still open

**Measurements needed.**

- Whether target budprompt deployments run `durable_engine_enabled`, how a background run surfaces
  an approval pause, and whether it honours the request's `instructions` or can mark the final item.
- Marker compliance and marker-then-tool breaches on the candidate background models (Inkling and
  Inkling-Small among them).
- Wasted provisional tokens per 100 turns and the paired time-to-answer gain, for the start at E and
  the eager start separately (W7, Release 1); E to first answer sentence per backend.
- budgateway's proxying and 429; resume-tail database load at 200 calls; admission values;
  speech-policy false blocks.

**Choices that need a person (default in brackets).** Needed by the Release 0 exit review
(2026-11-06): 6, 13, 14, 15, 16; before Release 2: 20, 24 (Addendum D4).

- Speculative start default, sign-off 6 [on for the LLM tier; off for Bud and external backends].
- Bud agents in Release 1 via the server default or an alias, sign-off 13 [yes, accepting the
  slower whole answer of agents without a marker].
- Spoken approvals of budprompt escalations, sign-off 14 [off unless the agent author allows].
- Session end and reconnect grace, sign-off 15 [cancel tasks with no side effects and no push
  target; finish possible actions; 60 s same-pod grace after an abnormal disconnect only].
- Caller words in Redis and caller-memory key and retention, sign-off 16 [`store_content: false`;
  memory off].
- Bud side runs, sign-off 20 [off unless the author opts in]. Phone-line continuation, sign-off 24
  [off].
- Speech policy strictness and costly-cancel confirmation [always on; confirm costly cancels].

## Where the detail is

- Full design: `/home/bud/ditto/waav/research/duplex-agent/design/W3-background-tasks.md` (3.16
  provisional runs, 3.17 streamed answers, 3.13 configuration, 13 and 14 alignment with the
  decisions); critiques `critique-W3-background-tasks-code.md` and `-adversarial.md` beside it.
