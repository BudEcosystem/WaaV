# Chapter 4. Turn-taking and interruption

## What this part is and why it exists

A *segmented session* is a live call whose speech-to-text model cannot take a live stream, so the gateway cuts the caller's audio into utterances and uploads each one. The transcript of an utterance therefore arrives after the caller has finished it, typically about a second later. Today every turn-taking decision in the gateway is made when text arrives: when the caller's turn starts, when the agent must stop talking because the caller talks over it (*barge-in*), whether a short "uh-huh" is ignored, and when an idle call is hung up. This part replaces "when text arrives" with "when the caller speaks", using the voice detector the segmenting engine runs on every frame. It covers the three places that take turns: the voice-agent loop, the `/ws` conversation loop, and the *cascade*, which is the `/v1/realtime` front door for voice agents. A fourth surface, a *DAG pipeline* on a live session (a customer-configured directed acyclic graph of steps, run once per caller turn on the same transcripts), has no barge-in; it gains only the speech-time gate described below.

Without it, a call on a file-only model would technically work and feel broken. The agent would keep talking for a second or more after the caller interrupts. Speech over a greeting that may not be interrupted (a *protected greeting*) would become a turn, and speech while the agent was idle could be dropped. A failed upload would be a silent open line. A short idle timer would hang up on a caller in mid-sentence, because nothing arrives while they speak.

## How it works

The engine reports what its detector hears on its own task, never behind the transcript. The voice manager asks, at that moment, whether the speech is input (the *gate*: the agent accepts speech and is not saying something that may not be cut). A caller turn that passes is *admitted*; one that does not is neither announced, uploaded nor shown. Admitted events go to a task of their own in each loop, so an interruption never waits behind a reply being generated. The turn controller is fed by two tasks, so one call to it becomes one atomic step.

| The engine reports | When | What turn-taking does |
| --- | --- | --- |
| Speech started, repeated with the voiced time so far | 224 ms after onset, at 384 ms, then every 128 ms | Admits the turn. Agent idle: opens the caller turn. Agent mid-reply: interrupts once voiced time reaches the threshold |
| Speech stopped | 224 ms after the caller pauses | Notes whether the agent talked through it |
| Endpoint decided | The engine judges the caller finished, before the transcript | Tells the client; arms the filler |
| Turn closed (had text, voiced time lost, reason) | Every upload of the turn has resolved | No text: ends the turn; if speech was lost, the caller is told (the lost-turn rule below) |
| The turn's one final transcript, with its turn id | After the upload | Starts the agent turn, holds it, or drops it |
| One outcome per upload | At release | Passed to metering, turn-taking and the wire |

Speech that overlapped the agent but stayed under the threshold interrupts only when its transcript arrives and passes three tests (a *text-confirmed interruption*): enough words, not an ignore phrase the agent talked through (a *backchannel*), and not the agent's own words coming back as echo. Text the gateway ignores is not sent to the client.

The end of a caller turn is decided by the engine: an audio model at the pause, a text model once the transcript is back, and a ceiling on silence. This part supplies a voice agent's settings for that decision and otherwise only reacts to it.

Four smaller rules follow from the same idea. Text that arrives while the agent may not be cut, or after the caller has started speaking again, is held and joined to what follows, for at most 15 s. Only text re-arms the idle timer, and the timer does not fire while a caller turn is open, so neither a long utterance nor a noisy empty line misleads it. A client commit (`audio_end`) flushes the engine and waits for that turn's final, which always comes, empty if nothing was recognised. A caller turn whose uploads all failed is *lost*: the caller hears the agent's fallback message as a *notice* (something the gateway says itself, never part of the agent's history, always interruptible), twice, and the third loss in a row, or one authentication failure, ends the call with the error `stt_unavailable`. The client gets the warning `stt_segment_failed` for each lost upload.

**Worked example.** The agent is speaking; the caller says a 2,400 ms sentence. Times are milliseconds after the caller's first speech frame.

- 224: onset confirmed, the turn is admitted, nothing else happens.
- 512: voiced time reaches the 500 ms threshold. The agent's reply is cancelled, history is cut to what was heard, and audio stops within about 50 ms.
- 2,624: the cut; the upload starts. About 2,690: the endpoint is decided.
- 3,600: the transcript returns (a typical 1,200 ms after end of speech), the turn closes, the agent turn starts.
- About 4,700: first audio of the reply. Reported response latency: 2,300 ms.
- If the vendor never answers: at 8,624 the upload's deadline passes (the newest cut plus `final_deadline_ms`, 6,000 ms), the turn closes as lost, and the notice is spoken.

| Number | Value | Source |
| --- | --- | --- |
| Barge-in threshold | larger of the agent's `min_speech_ms` and 500 ms; takes effect at 512 ms | Estimated. LiveKit publishes 0.5 s as its default; not measured on our calls |
| Onset confirmation; the cut pause (`cut_pause_ms`) | 224 ms each | Engine constants; no agent setting in Release 1 |
| Threshold on the energy detector | at least 800 ms | Estimated |
| Silence ceiling when no model says "finished" | 1,500 ms; for an agent 1,000, 1,250 or 1,500 ms by eagerness | Integration decision; matches the streaming path in code |
| Audio model threshold | 0.7 | Measured: the repository's accuracy test shows scores between 0.5 and 0.73 |
| End of speech to first agent audio | about 1.8 to 2.6 s typical when an end-of-turn model says "finished"; otherwise the later of transcript and silence ceiling | Computed: transcript 0.8 to 1.4 s (estimated; vendors publish only 99th percentiles, 0.65 to 2.01 s) + language model to first spoken token about 0.3 s (a non-reasoning model; the June 2026 figure of 235 ms was a reasoning model's first token) + first synthesized audio 0.7 to 0.9 s (measured June 2026) = about 1.8 to 2.6 s |
| Bound within which a turn closes | 6,922 ms by default, from the last voiced sample; read from the engine (`resolution_deadline_ms`) | Reported by the engine; not computed here |
| First filler due | end of speech + max(configured, min(2,000 ms, expected median + configured)); configured is 2,500 ms by default | The performance part's rule; the default is the agent contract's |
| Stand-in for an agent's `min_words` | 300 ms of speech per word, at most 1,500 ms | Estimated; no measurement |
| Lost-turn rule | 500 ms voiced, two notices, third ends | Resilience research; unmeasured |

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| What interrupts the agent | The detector, after sustained speech; short overlaps only by a transcript that passes the tests | The transcript is a second late, and barge-in cannot be undone before Release 6 | About 200 ms more talk-over than an onset trigger; a 700 ms cough still silences the agent |
| Where "is this input?" is decided | At speech time, once per caller turn, in the voice manager | Deciding at arrival answers the wrong question | A caller who pauses while speaking over a protected greeting loses the part before the pause |
| A lost turn | Notice, notice, then end the call; a marker for lost words goes only into the language model's input | A silent open line otherwise runs to the 1,800 s session limit | A long vendor outage ends calls |
| The agent's turn-detection settings | Threshold fixed at 0.7; eagerness picks the silence ceiling; an explicit `max_endpointing_ms` replaces it, clamped to 800 to 3,000 ms | Thresholds of 0.5 and 0.3 make every pause an end of turn | Eagerness no longer tunes the model; exactly 3,000 ms cannot be asked for yet |
| A client commit (`audio_end`) | Flush, wait for the turn's final, then commit; the final always comes, empty if nothing was recognised | A fixed 300 ms sleep answers one turn late | The commit can wait up to the engine's deadline |
| Fillers and the latency clock | Counted from end of speech; a filler may be spoken before the transcript (Release 2) | Otherwise seconds of dead air on slow turns | In Release 1 a slow turn is silent |
| Realtime speech events | `speech_started` only when the gateway takes speech as a turn; one opener and one close per item | Realtime clients stop playback on that event | In Release 1 these events are still timed by the transcript |
| Recovery after a false interruption (the agent stopped for a cough or a noise) | Release 6: pause, then commit or resume, only when pausing cannot let the agent act; on LiveKit and on `/ws` clients that declare support | A paused run keeps executing tools; the client holds the playout buffer | Never on `/v1/realtime` or the conversation loop |
| Greeting defect in shared code | Each utterance carries its own "may be cut"; on for segmented sessions in Release 1, behind a flag for streaming sessions in Release 2 | Detector barge-in uses the same clear; streaming must stay byte-identical in Release 1 | Two code paths until the flag is `all` |
| The existing continuous end-of-turn pipeline (SmartTurn), which by reading never runs its model on live audio | Confirm the defect with tests; do not fix it here | The fix changes streaming sessions, and the segmented path does not use that pipeline | Streaming agents keep having no model-based end of turn |

## What changes in the code

Paths are under `gateway/src` unless they start with `bud-auth/`.

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `core/turn/controller.rs`, `signal.rs` | `TurnController::feed`, `feed_for_turn`, `ControllerSignal`, `TurnEvent::Aborted` | Three speech signals; one lock around `feed`; a turn that closed without text is aborted | modified | 1 |
| `core/turn/strategies/detector.rs` | `DetectorSpeechStart` | The start strategy for detector and text | new | 1 |
| `core/turn/segmented.rs` | `CallerActivityTracker`, `agent_endpoint_tuning` | One record per caller turn: admission, expected final, lost words, expiry | new | 1 |
| `handlers/ws/turn_build.rs` | `agent_strategies`, `conversation_strategies`, `dag_strategies` | One builder per loop and for the DAG driver; the streaming arm is today's code moved | new | 0, 1, 2 |
| `core/voice_manager/manager.rs` | `VoiceManager::new`, result wrapper, `on_segment_outcome`, `speak_if_epoch` | Speech dispatcher, outcome dispatcher, gate; admission replaces the arrival-time drop; per-utterance interruptibility | modified | 1; flag in 2 |
| `handlers/ws/agent.rs` | `AgentTurnInput`, `wire_agent_turn_taking` | Callback extracted with a golden recording, then the segmented branch and speech task | modified | 0, 1 |
| `core/agent/engine.rs` | `start_turn_at`, `TurnKind::Notice`, `caller_turn_without_text`, `is_probable_echo` | Held input, lost-turn rule, end-of-speech clock, one lock for barge-in and turn start | modified | 1 |
| `core/agent/engine.rs`, `spoken.rs` | `arm_waiting_filler`; `pause_for_speech`, `rebase_for_resume` | Filler before the transcript; pause, commit, resume | modified | 2; 6 |
| `core/conversation/mod.rs`, `handlers/ws/config_handler.rs` | `initialize_conversation_loop` | `Aborted` arm; then speech task, hold, idle, masking, lost turns | modified | 1; 2 |
| `handlers/ws/processor.rs`, `audio_handler.rs` | `AudioEnd`, `handle_audio_end` | Flush, wait and commit in a spawned task | modified | 1 |
| `handlers/openai_realtime/cascade.rs` | commit act; `UserItem`, `core` | Shared commit helper; then detector-timed events and the notice response | modified | 1; 2 |
| `handlers/ws/bud_legs.rs` | `prepare_agent` | Builds the agent's `EndpointTuning` | modified | 1 |
| `bud-auth/src/voice_agent.rs` | `AgentTurnDetection.max_endpointing_ms` | Becomes optional, so a field left out can be told from a chosen value | modified | 1 |
| `handlers/ws/messages.rs` | outgoing message variants | Turn-level messages; pause and resume later. Shapes belong to the customer contract; the protocol version stays `"1.0"` | modified | 1; 6 |
| SmartTurn files, `core/voice_manager/tests.rs` | confirming tests | Tests only | new | 0 |

## What this part gives to and needs from the other parts

**Needs.** From the engine: `SpeechActivity` (started, stopped, endpoint decided, turn closed), `SegmentOutcome` with the failure class and the position of each gap, the Result contract (one final per caller turn; always one after a client commit), the flush request, and `resolution_deadline_ms`. From capability resolution: `ResolvedSttLive` inside `SttLiveSession`, carried with `SttLiveShared` on the voice manager's configuration, and the routing for a model whose only gateway client buffers audio until hang-up, applied before this part runs. Once the rollout switch covers a session, voice agents in either turn mode, conversation-loop sessions and DAG sessions go to the engine; a *plain* session (`/ws` with no voice agent, conversation loop or DAG pipeline; its client reads the transcripts) keeps today's client with the warning `stt_buffered_until_commit` unless it sets `transcription_mode: segmented` (from Release 2), because today it gets one transcript per `audio_end` and the engine could end a turn at a pause before the client's `audio_end`. On an uncovered session a voice agent with automatic turn detection is refused (`stt_live_unsupported`) and every other session keeps today's path with that warning. On `/v1/realtime` such an agent cannot work today, since a commit acts only in manual mode (`handlers/openai_realtime/cascade.rs:415-423`); on `/ws` its client's `audio_end` can end a turn (`handlers/ws/audio_handler.rs:301-322`), so the refusal there is a deliberate choice (the plan's sign-off item 3). A `/v1/realtime` agent admitted in manual mode while uncovered cannot turn turn detection on later: the cascade passes its manual flag to the resolver and rejects that `session.update` with the same code. OpenAI's live-only models are refused on every session until Release 4. A missing detector is refused with `stt_segmentation_unavailable`. From the customer contract: the wire messages for turn start, turn end and turn closed, `stt_segment_failed`, `stt_unavailable`, and the response kind `notice`; its code table is the single source for every code quoted here. From performance: every time limit, the latency store's expected median for the filler rule, and two observer hooks. From cost and rollout: the metering sink, the Release 0 test kit, and the recorded-call tests behind the gates.

**Gives.** The dispatcher that delivers each `SegmentOutcome` to metering, turn-taking and the wire, with one registration point that accepts late registration; `VoiceManager::new` installs it in the outcome-sink slot of `SttLiveSession` before it calls the engine's factory. The admission answer the engine asks before each upload, with "the agent was audible" as evidence for the quality filter. The `EndpointTuning` of a voice agent: the policy, `min_end_silence_ms` (the agent's `silence_ms`, the least silence that ends a turn) and `silence_ceiling_ms`; `cut_pause_ms` stays the engine's 224 ms. The resolver raises a deployment's upload deadline to at least this ceiling plus 2,500 ms. The translation of turn events and of `stt_segment_failed` into Realtime events. Counters for interruptions by trigger and outcome, lost turns, and turns by close reason.

## Tests to write first

Criteria: (1) ordered transcripts and one end-of-turn result per caller turn; (2) interruption decided by the detector, and an empty or invented overlap resumes the agent; (3) a live call on a file-only model yields a transcript and a reply (or one DAG run) on `/ws` and `/v1/realtime`; (4) end of speech to final transcript is measured; (5) streaming models take exactly today's path, apart from deliberate changes behind their own flags. This part has no live test; live vendor calls are in the rollout chapter. Its one test over a real socket lives in the new integration binary `segmented_stt_ws`, built with the gateway's `test-util` feature, which Release 0 adds. The full design names 121 tests: 12 for Release 0, 70 for Release 1, 22 for Release 2, 1 for Release 4 and 16 for Release 6. The table lists the ones that carry a criterion or a decision above.

| Test name | Level | What it proves | Criterion | Release |
| --- | --- | --- | --- | --- |
| `streaming_sessions_build_todays_strategy_sets` | unit | Streaming sessions get exactly today's strategies | 5 | 0 |
| `the_streaming_agent_callback_matches_the_golden_recorded_before_the_change` | unit | Extracting the callback changed nothing | 5 | 0 |
| `packets_of_20_ms_never_accumulate_a_model_window` | unit, `smart-turn` feature | SmartTurn's model never runs on live packets | none | 0 |
| `today_a_reply_after_a_non_interruptible_greeting_cannot_be_cleared` | unit | The greeting defect is real | none | 0 |
| `sustained_speech_interrupts_only_after_the_threshold` | unit | 224 and 384 ms do nothing; 512 ms interrupts | 2 | 1 |
| `a_cough_below_the_threshold_never_interrupts_and_leaves_no_open_turn` | unit | Short noise is harmless | 2 | 1 |
| `two_threads_feeding_the_controller_never_lose_a_final` | unit | The atomic feed loses no transcript | 1 | 1 |
| `two_segments_of_one_caller_turn_start_one_agent_turn` | unit | One reply per caller turn | 1 | 1 |
| `an_empty_final_after_a_commit_passes_the_wrapper_and_its_record_is_kept_until_then` | unit | A push-to-talk client always gets its answer | none | 1 |
| `segment_outcomes_reach_metering_turn_taking_and_the_wire_and_survive_late_registration` | unit | The fan-out and late registration | none | 1 |
| `the_dag_runs_once_for_a_two_segment_turn` | unit | Two segments of one turn run the DAG once, on the joined text | 1, 3 | 1 |
| `a_lost_caller_turn_is_answered_with_a_notice_twice_and_the_third_ends_the_call` | unit | No silent open line | none | 1 |
| `response_latency_is_measured_from_the_end_of_caller_speech_and_the_model_figure_is_not` | unit | The reported latency includes the transcript wait | 4 | 1 |
| `agent_chain_barge_in_fires_before_any_transcript_and_one_reply_follows` | in-process chain | The real engine, voice manager and agent loop together | 1, 2 | 1 |
| `agent_chain_a_cough_rendered_as_thank_you_leaves_the_agent_talking` | in-process chain | An invented phrase does not cut the agent | 2 | 1 |
| `cascade_chain_a_segmented_call_produces_a_transcript_and_a_reply` | in-process chain | The Realtime surface works; detector-timed events added in Release 2 | 3 | 1, 2 |
| `detector_barge_in_stops_the_agent_over_a_real_socket` | mock vendor over a real socket | Interruption end to end | 2 | 1 |
| `conversation_chain_barge_in_and_one_reply` | in-process chain | The conversation loop | 1, 2 | 2 |
| `livekit_chain_barge_in_empties_the_room_playout_and_one_reply_follows` | in-process chain | The LiveKit audio path and playout clear | 2 | 2 |
| `an_empty_overlap_transcript_resumes_the_agent` | unit | Resume after a false interruption | 2 | 6 |

## What ships in which release

- **Release 0, groundwork and honest refusal.** This part changes no call: golden recordings of the two streaming transcript callbacks, the snapshot of today's strategy sets, three SmartTurn confirming tests (the one needing model files runs in the accuracy job), and the confirming test for the greeting defect. This part's counters appear on `/metrics` at zero.
- **Release 1, first working calls.** The voice-agent loop: speech-time admission, detector and text-confirmed barge-in, held input, the lost-turn rule, the idle rule, the end-of-speech clock, client commit through the flush, the outcome dispatcher, `EndpointTuning` with the optional `max_endpointing_ms` in bud-auth, and the greeting fix on segmented sessions only. A conversation session or a `/v1/realtime` call on an allow-listed deployment works, with interruption and speech events still timed by the transcript. A DAG pipeline there runs once per caller turn, with speech-time admission (one test).
- **Release 2, dark launch complete.** The conversation loop; Realtime events timed by the detector; the filler before the transcript; the greeting fix for streaming sessions behind `WAAV_PER_UTTERANCE_INTERRUPTIBILITY`, with a release note; the LiveKit chain test.
- **Release 3, default on.** No code. The rollout gates this part defines are measured and applied: the rate of interruptions that end with no usable text, the rate of text-confirmed interruptions, and the share of turns closed by the silence ceiling. Bud's builder wording for interruption controls ships with it.
- **Release 4, live-only models and low latency.** No code. A session on a socket where the gateway sends the commit gets the same turn-taking (one test): OpenAI's live-only models on OpenAI and Azure OpenAI (after a live probe), Cartesia, and any row with a usable socket on a deployment set to the low-latency tier. Cartesia is a deliberate change to streaming sessions: its interruption becomes detector-timed, behind `WAAV_STT_CARTESIA_MANUAL_FINALIZE`, off until a live probe passes, with a release note.
- **Release 6, interruption recovery and interim text.** Pause, then commit or resume, on LiveKit and on `/ws` clients that declare support.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| The detector hears the agent's own voice (no echo cancellation on `/ws`, the cascade or SIP) | Medium, unmeasured | The agent interrupts itself and answers its own words | 500 ms floor; echo comparison; the threshold doubles after two empty interruptions; measured before Release 3 |
| A cough of 500 ms or more silences the agent | Medium | A broken reply and a pause until the caller speaks | Counted, with a gate; resume arrives in Release 6, but never on `/v1/realtime` |
| The audio model says "not finished" on a finished short answer | High | Up to 1.0 to 1.5 s more wait | The text model as a second step; the silence ceiling; turns counted by close reason |
| A vendor outage loses every turn; so does a mistyped model in Release 1, which has no setup probe | Low to medium | Notice, notice, then the call ends | The lost-turn rule; an authentication failure ends the call at once; the probe arrives in Release 2 |
| An event is lost and a caller turn never closes | Low | Idle never fires; held input sticks | Three expiry rules on the tracker; a 15 s hold limit |
| Two tasks feed one turn controller | Low once fixed | A transcript vanishes between a verdict and a state change | One lock around `feed`; a two-thread test |
| On `/v1/realtime`, Release 1 speech events are timed by the transcript | Certain | The client stops its playback late | Stated limit; the allow-list starts with `/ws` voice agents; fixed in Release 2 |
| The greeting fix changes streaming agents | Certain once the flag is `all` | Barge-in starts working after a protected greeting | Segmented only in Release 1; flag, release note and goldens in Release 2 |
| An agent's `max_endpointing_ms` cannot be told from the published default (`bud-auth/src/voice_agent.rs:57-78`) | Certain until Bud changes | An explicit 3,000 ms is ignored | Exactly 3,000 is read as "not set", so the eagerness ceiling applies; bud-auth makes the field optional in Release 1, and the reading ends when Bud stops publishing the default (a Bud-side task, no date) |

## What is still open

**Measurements needed.**

- The rate of interruptions that end with no usable text, at 300, 400 and 500 ms, per transport including SIP.
- How often the agent interrupts itself, and how often the echo comparison catches it.
- The share of caller turns closed by each step of the end-of-turn decision (audio model, text model, silence ceiling), and how often a one-word answer scores under 0.7.
- Words against voiced duration, for the 300 ms per word stand-in used when an agent sets `min_words`.
- Whether reference Realtime clients play a response that carries a notice and no user input.
- How many conversation-loop clients on OpenAI or Groq file models end each turn with `audio_end` (they must today); once covered, a pause before their `audio_end` can end the turn early.

**Choices that need a person.**

- Silence ceiling of 1,500 ms although the agent contract publishes 3,000 ms. Default: 1,500 ms.
- A marker such as `[inaudible]` only in the language model's input. Default: yes.
- Whether operators set the greeting-fix flag to `all` from Release 2, which covers streaming sessions too. Default: yes, with a release note.
- Launching without resume before Release 6, and never on `/v1/realtime`. Default: accept.
- Ending a voice-only call on the third lost turn in a row. Default: yes.
- Wording of the notice and closing line. Default: reuse the agent's `degradation_message`.
- Which deployments the Release 1 allow-list starts with. Default: those of `/ws` voice agents, because Realtime speech events are late until Release 2.
- Whether three lost turns within a few seconds count as three (an open breaker loses a segment at once). Default: keep the plain count in Release 1 and measure.

## Where the detail is

- Full design: `/home/bud/ditto/waav/research/segmented-stt/design/W4-turn-taking-and-interruption.md` (sections 13 and 14 list what the integration decisions and their Addendum A changed).
- Critiques, in the same directory: `critique-W4-turn-taking-and-interruption-code.md` and `critique-W4-turn-taking-and-interruption-adversarial.md`.
