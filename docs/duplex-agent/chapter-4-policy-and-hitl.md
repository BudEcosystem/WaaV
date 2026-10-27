# Chapter 4. The policy engine and human-in-the-loop: who speaks, when, and what binds

## What this part is and why it exists

The policy engine is the deterministic part of a WaaV interaction session that decides, at every event,
who speaks, what, when, on which speech lane, and what happens to each background task: acknowledgements,
progress and holding lines, relays of background answers, corrections and cancels mid-flight, and questions
and confirmations answered by voice ("human-in-the-loop"). It is a pure state machine,
`step(now, input) -> actions`, run by the interaction runtime (Chapter 1), so every rule can be tested on a
virtual clock. Design files name the workstreams W1 to W7: workstream 1 runtime, 2 front model, 3
background tasks, 4 policy and human in the loop (this chapter), 5 duplex audio, 6 providers and endpoints, 7
evaluation and rollout; Chapter N covers workstream N.

Without it, the front and the background would talk over each other or leave dead air; a relay could
start before a correction is understood; a "yes" meant for another question, or an echo of the read-back,
could approve a payment; a filler could claim "all set" before anything happened. The division of labour
(sign-off 1, reworded by Addendum B6): **the small model runs the conversation once it is fine-tuned and
gated; until then the deterministic router does, and the gateway always guarantees the words that bind.**
Once the rules have settled the clear cases, the front decides what each remaining turn is (new request,
correction, cancel, answer, small talk) and words acknowledgements and progress. This engine words every
confirmation, choice, value request and notice from templates, relays background answers verbatim, and
decides what an answer binds.

## How it works

**Inputs and outputs.** The runtime feeds the engine floor changes, voice onsets, front acts, lane events
(each with the "heard time", when the caller could first hear an item) and task events. Each confirmed end of
turn (**E**) carries its transcript, interruption class, speaker attribution (`High`, `Medium`, `Low`), a
`suspected_echo` flag and a language check. The engine orders speech, stops, front requests, `TurnSettled`,
task commands, client signals and history writes; the runtime executes, applies mechanical interlocks and
reports back.

**The turn, from E.** While a question is open the answer parser runs first; otherwise deterministic lexicons
do: continuers (seeded by the agent's `ignore_phrases`, unused while an answer is expected), presence and
status questions, cancel cues, repeat, closings, and amendment cues ("actually", "I meant", "make it"). A
definite match settles the turn from templates, so clear cases never wait for a front act, even with a model
running: the lexicons settle "make it four", "never mind" and "is it done", the parser a plain "yes" to an
open question. Only the rest reach the front. In Release 1 every deployment runs the `scripted` profile
(Addendum B1): Chapter 2's rule router decides at once, and its question rule sends a question addressed to
the agent (not status, social or repeat) to the background as `delegate`, never `wait`; a fragment ending on a
dangling word still waits (Addendum D1.1). A gated Liquid model answers in about 70 ms (act p95); the router
still decides at the 300 ms act budget if it is late. A rule layer then corrects the act: a `social` act
outside the allow-list becomes `delegate`, and a correction binds only to a task its words name. The settled
act is emitted once as `TurnSettled`; its `adopt` flag tells the runtime whether to keep the **provisional
run** started at E in parallel with the decision: `delegate` adopts it, any other outcome cancels it, and
Chapter 3 cancels any run not adopted by E + 600 ms (Addendum A1). Nothing from a provisional run is spoken or
reaches a client before adoption; side-effecting tools wait in Chapter 3's gate.

**What the caller hears first.** The acknowledgement is due at E+300 ms (`ack_due_ms`; 0 allowed). Once the
session's rolling median from E to the backend's first answer sentence is under 700 ms over at least 3
delegated turns (**fast mode**, `fast_answer_ms`), it is due at E+700 ms. A first answer sentence ready before
anything was said is the reply; no acknowledgement plays. Otherwise a clip pre-rendered in the session's voice
plays; a generated line is used only if predicted to be heard by E+800 ms (`ack_target_ms`), which the
measured first spoken chunk (575 to 656 ms p95) rules out on a shared GPU; `scripted` never tries. Something
is audible by E+2,000 ms. Clips are at most 24 characters, so a playing clip delays a ready answer by at most
about 1.2 s (Addendum C6.2). The first-audible-response targets apply where an acknowledgement plays; in fast
mode the first sound is the answer, held to the time-to-answer target and due by 1.6 s p50 from the acoustic
end (when the voice stopped, before E by the endpointing delay; Addendum C6.1). The clip target is split
(Addendum D7): from E, the gateway's part is at most `ack_due_ms` plus transport lead, playout and jitter;
from the acoustic end, a per-stack number set at the Release 0 exit from each arm's median delay to E (0.9 s
p50 holds only if that median is at most about 390 ms at `ack_due_ms` 300). Early-turn time to answer (before
fast mode) is reported apart from later turns.

**Streamed relay.** Where the backend authors the final answer, Chapter 3 releases it sentence by sentence
(`AnswerSentence { seq }`) after its checks: the LLM tier, cut by the conversation path's sentence
aggregator, and Bud agents that declare `background.answer_marker` (others are relayed whole at completion,
Addendum C1.4). Draft text before the `ANSWER:` marker is never released. The relay is **one open main-lane
item**: each checked sentence goes as a `StreamSpan` (task, attempt, sentence range, `last`) under the same
item id, one sentence ahead of the one playing so it is synthesised meanwhile; a `Speak` with no clauses and
`last = true` closes it. Text is byte-identical to today's pipeline. Any event that fails the check stops the
item at the next clause boundary, so an appended sentence not yet audible never starts. If any part was
heard, the row for the `AnswerStopped` reason applies:

| `AnswerStopped` reason | What the caller hears next |
|---|---|
| `Revised`, `Superseded`, `Regenerating` | "Scratch that." before the restatement; the new answer follows |
| `Cancelling` | the cancel intent line ("Okay, I'll stop that.") is the withdrawal |
| `SpeechPolicy` | "Sorry, I can't give you the rest of that answer." |
| `MarkerThenToolCall` | "Hold on, let me check that properly."; the wait continues |
| `RunFailed` | "Sorry, I lost the rest of that answer.", then the retry offer |
| `SpeakableCap` | nothing withdrawn; "The rest is on your screen." if the client renders tasks |

Answers over 600 characters pause with "There's more. Want me to go on?".

**Waiting.** Progress is spoken only on a real status change: none in the first 4 s, at least 5 s apart, at
most 3 per wait. A holding line comes after 8 s of silence, then one offer of a choice; an unanswered offer
becomes an agreed wait with 30 s check-ins (at most 10 minutes). A promise is always closed.

**Delivery priorities** (one floor-taking item audible at a time):

| Rank | Items |
|---|---|
| 1 | urgent `interrupt` items (configured task kinds only); may cut anything but the caller within 400 ms |
| 2 | the reply to the caller's latest turn; greeting; client `speak` (passes the interlocks for its own turn) |
| 3, 5 | question or confirmation for the focused or a background task |
| 4, 6 | relay or failure notice for the focused or a background task |
| 7, 8 | progress; holding lines, choice offer, check-ins |

Ranks 1 to 6 may cut ranks 7 and 8 at a clause boundary; ranks 2 to 6 are cut only by rank 1, the caller, or
the caller's amend or cancel of the task being relayed. Nothing starts while the caller holds the floor;
ranks 3 to 8 wait until the latest turn is settled; while a question is open nothing of rank 3 to 8 starts.
An answer binds only to the question asked last.

**Confirmations.** A confirmation is rendered from a structured `ActionProposal` built from the arguments
that will execute, never by a model. In Release 1 it is Chapter 3's tool gate (`Bound`: exactly those
arguments run); in Release 2, Bud approvals (`BackendBound`). The read-back is its own clauses. An accept
binds an action that cannot be undone only if four checks pass:

- **Heard.** The caller's voice onset is after the read-back's heard time plus a 150 ms margin. On `/ws`
  the heard time is delivery + 150 ms lead + 150 ms transport lag.
- **Strong.** The accept is "yes" or "go ahead", not "okay".
- **Attributed.** The turn's attribution is `High`, and the turn is not `suspected_echo`.
- **Clean.** No negation, and no leftover words that conflict with a slot value. "Yes, but on the
  Mastercard" is a decline with a correction.

A weak or doubtful yes gets one re-ask; unclear answers go to the front's classifier (none under `scripted`),
then one clarification. Sensitive values are never taken by voice: declined in Release 1, then client UI or
link (Release 2) and DTMF (Release 4).

**Language.** A turn in another language than the session's pack never binds or starts a task. After 2 such
turns, `/ws` conversation and agent sessions hear the persona's `language_not_supported` line and step down to
the plain conversation path with an empty history (`config_warning` `interaction_language_step_down`). Raw
`/ws`, the GA cascade and regimes B and C cannot: they play the repeat-request template and continue in
English (Addendum C7.1).

**Worked example 1: a lookup** (Release 1, LLM tier, first turn).

| Time | What happens | Caller hears |
|---|---|---|
| E | end of turn; no lexicon match; front asked; acknowledgement due E+300 | — |
| E+1 | runtime starts the provisional run | — |
| E+100 | act `delegate` (a gated model; the Release 1 router settles at about E+1); run adopted | — |
| E+300 | no answer sentence yet: a clip | — |
| E+360 to E+1,670 | | "Sure, let me check." |
| E+1,900 | first answer sentence after one tool round; opens the relay item | — |
| E+2,600 | relay starts (synthesised while waiting) | "Our Boston store closes at 9 p.m. today." |

From the fourth turn, fast mode speaks a no-tool answer ready at E+480 from E+1,180: the answer 350 ms
sooner, the first sound 820 ms later (about 1.6 s from the acoustic end).

**Worked example 2: a correction mid-answer** (Release 1). Sentences 0 and 1 play from E+1,600 and E+4,010.
At E+4,600 the caller says "Sorry, I meant the Cambridge store."; Chapter 5 ducks and cuts sentence 1 by
E+5,500; appended sentence 2 never starts. At E2 = E+6,800 the lexicon matches "I meant"; Chapter 3 reports
`AnswerStopped{Revised}`. The caller hears "Scratch that." (E2+360), "Got it, the Cambridge store."
(E2+1,260), then the revised answer (E2+3,200).

**Key numbers and their sources.**

| Number | Value | Source |
|---|---|---|
| Act committed / outer backstop / adoption deadline | 300 / 500 / E + 600 ms | decisions 2.3; Addendum A1 |
| Act latency of a gated model | 70 ms p95 (LFM2.5-1.2B, one call, shared GPU); within 300 ms up to 4 calls; 1,496 ms on 6 ARM threads, so the CPU sidecar is `scripted` | bake-off (`front-model/FRONT_MODEL_BAKEOFF.md` beside this chapter); Addendum B3 |
| Acknowledgement due (0 allowed); fast mode; generated heard by; hard limit; amendment; clip | 300; 700 after 3 turns; 800; 2,000; 1,200 ms; 24 characters | decision 3.3; Addendum C4, C6.2 |
| Generated first audio; first spoken chunk; speaking rate | 0.7 to 0.9 s; 575 to 656 ms p95; 14.5 characters per second | `WaaV/docs/segmented-stt/chapter-4-turn-taking-and-interruption.md:44`; bake-off; `core/agent/spoken.rs:14-16` |
| Progress cadence; holding line; check-in | 4 s, 5 s, 3 per wait; 8 s (hard 10 s); 30 s | Maslych; the design frame's filler rules; judgement |
| Answer window; re-ask; timeout; sign-in wait; agreed-wait cap; heard margin | 4,000; 8,000; 20,000; 120,000; 600,000; 150 ms | decision 4 (window, to be measured); sign-off 11 (cap); judgement |

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| Shape | pure state machine; the runtime executes | every rule testable on a virtual clock; timer tasks caused today's barge-in poisoning bug | one more layer before audio |
| Who words what binds | templates from `ActionProposal`; verbatim relay; front words acknowledgements, progress, small talk | the prompted 1.2B confirmed unreliably; 14 to 19% of early fillers contradicted the answer (design frame) | less fluent confirmations; personas re-word only within a validator |
| What the turn means | answer parser and lexicons, then the front's act, then a rule layer | clear cases need no model; prompted small models chose acts worse than the router (0.50 to 0.79 against 0.90, within the ±0.1 intervals; Addendum D1.3) | lexicons per language |
| Adoption | the settled act decides; `adopt` only for a plain `delegate` | a mis-routed "thanks" must not become a paid task | discarded runs cost prompt tokens |
| Acknowledgement timing | due E+300; fast mode E+700 with a skip; clips by default | decision 3.3; a ready answer beats a filler; generated lines miss 800 ms on shared hardware | fast mode moves the first sound to about 1.6 s |
| Relay | verbatim, sentence by sentence on one open item, re-checked, stop row on a heard stop | today's path streams sentences; paraphrase adds errors | occasional "Scratch that."; unmarked Bud answers come whole |
| Queued speech after E | waits until the turn is settled | a relay must not play before a correction is understood | up to 300 ms with a model; about 1 ms under `scripted` |
| Question adjacency | nothing else starts while a question is open | a failure's "want me to try again?" must not capture a booking's "yes" | slower delivery of other results |
| Voice binding | heard, strong, `High` attribution, clean | a television's or echoed "yes" must not book a flight | extra re-asks for genuine callers |
| Sensitive values | never by voice | vendor transcripts, logs and recordings would hold them | Release 1 can only decline them |
| Never silent | "never an unexplained silence": bounded updates, a choice, then agreed quiet | 88% fillers came with 47.9% interruptions (Full-Duplex-Bench v3) | silences up to 8 s inside a wait |
| One policy for every front profile | identical timing and binding under `scripted`, a gated `lfm-standard` or `lfm-large`, and Apache | the gates and the licence pick the profile; safety must not depend on it | `scripted` loses generated restatements and stage-2 classification |

## What changes in the code

Paths under `WaaV/gateway/src` unless stated. This chapter changes neither `core/agent/engine.rs` nor the
legacy agent loop's callbacks in `handlers/ws/agent.rs`: with the mode on, Chapter 1's branch in
`initialize_agent_loop` (`:127`) installs the runtime and never builds `AgentEngine` (`:150` today), so that
code runs only with the mode off. Its rule that drops "yeah" while the agent is busy (`:326-330`) becomes the
continuer lexicon above, seeded from `ignore_phrases` (`bud-auth/src/voice_agent.rs:97-102`). (Chapter 3
changes `core/agent/brain.rs`; Chapter 7 adds measurement spans to `engine.rs`.)

| File | Function or type | Change | Release |
|---|---|---|---|
| `core/interaction/contracts.rs` | `PolicyInput`, `PolicyAction`, `Utterance` (`stream: Option<StreamSpan>`), HITL types, `DeliveryPolicy` | its part of the shared contract file, golden test (new) | 0 |
| `core/interaction/policy/pack.rs`, `packs/en.rs` | `PolicyPack`, validator | format and validator; English lexicons, templates, banks, withdrawal lines (new) | 0, 1 |
| `core/interaction/policy/` (`mod`, `config`, `timing`, `delivery`, `truth`, `locale`, `interject`) | `PolicyEngine::step`, `PolicyConfig`, `DeliveryQueue`, rule layer, step-down | rules, clamps, ranks, lexicons (new) | 1 |
| `core/interaction/policy/stream.rs`, `weave.rs` | `AnswerStream`, relay planning | sentence spans on one open item, re-check, stop rows, pagination (new) | 1 |
| `core/interaction/hitl/mod.rs`, `parse.rs`, `sensitive.rs` | `HitlDialog`, heard check, parser, screening, capture, DTMF | as named (new) | 1, 2, 4 |
| `core/interaction/policy/regimes.rs` | regime B and C mappings, confirmation segment | provider verbs (new) | 3 |
| `core/llm/functions.rs` | private `execute_one_with` | confirmation gate and pre-adoption wait (Chapter 3 writes it) | 1 |
| `core/voice_manager/manager.rs` | STT wrapper (`:1034-1077`), audio send (`:636-639`) | detector; capture gate and silence to the vendor (Release 2) | 1, 2 |
| `core/interaction/duplex/backchannel/clips.rs` | `ClipBank` (Chapter 5) | its clip groups, rendered by Chapter 1's `capture_clip` on the session's own TTS provider (no second instance) | 1 |
| `bud-auth/src/voice_agent.rs` | `KNOWN_FIELDS` (`:330-354`) | `interaction` block; per-tool `confirmation_templates` | 1, 2 |
| `handlers/ws/messages.rs`, `docs/openapi.yaml` | wire events; `error` (`:372-376`, only `message` today) | Chapter 6's names, protocol 1.1; optional `code` on `error` for `question_closed` (Addendum A2) | 1 |

## What this part gives to and needs from the other parts

**Gives.**
- `PolicyEngine` (`step`, `next_wakeup`); probes `answer_expected()`, `holds_session_open()`, `policy_hash`.
- `PolicyAction` (`TurnSettled { turn, act, task, adopt }`, `StepDown`); `Utterance` (`kind`,
  `bypass_interlocks`, `reply_to`, `stream`); `DeliveryPolicy` (`interrupt` cuts only the agent's speech).
- HITL types (imported by Chapter 3); the pack validator (run by Chapter 2's persona compiler); lexicons for
  Chapter 6's `ClaimMonitor`.
- `PolicyConfig` (`interaction.policy.*`, in Chapter 6's session echo): `ack_due_ms`, `fast_answer_ms`,
  `ack_target_ms`, `ack_hard_deadline_ms`, `ack_clip_max_chars`, `answer_window_ms`, `agreed_wait_max_ms`,
  cadence, question timing, `heard_margin_ms`.
- Signals: `task_question`, `task_answer_received` (`by` includes `dtmf`, `session_end`), `floor_state`,
  `assistant_transcript`'s `kind` and `verbatim`.

**Needs.**
- **Chapter 1:** `VadOnset` from `VoiceOnset`; `LaneEvent`s with `heard_at`; the open relay item for
  `StreamSpan`; `play_clip`, `capture_clip`; `StopAtBoundary`; interlocks; `TurnDecisionMax`; adoption on
  `TurnSettled`; `StepDown`; `EndOfTurn` with `vendor_confidence` and `language_ok`.
- **Chapter 2:** the ten acts; the 300 ms budget and rule router (the `scripted` default); the classification
  slot; `PersonaSpec.template_overrides` and `banks`; the language check; `CallerMemory`.
- **Chapter 3:** `AnswerSentence { seq }` and `AnswerStopped` (eight reasons); the `ANSWER:` marker rule;
  provisional runs and the `AwaitAdoption` gate; `TaskCommand`s; states and `effects`;
  `live_actions_without_push_target()`; the tool gate.
- **Chapter 5:** interruption classes, attribution, `suspected_echo`, `Held`, `SideTalkHeld`,
  `EchoWithdrawn`, `ClipBank`; classifier rule 5 reading `answer_expected()`.
- **Chapter 6:** the `interaction` homes (`defaults.agent`; `defaults.realtime` in Release 3);
  `config_warning` codes; verbs; `ProviderSpeech` verdicts; `speech.truth_ack`; `interaction.hitl.channels`.
- **Chapter 7:** acknowledgement and fast-mode arms, time to answer, dead air, wasted runs, mis-delegation,
  the marker-then-tool-call rate.
- **budapp:** `confirmation_templates`, `background.voice_approvals`, `background.answer_marker`.

## Tests to write first

Every test runs the pure engine, on a virtual clock ("paused time") where timing matters, unless the row says
otherwise. Chapter 7 runs the audio-tier twins.

| Test name | What it proves | Release |
|---|---|---|
| `policy_contract_golden` | the shared types do not drift | 0 |
| `interaction_off_policy_engine_never_constructed` | mode off is today's behaviour | 1 |
| `ack_due_300_without_history_700_in_fast_mode` | decision 3.3 exactly | 1 |
| `first_sentence_before_due_time_is_the_reply_and_skips_ack` | no filler before a ready answer | 1 |
| `generated_ack_only_if_predicted_heard_by_800` | clips by default | 1 |
| `clear_cases_settle_before_any_front_act` | "make it four", "never mind", "is it done", a plain "yes" | 1 |
| `settled_delegate_adopts_provisional_run` | only plain delegations keep the run | 1 |
| `update_abort_social_answer_merge_cancel_provisional_run` | no paid task from a non-request | 1 |
| `stream_spans_share_one_item_and_close_with_last` | one open item, byte-identical, in order | 1 |
| `every_answer_stopped_reason_has_a_stop_row` | all eight reasons are handled | 1 |
| `streamed_relay_correction_mid_answer` | no stale sentence heard; withdrawal first (then audio) | 1 |
| `early_yes_over_question_routed_to_parser_before_class_filter` | an early "yes" is not lost | 1 |
| `hitl_accept_rejected_when_onset_precedes_readback_end` | an unheard read-back never binds | 1 |
| `irreversible_accept_binds_only_from_high_attribution` | bystander and echo "yes" never bind | 1 |
| `hitl_yes_with_different_slot_value_is_decline_with_correction` | "yes, but the Mastercard" does not charge the Visa | 1 |
| `wait_silence_never_exceeds_hard_limit` | never an unexplained silence (property test) | 1 |
| `language_mismatch_steps_down_only_on_ws_conversation_and_agent` | elsewhere a repeat request in English | 1 |
| `capture_window_suppresses_all_stt_egress_and_span_text` | sensitive values never reach the vendor (in-process) | 2 |
| `regime_c_confirmation_holds_provider_output_until_outcome_injected` | confirmation segment (fake provider) | 3 |
| `timeline_confirmation` and the ten other golden timelines | whole sequences with timings | 1, 2 |
| `policy_step_bench` | p99 at most 50 µs (benchmark) | 1 |

## What ships in which release

Dates are Addendum D3's targets; Releases 4 and 5 are dated at the Release 3 exit.

- **Release 0, measure and lay groundwork (2026-10-05 to 2026-11-06):** contract types and golden test; pack
  format and validator; the virtual-clock harness; scenarios for the bake-off, distillation sets and Chapter 7;
  `policy_hash`.
- **Release 1, cascade interaction on `/ws` (2026-11-09 to 2027-01-29):** the policy core on `/ws`
  conversation, agent and raw sessions with streaming speech-to-text (LiveKit and SIP if the echo gates pass),
  under the `scripted` front with the router's question rule: English pack and step-down; acknowledgements,
  fast mode and clips; waiting, settled-turn, adjacency and continuation rules and `adopt`; the streamed relay
  (LLM tier; Bud foreground runs with a marker); never-untrue rules; voice `Confirm` (tool gate), `Choose` and
  `Value` with the binding checks; sensitive redaction and decline; the Bud approval notice; end of call;
  Chapter 6's signal names.
- **Release 2, full duplex audio, the GA cascade and durable tasks (2027-02-01 to 2027-03-12):** segmented
  (file-only) speech-to-text sessions (Addendum D2: after the segmented plan's Release 1, 2026-12-18, for
  agent sessions and its Release 2, 2027-01-29, for conversation); `bud.*` signals on `/v1/realtime`; Bud
  `BackendBound` approvals and streamed Bud background runs (marker only); webhook questions; sensitive values
  by client UI and link, `Form`, `AuthUrl`; per-tool confirmation templates; the tenant persona API through
  the same validator; the caller-memory rule.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs (2027-03-15 to
  2027-04-23):** regimes B and C on `/realtime` and `/v1/realtime` (verb mappings, confirmation segment,
  provider-claim corrections; Bud realtime deployments through `interaction.defaults.realtime`); A2A, MCP and
  OpenAI Responses questions; streaming for OpenAI Responses and DAG text.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF:** the same mappings on LiveKit and SIP;
  DTMF on SIP; Hindi and code-switched packs; re-tuned cadences and thresholds.
- **Release 5, native duplex on WaaV Infer (conditional):** a regime D mapping table, only if Chapter 7's
  criteria hold and the model has an injection channel.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| Fast mode's first sound lands about 1.6 s from the acoustic end (2.0 s with an 800 ms silence ceiling), at the 1.6 s p50 bound | medium | slow starts on fast backends | `fast_answer_ms` configurable (0 = off); Chapter 7 runs both arms |
| A playing clip delays a ready answer in early turns | medium | slower early answers | clips at most 24 characters (about 1.2 s); early turns reported apart (Addendum D7) |
| Mis-delegation: a non-request settles as `delegate` | medium | a paid run | lexicons and rule layer; a release metric |
| A genuine "yes" gets low attribution | medium | extra re-asks | measure attribution; client UI path |
| A marker followed by a tool call after text was heard | low to medium | a premature sentence | stop row; gated as a rate (at most 2% proposed) |
| A gated front model saturates | medium at scale | wording degrades | admission permits (floor 4 calls per LFM instance) send excess to the router, never a queue |
| A provider in regimes B and C says something untrue | medium | heard before corrected | detect and correct (sign-off 7); only providers in `speech.truth_ack` (sign-off 10) |

## What is still open

**Measurements needed.**
1. Acknowledgement arms at 0, 150 and 300 ms, each with its median delay to E (Addendum D7); both fast-mode
   arms against the 1.6 s p50 bound; the generated-acknowledgement target, re-derived on dedicated front
   hardware at the Release 0 exit.
2. Time to answer against the Release 0 baseline + 150 ms, early turns apart from later ones (Addendum D7).
3. Answer gaps after questions (confirms `answer_window_ms`); heard margin per transport.
4. Cadence numbers, the 2 s continuation merge and the 2 s stall clip (Full-Duplex-Bench-style run and a
   small user study).
5. Attribution accuracy on genuine answers; short-answer accuracy against vendor confidence per provider.
6. Per-provider first audio for generated lines; stops by `AnswerStopped` reason.
7. SIP DTMF delivery by the pinned LiveKit crate, and pausable recordings (Release 4).

**Choices that need a person** (default in brackets; needed by the Release 0 exit review, 2026-11-06, except
17 by 2027-01-29, 10 by 2027-03-12 and 26 before Release 4, Addendum D4).
1. Front role per Addendum B6 [adopted] (sign-off 1); reworded goals, including "never an unexplained
   silence" and "a voice approval is passed on, not executed" [adopted] (sign-off 7).
2. `speech.truth_ack` per provider for regimes B and C [required] (sign-off 10).
3. Agreed-wait cap [10 minutes with check-ins] (sign-off 11).
4. Spoken approvals of budprompt escalations [off unless the agent author allows] (sign-off 14).
5. Session end [Chapter 3's proposal; the 60 s grace only after an abnormal disconnect] (sign-off 15);
   privacy of caller words and memory [off] (sign-off 16).
6. Tenant persona API [Release 2] (sign-off 17); unsupported language [as in "Language" above] (sign-off
   18); native-speaker Hindi packs (sign-off 26).
7. Strong `High`-attribution yes for irreversible actions [as stated; no sign-off number yet].
8. Clip-rendering billing on Bud [once per voice per process, shared]; per-tool templates in budapp
   [generic template].

## Where the detail is

- Full design: `/home/bud/ditto/waav/research/duplex-agent/design/W4-policy-hitl.md` (rules 3.3, speculative
  start 3.5.5, streamed relay 3.6.1, human-in-the-loop 3.8, step-down 3.13.1, timelines 3.14, alignment
  records 13 and 14).
- Same folder: critiques `critique-W4-policy-hitl-code.md` and `critique-W4-policy-hitl-adversarial.md`; the
  design frame `DESIGN_FRAME.md` (lines 99-110).
