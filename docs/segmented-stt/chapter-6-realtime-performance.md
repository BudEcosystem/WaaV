# Chapter 6. Real-time performance: latency budget, tiers and measurement

## What this part is and why it exists

Segmented speech-to-text is slower than streaming by construction: the gateway waits for a pause, uploads the utterance as a file, and only then has text. This part makes sure the gateway adds nothing to that wait but the pause when the end-of-turn models judge the caller finished, bounds what a caller waits when a vendor is slow or stuck, measures what each vendor really takes, and tells the customer honestly. It owns every time limit of one upload, the latency store (seeded and measured figures per deployment), the rule that keeps connections warm, the benchmark and release gate, and the later low-latency options.

Without it three things go wrong. A stuck request holds the caller's turn for today's file-sized timeouts (30 s on the OpenAI client, 120 s on Groq: `core/stt/openai/client.rs:54`, `core/stt/groq/client.rs:57`; paths are under `/home/bud/ditto/waav/WaaV/gateway/src` unless they start with `tests/` or name another part). Every call starts on a cold connection, because each session builds its own HTTP client (`core/stt/openai/client.rs:207`), costing 100 to 440 ms on the first utterance. And nobody can say whether a model is fast enough for a call, because end of speech to final transcript is measured nowhere today.

## How it works

**Terms.** A *segment* is one utterance cut out by the gateway's voice detector. An *upload unit* is the audio sent as one file: one segment under the per-pause policy, or a whole caller turn under the per-turn policy that rows with tight request limits use (Azure OpenAI at default quota). *End of speech* is the caller's last voiced sample. The *short pause* (224 ms, the engine constant `cut_pause_ms`) is the silence the detector waits for before the *cut*. The *hand-over* is when the engine gives a unit to the *attempt loop*, the transcriber layer's only sender of requests. A *round trip* is one request from first byte to parsed response. *P50*, *P95* and *P99* are the values that 50, 95 and 99 percent of samples are at or below. A *seed* is a starting latency figure from the capability map (the table of how each model is reached).

**The standard tier, always on.** A per-pause row uploads the unit at the cut, in parallel with the end-of-turn decision, with up to two uploads in flight; a per-turn row uploads once that decision is made. Uploads use shared clients whose connection pool is kept warm *by depth* (idle timeout 50 s, warm-up 2,000 ms): as many idle connections per vendor host as the sessions on it may have uploads in flight, topped up at session start, at speech start and after any cancelled request (a cancelled HTTP/1.1 request closes its connection and the pool reuses its newest one: `hyper-1.10.1/src/proto/h1/dispatch.rs:292-300`, `hyper-util-0.1.20/src/client/legacy/pool.rs:299-309`). Audio goes up as 16 kHz mono WAV with the session's language whenever it is known. Every time limit comes from one function, `segment_deadlines`:

| Limit | Counted from | Value |
| --- | --- | --- |
| Queue allowance: longest wait at the rate limiter | Hand-over | 1,500 ms |
| Stall timeout: send one second request beside the first, which keeps running | First request's first byte | 1.25 x round-trip P99 + 250 ms, + 50 ms per second of audio beyond 5 s; at least 1,500 ms, at most deadline less 2,500 ms |
| Request limit: cancel one hung request | Each request's start | The row's value, at most 10,000 ms |
| Deadline: give the unit up | Fixed at hand-over: the turn's newest cut plus the value | 6,000 ms; a deployment may set 3,000 to 10,000, and the resolver raises a value below the silence ceiling (defined below) plus 2,500 ms to that figure |

Nothing times out while the engine holds a unit. A unit gets at most two requests. A request that never answered is stored as a *rank-only entry* (it counts as slower than every measured sample and has no value), so stuck requests cannot lengthen any limit.

The engine reports `final_deadline_ms`, the deadline in force (6,000 ms by default, or the deployment's value after any raise), counted from the turn's newest cut, when `Stopped` is reported and `speech_end` sent; it does not include the 224 ms pause, and the customer contract copies it unchanged. It also reports `resolution_deadline_ms`, the bound for a turn to close, counted from the last voiced sample: pause + deadline + 250 ms engine guard on an overrunning upload + 448 ms speech-onset window = 6,922 ms by default.

**The latency store.** Release 1 computes limits from seeds only. From Release 2 the store keeps two rolling series per deployment: round trips, which drive the stall timeout, and end of speech to final, which is reported. An estimate is Seed below 30 fresh samples, Provisional from 30, Measured from 300. Each deployment's record is published with its capability record (every 30 s, chapter 3), with a class: realtime (P99 at or under 600 ms), fast (1,200 ms) or slow.

**Worked example.** ElevenLabs `scribe_v2`: seed P99 2,010 ms, measured with a 200 ms pause, so round trip 1,810 ms and stall timeout 2,512 ms. Times from end of speech.

| Time | Event |
| --- | --- |
| -2,976 ms | Detector confirms speech, 224 ms into a 3.2 s utterance. Pool depth checked, topped up if short |
| 0 | End of speech |
| +224 ms | Cut, hand-over, request written, `speech_end` sent. Deadline fixed at +6,224 ms (`final_deadline_ms`, 6,000, after the cut). End-of-turn decision starts |
| about +275 ms | Audio end-of-turn model says "finished". The turn waits only for text |
| about +800 to +1,400 ms | Normal case (estimated median): response parsed, the turn's one final result emitted, language-model request 12 ms later |
| +2,736 ms | Slow case: stall timeout. A second request starts on another connection |
| about +3,300 to +3,900 ms | The second answers. The first is cancelled and its connection replaced |
| +6,224 ms | Stuck case: both cancelled, the unit resolves as `timed_out`, the turn closes with whatever text it has, the client gets the warning `stt_segment_failed` |

**The turn, not the segment.** A caller waits for the later of the transcript and the end-of-turn decision. The engine decides in three steps: an audio model at the cut; if that says "not finished", a text model when the transcript is back; otherwise the *silence ceiling*, 1,500 ms by default, scaled by an agent's eagerness. An agent's explicit `max_endpointing_ms` replaces it, clamped to 800 to 3,000 ms; until bud-auth makes that field optional, exactly 3,000, its published default, counts as not set. With that ceiling a wrong "not finished" cannot by itself push a per-pause turn past 2,500 ms. On a per-turn row the upload waits for the decision, so a wrong "not finished" costs the ceiling plus a round trip: about 3,300 ms at a round-trip P99 of 1,800 ms.

**Masking the wait.** A *filler* is a short holding phrase. From Release 2 turn-taking speaks one before a late transcript, at `end of speech + max(configured, min(2,000 ms, expected median + configured))`; this part supplies the rule and the expected median.

**Telling the customer.** At session start `ready.stt` carries the latency class, the expected latency (seed P99 or measured P95, labelled), its basis (seed, provisional or measured) and any streaming alternative at the same provider. A slow class adds a warning; the session is still served. Before Release 3 only sessions the rollout switch covers get `ready.stt`; from then on every session does. Codes are quoted from chapter 5's table. Bud labels a deployment "slower on calls" from the published record.

**Later tiers.** Release 4 adds the *commit transport* (audio streams to a vendor socket and the gateway's detector sends the vendor's "utterance ends here" message; the only way to reach OpenAI's and Azure OpenAI's live-only models on a call; the attempt loop sees it as an ordinary `SegmentTranscriber`) and the *low-latency tier*, the deployment setting `latency_tier` (`standard` by default, or `low_latency`). On `low_latency`, a session with `transcription_mode` `auto` prefers a usable commit transport to file upload when its row has both, keeping upload as the fallback: the route to `gpt-transcribe` on OpenAI's socket. The tier also turns on the *hedge* (the second request sent early, at the round-trip P95, on at most 5 percent of requests). Release 6 adds interim text for self-hosted models by re-decoding a growing window.

**The gate.** Part A runs in continuous integration on every build: no wait before the upload or after the response, a second request at the stall timeout, release at the deadline, no cold upload in four scripted scenarios. Part B is a live benchmark that drives the real engine at real-time pace: 3,000 turn-final segments per vendor, passing with at most 20 above 2,500 ms. For the vendors of Release 1 it is an exit condition of Release 2 (per-turn rows exempt and labelled): a vendor that fails stays behind the allow-list until it passes or product accepts its "slow" label. For later vendors it only sets the label.

| Key number | Value | Source |
| --- | --- | --- |
| P99, end of speech to final | AssemblyAI synchronous 650 ms; Groq 1,540 ms; ElevenLabs `scribe_v2` and OpenAI `gpt-4o-mini-transcribe` (file) 2,010 ms. `gpt-transcribe`: none published; it starts unmeasured at the default of 2,000 ms | Published, Pipecat, 2026-02-09, 200 ms pause. No file median is published |
| Median for hosted file models, turn judged finished | 800 to 1,400 ms | Estimated |
| Cold connection | 100 to 440 ms | Measured 2026-10-03, one machine, four vendor hosts |
| Deadline; silence ceiling | 6,000 ms; 1,500 ms (the streaming path's, `core/voice_manager/config.rs:39`) | Integration decisions |
| Stall margins, queue allowance | 1.25 and 250 ms; 1,500 ms | Estimated |
| "At most 20 of 3,000" | 96.5 percent confidence that P99 is within 2,500 ms; a vendor twice as good as required passes 92 percent of the time | Computed, binomial |
| Hedge gain | About 160 ms off P99 | Computed from an assumed distribution |
| Commit socket | Median 640 ms, P99 1,660 ms, on `gpt-4o-transcribe` | Published, Pipecat |
| After the transcript | 490 ms at industry targets; 1,050 ms at this gateway's June 2026 measurements | Published, Twilio; Measured |

In short, the standard tier removes waste and bounds the tail but does not make a hosted file model conversational. With the caller judged finished, a typical turn takes about 1.9 to 2.5 s from end of speech to the agent's first audio (final at 0.8 to 1.4 s, plus 1,050 ms); otherwise the final waits for the silence ceiling, longer on a per-turn row. Only a streaming model, the commit socket or a co-located self-hosted model does better.

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| How a unit is timed | A measured stall timeout that adds a second request, and a constant deadline fixed per unit at hand-over | One adaptive deadline threw away words the caller was still adding to, and fed on its own timeouts | A unit whose two requests are both silent after 6 s is lost even if the caller is still talking. A truly stuck vendor costs up to 6.2 s. Under 1 percent more requests |
| Who computes limits | One function here; the engine and transcriber layer compute none | Four designs had four deadlines | A wrong seed mistimes the second request for the first few hundred units |
| What a timeout records | A rank-only entry | An invented sample doubled the limit for 30 minutes | The far tail has no number |
| Warming | By pool depth, HTTP/1.1 by default, HTTP/2 per host after measurement | "Host used recently" says nothing about how many connections are alive | Small extra requests; whether vendors count them against rate limits is unverified |
| The 2.5 s criterion | As under "The gate"; a slow vendor is warned, not refused | The gateway controls only its own waiting | 3,000 samples per vendor; a gate that can fail |
| Commit transport | Refuse OpenAI's live-only models now on every session that builds a voice manager (`stt_live_unsupported`, reason `client_not_implemented`); ship the transport in Release 4 | Vendor documentation conflicts; probes needed | Those models are unusable on calls until Release 4 |
| The store | In memory per replica, seeded, published per deployment | A static number is wrong for a self-hosted model; Redis per segment is a turn-path dependency | Replicas differ; 300 samples before "measured" |
| Format | 16 kHz WAV for every vendor | Fastest documented format | One extra network flight on 8 kHz telephone audio |
| Who picks a tier | A deployment setting (`latency_tier`), not a session field; it selects the hedge and `gpt-transcribe`'s socket | Hedges spend the deployment owner's money; one customer knob is the contract | End customers cannot choose their own latency tier |

## What changes in the code

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `core/stt/segment_limits.rs` | `segment_deadlines`, `SegmentDeadlines` | Only producer of time limits; a leaf file the transcriber layer can import | New | 1 |
| `core/stt/segmented/latency_store.rs` | `LatencyStore` trait, keys, seeds | Seed-only store; record calls feed histograms | New | 1 |
| same | `SttLatencyStore` | Rolling series, three states, rank-only entries, snapshot | Modified | 2 |
| `core/stt/segmented/runtime_config.rs` | `SttSegmentedConfig::try_from_env` | Settings with prefix `WAAV_STT_SEGMENT_`, read as the profiler's are (`core/state.rs:162-165`) | New | 1 |
| `core/metrics/bridge.rs` | Histograms, counters | End of speech to final (bucket at 2,500 ms; today's ladders jump 1,000, 2,000, 5,000 at `:119-121`), round trip, queue wait, second requests. Registered in Release 0 (chapter 7); fed from Release 1 | Modified | 0, 1, 2 |
| `core/stt/segmented/latency_class.rs` | `SttLatencyRecord` | Class with hysteresis; publishing; adoption by a replica with no samples in Release 3 | New | 2, 3 |
| `core/observability/observer.rs`, `async_observer.rs`, `turn_profile.rs` | Two defaulted hooks: speech ended, filler spoken | Profiler measures a segmented turn from end of speech; today's headline starts at the transcript (`turn_profile.rs:212-219`) | Modified | 2 |
| `core/stt/segmented/host_tuning.rs`, `handlers/debug_profile.rs` | Kernel check; snapshot | Warn when idle TCP windows reset; expose the store | New, modified | 2 |
| `core/stt/segmented/latency_bench.rs`, `tests/segmented_stt_latency_probe.rs` | Benchmark driver, mock twin, live runner | The release gate's measurement | New | 2 |
| `core/stt/segmented/commit/` | `LiveAudioTap`, `CommitTransport`, OpenAI and Cartesia transports | Gateway-driven commit; OpenAI's transport also serves Azure OpenAI | New | 4 |
| `core/stt/cartesia/client.rs`, `core/realtime/openai/messages.rs`, `handlers/openai_realtime/upstream.rs` | Raw `finalize` variant and event channel; transcription delta event; address function | Support for the transports; Cartesia's bytes unchanged while its flag is off | Modified | 4 |
| `core/stt/segmented/hedge.rs` | `HedgeRule`, `HedgeBudget` | Values and a predicate for the attempt loop | New | 4 |
| `core/stt/segmented/interim.rs` | `InterimRedecoder` | Interim text, self-hosted only, sent through `run_interim`, the attempt loop's second entry point | New | 6 |

This part adds no field to `CoreState` or `VoiceManagerConfig` and does not edit `create_stt_standard` or `create_stt_standard_prerecorded`.

## What this part gives to and needs from the other parts

**Gives.** To the engine (W1): the `LatencyStore` trait and the deadline from which it fixes each unit's instant and reports `final_deadline_ms` and `resolution_deadline_ms`. To the transcriber layer (W2): `SegmentDeadlines`, carried whole in its `SegmentRequest`; the warming rule; the client requirements. To the capability map and resolver (W3): the store as a member of `SttLiveShared`; `SttLatencyRecord` for the published record; what `latency_tier` selects. To turn-taking (W4): `MaskingBudget` and the filler rule. To the customer contract (W5): latency class, expected latency, basis and streaming alternatives for `ready.stt`; the live-only refusal's text. To cost, tests and rollout (W7): the queue allowance, the latency series and the benchmark result.

**Needs.** From W1: `SpeechActivity`, hand-over and timings in `SegmentOutcome`, the end-of-turn decision as an event, a count of units timed out while their turn was open. From W2: the one attempt loop, the shared clients, time to response headers, a per-request observer, and `run_interim` in Release 6. From W3: `ResolvedSttLive` with seed, pause, limits, upload policy, row identifier and latency tier, and the deployment deadline already raised; `SttLiveSession` with `EndpointTuning` (`cut_pause_ms`, `silence_ceiling_ms`); the publisher. From W4: the waiting-filler notice and the dispatcher that calls the speech-ended hook. From W7: the mock vendor server and the limiter gate. From people: working vendor keys and gateways near each vendor.

## Tests to write first

All but the last are lib tests without detector features, run by continuous integration (`.github/workflows/ci.yml:94`): unit and in-process chain tests on paused tokio time (the gateway's `test-util` feature, Release 0), real-socket tests on real time against W2's in-process mock vendor, not the `segmented_stt_ws` binary. Criteria are the brief's: 1 is ordered, complete segmentation; 4 is end of speech to final within 2.5 s; 5 is streaming sessions unchanged.

| Test name | Level | What it proves | Criterion | Release |
| --- | --- | --- | --- | --- |
| `a_live_only_openai_model_is_refused_on_every_voice_manager_session_with_the_date` | In-process chain | Every kind of voice-manager session on a live-only model is refused (`stt_live_unsupported`, reason `client_not_implemented`), naming `gpt-transcribe` and the transport's target date; push-to-talk on a model whose client returns text only at `audio_end` is warned (`stt_buffered_until_commit`), not refused | None of the five; the brief's plain-refusal rule | 0 |
| `stall_timeout_is_p99_times_margin_plus_constant_within_floor_and_cap` | Unit | The worked values | 4 | 1 |
| `an_unmeasured_row_starts_from_the_global_default` | Unit | `gpt-transcribe` does not borrow the 2,010 ms seed | 4 | 1 |
| `the_unit_deadline_is_the_newest_cut_at_hand_over_plus_the_deadline` | In-process chain | The fixed deadline | 4 | 1 |
| `a_held_unit_never_times_out_and_gets_its_whole_queue_allowance_at_hand_over` | In-process chain | Held audio is under no limit | 4 | 1 |
| `upload_starts_at_the_cut_without_waiting_for_the_turn_decision` | In-process chain | No added wait before the upload | 4 | 1 |
| `the_turn_final_is_emitted_at_the_response_instant` | In-process chain | No added wait after the response | 4 | 1 |
| `second_segment_does_not_wait_for_the_first` | In-process chain | Two uploads in flight | 4 | 1 |
| `a_stalled_request_gets_a_second_request_and_the_first_is_not_cancelled` | In-process chain | Slow is not lost | 4 | 1 |
| `a_stuck_vendor_releases_the_turn_at_the_newest_cut_plus_the_deadline` | In-process chain | The turn is bounded | 4 | 1 |
| `the_second_upload_slot_is_still_warm_after_a_long_run_of_single_uploads` | Mock vendor over a real socket | Depth, not recency | 4 | 1 |
| `a_cancelled_upload_is_followed_by_a_replacement_warm_up` | Mock vendor over a real socket | No cold upload after a cancel | 4 | 1 |
| `a_streaming_session_builds_no_segmented_machinery` | In-process chain | Streaming path and `stt_buffered_until_commit` sessions untouched (from Release 4, except Cartesia with its finalize flag on) | 5 | 1 |
| `stuck_segments_do_not_move_any_time_limit` | Unit | No feedback from timeouts | 4 | 2 |
| `limiter_wait_does_not_lengthen_any_time_limit` | Unit | Queue time is not vendor speed | 4 | 2 |
| `a_p99_statement_needs_three_thousand_samples_and_at_most_twenty_exceedances` | Unit | The gate's arithmetic | 4 | 2 |
| `harness_measures_a_known_injected_latency_within_five_milliseconds` | In-process chain | The measurement itself | 4 | 2 |
| `a_rejected_commit_does_not_shift_later_acknowledgements` | In-process chain | Commit text reaches the right segment | 1 | 4 |
| `tests/segmented_stt_latency_probe.rs` (ignored) | Live | Each Release 1 vendor's tail; a per-turn row only for its label | 4 | 2 |

## What ships in which release

- **Release 0, Groundwork and honest refusal.** The text of the `stt_live_unsupported` refusal for OpenAI's live-only models on every voice-manager session; this part's histogram ladders registered and counters on `/metrics` at zero; requirements on the mock vendor server; the HTTP/1.1 against HTTP/2 probe.
- **Release 1, First working calls.** `segment_deadlines` on seeded values; the fixed deadline; the second request at the stall timeout; the warming rule; 16 kHz WAV with the language sent whenever known; segment histograms; seed figures in `ready.stt`; Part A of the gate.
- **Release 2, Dark launch complete.** The measuring store; the published record; the benchmark with live cells and Part B of the gate, an exit condition of this release; the filler rule; profiler hooks; host check and operator guide.
- **Release 3, Default on.** Production objectives and alerts; record adoption; cells for AssemblyAI's synchronous endpoint and Deepgram hosted Whisper; measured seeds in the map.
- **Release 4, Live-only models and low latency.** Targets 2027-02-26, OpenAI's removal date for `whisper-1` and the `gpt-4o` transcribe models; missing it takes nothing from customers. The item that must ship before that date, OpenAI's code default moving to `gpt-transcribe`, is outside this part (Release 1 or a standalone patch). The commit transport for OpenAI's and Azure OpenAI's live-only models (each after a live probe) and for Cartesia; `latency_tier` and the hedge. Cartesia's gateway-driven finalize deliberately changes streaming sessions (today's client marks every final as an utterance end, `core/stt/cartesia/messages.rs:108`); it is behind `WAAV_STT_CARTESIA_MANUAL_FINALIZE`, off until a live probe passes, with a release note and re-recorded golden recordings.
- **Release 5, Wider vendor coverage and hardening.** Latency probes that enable each later vendor; the movable deadline if its counter justifies it.
- **Release 6, Interruption recovery and interim text.** Interim text for self-hosted models.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| Hosted file medians are worse than the estimate | Medium | Turns feel slow | Measure in Release 2; "slow" label and warning; name the streaming alternative; filler |
| A Release 1 vendor fails the 2.5 s gate | Medium | Kept out of the Release 3 default | Not refused: labelled slow and warned, as under "The gate" |
| The audio end-of-turn model often says "not finished" | High; its scores sit near its threshold | 0.5 s extra on per-pause rows; about 3.3 s turns on per-turn rows | Text model step and 1,500 ms ceiling; reported per step; no caller-turn objective on per-turn rows |
| No working keys or nearby gateway for the benchmark | Medium; the ElevenLabs key was dead at last check | Gate cannot be evaluated | That vendor stays behind the allow-list |
| Warm-ups count against vendor rate limits | Unknown | Uploads throttled | None sent when the pool is warm; per-vendor off switch |
| A unit is lost at 6 s while the caller still talks | Low | Missing words in a long turn | Counted from Release 1; movable deadline if it matters |
| Second requests add load during a vendor incident | Medium | Incident worsens | Retry budget; no hedge after a rate-limit response or with the breaker open |
| Release 4 misses its 2027-02-26 target | Medium | No OpenAI model with text during speech on calls; `gpt-transcribe` upload unaffected | Build the commit transport alongside Releases 2 and 3 (it needs only Release 1, its audio tap and the probes); cut the hedge first |

## What is still open

**Measurements needed.**

1. Median and 95th percentile, end of speech to final, for `gpt-transcribe`, `scribe_v2` and Groq (unpublished).
2. How often each end-of-turn step closes a turn, and the false "not finished" rate.
3. HTTP/1.1 against HTTP/2 per vendor host, including the cost of a cancelled request.
4. The 3,000-sample tail of each Release 1 vendor from a nearby gateway, except rows that upload once per turn.
5. Latency against audio length, and per-turn against per-pause uploads.
6. How often a unit times out while its turn is still open; how often the second request wins.
7. Whether vendors bill cancelled requests; telephone audio at 8 against 16 kHz.
8. Before Release 4: the OpenAI transcription socket and Cartesia `finalize` protocols.

**Choices that need a person.**

1. The silence ceiling. Default: 1,500 ms, though the agent contract publishes 3,000 ms (read as not set).
2. Reading the 2.5 s criterion as a gate for Release 1 vendors and a label elsewhere, never a refusal; and, when a vendor fails, whether product accepts its "slow" label.
3. Class thresholds of 600 and 1,200 ms for the "slower on calls" label. Default: as stated.
4. Who chooses the low-latency tier and pays for hedges. Default: the deployment owner.
5. A filler on slow turns, bounded at 2 s, including after one-word answers. Default: yes.
6. Whether the kernel setting `tcp_slow_start_after_idle=0` may be applied. Default: no; the gateway warns.
7. Whether to build gateway-side interim decoding. Default: wait for WaaV Infer's incremental session.

## Where the detail is

- Design: `/home/bud/ditto/waav/research/segmented-stt/design/W6-realtime-performance.md`.
- Critiques, same folder: `critique-W6-realtime-performance-code.md` and `critique-W6-realtime-performance-adversarial.md`.
