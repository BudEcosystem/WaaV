# Chapter 1. The interaction runtime: one actor per call, the floor, speech lanes and the timeline

Workstream codes, one per chapter: W1 the interaction runtime (this chapter), W2 the front model, W3 background tasks,
W4 policy and human in the loop, W5 duplex audio, W6 providers and endpoints, W7 evaluation and rollout.

## What this part is and why it exists

The interaction runtime is the per-call core of interaction mode in the WaaV gateway. For each voice session it runs
one *actor*: a single tokio task that owns the call's interaction state and changes it only by handling inbox
messages one at a time. The actor holds the *floor* (who may speak now: the caller, the agent or nobody), the
*speech lanes* (front for acknowledgements and progress, main for answers, questions and confirmations, backchannel
for "mm-hmm" clips), the *lane executor* that plays what the policy engine schedules, the metered playback pump and
the *timeline* (a session clock plus a record of what the caller heard). It hosts the other workstreams' pure
machines, talks to the background task registry, fires the background at the end of the caller's turn and plays its
answer sentence by sentence.

Without it, the gateway keeps state spread over shared locks and callbacks, the source of the interleaving bugs WaaV
keeps fixing (history interleave, epoch re-checks, "working counts as speaking"). The conversation path runs its
whole LLM turn inside the speech-to-text callback, so the caller cannot barge in during generation, and the playback
pump does not meter, so a duck or stop acts on audio already sent. A duplex agent needs "the answer arrived while the
caller is mid-sentence and a confirmation is pending" to be one decision, not a race.

Smart Turn, the gateway's audio end-of-turn model, never runs on live audio today: the mel extractor rebuilds its
frames from a buffer trimmed to 400 samples (`core/smart_turn/mel_extractor.rs:411`, `:428-431`), but inference
needs 50 frames (`core/smart_turn/processor.rs:686`, default `:117-118`). Turns end on the LiveKit text detector,
speech-to-text finals or the silence timer. Interaction mode fixes this for its own sessions in Release 1 (W5's
analyser runs Smart Turn on its buffered utterance); the Release 0 baseline records it; mode off is unchanged
(Addendum D6.4).

## How it works

**One actor, pure machines inside.** Callbacks never block: the speech-to-text callback does a `try_send`. W5's audio
analyser, the only voice activity detector, sends `DuplexEvent`s on a bounded channel (64 slots, drops reported); the
runtime builds W4's `VadOnset` from its `VoiceOnset`. Each message runs through pure, synchronous machines in a fixed
order: W5's duplex engine (end of turn, barge-in, echo), the turn controller (W5's `ClassifiedStart` and
`EndpointStop`), the floor machine, W4's `PolicyEngine::step`, the provisional-run tracker and the lane executor.
Effects never await I/O: model calls, task starts and TTS submissions are spawned or queued. A supervisor catches a
panic, rebuilds state from a snapshot, keeps the front call and the task registry, and tells W4 `Restarted`; a second
panic within 60 s ends the session (`interaction_failed`).

**The floor** has six states: `Listening`, `Thinking` (a reply is owed, a task runs or an answer is parked),
`UserSpeaking`, `FrontSpeaking`, `MainSpeaking` and `Yielding` (the caller started over the agent; W5's machine ducks,
pauses and decides). A property test checks liveness: every state except `Listening` has audio playing or a timer
armed. Seven backstops guarantee it: the end-of-turn watchdog, the no-caller-transcript timer, the turn-decision
backstop, the yield backstop, the thinking backstop, the level drain check and the stream-gap hold.

**W4 orders, W1 executes.** The policy engine decides what is said and when. The executor holds one playing
utterance, one next utterance and one parked streamed answer. It adds only mechanical interlocks: nothing starts
while the caller holds the floor; only replies to the caller's latest turn start before that turn is settled; only
replies start inside W4's 4,000 ms answer window after a question. Stops are per utterance. A stopped item that was
never heard goes back to W4 as `Returned`; one cut while audible is `Dropped`, with the characters heard.

**The background starts at E** (E is the confirmed end of the caller's turn). When W4 asks the front for a turn
decision with `delegate` allowed, the runtime calls W3's `start_provisional` in the same step: a *provisional run* on
the caller's words, named by a `ProvisionalKey`, with no task id, never spoken, side effects held. It can also start
one at W5's *eager end of turn*, a revocable early guess; that run has no deadline until E's words match and the
runtime calls `confirm_provisional(key, E)`. The adoption deadline is E + 600 ms, anchored at E (300 ms act budget,
200 ms slack, 100 ms hop grace); W3 cancels an eager run 1,500 ms after its start if E never comes (Addendum A1).
When W4 settles the turn with `adopt: true`, W3 adopts the run as the task with no restart. Otherwise the runtime
calls `discard_provisional` with W3's reason: another act, a withdrawn guess, changed words, a newer turn, a turn
unsettled after the 500 ms backstop, the step-down (`StepDown`) or the session end.

**The answer streams.** W3 releases each checked sentence of the final answer as `AnswerSentence { seq }`; text
before the `ANSWER:` marker and rounds ending in a tool call are never relayed. W4 sends each sentence as the next
`StreamSpan { task, attempt, seq_from, seq_to, last }` of one open main-lane item, and a `Speak` with no clauses and
`last = true` closes it. Relay text is the background's own words, byte for byte, never passed through the front
model. A gap between sentences keeps the floor for up to 600 ms; then the answer is *parked*: the floor becomes free,
W4's "One moment." clip can play at 2,000 ms, and the next sentence resumes it. A Bud agent streams only when it
declares `background.answer_marker`; otherwise its answer is relayed whole at completion.

**Heard time is one function.** `heard_at = delivered_at + lead(transport) + transport_lag_ms(transport)`. The
metered pump paces 20 ms frames on a playout clock, so this equals the frame's playout start plus the transport lag.
A cut utterance commits only its heard prefix to history; the rest is silent context.

**Shutdown.** On SIGTERM the gateway cancels an app-wide shutdown token before the HTTP drain (`state/mod.rs:108`;
`main.rs:536`, `:562`). From then on the actor starts no new task and no provisional run; side-effecting tool calls
already running finish within the drain window (30 s on TLS, `main.rs:538`); W3 records unfinished tasks as
`unknown`, since Release 1 keeps tasks in memory and a deploy loses them. Rollout: canary pods
(`WAAV_INTERACTION_ENABLED`), one pilot Bud agent, then agent by agent; the switch is the rollback (Addendum D6.3).

**Worked example** (LLM background, streaming speech-to-text, `/ws`; t = ms after the caller stops; illustrative;
the act is slower than Release 1's rule router, to show the speculative start):

| t | What happens |
|---|---|
| 150 | W5's eager end of turn: provisional run started on "is my Lisbon booking still confirmed", no deadline yet; the request leaves at about 155 |
| 400 | E confirmed with the same words: `confirm_provisional`, adoption deadline E + 600 = 1,000; front decision released |
| 420 | act `delegate`, `adopt: true`: the run is adopted, `task_created` goes to the client |
| 700 | acknowledgement due at E + 300 ms: cached clip "Let me check that." leaves the gateway; heard at about 910 (150 ms lead, 60 ms playout), over 0.9 s as E is 400 |
| 1050 | first answer sentence (`AnswerSentence` 0) released; queued behind the clip, pre-synthesised |
| 1600 | clip ends; answer starts with no synthesis gap; later sentences append |
| 4800 | last sentence drains; floor back to `Listening` |

In fast mode (rolling median from E to the first sentence under 700 ms after three turns) the acknowledgement is due
at E + 700 and skipped if the answer is ready; the first answer audio leaves at about E + 710, as on today's
conversation path, and is heard at about 1.32 s, inside the 1.6 s p50 limit. Without the speculative start the
request would leave about 265 ms later.

**Key numbers.**
- Front act budget 300 ms (W2); turn-decision backstop 500 ms (act budget + 200 ms, decision 2.3). Release 1's rule
  router decides at once; LFM2.5-1.2B's warm act p95 is 70 ms on a shared GPU (Addenda B3, B7).
- Acknowledgement due E + 300 ms, or E + 700 ms for a fast backend (decision 3.3); clips at most 24 characters
  (Addendum C6.2).
- Pre-emption at a clause boundary within 400 ms; answer window 4,000 ms (both W4's; to be measured).
- Yield backstop about 7,700 ms (W5's longest episode + 1,000 ms, decision 5).
- Stream-gap hold 600 ms (a judgement, to be measured); stall clip at 2,000 ms (W4).
- Pump frames 20 ms. Leads: LiveKit 40, `/ws` 150 ms. Lags: `/ws` 150, LiveKit 80, SIP 150 ms (to be measured).
- Eager end of turn 200-450 ms before E (W5 estimate).
- Runtime overhead under 5 ms per message at p99 (target). Timeline at most 400 entries, about 120 KB.
- Experience targets (decision 3.4, Addenda C6, D7). The cached-clip first audible response is split: a
  gateway-owned part from E, at most `ack_due_ms` + transport lead + playout + jitter (`/ws`: 300 + 150 + 60 ms plus
  jitter), and a per-stack number from the acoustic end, set at the Release 0 exit from the measured median E per
  `ack_due_ms` setting; p50 0.9 s holds only if the median E is at most about 390 ms at `ack_due_ms` 300. Fast mode:
  time to answer within the Release 0 baseline + 150 ms at p50, first sound by 1.6 s p50. Early-turn time to answer
  (before fast mode) is reported apart. Unexplained silence: target 2 s, hard 3 s.
- Barge-in (Addendum C6.5): the duck audible at the ear p50 ≤ 0.3 s from onset; the agent silent (paused or cut)
  p50 ≤ 0.7 s at a `/ws` ear (400 ms pause, onset detection, 150 ms lead); a 250 ms pause is a listening test.
- Thinking Machines Lab's 0.40 s (Full-Duplex-Bench v1) is out of reach for a cascade.

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| Concurrency model | One actor per session; hot-path readers use a `watch` snapshot | Removes the interleaving bug class; every rule replayable with paused time | One channel hop per signal |
| Engine convergence | In interaction mode neither `AgentEngine` nor `ConversationOrchestrator` is built; their brains become W3 task backends | One implementation for `/ws`, LiveKit, SIP and the GA cascade; mode off unchanged | `resolve_llm_legs` keeps the URL checks and the Bud rewrite |
| Scheduling | W4 orders, W1 executes, utterance-scoped stops | Two schedulers disagreed and a lane-wide stop lost a second task's answer | W4's interface is the most-edited seam (in `contracts.rs`) |
| Front calls | W2 owns supersede, cancel-and-rewrite, the record, admission and its breaker; W1 keeps priority classes and the 500 ms backstop | One owner per call lifecycle (decision 2.3); cancel-and-rewrite reuses the cache (Addendum B4) | W1 has no front rules or sizing of its own |
| Voice activity | W5's analyser is the only VAD; builds without the VAD features refuse the mode | One detector per onset; barge-in needs an onset | Such builds get no interaction mode |
| Speculative start | Start at E and at the eager end of turn; confirm at E; adopt on `adopt: true` | The request leaves when the turn ends, up to about 450 ms before E from an eager start | Tokens of discarded runs; off for Bud and external backends unless declared side-effect free (sign-off 6, Addendum C2.2) |
| Streamed relay | One open utterance per answer, explicit close, parked after a 600 ms gap | One GA response and one answer window per answer; the caller can speak into a slow gap | A third executor slot and new floor rows |
| Pump | Metered constructor for interaction sessions only, 20 ms frames | Today's pump does not bound the transport; the legacy pump stays untouched (sign-off 9) | Two pacing paths in one file |
| Audio attribution | Per-provider boundary classes (`PerRequest`, `InsideSpeak`, `CompletionOnly`) | 18 providers bypass both shared TTS paths | A gap at a lane switch for `CompletionOnly` |
| Step-down | For an unsupported language, `/ws` conversation and agent sessions hand over to the plain path mid-call | Decision 9, Addendum C7.1; the callback and smart-turn slots are replaceable | Fresh history; other surfaces repeat the request and stay in English |
| Server switch | One `interaction` key outside `ServerConfig`: same YAML file, else `WAAV_INTERACTION_*` variables | One switch, no edits to config literals (Addendum C7.4) | A second YAML parse; `main` records the source |

## What changes in the code

Paths are under `gateway/src`.

| File | Function or type | Change | New or modified | Release |
|---|---|---|---|---|
| `core/interaction/contracts.rs` | W1's shared types | `TimelineMs`, `UtteranceId`, `LaneEvent`, `PlaybackEvent` with `heard_at`, `SpeakPurpose`, `SpeakTag`, `StreamSpan`, `InteractionEvent`; golden test | new | Release 0 |
| `core/interaction/server_config.rs`, `state/mod.rs`, `main.rs` | `InteractionServerConfig`, `AppState.interaction` | One server key from the same YAML file or `WAAV_INTERACTION_*` variables; `main` (`:155-160`) records the source before `AppState::try_new` (`:208`; built at `state/mod.rs:552`) | new / modified | Release 0 |
| `core/conversation/{legs,mod}.rs`, `core/llm/mod.rs` | `resolve_llm_legs`, `validate_llm_url`, `stream_messages` | Shared LLM-leg constructor and history-free streaming (identity refactors); `spawn_turns` flag, off | new / modified | Release 0 |
| `core/audio/playback_pump.rs`, `playback_queue.rs` | `spawn_metered`, `clear_utterance`, open flag, `PlaybackTag` | Playout clock, 20 ms frames, W5's lanes and gates, events with `heard_at`, gap causes | modified (dark) | Release 0 |
| `core/tts/{base,provider,websocket}.rs`, provider directories | `on_utterance_start`, `boundaries()`, `RequestEntry`, `Cleared` notify | Audio attribution by boundary class | modified | Release 0 |
| `core/voice_manager/{manager,callbacks}.rs` | `speak_tagged`, `enqueue_clip`, `capture_clip`, stop set, tag state | Tagged speech, stops by utterance, silent clip rendering | modified | Release 0 dark, Release 1 |
| `core/interaction/{actor,supervisor,floor,executor,speculate,front_sched,timeline,views,clock,perceive,events,host,ports}.rs` | `InteractionRuntime`, `FloorMachine`, `LaneExecutor`, `Speculation`, `Timeline` | The runtime | new | Release 1 |
| `handlers/ws/{config_handler,agent,state,processor,audio_handler,handler}.rs` | brain selection, `initialize_agent_loop`, entry points | Branch to `host::install_ws`; route `clear`, `speak`, `truncate`, `agent_input`, `audio_end`, `task_answer`, `task_cancel`; step-down re-entry; teardown tells W3 if the close was clean (no reconnect grace) | modified | Release 1 |
| `handlers/ws/messages.rs` | audio send variant, segment marker, `error` | Delivery-reporting send; additive events (W6 names); optional `code` on `error` (today only `message`, `:372-376`; Addendum A2 item 9) | modified | Release 1 |
| `handlers/openai_realtime/cascade.rs` | `CascadeGa`, `audio()`, `start_core`, `apply` | One GA response per segment; markers open responses | modified | Release 2 |
| `gateway/tests/interaction_mode_off_golden.rs`, `interaction_runtime_e2e.rs` | golden traces, end-to-end | Mode-off proof; streaming mock LLM end-to-end | new | Releases 0 and 1 |

## What this part gives to and needs from the other parts

**Gives:** `InteractionRuntime`, `FloorState`, `SpeechLane`, `Timeline`; the executor interface
(`LaneEvent::{Started, ClauseBoundary, Done, Dropped, Returned}` with `heard_at`; the one open main-lane relay item
with `StreamSpan` appends and the explicit close); `SpeechPort` and `SpeakPurpose` (W6's `ProviderVoice` implements
the port, with `mark_stopped -> PumpStopReport` and `set_stream_open`); `PlaybackEvent` with `heard_at` and
`estimated`, and the `heard_at` function (W4 adds `heard_margin_ms`); `TimeView`, `HeardNote`, `OverlapNote`,
`CallerWords` for W2 and `HeardHistory` for W3; `InteractionEvent`s for W6's wire events and
W7's `InteractionMeter`; `FloorSnapshot`; clip rendering for W5's `ClipBank`; the server switch;
`interaction.runtime.*`.

**Needs:**
- **W2:** `FrontModel`/`FrontCall` (`decide`, `settle`, `promote`, `discard`), `FrontEvent`, `act_ms`.
- **W3:** `TaskRegistry` with `start_provisional(origin, words, adopt_by) -> ProvisionalKey` (origins `FrontAtE`,
  `EagerEndOfTurn`), `confirm_provisional(key, e_at)`, `discard_provisional(key, DiscardReason)` (including
  `StepDown`), `delegate` with `provisional: Option<ProvisionalKey>`; `TaskEvent` with `AnswerSentence { seq }` and
  `AnswerStopped`; `events_since`, `TaskView`, `DagBackend` (Release 3); the `TaskRef(String)` newtype.
- **W4:** `PolicyEngine::step` with `RequestFront`, `TurnSettled { adopt }`, `Speak` with `StreamSpan` and the empty
  closing `Speak`, `StopAtBoundary`, `StepDown { LanguageUnsupported }`; `VadOnset`; `answer_expected()`,
  `holds_session_open()`, and `PolicyConfig` with `answer_window_ms`.
- **W5:** `DuplexEngine` (`BargeInFsm`, classifier, `EndpointPolicy` with `EagerEndOfTurn`, `EagerWithdrawn`,
  `EndpointDetected`), `DuplexEvent::VoiceOnset`, `EndOfTurnReport` with `eager: EagerMatch`, `ClassifiedStart`,
  `EndpointStop`, `PlayoutControl`, `PumpStopReport`, `ClipBank`, `BargeInConfig::longest_episode_ms()`.
- **W6:** `InteractionConfig` homes, `Regime`, `ProviderCapabilities.owns_turns`, `ProviderVoice`, `ClipRenderer`,
  wire names and `config_warning` codes (the runtime uses `interaction_missing_build_features`,
  `interaction_language_step_down`, `interaction_surface_unavailable`; `floor_state` reason `stream_gap`).
- **W7:** the audio-scored harness and release gates.

## Tests to write first

Paused clock: in-process, on tokio's paused time.

| Test name | Level | What it proves | Release |
|---|---|---|---|
| `interaction_off_conversation_session_wire_trace_matches_golden` (and agent, pump-switch-on) | scripted audio | Mode off changes nothing on the wire | Release 0 |
| `runtime_contract_golden` | unit | W1's shared types cannot change alone | Release 0 |
| `resolve_llm_legs_refactor_keeps_bud_rewrite_url_check_attribution_and_fatal_handler` | unit | Skipping the orchestrator skips no safety check | Release 0 |
| `env_only_server_reads_interaction_from_waav_interaction_variables` | unit | The key loads without a YAML file | Release 0 |
| `pump_keeps_transport_ahead_within_lead_for_a_3s_cached_unit` | paused clock | The pump meters | Release 0 |
| `stt_callback_returns_before_the_turn_finishes` | paused clock | No inline turns in interaction mode | Release 1 |
| `floor_liveness_property_every_non_listening_state_has_audio_or_a_timer` | unit (property) | No state without a way out | Release 1 |
| `turn_decision_max_lets_w4_settle_then_discards_an_unadopted_run_with_deadline` | paused clock | The 500 ms outer net | Release 1 |
| `confirm_provisional_is_called_with_e_and_the_deadline_is_e_plus_600ms` | paused clock | Eager runs are anchored at E | Release 1 |
| `delegate_adopts_the_provisional_run_without_restart` | paused clock | Adoption keeps the same run | Release 1 |
| `side_effecting_tool_before_adoption_never_commits` | paused clock | The side-effect gate holds | Release 1 |
| `vad_onset_reaches_w4_from_the_analysers_voice_onset` | unit | One onset source | Release 1 |
| `streamed_relay_parks_after_600ms_and_releases_the_floor` | paused clock | A slow answer never traps the caller | Release 1 |
| `explicit_close_with_no_clauses_closes_the_relay_and_allows_a_true_drain` | unit | A relay always ends | Release 1 |
| `bud_answer_without_a_declared_marker_is_relayed_whole_at_completion` | paused clock | No Bud draft is ever spoken | Release 1 |
| `early_yes_during_a_question_reaches_the_answer_parser_as_an_interrupt` | paused clock | An early answer is not lost | Release 1 |
| `step_down_finishes_the_audible_clause_and_installs_the_conversation_loop` | in-process | The mid-call hand-over works | Release 1 |
| `shutdown_starts_no_task_or_provisional_run_and_marks_unfinished_tasks_unknown` | paused clock | A deploy never loses a task silently | Release 1 |
| time to answer against the Release 0 baseline, early turns apart (W7 harness) | live | Interaction mode is not slower than today | Release 1 |
| `streamed_relay_is_one_ga_response_across_a_park` | scripted audio | GA clients keep playing | Release 2 |
| `regime_c_floor_never_waits_for_a_gateway_drain_event` | paused clock | Observer floor liveness | Release 3 |

## What ships in which release

Target windows and this chapter's sizing are Addendum D3's (sizing is not a commitment); the actor and
executor head the critical path to Release 1.

- **Release 0, measure and lay groundwork (2026-10-05 to 2026-11-06; 4 engineer-weeks):**
  - W1's sections of `contracts.rs`; the server key parser (dark), verified from a YAML file and from variables.
  - Identity refactors (`resolve_llm_legs`, `stream_messages`); `ConnectionState.interaction` (always empty).
  - The metered pump, tags, stop set and boundary classes, all dark.
  - Golden mode-off traces, captured on `main` before the Lifecycle track's first patch (Hume, by 2026-10-23), or
    that patch is recorded in W7's parity ledger.
- **Release 1, cascade interaction on `/ws` (2026-11-09 to 2027-01-29; 8 engineer-weeks):**
  - The actor, supervisor, floor, executor, speculative start, streamed relays, timeline, timers, shutdown rule.
  - The entry points, clip rendering, step-down and the live server switch (default off); the `error` `code` field.
  - The front resolves to `scripted` (Addendum B1) until a fine-tuned `lfm-standard` or `lfm-large` passes the gates.
  - Surfaces, on streaming speech-to-text only (Addendum D2): `/ws` conversation, `/ws` agent (Bud through the
    operator default), raw `/ws` with a background target. LiveKit and SIP only if W7's echo gates pass.
- **Release 2, full duplex audio, the GA cascade and durable tasks (2027-02-01 to 2027-03-12; 3 engineer-weeks):**
  - Sessions on segmented (file-only) speech-to-text models, after the segmented plan's Release 1 (2026-12-18,
    agent sessions) and Release 2 (2027-01-29, conversation sessions).
  - GA cascade segment responses; LiveKit and SIP if not in Release 1.
  - The backchannel emitter's clip path; Bud background answers streaming (with a declared marker).
  - Caller-memory handle passed to W2 (`front_model.memory.*`); the shared front pool consumed unchanged.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs (2027-03-15 to 2027-04-23; 4
  engineer-weeks):**
  - Observer floor for regimes B and C on `/realtime` and `/v1/realtime`, only for providers in `truth_ack`; Bud
    realtime deployments through `interaction.defaults.realtime`.
  - Provider audio through the pump; text-output DAG sessions as tasks.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF (dated at the Release 3 exit; 1 engineer-week):**
  - Regimes B and C on LiveKit and SIP reuse the Release 3 paths.
  - DTMF arrives as a typed caller input. Hindi needs only W4's packs.
- **Release 5, native duplex on WaaV Infer (conditional, dated with Release 4):**
  - An observer-floor variant for regime D, designed then.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| Actor inbox bottleneck at high session counts | low | Late ducks and decisions | p99 inbox gate at 5 ms; playback accounting can move to a second actor |
| A floor state stalls (missed event) | medium | Dead air | Liveness property test; seven backstops, each with a metric W7 gates |
| Discarded provisional runs waste tokens | medium | Background cost | Off for Bud and external backends by default; W7 reports the rate by reason (sign-off 6) |
| A provisional run commits a side effect before adoption, or is never discarded | low | Unasked action; a run with no owner | W3's side-effect gate; zero-tolerance W7 gate; the 600 ms deadline and 1,500 ms eager ceiling cancel it |
| Long gaps mid-answer | medium | Silence or a trapped floor | Park after 600 ms; stall clip at 2,000 ms |
| Heard time wrong for a transport | medium | Confirmations judged heard too early or late | Lags measured per transport; client truncation overrides; W4's 150 ms margin |
| Speakerphone echo on phone legs | high before echo cancellation | False ducks or turns | No duck on a bare onset until echo cancellation; echo tail; LiveKit and SIP gated by W7 |
| Some TTS providers resist attribution | medium | Wrong stops and heard text | `CompletionOnly` fallback; on a mismatch, switch and report `Degraded` |
| Step-down fails or is unavailable | low | Session ends, or stays in English | Today's setup error; repeat-request template where no plain path exists |
| Mode-off behaviour drifts | low | Customer regressions | Golden traces first; additive changes; flags off |
| The segmented speech-to-text plan edits the same voice manager, Smart Turn and result files | medium | Merge conflicts | A shared weekly merge review with that team (Addendum D2); golden traces |

## What is still open

**Measurements needed:**
1. Actor inbox latency at p99 at the target sessions per pod.
2. Pump-to-ear delay per transport (`transport_lag_ms`); the `/ws` lead with 20 ms frames (Release 0).
3. Time to answer with the speculative start on and off, per backend, against the baseline.
4. Discarded provisional runs per 100 turns by reason, and their tokens; eager starts confirmed at E.
5. Gaps between streamed sentences, to set the 600 ms hold; the rate of parks and stall clips.
6. Whether unmodified GA SDK clients play an out-of-band response overlapping another (Release 0).
7. Heard-prefix accuracy against a stereo recording after interruptions.
8. The boundary class of each of the 18 direct TTS providers.
9. Echo tail and false onsets on SIP recordings, with and without echo cancellation.
10. Backstop, watchdog and step-down rates on the scenario suite; both barge-in metrics at a `/ws` ear.

**Choices that need a person** (default in brackets). Addendum D4 needs sign-offs 5, 6, 11, 12, 16 and 18 by the
Release 0 exit review (2026-11-06), and sign-off 9's first-patch and detector-removal parts in week 1 and wire-fix
parts before 2026-11-13; it leaves the `spawn_turns` and pump parts undated (default until signed).
- Sign-off 9, with the AssemblyAI default move: `spawn_turns` and the legacy pump [flags off; pump untouched].
- Sign-off 6: speculative start default [on for the LLM tier, off for Bud and external backends].
- Sign-off 11: cap on an agreed wait before the idle hang-up [10 minutes with check-ins].
- Sign-off 12: LiveKit and SIP in Release 1 [only if the echo gates pass].
- Sign-off 16: timeline and front-log retention [not stored].
- Sign-off 18: an unsupported language [the step-down decision above].
- Sign-off 5: front placement [scripted CPU sidecar in Release 1; shared GPU pool in Release 2, floor 4 calls per
  LFM instance, with an owner, GPU class and budget named at the Release 0 exit; no qualified device means scripted].
- No sign-off number: named time zones and local time from telephony metadata [fixed offsets only]; forcing PCM
  egress so ducks always work [request linear PCM16; otherwise pause instead of duck].

## Where the detail is

- Full design: `/home/bud/ditto/waav/research/duplex-agent/design/W1-interaction-runtime.md` (section 14: alignment
  with Addenda A to C; section 9: open contract requests). Critiques, same folder:
  `critique-W1-interaction-runtime-code.md`, `critique-W1-interaction-runtime-adversarial.md`.
- Shared types: `/home/bud/ditto/waav/WaaV/docs/duplex-agent/contracts.md`.
