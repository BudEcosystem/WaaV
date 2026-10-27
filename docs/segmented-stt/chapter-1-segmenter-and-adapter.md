# Chapter 1. The segmented session engine

Code paths are relative to `/home/bud/ditto/waav/WaaV/gateway/src` unless they start with `/`. A release number in a table refers to the release of that number under "What ships in which release". Codes are quoted from the customer-contract chapter, their single source.

## What this part is and why it exists

Some speech-to-text models accept only a finished audio file. A live call has no finished file: audio arrives in small packets for as long as the caller is connected. The segmented session engine sits between the two. It listens to the caller with the gateway's own voice-activity detector (Silero, a small neural model that gives a speech probability for every 32 ms of audio), cuts the audio into *segments* at pauses, hands each segment to the vendor as one file, puts the returned text back in order, and decides when the caller's *turn* (everything said before the gateway judges the caller has finished) is over. To the rest of the gateway it looks like a streaming vendor: it implements the existing `BaseSTT` trait (`core/stt/base.rs:658`) and emits the existing result type.

Without it, a call on a file-only model fails at setup, or returns no text until the client sends `audio_end` or hangs up, which is today's behaviour. A naive version fails in other ways, and this design exists to prevent them: one sentence becomes two agent replies because a per-segment result was treated as the end of the turn; a slow upload blocks the session's only audio task; a late transcript is attached to the wrong turn; a vendor that never answers leaves the caller's turn open for ever.

## How it works

A new module, `core/stt/segmented/`, with one task per session that owns all state.

| Part | What it does |
| --- | --- |
| Adapter (`SegmentedStt`) | Implements `BaseSTT`. `send_audio` runs under the voice manager's exclusive lock on the session's only audio task (`core/voice_manager/manager.rs:636-639`), so it only stamps the chunk and puts it on a queue. |
| Front end | Turns wire bytes (16-bit PCM or G.711, any rate) into 16 kHz mono frames of 512 samples. Time is counted in samples, so behaviour does not depend on delivery speed. |
| Segmenter | A state machine. Confirms speech, opens a segment that starts with *pre-roll* (audio from before the first speech frame), and makes a *cut* when a pause is long enough. Splits very long speech at a micro-pause. |
| Endpointer | Decides the end of the turn with a three-step ladder: an audio end-of-turn model (SmartTurn) asked once at the pause; if that says "not finished", the gateway's existing text end-of-turn model once the transcript is back; otherwise a *silence ceiling*. |
| Sequencer | Holds, joins and starts uploads. Makes exactly one call per *upload unit* (one segment, or several held segments joined) to the upload component, which belongs to the transcriber layer, sends at most two vendor requests, and contains the limiter that keeps uploads under the vendor's request limit. Releases results in sequence order and applies the transcriber layer's check for invented text at release. |

**What the rest of the gateway receives.** While a turn is open, each returned segment produces an interim result carrying the turn's text so far. When the turn closes, exactly one result is sent that is both final and end-of-turn, with the whole text. A result that is final but not end-of-turn is never sent; that is the only shape that arms the existing 600 ms and 1,500 ms transcript timers (`core/voice_manager/stt_result.rs:171-173`), so the result processor is not edited. Speech events (`Started`, `Stopped`, `EndpointDecided`, `TurnClosed`) are reported from the detector, before any transcript exists, so that interruption does not wait for an upload.

**Time limits.** The real-time performance part owns every value. The sequencer passes them on whole and computes one thing: the *deadline instant* of a unit, which is the newest cut of the caller's turn at the moment the unit is handed over, plus the deadline (6,000 ms by default). Nothing times out while audio is still held in the engine. The resolver raises a deployment's deadline that is below the silence ceiling plus 2,500 ms to that value and reports the change, so that held audio keeps time to upload. The engine reports `final_deadline_ms`, the deadline in force (6,000 ms, or a deployment's raised value), counted from the turn's newest cut, which is when `Stopped` and the wire's `speech_end` are sent; it excludes the 224 ms cut pause. It also reports `resolution_deadline_ms`, the bound within which a turn closes: 6,922 ms by default, counted from the last voiced sample (the last audio judged to be speech).

**Worked example: one 3-second utterance.** Times in milliseconds from the first speech sample.

| Time | Event |
| --- | --- |
| 224 | Seven speech frames: speech confirmed, `Started`. The segment opens with audio from −400. |
| 3,008 | Last speech frame ends. |
| 3,232 | 224 ms of silence: cut, `Stopped`. 4,132 ms of audio is handed over (3,632 real, 500 of zeros). Deadline instant 9,232. The audio model is asked. |
| about 3,280 | Model says "finished": `EndpointDecided`. |
| 4,032 | The vendor answers (an 800 ms round trip, assumed for illustration). `TurnClosed`, then the one final result. |

End of speech to final: 1,024 ms. Three variations on the same call: a one-word "Yes." that the audio model calls "not finished" is rescued by the text model at about 1,070 ms, or ends at the 1,504 ms ceiling on a build without it. If the vendor's rate limit admits nothing, the audio is held, handed over at 3,280 and refused at 4,780 (a 1,500 ms wait); the turn closes with an explicit "speech was lost" signal. If the vendor never answers, a second request goes out at about 5,740 and the unit is given up at 9,232, 6,000 ms after the cut: the reported `final_deadline_ms`.

| Number | Value | Where it comes from |
| --- | --- | --- |
| Frame | 32 ms (512 samples at 16 kHz) | Required by the Silero model |
| Speech confirmed after | 224 ms of speech frames | Estimated; Pipecat, an open-source voice-agent framework, uses 200 ms |
| Pre-roll | 400 ms before the first speech frame | Practice (LiveKit keeps 500 ms). Pipecat measured, on one model, that 400 ms counted from the later "speech confirmed" event cuts the first word, hence the anchor |
| Cut pause (`cut_pause_ms`) | 224 ms, an engine constant in Release 1 | Pipecat's recommended value (200 ms), rounded to frames |
| Zeros appended | 500 ms; trailing audio at most 1,000 ms | Measured by Pipecat on one local model |
| Shortest segment uploaded | 250 ms of speech span | Silero's published default |
| Split of long speech | soft: the smaller of 20 s and the vendor maximum less 5 s; hard: 25 s | Estimated, from Whisper's 30 s window |
| Audio model threshold | 0.7, fixed | The repository's calibrated default; its test notes put probabilities at 0.5 to 0.73 |
| Silence ceiling | 1,500 ms; 800 to 3,000 when a session sets it | The streaming path's ceiling (`core/voice_manager/config.rs:39`) |
| Deadline; second request; wait at the limiter | 6,000 ms from the cut; about 2,500 ms; 1,500 ms | Estimated by the real-time performance part |
| A turn closes within (`resolution_deadline_ms`) | 6,922 ms of its last voiced sample | Computed: 224 + 6,000 + a 250 ms guard + 448, the longest an unconfirmed onset of speech can hold the close |
| Audio uploaded per unit | speech plus about 1.12 s | Computed |
| Vendor 99th percentile, end of speech to final | 2,010 ms for `gpt-4o-mini-transcribe` and ElevenLabs `scribe_v2`; none for `gpt-transcribe` | Published by Pipecat, without this padding |

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| How results are encoded | Interims with the turn's text so far, then one final end-of-turn result | The transcript timers can never fire; a dropped interim loses nothing; a late fragment cannot leak into the next turn | `/v1/realtime` shows no text until the turn ends |
| Who owns the detector | One per segmented session, inside the engine | The voice manager's existing detector skips frames under contention and its output is unusable here | No speculative language-model start on these sessions |
| Detector model cannot be loaded | Refuse the session at admission (session setup, before anything is built) with `stt_segmentation_unavailable`; `WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1` allows a volume-based detector; builds without Silero use it by default | The volume-based detector is much worse and would silently become permanent | A missing file refuses calls, so the image must carry the files. Needs sign-off |
| A transcript after its deadline | Given up, never delivered | Late delivery splits turns and can repeat an agent's tool call | Words are lost when a vendor is slow; reported, not hidden |
| Retries | None in the engine: one call per unit | Only the layer that sees the HTTP status can classify a failure; stacked retries would multiply requests | The engine depends on that call returning by the deadline; a 250 ms guard covers a defect |
| End of turn | Three-step ladder; threshold fixed at 0.7; an agent's eagerness changes the ceiling | The model often says "not finished" on finished speech | The agent contract publishes 3,000 ms as its default (needs sign-off). Exactly 3,000 is read as "not set" until bud-auth makes the field optional, a Release 1 task for bud-auth and Bud. With one upload per turn the text step cannot apply |
| Stopping and flushing | `disconnect()` stops at once and drops uploads; a commit travels on the audio queue | A flush on a side channel would overtake the caller's last words | One possibly billed request per hang-up. It is not charged; its sent seconds and an estimate of the vendor's bill are recorded |
| Client commit | Cut at once; upload unconfirmed audio that is not silence; always one final, empty if nothing was recognised | Push-to-talk clients wait for an answer | Unconfirmed audio can produce invented text |
| Lost words | No marker in the transcript; the position travels on the outcome | Customer text holds only what the vendor recognised | A marker reaches only the language model. Needs sign-off |

## What changes in the code

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `core/stt/segmented/` (16 files) | Front end, detectors, `Segmenter`, `Endpointer`, `Sequencer`, turn assembler, engine and emitter tasks, `SegmentedStt::new_live(shared, session)` | The engine | New | 1; the detector trait and clock helpers in 0 |
| `core/stt/segmented/testkit.rs` | Scripted detector; scripted upload and end-of-turn models; a probe that counts engine tasks | Contributions to the shared test kit | New | 0, then 1 |
| `core/stt/speech_activity.rs` | `SpeechActivity`, `FlushOutcome`, `SttLiveFacts`, `SttNotice`, the admission hook types | Types the trait refers to | New | 1 |
| `core/stt/base.rs:817-819` | `on_speech_activity`, `set_segment_admission`, `request_flush`, `on_notice`, `live_facts` | Five defaulted trait methods; other providers are untouched | Modified | 1 |
| `core/stt/base.rs:351` | `STTResult.speech_turn_id` | Optional field, `None` from the constructor | Modified | 1 |
| `core/voice_manager/manager.rs:1648-1672` | `finalize_stt` | Ask for a flush and await it outside the lock; otherwise today's disconnect and reconnect, with any Release 0 repair of the OpenAI and Groq clients | Modified | 1 |
| `core/voice_manager/manager.rs` | Accessor for `live_facts()` | Lets session setup read what the engine reports | Modified | 1 |
| `core/smart_turn/detector.rs` | `probability_blocking` | Crate-private synchronous inference for the shared on-demand pool | Modified | 1 |
| `core/audio/resampler.rs` | New constructor | Turns off the 200 ms wall-clock reset (`:31`, `:192-199`) | Modified | 1 |
| `core/state.rs` near `:145` | `segmented::models::warm_up()` | Load detector and end-of-turn models once per process | Modified | 1 |
| `init.rs:36-58` | `run` | Also fetch the Silero and SmartTurn files (done by the rollout part) | Modified | 0 |
| `core/stt/segmented/engine.rs` | Audio shortfall figure | Count audio lost before the engine | Modified | 2 |
| `core/stt/segmented/engine.rs` | Live-audio tap | Drive vendor sockets that need a gateway commit | Modified | 4 |
| `core/stt/segmented/{emitter,sequencer}.rs` | Interims inside a segment | Forward text a vendor streams for an uploaded file, and re-decoded text | Modified | 6 |

Not changed by this part: `create_stt_standard`, every existing provider, the result processor, the turn controller.

## What this part gives to and needs from the other parts

**Gives.**

- The result contract, to turn-taking: the two result shapes above, and a turn id on every result. A DAG pipeline on a live session gets the same contract.
- `SpeechActivity`, to turn-taking and the customer contract. `TurnClosed` carries whether text exists, how much speech was lost, and `result_follows`.
- `SegmentOutcome`, one per upload unit: timings, uploaded and billed seconds, the result kind (text, empty, filtered, failed, timed out), requests made. The engine calls one sink, turn-taking's dispatcher, which delivers to turn-taking, to the wire (as the warning `stt_segment_failed`) and to metering.
- Typed facts: the detector in use and `final_deadline_ms`, which `ready.stt` copies unchanged; `resolution_deadline_ms`, which turn-taking reads as its expiry.
- Typed notices: `AudioDropped` (the warning `stt_audio_dropped`) and `NoiseThresholdRaised` (logged and counted, not sent to the client).
- `build_support()` and `can_decode_encoding()`, to the resolver, so that refusals happen at admission.
- Latency samples, to the latency store.

**Needs.**

- From the capability map and resolver: `ResolvedSttLive` with the segment profile; `SttLiveShared` (process-wide, with the retry budgets) and `SttLiveSession` (per session). The session value carries `EndpointTuning`, the standardized speech-to-text configuration (language, wire audio format), the leg credential, and the outcome-sink slot, which the voice manager fills with its dispatcher before it calls the factory. `EndpointTuning` has three named fields beside the policy: `cut_pause_ms`, `min_end_silence_ms` (an agent's `silence_ms`, or the canonical `endpointing_ms`) and `silence_ceiling_ms`.
- From the transcriber layer: `SegmentTranscriber` behind its attempt loop, `SegmentTranscript`, and the check for invented text.
- From real-time performance: `SegmentDeadlines` from the latency store, for every unit. The type lives in a neutral module, `core/stt/segment_limits.rs`, shared with the transcriber layer.
- From turn-taking: the admission hook (is this speech input?) and the dispatcher.
- From cost, observability, tests and rollout: the rules for holding and joining audio, the limiter, the upload adapter and its record of what reached the vendor, the test kit, and the model files in the image.

## Tests to write first

Acceptance criteria of the brief, in short: (1) segments are correct and each turn gets exactly one end-of-turn result; (2) interruption comes from the detector; (3) a call on a file-only model yields transcripts and a reply; (4) end of speech to final within 2.5 s at the 99th percentile; (5) streaming sessions are unchanged. Tests over a real socket against a mock vendor (the integration binary `segmented_stt_ws`) and live tests belong to the rollout part and drive this engine. Tests that need the real Silero or SmartTurn files run in an integration binary in the continuous-integration accuracy job, the only job that downloads them. Tests that drive time use tokio's paused clock, enabled by the gateway `test-util` feature added in Release 0.

| Test name | Level | What it proves | Criterion | Release |
| --- | --- | --- | --- | --- |
| `the_continuous_pipeline_does_not_reach_inference_with_20_ms_packets` | integration binary, real model files | The existing continuous end-of-turn pipeline never runs, so the engine must ask on demand | groundwork | 0 |
| `pre_roll_is_measured_back_from_the_first_speech_frame` | unit | The first word is in the upload | 1 | 1 |
| `uploaded_audio_ends_with_the_hangover_then_the_configured_zeros` | unit | Padding is real audio, then zeros | 1 | 1 |
| `a_segment_under_the_minimum_speech_span_is_discarded_with_an_outcome` | unit | Noise blips are never uploaded | 1 | 1 |
| `a_split_shares_the_pause_and_no_speech_sample_is_lost_or_duplicated` | unit | A 40 s utterance is split, not truncated | 1 | 1 |
| `results_are_released_in_sequence_when_responses_arrive_out_of_order` | unit | Transcript order | 1 | 1 |
| `no_emitted_result_is_ever_final_without_speech_final` | unit | The timer-arming shape cannot be built | 1 | 1 |
| `segmented_result_shapes_arm_no_forced_final_timers` | unit, in `stt_result.rs` | The real result processor arms nothing | 1 | 1 |
| `transcripts_reach_a_real_turn_controller_in_order_and_stop_the_turn_once` | in-process chain | Two segments start one agent turn | 1, 3 | 1 |
| `the_upload_is_called_exactly_once_per_unit` | unit | The engine never retries | 1 | 1 |
| `one_utterance_never_produces_more_than_two_vendor_requests` | in-process chain | The same, with the real upload component | 1 | 1 |
| `under_the_per_turn_policy_nothing_is_uploaded_before_the_endpoint` | unit | One upload per turn for rate-limited vendors | 3 | 1 |
| `the_deadline_instant_is_the_newest_cut_of_the_turn_at_hand_over_plus_the_deadline` | unit | The one time limit the engine computes | 4 | 1 |
| `the_reported_final_deadline_is_the_deadline_in_force_counted_from_the_cut` | unit | 6,000 by default, else the raised value; the cut pause is not added | 4 | 1 |
| `the_reported_resolution_deadline_bounds_every_scripted_turn` | unit | Every turn closes within it, even with a silent vendor | 4 | 1 |
| `end_of_speech_to_final_is_the_cut_delay_plus_the_upload_delay_when_the_verdict_is_finished` | unit | The gateway adds no hidden wait | 4 | 1 |
| `speech_events_are_not_delayed_by_a_blocked_result_callback` | unit | Interruption does not queue behind a reply | 2 | 1 |
| `a_commit_after_only_silence_uploads_nothing_and_closes_one_turn_with_an_empty_final` | unit | A commit is always answered | none: the commit rule | 1 |
| `stopping_the_session_ends_the_engine_tasks_even_when_the_callback_holds_the_manager` | in-process chain | No task leak per call | none: teardown | 1 |
| `finalize_stt_on_a_streaming_provider_is_unchanged` | in-process chain | Today's disconnect and reconnect, which a warned session on a buffering model (`stt_buffered_until_commit`) relies on at every `audio_end`, plus any Release 0 repair on OpenAI and Groq | 5 | 1 |
| `providers_that_do_not_override_the_new_hooks_keep_the_defaults` | unit | New trait methods do nothing elsewhere | 5 | 1 |
| `silero_keeps_one_word_answers_on_eight_khz_g711_audio` | integration binary, real model files | "Yes" on a telephone line is uploaded | 1 | 1 |
| `a_dropped_frame_through_receive_audio_advances_no_sample_time_and_is_counted` | in-process chain | LiveKit and SIP: lost audio delays thresholds, never shortens them | 1 | 2 |
| `the_live_audio_tap_sees_every_frame_and_each_speech_edge_on_the_engine_task` | unit | Commit sockets are driven by the same engine | 3 | 4 |

## What ships in which release

- **Release 0, groundwork and honest refusal.** The detector trait, scripted detector and clock helpers for the test kit; the confirming test on the continuous end-of-turn pipeline, run by the accuracy job. No engine behaviour.
- **Release 1, first working calls.** The whole engine: front end, Silero, volume-based and scripted detectors, segmenter, the end-of-turn ladder, the sequencer with both upload policies (per pause and per turn), the result contract, the trait additions, the `finalize_stt` change, the facts and the checks the resolver uses. Time limits come from seeded values. Every upload carries the session's language when known. Streaming sessions match the golden recordings, differing only by `ready.stt` where the rollout switch covers them. A conversation-loop or `/v1/realtime` call on an allow-listed deployment works, with interruption and speech events still timed by the transcript. With no setup probe yet, a mistyped model fails at its first utterance ("model not served").
- **Release 2, dark launch complete.** Samples and timeouts feed the latency store; the LiveKit and SIP rule gets its test and its counter; the commit test for `/v1/realtime`; detector timing for the conversation loop and `/v1/realtime`; measurements on allow-listed traffic. The transcriber layer's session language vote (pin the language the first segments agree on) moves here from Release 5; it sets the request's existing language override, so the engine is unchanged. The 2.5 s criterion is an exit condition of this release for the Release 1 vendors; rows that upload once per turn are exempt and labelled.
- **Release 3, default on.** No new code. Unmeasured thresholds are re-set from Release 2 data; the share of turns ending at the ceiling and the rate of lost units are gates.
- **Release 4, live-only models and low latency.** The live-audio tap, so that sockets needing a gateway commit (OpenAI and Azure OpenAI live-only models, Cartesia's manual finalize) use the same engine. A covered Cartesia session then leaves today's client, behind the flag `WAAV_STT_CARTESIA_MANUAL_FINALIZE`, off until a live probe passes. The release targets 2027-02-26, when OpenAI removes `whisper-1` and the `gpt-4o-*transcribe*` models. What must ship before that date is the change of OpenAI's default model to `gpt-transcribe`, dated separately and outside this part.
- **Release 5, wider vendor coverage and hardening.** No change expected. Two conditional items: a deadline that moves when the caller speaks again, only if a counter shows it is needed; a cut pause a capability row may lengthen (see open choices).
- **Release 6, interruption recovery and interim text.** Interim text inside a segment: text a vendor streams for an uploaded file, and re-decoding through a second entry point on the attempt loop (`run_interim`), so that the transcriber still has one caller.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| The audio model says "not finished" on finished speech | High | Turns wait for the text model or the ceiling | The ladder; the share of turns closed by each step is recorded and gates Release 3 |
| Thresholds are unmeasured on telephone audio | Medium | One-word answers dropped, or noise uploaded | Real-model tests on 8 kHz audio; measurements in Release 2; re-set in Release 3 |
| Steady noise or background speech keeps a segment open | Medium | Noise uploaded in 20 s pieces; the turn does not end | Every state is time-bounded; thresholds rise when a split segment returns no text; a turn is sealed at 60 s. Competing speech at the caller's level is not solved |
| The vendor's rate limit is reached | High at scale | Later finals; lost units | Audio is held and joined; one upload per turn where the row says so; refusal after 1,500 ms with a signal |
| The upload call does not return by its deadline | Low | A turn would hang | A guard releases the unit 250 ms later and counts a defect |
| The detector model file is missing in production | Low once the image carries it | Calls refused | Release 0 image task; model state known per process before any call; operator switch |
| Engine tasks outlive the call | Low | Tasks and a detector session leak per call | `disconnect()` joins all tasks; tested through a real voice manager |
| A client stops sending audio in silence | Medium | The cut comes at 1,000 ms, not 224 ms | Input-stall rule; new SDK versions send audio continuously |
| Invented text on short or unconfirmed audio | Medium | A false turn or interruption | The transcriber layer's check runs at release; outcomes flag suspects and overlap with agent speech |

## What is still open

**Measurements needed.**

1. Audio-model probabilities at a 224 ms pause on real calls, and the share of turns closed by each ladder step.
2. The 250 ms minimum, the loudness floor and the confirmation time on 8 kHz telephone audio and on narrowband audio delivered at 16 kHz through LiveKit.
3. Appended zeros of 0, 300 and 500 ms on each hosted file model.
4. Rates of timed-out and limiter-refused units per deployment, and of units that time out after the caller resumed speaking (the counter for the moving deadline).
5. Whether the noise step (0.1, up to 0.8) stops steady noise without deafening the session.
6. How many clients stop sending audio in silence.
7. CPU and memory per session at a few hundred concurrent calls.
8. How often the LiveKit forwarder sheds frames on these sessions.

**Choices that need a person**, each with the recommended default.

1. Volume-based detector on a production build: refuse by default (as designed).
2. Marker for lost words: never in customer-visible text (as designed).
3. Silence ceiling: 1,500 ms default, 800 to 3,000 when an agent sets it (as designed), although the agent contract publishes 3,000 as its default.
4. Longest turn, 60 s: keep.
5. Letting a capability row lengthen the cut pause for vendors with a billing minimum: not before a measurement; revisit in Release 5.

## Where the detail is

- Design: `/home/bud/ditto/waav/research/segmented-stt/design/W1-segmenter-and-adapter.md` (section 13 and the last section list what the integration decisions and their Addendum A changed). Where the design differs from Addendum B, as on `final_deadline_ms`, Addendum B wins.
- Critiques: `/home/bud/ditto/waav/research/segmented-stt/design/critique-W1-segmenter-and-adapter-code.md` and `/home/bud/ditto/waav/research/segmented-stt/design/critique-W1-segmenter-and-adapter-adversarial.md`.
