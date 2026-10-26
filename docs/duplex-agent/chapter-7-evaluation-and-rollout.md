# Chapter 7. Evaluation, release gates, licences and the road to native duplex

## What this part is and why it exists

This part decides whether each release of the duplex agent is good enough to ship, and proves it from what a caller
would hear. It owns an audio-scored harness outside the gateway, an in-process scenario runner in the gateway's unit
tests, the service levels and their statistics, load probes, front-model checkpoint checks, the licence and technology
registers, the CI runners, the release plan with its dated vendor changes, the rollout, and the conditions for native
duplex on WaaV Infer. It reads every policy value from the gateway's reported configuration and keeps no copy.

Without it, the plan would rest on server timings that start after endpointing, on gates that pass when no
confirmation was exercised, on load tests where a model front quietly falls back to canned lines, and on unchecked
licences. The failure it exists to stop: a speakerphone handset echoes "Shall I go ahead?" back up the line and
approves a non-refundable booking.

## How it works

**Terms.** W1 to W7 name the seven workstreams, one per chapter: workstream 1 runtime, 2 front model, 3 background
tasks, 4 policy and human in the loop, 5 duplex audio, 6 providers and endpoints, 7 evaluation and rollout (this one).
*E* is the confirmed end of the caller's turn; the *acoustic end* is the end of the caller's last word (from the
stimulus labels). The *front* answers at once; the *background* (an LLM or agent) does the task. The **`scripted`
profile** is the front with no model: a deterministic *rule router* picks the act and persona clips speak; it is **the
Release 1 front everywhere** (integration decisions, Addendum B1). A *provisional run* is a background run started at E
before the front decides, *adopted* if the act is `delegate`, else cancelled, by E + 600 ms (Addendum A1). The *answer
marker* is the line `ANSWER:` a background model writes before its final answer. *Regime A* is the cascade, *B* a
speech-to-speech provider under gateway control, *C* one that is itself the front, *D* native duplex on WaaV Infer.

**One rule for scoring.** Every scored number comes from a stereo recording on one clock: left is what the caller
said, right what the caller would have heard (agent audio through a reference playout model, cut where the client
would flush). Scores may also use stimulus labels, harness-owned mock logs, client-visible wire events and the
effective configuration in W6's `interaction` echo. **Telemetry may trigger and attribute, never score.** Latencies
are reported from the acoustic end (what the caller feels) and from E (what the policy controls).

**Six tiers, each catching what the one above cannot.**

| Tier | Runs on | What is real | What gates |
|---|---|---|---|
| In-process scenarios | every merge, `cargo test --lib`, paused clock | runtime, policy, task registry, fact guard | policy timings from E, content: exact |
| Scripted audio | merges touching `gateway/` (path-filtered, required) | the gateway with its `scripted` front; mocks for the rest | content, routing, zero-tolerance metrics: exact; timing against the baseline binary |
| Local front | merges touching the front, GPU time | `llama-server` with the Qwen3.5-2B candidate; LFM opt-in | advisory until a model profile ships |
| Front stress | nightly, GPU time | each distillation checkpoint plus the guard, 300+ turns | upper bounds on leaks, injection, false blocks |
| Probes under load | nightly, release candidates; x86 runner and GPU time | production settings at 50/100/120% of the per-node budget | service levels, fallback share, dropped audio |
| Live matrix | nightly, release candidates, within the budget cap | real providers | release sign-off; never a merge gate |

A model front needs a GPU: on 6 ARM CPU threads no model met the 300 ms act budget at one call. A tone-coded
text-to-speech mock makes scripted content exact without speech recognition; the scripted task server logs every tool
commit, so a confirmation scenario with no commit fails as "not exercised".

**Infrastructure** (Addendum D6.1; owner: the evaluation lead, with operations). Every CI job today runs on
GitHub-hosted `ubuntu-latest` (`.github/workflows/ci.yml:51`, `:273`, `:369`) on push, pull request or by hand
(`ci.yml:17-21`), so nothing nightly runs; the shared GB10 ran at a median 95% use from other work in the bake-off
(`front-model/FRONT_MODEL_BAKEOFF.md:457`). The plan adds a **dedicated quiet x86 runner by Release 0 week 2**
(absolute timing, CPU-sidecar probes, the x86 front run) and **dedicated GPU time before the Release 0 exit** (a runner
or a reserved GB10 window). The `required` gate fails on any result but `success`, skips included (`ci.yml:389-398`),
so a path-filtered required job reads its filter from a small hosted path-filter job, always starts, and runs its steps
only when its paths changed. GPU jobs stay out of `required`; nightly tiers run from `interaction-eval-nightly.yml`.

**What is measured** (besides waits, confirmations, corrections, echo and overlap):

- *Silence*, decided from the agent's words: owed by the agent (unexplained), hold (after an expectation was set),
  owed by the caller (after a question), agreed wait.
- *Truth*: every number, date, name and code spoken must be in an evidence set that can lose facts (a corrected value,
  a cancelled result); invented entities and stale relays are gated at zero.
- *The speculative start*: wasted runs and tokens per 100 turns; a tool commit before adoption is a zero-tolerance
  violation.
- *The streamed answer*: W3's `AnswerSentence { seq }` sentences must decode exactly, never early, and stop at the
  boundary after a correction. The **draft-leak gate** is zero heard words of a draft canary (Addendum C1); W3's
  `no_marker` and `draft_dropped` counts must match the task server's log; marker-then-tool-call is gated as a rate
  (proposal at most 2%). A Bud agent streams only if it declares `background.answer_marker`; the `/ws` agent
  time-to-answer gate covers only those agents, the others are reported against the agent-path baseline (Addendum D5).
- *The acknowledgement rule*: nothing before E + `ack_due_ms` (300 ms); in fast mode (rolling median from E to the
  first answer sentence under 700 ms after 3 delegated turns) it waits to E + 700 ms and is skipped if the answer is
  ready; something audible by E + 2,000 ms; clips at most 24 characters.
- *The front*: the rule router is gated on labelled turns both ways: a status question, answer, repeat request or
  instruction override never opens a task, and a complete addressed question is never met with silence (Addendum
  D1.1; seed decisions T11.d4 and T16.d3 are regression cases). At the Release 0 exit it must reach **delegate recall
  at least 0.95 on a frozen held-out set**, lower confidence bound published, tuned only on a separate split (D1.2).
- *Gate sets* (D1.5): the 68 seed decisions keep their human labels; about 500 more are written outside the synthetic
  generator and labelled by two people who wrote neither it nor the router. Teacher labels qualify a profile only once
  the teacher agrees with the humans at kappa 0.7 or more, and act-choice rows must pass on both sets. Every checkpoint
  report scores the router on the same sets.

**Worked example** (scripted stack, Release 1 `scripted` front, LLM tier, first turn of a session, "What time does the
Boston office open on Saturday?"; times from the acoustic end):

1. 0 ms: the caller stops speaking. 450 ms: E (oracle final delay plus endpointing).
2. 455 ms: the task server sees the provisional run's request; the rule router commits `delegate`; the run is adopted,
   not restarted; `task_created` reaches the client.
3. 750 ms: the acknowledgement is due (E + 300 ms); the clip "Let me check." starts and, with W1's 150 ms `/ws` lead
   and 60 ms playout, is heard at about 960 ms, which is E + 510 ms. Over the 250 to 700 ms range of E on real
   speech-to-text this lands at about 0.76 to 1.21 s, so the 0.9 s p50 clip target holds only where median E is at most
   about 390 ms at `ack_due_ms` 300. This example misses it by 60 ms.
4. 870 ms: the background writes `ANSWER:`. 970 ms: the first answer sentence is complete (E + 520 ms) and passes W3's
   check; the lane is busy until the clip ends at E + 700 ms, so the relay starts at E + 780 ms (80 ms text-to-speech
   first byte) and is heard at about 1,440 ms.
5. Today's conversation path, speaking each confirmed sentence (`core/conversation/mod.rs:1366-1409`), would be heard
   at about 1,260 ms. This early turn costs 180 ms, over the +150 ms tolerance, which is why time to answer before fast
   mode is reported separately from later turns (Addendum D7). From the fourth turn (fast mode) no clip plays and the
   answer is the first sound, at about 1,260 ms. (Scripted tiers gate timings only against the baseline binary.)

**Key numbers.**

- Experience targets (decision 3.4 with Addenda C6 and D7; regime A, streaming speech-to-text, `/ws`, unloaded). The
  cached-clip first audible response is split: a gateway-owned part from E, at most `ack_due_ms` + transport lead +
  playout + jitter (300 + 150 + 60 ms plus jitter by default), and a per-stack number from the acoustic end, set at the
  Release 0 exit from the measured median E for each `ack_due_ms` arm; the provisional p50 0.9 s (p95 1.3 s) holds only
  where median E is at most about 390 ms. Fast mode: the answer is the first sound, p50 at most 1.6 s. Time to answer
  on no-tool turns: at most the baseline + 150 ms at p50, early turns reported separately. Unexplained silence: target
  2 s, hard 3 s. Barge-in: "yields" (duck at the ear) p50 at most 0.3 s; "silent" direction p50 at most 0.7 s at a
  `/ws` ear. The generated-acknowledgement target is re-derived at the Release 0 exit on dedicated hardware; clips are
  the default.
- Front bake-off (GB10, 68 decisions; Addenda B and B7): no prompted model passed (acceptable acts 0.50 to 0.79,
  delegate recall at best 0.96 against 0.98); warm act p95 on a shared GPU 70 ms (LFM2.5-1.2B) to 311 ms (Gemma 4
  E2B); calls per instance within 300 ms: 4 per LFM model, 2 for Qwen3.5-2B, none on CPU.
- The rule router, disclosed (D1.1, D1.3): with the question step, acceptable acts 0.87 to 0.90, delegate recall 0.92
  to 1.00, precision 0.81 to 0.82; it misses the model gates too; its seed score is optimistic (one author wrote its
  lexicons and the set) and its lead over the best Apache models is within the ±0.1 intervals. It is the least-bad
  front, not one that passes.
- Front qualification (D1.4): act macro-F1 at least 0.85, every act at least 0.75; delegate recall at least 0.98,
  precision at least 0.95; status questions as new tasks and injection success at most 1% each; single-call act p95 at
  most 100 ms on GPU or 200 ms on CPU on the target device; act p95 within 300 ms under Poisson load.
- Thinking Machines Lab's interaction model: 0.40 s on Full-Duplex-Bench v1; only regimes C and D can reach it; every
  release report prints the gap.
- "Zero" in model-dependent gates is an upper bound: 300 clean stress turns bound a rate near 1% (rule of three).

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| Where scoring happens | Client-edge stereo recording; telemetry never scores | Server timings start too late | A tested playout model per transport |
| What gates a merge | Hosted deterministic tiers; GPU jobs advisory or nightly; live providers gate releases | Live providers and shared GPUs are flaky | Absolute timing only on the quiet x86 runner |
| Experience targets | Decision 3.4 with Addenda C6 and D7; clip target split per stack | Endpointing differs per stack | Per-stack numbers wait for baselines |
| Release 1 front | `scripted` with a recall gate; models dark; a passing checkpoint may ship in a point release | No prompted model passed | No model wording in Release 1 |
| Front qualification | D1.4 rows on human-labelled sets; the first-chunk row only enables generated acknowledgements | Self-labelled sets flatter | About 500 decisions to label |
| Front profile in CI | `scripted` and the forced Qwen3.5-2B candidate; LFM opt-in (sign-off 4) | Testing a product may be commercial use | "Not measured" is not passed |
| Draft text | Answer-marker rule; marker-then-tool as a rate | The token callback carries text only (`core/llm/mod.rs:713-717`) | A rare withdrawn sentence |
| Acknowledgement floor | Arms 0, 150, 300 ms, fast mode on and off; W4 sets values | A floor adds latency; too early collides | About 600 live turns per stack |
| Phones before echo cancellation | SIP and LiveKit in Release 1 only if echo gates pass | An echoed "go ahead" must never approve | Phones may wait for Release 2 |
| Rollout | Canary pods, a pilot Bud agent, agent by agent; switch rollback | Interaction off is today's behaviour | Slower reach |
| Native duplex | Regime D only when four criteria hold | No open duplex model beyond English and Chinese | The 0.40 s experience waits |
| Licences | One register in both repositories; references pinned by revision or hash | Branch references drift | Today's URLs must be pinned |

## What changes in the code

| File | Function or type | Change | New or modified | Release |
|---|---|---|---|---|
| `WaaV/eval/` (Python `duplex_eval`), `WaaV/eval/loadgen/` (Cargo) | harness; load generator | Runner, adapters, mocks, reports; Poisson load | new | Release 0 |
| `gateway/src/core/voice_manager/manager.rs` | `receive_audio` | Emit VAD and bot-speaking edges (`core/observability/observer.rs:76-88` hooks have no production caller) | modified | Release 0 |
| `core/observability/turn_profile.rs`; `handlers/ws/config_handler.rs:2007-2010`; `core/agent/engine.rs`; `core/conversation/mod.rs` | `TurnTrace`; `TurnProfiler::new`; spans | Endpointing-inclusive latency; the session's `stream_id`; barge-in stop time | modified | Release 0 |
| `gateway/src/observability/interaction_attrs.rs` | attribution names | Outside `voice_attrs::ALL`, declare-every-name tests | new | Release 0 |
| `gateway/src/core/interaction/metrics.rs`, `core/interaction/tests/` | `InteractionMeter`; in-process runner | One span per wait; scenarios on a paused clock | new | Release 1 |
| `gateway/tests/{turn_profile_attribution,agent_barge_in_span,realtime_provider_matrix}.rs` | telemetry tests; provider list | New binaries; fix the 12-versus-11 drift with W6 | new; modified | Release 0 |
| `WaaV/.github/workflows/ci.yml` | `integration-mock` (`:282`), `required.needs` (`:375-385`) | New tests; hosted path-filter job; scripted audio and register check required; local front advisory; LFM opt-in | modified | Release 0 |
| `WaaV/.github/workflows/interaction-eval-nightly.yml` | scheduled workflow | Stress, load probes, live matrix and experiments on the dedicated runners, like `waav-infer/.github/workflows/live-matrix.yml:13` (schedule), `:30` (self-hosted) | new | Release 0 |
| `WaaV/LICENSES/MODEL_AND_DATA_REGISTER.yaml`, `WaaV/eval/registers/technology.yaml`, `WaaV/scripts/check_registers.py`, vendored into `waav-infer/` | registers, check | Revision-or-hash rule, image variants, agreement | new | Release 0 |
| `WaaV/clients_sdk/python/bud_waav/ws/session.py` | measurement mode | No keepalive or reconnection; typed agent events | modified | Release 0 |
| `waav-infer` server; gateway `core/realtime/mod.rs`, `plugin/dispatch.rs`, `config/env.rs` | `InferRealtime` | Step worker batching every live session per frame | new; modified | Release 5 |

Gateway changes are additive telemetry; with interaction mode off, clients see no change.

## What this part gives to and needs from the other parts

**Gives.** Release verdicts against `slo.yaml`; checkpoint reports; the parity ledger; registers; runners; sessions
per node (Release 2); data for `ack_due_ms`, `fast_answer_ms`, `answer_window_ms` (W4), `speculative_start` (W3),
calls per instance (W2), the barge-in split (W5).

**Needs** (contract names from decision 4 and Addendum C):

- All: `core/interaction/contracts.rs` with a golden test per workstream (Release 0).
- W1: the `InteractionEvent` bus with tokio-clock offsets for turns, lanes, front acts, provisional runs,
  `AnswerSentence` and `StreamSpan`; `heard_at`.
- W2: `scripted` with `injection_cue`, the repeat-request exemption and the question step; `allow_unqualified`;
  `qualified_profiles.json`; human-labelled sets and checkpoints from Release 0; the x86 run; the pool (Release 2).
- W3: `AnswerSentence.first_after_e_ms`; marker and discarded-run counters; `confirm_provisional`; the drain rule.
- W4: `PolicyConfig` and `policy_hash` with Addendum C4's values (`ack_due_ms` admitting 0, `ack_clip_max_chars`).
- W5: `InterruptionClass`; `attribution` and `suspected_echo` on `EndOfTurn`; `ClipBank`; the segmented onset tap
  (Release 2).
- W6: protocol 1.1 events; the echo with profile, `qualified`, attestation, placement, `policy_hash`, barge-in values.
- Operations: the x86 runner (Release 0 week 2), GPU time (by 2026-11-06), on-call for the new components.
- Other teams (estimates): budapp `VoiceAgentEntry.interaction` and `background.answer_marker` (2026-11-02),
  confirmation templates (2026-11-30); budprompt push and a final-answer marker, budgateway resume proxying
  (2026-11-30); legal by the dates below; the segmented team in a weekly merge review (D2).

## Tests to write first

| Test name | Level | What it proves | Release |
|---|---|---|---|
| `pacer_sends_frames_within_2ms_of_schedule`; `interaction_echo_missing_or_policy_hash_mismatch_marks_run_invalid` | unit (harness) | The harness neither measures itself nor assumes values | Release 0 |
| `interaction_off_changes_only_additive_telemetry`; `parity_interaction_off_matches_baseline_recordings` | unit; scripted audio | Interaction off is unchanged | Release 0; 1 |
| `rule_router_never_meets_an_addressed_question_with_silence`; `rule_router_delegate_recall_on_frozen_held_out_set`; `rule_router_routes_every_labelled_caller_turn` | in-process; report | D1.1; D1.2; the Release 1 front, both ways | Release 0; 1 |
| `ack_starts_no_earlier_than_e_plus_ack_due_ms_and_is_audible_by_hard_limit`; `ack_fast_mode_skips_when_first_answer_sentence_ready_by_e_plus_700` | in-process, paused | Decision 3.3 | Release 1 |
| `speculative_side_effect_waits_for_adoption_or_never_runs`; `speculative_adoption_deadline_is_e_plus_600ms_anchored_at_e` | in-process, paused | Decision 3.1, Addendum A1 | Release 1 |
| `draft_text_never_spoken`; `marker_then_tool_call_stops_at_the_clause_boundary_and_withdraws` | in-process, paused | Decision 3.2 | Release 1 |
| `hitl_irreversible_commit_only_after_template_readback_and_yes`; `echo_self_approval_speakerphone_zero_commits` | in-process; scripted audio | Confirmation; an echo never approves | Release 1 |
| `answer_no_tool_streams_within_baseline_plus_150ms`; `fast_mode_first_sound_is_scored_against_1600ms` | scripted audio, paired | Later-turn time to answer | Release 1 |
| `drain_finishes_side_effecting_calls_and_marks_the_rest_unknown`; `front_profile_auto_resolution_matrix` | scripted audio | Shutdown rule; `scripted` everywhere, no LFM unattested | Release 1 |
| `probes_at_full_budget_meet_release_levels`; `admission_overflow_goes_to_the_rule_router_never_a_queue` | live (load) | Service levels at the per-node budget | Release 1; 2 |
| `model_reference_without_revision_or_hash_fails` | unit (register) | Licence rule | Release 0 |
| `duplex_step_worker_batches_all_live_sessions_per_frame` | unit (Infer) | Regime D does not serialise sessions | Release 5 |

## What ships in which release

**Sizing and dates** (Addendum D3, replacing the old durations): indicative engineer-weeks, not a commitment
(per-workstream split in D3), for six gateway engineers and one machine-learning engineer dedicated to this plan,
separate from the segmented plan's three, plus part-time Bud, legal and operations support.

| Release | Target window | Engineer-weeks | Of which this chapter |
|---|---|---|---|
| Lifecycle track | first patch (Hume) by 2026-10-23 | 6 | 1 |
| Release 0 | 2026-10-05 to 2026-11-06 | 26 | 7 |
| Release 1 | 2026-11-09 to 2027-01-29, year-end break included | 45 | 3 |
| Release 2 | 2027-02-01 to 2027-03-12 | 33 | 3 |
| Release 3 | 2027-03-15 to 2027-04-23 | 30 | 4 |
| Releases 4 and 5 | scheduled at the Release 3 exit | 19 (Release 4) | 2 |

Critical path to Release 1: runtime actor and executor, then policy core and task layer, then duplex audio and the
echo gates. Fewer engineers move the dates in proportion, but not the lifecycle track.

- **Lifecycle track.** Per dated vendor change, a deprecation canary, a parity-ledger entry and a live-probe cell: Hume
  warning in the first patch, refusal from 2026-11-13; Gemini, Deepgram, Nova 2 Sonic and Ultravox wire fixes (target
  2026-11-13); OpenAI default moves by 2027-01-06, 2027-01-20 and 2027-02-26; AssemblyAI to Universal-3.6 Pro.
- **Release 0, measure and lay groundwork.** Harness, mocks, runners, CI and the nightly workflow; baselines of today's
  gateway per endpoint and stack (time to answer, median E, the SIP echo path, and which detector ended each turn:
  Smart Turn does not run on live audio today, Addendum D6.4, `core/smart_turn/mel_extractor.rs:428-431`); telemetry;
  both registers; the **distillation track** (sign-off 28) with human-labelled sets; the router's recall gate; the x86
  run. The exit review (2026-11-06) confirms targets, sets per-stack clip numbers and re-derives the generated target.
- **Release 1, cascade interaction on `/ws`, streaming speech-to-text only (D2).** `scripted` everywhere; every Release
  1 gate above, with the echo gates deciding SIP and LiveKit; the floor, fast-mode, speculative and answer-window
  experiments; the 250 ms pause test; the staged rollout. A passing fine-tuned `lfm-standard` (or `lfm-large`) may
  ship to attested deployments in a point release.
- **Release 2, full duplex audio, the GA cascade and durable tasks.** Barge-in, duck, false-yield and takeover gates
  with echo cancellation; `/v1/realtime` cascade; streamed Bud background runs; the shared GPU front pool's capacity
  test and sessions per node; `apache-standard` only if a fine-tuned Qwen3.5-2B (or Gemma 4 E2B) passes; interaction
  mode on segmented (file-only) speech-to-text sessions with W5's segmented gates, after the segmented plan's Release 1
  (2026-12-18) and Release 2 (2027-01-29); Full-Duplex-Bench v1/v1.5, MTR, Big Bench.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs.** Per-regime truth, verbatim
  claims checked from audio; A2A, MCP and Responses; DAG scenarios; tau-Voice, Full-Duplex-Bench v2 and v3.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF.** Regime B and C telephony gates; DTMF; Hindi on a
  verified recogniser, with LFM2.5-VL-3B and Qwen3.5-2B as Hindi front candidates.
- **Release 5, native duplex on WaaV Infer (conditional).** Only if sessions hold the frame budget at p95, a commercial
  model beats regime A on overlap (stop ≤ 0.25 s), the handover works or no background is needed, and a customer
  accepts English and a GPU per site.

## Operations and rollout

- **Rollout in stages** (Addendum D6.3). Canary pods with `WAAV_INTERACTION_ENABLED` set (the environment form of
  `interaction.enabled`, default off); one pilot Bud agent; then agent by agent, through each agent's `interaction`
  entry or `interaction.defaults.agent`. **Rollback is the environment switch**: unset it and pods return to today's
  paths, which the interaction-off parity tests hold to the baseline.
- **Shutdown.** A draining pod starts no new tasks or provisional runs; in-flight side-effecting tool calls finish
  within the drain window; unfinished tasks are recorded as `unknown` (Release 1 tasks live in memory).
- **Alerts and runbooks** for the `llama-server` supervisor (W2), the post-call task supervisor (W3) and the analyser
  compute pool (W5). Proposed signals: restarts, rule-router fallback share (probes gate it at 2% or less), backstop
  firings, floor stalls, pool saturation.
- **Capacity** (D6.2). Sessions per node come from the Release 2 load test. The shared front pool gets an operations
  owner, GPU class and budget at the Release 0 exit; GPUs come once a profile qualifies. A deployment with no qualified
  device class, including on-premises without a GPU, runs `scripted`.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| Speakerphone echo approves an action | medium | high | Zero-tolerance echo gates; phones only if they pass |
| The router leaves real requests undone | medium | high | Question step; held-out recall gate |
| No fine-tuned checkpoint passes | medium | medium | Two Liquid bases and an Apache one trained from Release 0; else `scripted` |
| Load collapses a model front behind green timings | high | high | Probes gate fallback (≤ 2%) and router share (0) |
| A busy GPU runner blocks merges or adds noise | high | medium | GPU jobs advisory or nightly; dedicated runners |
| Side effect or draft text too early | medium | high | Zero gates, always exercised; answer-marker rule |
| A deploy drops tasks or a component fails unseen | medium | high | Drain rule, alerts, runbooks, switch rollback |
| Harness measures its own pacing | medium | high | Jitter self-tests, loopback fixture |

## What is still open

**Measurements needed.** (1) Release 0 baselines per endpoint and stack, with median E. (2) The real SIP echo path,
replacing the provisional 150/250/400 ms sweep. (3) Floor and fast-mode arms with early-turn time to answer;
speculative waste and gain; answer onsets for `answer_window_ms`; marker rates on real backends. (4) The x86 run;
front latency on dedicated hardware; the pool's capacity; the router's held-out recall. (5) Whether the designed pause
(near 0.7 s at the ear, inferred) meets the silence direction. (6) Judge agreement on 200 items per judge; the
recognition floor; the speaker-similarity threshold; PersonaPlex frame time on the GB10.

**Choices that need a person** (default in brackets; "needed by" from Addendum D4; week 1 is Release 0's first week):
- Liquid terms and Bud's attestation [ask now; attest after legal confirms] (sign-off 2; week 1).
- Publishing `-front-lfm` [Bud's and attested customers' registries] (sign-off 3; 2026-11-06).
- CI on LFM weights [`scripted` and the forced Qwen3.5-2B; LFM opt-in] (sign-off 4; week 1).
- The distillation track and teacher terms [start now] (sign-off 28 and 21's teacher part: week 1; 21's transcript
  and recording parts: by 2027-01-29).
- Interaction-off changes, including the LiveKit detector removal and the AssemblyAI move [escape flags] (sign-off 9
  with C8: first-patch and detector parts week 1; wire fixes before 2026-11-13).
- SIP and LiveKit in Release 1 [only if the echo gates pass] (sign-off 12; 2026-11-06).
- Speculative default [on for the LLM tier if p50 gain ≥ 150 ms and waste ≤ 25%] (sign-off 6; 2026-11-06).
- Non-commercial data (Full-Duplex-Bench, TurnBench: evaluation only, never a release gate), Krisp, MaAI, MoshiRAG,
  PersonaPlex hosting [not used commercially] (sign-off 25; by 2027-03-12).
- The live matrix's monthly budget cap [the product owner sets it] (unnumbered, D4; before nightly runs start).
- This chapter's thresholds (guard false blocks ≤ 5%, tasks per request ≤ 1.05, entity recall ≥ 0.95 in regimes B and
  C, marker-then-tool ≤ 2%) [confirm at the Release 1 exit].

## Where the detail is

- Under `/home/bud/ditto/waav/research/duplex-agent/`: `design/W7-eval-native-rollout.md` (Addendum D wins) and its
  critiques; `review/final-review-*.md`; bake-off data in `bench/bakeoff/`.
- In this folder: `front-model/FRONT_MODEL_BAKEOFF.md`, `technology-register.md`.
