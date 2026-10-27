# Chapter 5. Duplex audio: barge-in, echo, backchannels and endpointing

W1 to W7 name the seven workstreams, one per chapter: workstream 1 runtime, 2 front model, 3 background tasks, 4
policy and human in the loop, 5 duplex audio (this part), 6 providers and endpoints, 7 evaluation and rollout.

## What this part is and why it exists

This part decides, from the audio itself, who has the floor. It hears the caller start to speak over the agent and
lowers the agent's voice within about 100 ms at the gateway, then decides whether the sound was a real
interruption, a "mm-hm", a cough, a television, someone else in the room or the agent's own voice coming back as
echo. It cuts or resumes the agent's speech without ever cancelling the background task. It decides when the
caller has finished (the moment called **E**), with an earlier, revocable "probably finished" signal so the
background can start before E. It plays short listening sounds, records what the caller said over the agent, and
defends against echo. It covers regime A (the cascade: speech-to-text, a language model, text-to-speech) fully and
gives the speech-to-speech regimes B and C what they can use.

Without it, today's gateway is half duplex. Barge-in fires on the first transcribed word, clears all queued speech,
and on the agent path cancels the background run (`core/agent/engine.rs:438-448`, `:613-630`); there is no duck,
pause or resume. The voice activity onset is thrown away (`core/smart_turn/processor.rs:659`). **Smart Turn never runs
on live audio**: each chunk replaces the mel frames and trims the audio buffer to 400 samples
(`core/smart_turn/mel_extractor.rs:411`, `:428-431`), so 20 ms chunks leave a handful of frames while inference needs
50 (`core/smart_turn/processor.rs:686`, default `:117-118`). Turns end on the LiveKit text detector, speech-to-text
finals or the silence timer. Interaction mode fixes this for its own sessions in Release 1; the Release 0 baseline
records it; mode-off sessions are unchanged (Addendum D6.4). There is no echo defence
(`livekit/client/audio.rs:30-34`). The LiveKit text turn detector runs in every production session against its
licence (`core/turn_detect/config.rs:27-36`). Speakerphone echo would interrupt the agent, every "mm-hm" would stop
it on the conversation path (the agent path drops four fixed phrases, `handlers/ws/agent.rs:326-330`), and a "yes"
over a confirmation could be lost or taken from a bystander.

## How it works

**Ingress.** Every caller frame is copied, without delaying speech-to-text, into a per-session analyser on a
dedicated process-wide compute pool (default half the cores; set only by the server-side
`interaction.duplex.compute.*`). It runs Silero VAD v6.2 (a voice activity detector, VAD) on 32 ms chunks and
confirms an onset after two voiced chunks (64 ms); that `VoiceOnset` is the only onset W1 receives, and W1 builds
W4's `PolicyInput::VadOnset` from it. After a 200 ms caller pause it runs Smart Turn v3.2 once on its own buffered
utterance (at most once per 300 ms). On LiveKit the tap sits per participant, before the noise filter;
only the primary caller feeds it. **Release 1 covers streaming speech-to-text only** (Addendum D2). Sessions on
file-only ("segmented") models follow in Release 2, where the analyser takes onsets from the segmented engine's
detector through one onset tap instead of a second VAD.

**Egress.** Each interaction session has one metered playback pump (W1 owns the file; this part sets its
requirements). It paces 20 ms frames on an absolute playout clock, so the gateway holds all audio beyond a short lead
(LiveKit 40 ms, `/ws` 150 ms) and can lower, hold or cut it. It keeps one sub-queue per lane (front, main,
backchannel), records what was delivered and when, and feeds that audio to the echo defence.

**Two-stage barge-in.** On an onset while the agent holds the floor, the analyser ducks the agent itself (−15 dB over
60 ms) from a lock-free snapshot. One state machine, `BargeInFsm`, then owns the episode. It pauses the agent 400 ms
after the onset if still undecided, and gets a verdict from the classifier by 900 ms at the latest. An interruption
cuts only the audible utterance; queued items go back to W4 untouched, and tasks keep running. Other classes resume
the agent, with a short rewind after a long pause.

| Class | Typical cue | What happens to the agent | Reaches |
|---|---|---|---|
| `interrupt` | stop or repair word ("wait", "what?"), three content words, 900 ms of speech, a short answer while W4 expects one | audible utterance cut; a caller turn starts | front and background, with what the caller had heard |
| `backchannel` | only listening words (the lexicon plus the agent's ignore phrases), offset seen | resumed from the duck, usually never paused | front, as a note |
| `talking_to_others` | side-talk cue with a level drop | paused; resumed after 700 ms of silence; cut at 6,000 ms and reported to W4 | front, as a note |
| `background` | 12 dB or more below the caller's own level | resumed | timeline only |
| `noise` | under 250 ms voiced, no words | resumed | timeline only |
| `echo` | matches the agent's own delivered words or audio | resumed; a cut that proves to be echo is withdrawn | timeline only |

**Echo defence**, in layers: a transcript echo filter scores every interim against the agent's delivered words,
aligned in time; an echo probe estimates echo delay and level from the delivered audio, and a correlation gate
blocks onsets that echo explains; from Release 2, `sonora` (WebRTC's echo canceller AEC3, in Rust) runs on phone
legs and on `/ws` clients that do not cancel echo themselves. Every caller turn carries an
attribution (high, medium, low) and a `suspected_echo` flag; W4 binds an irreversible "yes" only from a
high-attribution turn not suspected of echo.

**Endpointing.** In interaction mode the endpoint policy owns the end of the caller's turn. It fuses Smart Turn,
provider end-of-turn events, speech-to-text finals and a 1,500 ms silence ceiling, and holds once for 800 ms when
Smart Turn is unsure and the words end in "and", "to", "I want to". When it decides, it *claims* the
speech-to-text segment with the existing generation-checked claim (`core/voice_manager/stt_result.rs:504-535`), so
today's 1,800 ms and 2,500 ms timers cannot fire a duplicate turn. Before E it may emit an **eager end of turn**
when Smart Turn says complete with probability 0.8 or more before the final transcript; it withdraws it if the caller
adds a word or speaks 300 ms more, and emits at most two per turn, none for suspected echo. E is one
`EndOfTurnReport` (words, `source`, `confident`, `attribution`, `suspected_echo`, and `eager`: `Same` when the final
words equal the eager words, `Different`, or `None`). W1 starts a provisional run at the eager signal, discards it on
a withdrawal or `Different`, and on `Same` confirms it with W3, anchoring adoption at E + 600 ms; W3 cancels a run
whose E never comes 1,500 ms after the eager start (Addenda A1, C2).

**Backchannels and clips.** One clip bank holds every pre-rendered clip in the session's voice: listening sounds and
W4's acknowledgement, holding and check-in lines, rendered by W1 on the session's own provider (Release 1).
Acknowledgements default to these clips (at most 24 characters; a generated line's first chunk took 575-656 ms p95
on a shared GPU), so the bank is on the path of the first audible response. From Release 2 an emitter plays a
listening sound at a caller pause when Smart Turn says "not finished", after at least 700 ms of caller speech and
2,500 ms since the last one, with probability 0.8; never after an agent question, during dictation of private
details, on phone lines by default, or after E (one playing at E fades out in 40 ms).

**Clips per language and voice** (Release 2). The customer may choose any language and any voice, so nothing about
a listening sound or a filler line is fixed in code:
- *Words.* The language pack's `backchannel` group lists the listening sounds and their fallback words (in English
  "mhm", "okay", "I see", with "okay" as the fallback). Acknowledgement, holding and check-in lines already come from
  the same pack's phrase banks.
- *Timing.* The pack carries the emitter's numbers per language: the minimum gap (English 2,500 ms), how often a
  pause is skipped (English 1 in 5), and the shortest caller speech before a sound (English 700 ms). Languages differ
  widely; Japanese listeners, for example, give listening sounds far more often than English ones. The persona's
  `backchannel_intensity` scales these numbers and does not replace them.
- *Audio.* Each clip rendered in the session's voice is checked before it enters the bank: at most 600 ms long once
  leading and trailing silence are trimmed, and not silent. Where the speech-to-text is cheap to run on the clip, its
  words must match the text, which catches a voice that spells "mhm" out. A clip that fails is replaced by the
  pack's fallback word rendered and checked the same way; if that fails too, the session makes no listening sounds,
  and says so once in `interaction_updated`.
- *Recorded clips.* An operator may supply recorded clips per provider and voice (`duplex.clips_dir`, files named
  `<provider>/<voice>/<language>/<clip id>.wav`). A recorded clip replaces the rendered one; it is resampled to the
  session's output format and passes the same checks.
- *Changes mid-session.* When the session's voice or language changes, the bank is rebuilt before the next sound;
  until it is ready no listening sound plays, so a caller never hears the old voice.

**Languages.** English in Releases 1 to 3, Hindi in Release 4. Another language makes `/ws` conversation and agent
sessions step down (`interaction_language_step_down`), removing this engine; raw `/ws`, the GA cascade and regimes B
and C keep it, and W4 asks the caller to repeat.

**Example 1, an interruption** (LiveKit; ms from the caller's first sample). The agent reads flight times, with a
question queued. At 0 the caller says "Wait, no, I meant Tuesday." At 96 the onset is confirmed, the echo gate
passes and the duck starts (first quieter frame about 110). At about 380 the interim "wait" is a stop cue:
`interrupt`. At 381 the flight times are cut (W4 gets `Dropped` with the characters heard), the question returns to
W4 as `Returned`, and the task keeps running. At about 1,500 the endpoint claims the segment.

**Example 2, the eager end of turn** (`/ws`, Deepgram; ms from the caller's acoustic end of "check the weather in
Lisbon tomorrow"). At 240 Smart Turn returns 0.93 with no final, so the policy emits `EagerEndOfTurn` and asks
Deepgram to finalize; at 241 W1 starts a provisional run that cannot call side-effecting tools. At about 430 the
final has the same words: E = 430, `Same`, and W1 confirms the run, which the front's `delegate` adopts at about
E + 200. A new word ("...and Porto") would have withdrawn the signal and the run.

**Key numbers** (p50 and p95: the median and the 95th percentile).
- Duck at the gateway egress: about 100 ms median, 145 ms worst; target p95 ≤ 150 ms unloaded, ≤ 200 ms at 200
  calls (estimated). At a `/ws` ear about 250-300 ms (adds W1's 150 ms lead).
- Barge-in is two experiences (Addendum C6.5): "the agent yields" (duck audible at the ear, p50 ≤ 0.3 s from onset)
  and "the agent is silent" (paused or cut, p50 ≤ 0.7 s at a `/ws` ear; budget 0.53-0.65 s: onset detection, the
  400 ms pause, the 150 ms lead). A 250 ms pause is a Release 1 listening test.
- Krisp's test set: a VAD-only trigger stops in 0.375 s with 66.3% false positives, a word gate in 1.528 s; 1,182
  backchannels to 539 interruptions. Hence duck first, decide later.
- Smart Turn v3.2: 8M parameters, 9-20 ms on 8 vCPU, 37-57 ms on 1 vCPU. Silero v6.2: 16% fewer errors on noisy
  audio. `sonora` AEC3: 4.2 µs per frame. Deepgram Flux: about 260 ms end of turn; eager mode trims 100-200 ms for
  50-70% more language-model calls. Estimates: eager lead over E 200-450 ms; Silero at 200 calls about six cores.

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| What starts barge-in | Local VAD onset, not the first word | Works with every speech-to-text; words come about 1.5 s late | Builds without the VAD features are refused (`interaction_missing_build_features`) |
| Speech-to-text in Release 1 | Streaming only; segmented in Release 2 (Addendum D2) | The segmented plan's detector work lands 2026-12-18 and 2027-01-29 | File-only models wait |
| First stage | Duck at onset, pause at 400 ms, decide by 900 ms | Most overlaps are not interruptions | Silence about 0.65 s after onset at a `/ws` ear |
| What a cut stops | The audible utterance only, by utterance id; tasks never | A barge-in never cancels a task; queued speech must not vanish | W4 handles `Returned` items |
| Who owns an episode | `BargeInFsm` alone, one table of numbers | Two machines gave one pump conflicting commands | W1's backstop (7,700 ms) derives from it |
| Classifier | Heuristic rules; Krisp plug-in (English) in Release 4; a trained two-channel model later | No openly licensed production classifier | Side talk without a cue counts as an interruption |
| Echo defence | Filter, probe and gate in Release 1; `sonora` AEC3 from Release 2, automatic on phone legs | Cheap layers fit any speech-to-text; double cancellation harms browsers | Phones duck 300-500 ms later until AEC runs |
| A "yes" over a question | While W4 expects an answer (its 4,000 ms window), a short answer interrupts, even mid-question | A "yes" was dropped as a backchannel | Relies on W4's heard check |
| Endpoint ownership | The endpoint policy claims the segment; today's timers cannot fire for it | An early endpoint would make a duplicate turn | Interaction-only change to `STTResultProcessor` |
| Eager end of turn | Smart Turn ≥ 0.8 before the final; withdrawable; at most 2 per turn; only where speculative start is on | Background starts 200-450 ms sooner | Wasted runs when the caller continues |
| Backchannel choice | Rules; W2's `backchannel_via_model` lets the front pick the clip within 300 ms | W2 owns all prompt text | Needs a qualified model front on a GPU |
| Listening-sound words, timing and audio | From the language pack and the session's voice; each rendered clip checked, with a fallback word; optional recorded clips per voice | Customers choose any language and voice; timing norms differ by language; many voices misread non-words | A few extra synthesis calls at session start; recorded clips are operator work per voice |
| LiveKit text turn detector | Removed from the image for all sessions | Its licence forbids this use | Legacy turn timing changes (named test) |
| AssemblyAI | Universal-3.6 Pro on the Lifecycle track; deprecated threshold never tuned | The threshold is deprecated | Changes mode-off sessions: sign-off 9, escape flag |
| Audio-output DAGs | Refused in interaction mode | Their audio would bypass the pump | No duplex mode for them until "Later" |

## What changes in the code

Paths are under `gateway/src`, and `duplex/` means `core/interaction/duplex/`; "Lifecycle" is the Lifecycle track
(with Release 0, after legal sign-off for the detector). The legacy agent code is unchanged (`core/agent/engine.rs`,
`handlers/ws/agent.rs`): interaction mode never builds `AgentEngine` (chapter 1), so its barge-in and ignore-phrase
return keep working with the mode off; in interaction mode the runtime and the classifier carry their rules.

| File | Function or type | Change | New or modified | Release |
|---|---|---|---|---|
| `core/interaction/contracts.rs` (W1's file) | duplex section | duplex contract types; golden test | new section | 0 |
| `duplex/mod.rs`, `config.rs` | `DuplexEngine`, `DuplexConfig` | engine owned by W1's runtime; `check_build()`, `longest_episode_ms()`; server-only `compute` parser | new | 1 |
| `duplex/analyser.rs`, `pool.rs`, `ingress_tap.rs` | `IngressAnalyser`, compute pool, `IngressTap` | Silero onsets, drop-oldest ring, reflex duck, Smart Turn buffer and limit; segmented onset tap | new | 1 (segmented 2; `/realtime` 3) |
| `duplex/barge_in.rs` | `BargeInFsm` | duck, pause, decide, resume, side talk, utterance-scoped stops | new | 1 |
| `duplex/classifier/*` | `InterruptionClassifier`, heuristic stack | rules 1-15, English lexicons, attribution; the agent's `interruption.ignore_phrases` seed the backchannel lexicon (the short-answer rule runs first) | new | 1 (Hindi 4; Krisp 4) |
| `duplex/echo/*` | `RenderReference`, `TranscriptEchoFilter`, probe | echo layers | new | 1 (AEC wrapper 2) |
| `duplex/backchannel/clips.rs`, `mod.rs` | `ClipBank`, `ClipRender`; `BackchannelEmitter` | one clip bank with W4's groups; emitter with yield at E | new | 1 (bank); 2 (emitter) |
| `duplex/backchannel/clips.rs`; the language pack's phrase banks; `duplex/config.rs` | `ClipCheck`, pack `backchannel` timing, `duplex.clips_dir` | per-language words, fallback and timing; clip checks; recorded clips per provider and voice; rebuild on a voice or language change | new, modified | 2 |
| `duplex/endpoint.rs` | `EndpointPolicy` | segment claim, fusion, lexical hold, eager signal and withdrawal, `EndOfTurnReport` | new | 1 (Flux 2; veto 4) |
| `core/audio/playout_gate.rs` | `PlayoutControl`, `GateCommand`, `StopTarget` | gates, gain for linear PCM16 only, rewind ring | new | 1 |
| `core/audio/playback_pump.rs`, `playback_queue.rs` | W1's `spawn_metered`; metered queue | playout clock, lane sub-queues, cursor, render tap; legacy pump untouched | modified | 1 |
| `core/voice_manager/manager.rs`, `stt_result.rs` | `receive_audio`, pump construction, `claim_segment` | tap before the STT send (`:564-641`); pump built once; generation-checked claim; 1,500 ms late-final merge | modified | 1 |
| `core/stt/base.rs`, `cartesia/client.rs`, `deepgram.rs`; W6's Flux adapter | `BaseSTT::finalize_segment`, `STTResult.turn_event` | finalize without closing the stream; Flux turn events mapped (2) | modified | 1 |
| `core/stt/assemblyai/*` | model enum, threshold | Universal-3.6 Pro; threshold never sent for it | modified | Lifecycle |
| `core/turn/signal.rs`, `strategies/classified.rs` | `ControllerSignal`, `ClassifiedStart`, `EndpointStop` | start on a confirmed interrupt; stop on the policy's decision | modified, new | 1 |
| `handlers/ws/config_handler.rs`, `messages.rs` | `deliver_tts_audio`, `send_with_policy` | delivery outcome; participant identity | modified | 1 |
| `handlers/openai_realtime/cascade.rs` | user items | hold an episode's transcripts; `speech_started` on the floor change | modified | 2 |
| `handlers/realtime/handler.rs` | ingress and audio route | tap before `send_audio`; regime B audio through the pump | modified | 3 |
| `Cargo.toml`, `Dockerfile`, `init.rs`, `core/turn_detect/config.rs`, `core/state.rs`, CI | features, image, model paths | detector removed; models hashed on load; `aec` (2), `krisp-ip` (4) | modified | Lifecycle |

## What this part gives to and needs from the other parts

**Gives (names from decisions sections 4 and 5).** `InterruptionClass`; `DuplexEvent` on `RuntimeMsg::Duplex`
(bounded, with a drop counter), whose `VoiceOnset` W1 turns into `PolicyInput::VadOnset`; `ClassifiedStart` and
`EndpointStop`; `FloorSnapshot::agent_holds_floor()`, which W1's runtime controller installs as its bot-speaking
probe; `PlayoutControl::stop` by utterance id with a `PumpStopReport`, from which W1 builds one `LaneEvent::Dropped`
or `Returned`; `BargeInConfig::longest_episode_ms()` for W1's backstop; `Held`, `SideTalkHeld`, `EchoWithdrawn` for
W4; `EagerEndOfTurn`, `EagerWithdrawn`, `EndOfTurnReport` (`confident` for W2, `attribution` and `suspected_echo` for
W4, `eager: EagerMatch` for W1 and W3); `ClipBank`; `echo_defence_active()` for W6's input mute; the session block
`interaction.duplex.*` and the server-only `interaction.duplex.compute.*`; scenarios, metrics and measurement points
for W7.

**Needs.** From W1: the runtime and executor, `spawn_metered` meeting this part's requirements, the heard-time
function, `capture_clip`, the `ClientClear` path, the turn controller with `ClassifiedStart`, `EndpointStop` and the
`agent_holds_floor()` probe installed, and W3's `start_provisional`, `confirm_provisional`, `discard_provisional`
called on this part's signals. From W2: `PersonaSpec.interruptibility`, `backchannel_intensity`, `backchannel_clips`;
the `UserPause` decision; a typed `wait` reason before the Release 4 veto. From W3: background runs that no barge-in
cancels; adoption anchored at E. From W4: `answer_expected()` with its one `answer_window_ms` (4,000 ms), the
sensitive-capture flag, consumption of the safety outputs. From W6: `InteractionConfig` carriage, wire names
(`floor_state` reason `stream_gap`), `stop_output`, `hold_floor`, the Flux `/v2/listen` adapter, `ClipRenderer`, and
the codes `interaction_dag_audio_output`, `interaction_override_refused` and the two named above. From W7: the
harness, overlap-v1, the load test. From the segmented speech-to-text plan: nothing in Release 1; in Release 2 its
detector, for the onset tap this part owns and that team reviews. Both plans touch the voice manager, Smart Turn and
speech-to-text result processing, so they share a weekly merge review.

## Tests to write first

Levels: *unit*; *paused* (in-process, paused clock); *scripted audio* (W7's harness plays recordings); *live*.

| Test name | Level | What it proves | Release |
|---|---|---|---|
| `unverified_cached_model_is_not_loaded` | unit | the supply-chain fix | Lifecycle |
| `duplex_contract_golden`; `end_of_turn_report_and_eager_match_match_the_contract` | unit | the duplex types and `EagerMatch` | 0 |
| `smart_turn_runs_at_vad_stop_over_the_buffered_utterance`; `agent_ignore_phrases_extend_the_backchannel_lexicon` | unit | Smart Turn runs on live audio; the agent's ignore phrases carry over | 1 |
| `pump_keeps_transport_at_most_lead_ahead_with_20_ms_frames` | paused | the gateway holds the audio it ducks | 1 |
| `onset_while_agent_speaks_ducks_within_150_ms_of_first_voiced_sample`; `agent_yields_and_agent_is_silent_are_reported_at_the_ws_ear` | scripted audio | the duck target and both barge-in experiences | 1 |
| `exactly_one_component_issues_gate_commands_per_overlap_episode` | paused | single ownership of an episode | 1 |
| `interrupt_cue_stops_the_audible_utterance_and_leaves_tasks_running`; `interrupt_returns_a_not_started_question_to_w4_unasked` | paused | tasks survive; queued speech is not lost | 1 |
| `yes_while_an_answer_is_expected_is_interrupt_even_before_the_last_clause`; `single_word_echo_of_a_cue_word_does_not_cancel` | paused | an early "yes" is never dropped; a "Sorry, …" echo does not self-cancel | 1 |
| `bystander_yes_is_low_attribution_and_never_confirms_an_irreversible_action` | scripted audio | attribution reaches W4's binding rule | 1 |
| `caller_sound_in_a_streamed_answer_gap_opens_an_episode_not_a_turn`; `early_endpoint_never_produces_a_second_turn_from_the_forced_final` | paused | streamed answers hold the floor; the segment claim | 1 |
| `eager_is_withdrawn_when_the_caller_adds_a_word`; `at_most_two_eager_signals_per_caller_turn`; `eager_started_run_is_confirmed_at_e_and_discarded_on_withdrawal_or_different` | paused | bounded speculative waste; adoption anchored at E | 1 |
| `heuristic_beats_min_words_on_the_overlap_curve`; `two_hundred_call_overlap_load_gate` | scripted audio | the classifier earns its place; 200-call load | 1 |
| `backchannel_clip_yields_at_end_of_turn_and_never_delays_the_acknowledgement` | paused | acknowledgement timing | 2 |
| `listening_sound_timing_comes_from_the_language_pack`; `a_clip_that_fails_its_check_is_replaced_by_the_fallback_word`; `a_voice_with_no_usable_clip_makes_no_listening_sounds`; `a_recorded_clip_replaces_the_rendered_one`; `the_bank_is_rebuilt_when_the_voice_changes` | unit; paused | listening sounds follow the session's language and voice | 2 |
| `listening_sounds_in_two_voices` | live | each voice's clips pass the checks and play in that voice | 2 |
| `segmented_session_uses_the_engine_onset_tap_and_runs_no_second_vad`; `segmented_backchannel_does_not_pause_the_agent`; `late_segmented_stop_cancels_after_resume` | unit; paused | segmented: one detector, no stutter, late stops cut | 2 |
| `aec_reduces_synthetic_echo` | accuracy job (real models) | AEC3 works on recorded echo | 2 |
| `regime_b_confirmed_interrupt_calls_stop_output_with_the_delivered_cursor` | paused | provider truncation at what was heard | 3 |
| `live_assemblyai_universal_3_6_pro_turn_messages` | live | the migration is safe | Lifecycle |

## What ships in which release

Dates are Addendum D3's target windows for six gateway engineers and one machine-learning engineer on this plan
(fewer engineers move them in proportion). Indicative sizing of this part, in engineer-weeks: 2, 6, 5, 3, 3 for
Releases 0 to 4; it and its echo gates end the critical path to Release 1.

- **Lifecycle track.** With Release 0, after legal sign-off: the LiveKit text turn detector out of the image and the
  feature set; model files versioned under `CACHE_PATH` and hashed on every load; CI changes. After its live probe
  and sign-off 9: AssemblyAI Universal-3.6 Pro, default behind an escape flag.
- **Release 0, measure and lay groundwork (2026-10-05 to 2026-11-06).** The duplex contract section and golden test;
  dark configuration parsing; measurements for Release 1 (Silero v6.2 interface, `sonora` on recorded SIP echo,
  client buffer depths); a baseline that records Smart Turn not running.
- **Release 1, cascade interaction on `/ws` (2026-11-09 to 2027-01-29).** Streaming speech-to-text only; nothing from
  the segmented plan (no `TurnEvent::Aborted`, `DetectorSpeechStart` or test kit). The code table's Release 1 rows,
  the refusal of audio DAGs, the load gate and the 250 ms pause test; gates count streaming sessions only. LiveKit
  and SIP (phone) only if W7's echo gates pass.
- **Release 2, full duplex audio, the GA cascade and durable tasks (2027-02-01 to 2027-03-12).** `sonora` AEC3 and
  bare-onset ducking on phone legs, the backchannel emitter (web on, phone off), listening sounds and fillers chosen
  by the session's language and voice (pack words and timing, clip checks, recorded clips, rebuild on change), the
  `/v1/realtime` cascade fixes,
  Deepgram Flux mapping (eager mode off until measured), LiveKit participant kind. Segmented sessions, after the
  segmented plan's Release 1 (2026-12-18, agent sessions) and Release 2 (2027-01-29, conversation sessions): the
  onset tap, pausing only after 700 ms of speech, late text that can still cut, the segmented gate rows and tests.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs (2027-03-15 to 2027-04-23).**
  Regime B taps and pump on `/realtime` and `/v1/realtime`, `stop_output` at the delivered position, regime C
  observe-only with a render tap.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF (scheduled at the Release 3 exit).** The same taps
  for regimes B and C on LiveKit and SIP; Hindi and code-switched lexicons and clips; the Krisp plug-in if cleared;
  the optional front-model veto.
- **Release 5, native duplex on WaaV Infer (conditional; scheduled then too).** Regime D bypasses this engine.
- **Later.** Duplex output for audio DAGs (this part owns it); a trained two-channel classifier; telephony and Indic
  Smart Turn fine-tunes; proactive interjection research.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| Speakerphone echo ducks or cuts the agent | High on phones | Agent stops itself | Echo layers; no bare-onset duck on phones until AEC; phones in Release 1 only past the echo gates |
| Classifier cancels on a backchannel, or resumes over an interruption | Medium | Agent stops, or talks over the caller | Deadline rule; gate: false cancels ≤ 10% at ≤ 1.0 s |
| Analyser falls behind at scale | Medium | Late ducks | Dedicated pool, drop-oldest ring, text fallback, load gate |
| Eager signals on turns the caller continues | Medium | Wasted tokens | Withdrawal, cap of 2, threshold 0.9 if retractions pass 15% |
| Bystander or television "yes" | Low | Wrong action confirmed | Level baseline, attribution; W4's high-attribution rule |
| Client buffers past the lead, or egress not linear PCM16 (no duck) | Medium | Barge-in targets missed | Playback-control event; measured depths; ask for PCM16; 250 ms pause test |
| Detector removal shifts legacy turn timing | Certain, small | Turns end at the timer | Named test, release note, W7 measures before and after |
| The segmented plan slips | Medium | Segmented sessions wait | Release 1 does not depend on it |
| AssemblyAI changes turn messages on Universal-3.6 Pro | Low | Missing or doubled turn ends | Live probe first; escape flag |
| Pump task dies | Low | No audio | W1's supervisor recreates it, else the session steps down |

## What is still open

**Measurements needed.**
- Duck latency at egress and ear, the false-cancel curve and both barge-in experiences, per transport and
  speech-to-text mode; the 250 ms pause test; the 200-call load test and whether one shared Silero session is safe.
- AEC3 echo reduction on real SIP speakerphone, car and handset recordings; bulk-delay drift; client buffer depths.
- Smart Turn accuracy on 8 kHz audio, English and Hindi; eager lead time and retraction rate; Flux eager mode's
  extra calls; stream-gap lengths; AssemblyAI Universal-3.6 Pro messages and endpoint quality.
- For Release 2: pause time per segmented backchannel and late-cancel latency.
- For Release 2: which supported voices render each pack's listening sounds acceptably (clip length and the
  speech-to-text check per provider and voice), and the timing numbers for each language beyond English, from
  native-speaker review.

**Choices that need a person** (the plan proceeds on the default; "by" dates from Addendum D4).
- Sign-off 9 (mode-off changes): detector removal with Release 0 and past-image remediation (week 1); the
  AssemblyAI default moved to Universal-3.6 Pro after the probe (Addendum C8).
- SIP and LiveKit in Release 1 (sign-off 12, by 2026-11-06): only if the echo gates pass.
- Duck depth and backchannel defaults (sign-off 19, by 2027-01-29): −15 dB; web `medium`, phone `off`.
- Two-channel recording and production transcripts (sign-off 21, by 2027-01-29): none until approved.
- Krisp terms, MaAI provenance, Full-Duplex-Bench data (sign-off 25, by 2027-03-12): unused commercially until cleared.
- Native-speaker Hindi lexicons and clips (sign-off 26): required before Release 4.
- Linear PCM16 egress from text-to-speech providers: ask for it where offered.
- Primary participant in multi-party rooms: `listen_participants`, else the first subscribed participant.

## Where the detail is

- Full design: `/home/bud/ditto/waav/research/duplex-agent/design/W5-duplex-audio.md` (3.8.7: the end-of-turn
  report; 13 and 14: alignment with the decisions); Addendum D of the decisions wins where they differ.
- Critiques, same folder: `critique-W5-duplex-audio-code.md`, `critique-W5-duplex-audio-adversarial.md`.
