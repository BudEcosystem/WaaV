# Chapter 7. Cost, limits, resilience, observability, tests and rollout

Paths are relative to `/home/bud/ditto/waav/WaaV/gateway/src` unless they start with `gateway/`, `bud-auth/` or `.github/` (then relative to `/home/bud/ditto/waav/WaaV`), with `capability-map/` (then in this chapter's folder, the authoritative map) or with `verify/`, `providers/` or `external/` (then under `/home/bud/ditto/waav/research/segmented-stt`). Vendor prices and limits come from fact-checked evidence (`verify/group-1.md`, `providers/elevenlabs.md`, `capability-map/rows/groq.json:64`).

## What this part is and why it exists

A *segmented session* is a live call whose speech-to-text model cannot take a live audio stream, so the gateway cuts the caller's audio into utterances and uploads each one to the vendor as a file. One uploaded piece is an *upload unit*; a *caller turn* is everything the caller says before the gateway decides they have finished. This part decides what such a session is charged, how uploads stay inside vendor rate limits, what is recorded when an upload is lost, which numbers show it works, which tests come first, and the order in which everything ships.

Without it, four things go wrong. The meter would bill every streamed byte, silence included (`handlers/ws/audio_handler.rs:127-129`, `handlers/ws/bud_legs.rs:482-496`), though only speech is uploaded. Two hundred calls at fifteen uploads a minute make 3,000 requests a minute, far above Groq's 400, so every call would lose turns together. Nothing measures end of speech to transcript, so nobody could say whether a model is usable on a call. And a rollback would need a restart, ending every live call within the 30-second drain (`main.rs:535-538`).

## How it works

Work on one upload unit is never done twice. The engine cuts the audio and calls the upload component once; the transcriber layer's *attempt loop* is the only code that sends requests, at most two per unit per vendor. This part supplies three pieces around that loop.

| Piece | What it does | Key numbers and their source |
| --- | --- | --- |
| Limiter, the loop's *gate*: every request takes one pass | One limiter per vendor host, credential and model, the scope of a vendor's limit. A token bucket and a concurrency cap at 80% of the capability row's ceiling. A *reserve* only the upload a closed turn is waiting for (the *turn-final* upload) may use. A rate-limit answer halves the rate once per event; recovery is 5% of the ceiling per 10 s | 80%, the reserve (2 s of turn-final demand) and the recovery step are estimates. Vendor ceilings: Groq 400 requests a minute, OpenAI tier 1 500, ElevenLabs 12 to 60 concurrent by plan |
| Merge rule | An upload at a pause (*speculative*) starts only if it can start at once; otherwise the audio is held and joined into one upload when the turn closes. A row may fix one upload per turn | Azure OpenAI's default quota is 3 requests a minute (`capability-map/rows/azure_openai.json:114`), so its rows upload per turn |
| Metering and telemetry | One analytics row per upload unit: *uploaded seconds* (audio in the answered request) set the cost; *billed seconds* (the estimate of the vendor's charge after its minimum) are recorded beside it, on failed rows too | Groq bills at least 10 s per request |

Real-time performance owns the time limits: a queue allowance of 1,500 ms from hand-over, a stall timeout (2,512 ms for ElevenLabs `scribe_v2`, from its published 2.01 s 99th percentile), and a deadline (`final_deadline_ms`) of 6,000 ms from the turn's newest cut, excluding the 224 ms *cut pause* (silence before the cut).

**Worked example: the vendor hangs** (ElevenLabs `scribe_v2`). Times in ms from the *end-of-speech anchor*, the gateway's stamp for the end of speech: the arrival of the audio chunk holding the caller's last voiced sample.

| t | Event |
| --- | --- |
| 0 | Anchor, stamped where audio enters the engine |
| 224 | After the cut pause the detector declares the stop; the unit is cut and handed over; deadline fixed at 6,224 (6,000 after the cut). Row opened; immediate gate pass; request 1 starts |
| 2,736 | Stall timeout (224 + 2,512). The gate grants a second pass; request 2 starts, request 1 keeps running |
| 6,224 | Deadline. Both dropped. Row: class `vendor_timeout`, no charged seconds, no cost, billed estimate for two requests. Latency histogram observed at 6,224 as `lost`. Client gets the warning `stt_segment_failed`; the agent speaks its degradation message |

Had the key been at its limit, the unit would wait at the gate at most 1,500 ms, then resolve as lost with class `rate_limited` and no vendor call; three such losses in 30 s mark the key overloaded and new sessions are warned, later refused with `stt_overloaded`.

**Capacity and cost at 200 calls** (computed; assumes 5 caller turns a minute of 3 segments, 2.0 s of speech and 3.0 s uploaded each, from `external/cost-limits-resilience.md` section 2.2):

| | One upload per pause | One upload per turn |
| --- | --- | --- |
| Requests a minute | 3,000 | 1,000 |
| Calls one Groq key carries (80% of 400) | 21 | 64 |
| Calls one OpenAI tier 1 key carries (80% of 500) | 26 | 80 |
| Groq's bill per call-hour ($0.04 per audio-hour, 10 s minimum) | $0.100 | $0.033 |
| Charged to the customer at uploaded seconds, Groq | $0.030 | $0.026 |

Between 21 and 64 calls on one Groq key the average falls smoothly from three uploads a turn to one, losing no turn. Past 64 the oldest sessions keep working and the newest visibly lose turns.

**What is recorded.** Per upload unit: one analytics row (a root `voice.turn` trace span) and about fifteen Prometheus updates, labelled by provider, capability-row identifier and transport, never by session, credential or host (`core/metrics/bridge.rs:78-81` forbids unbounded labels). A streaming session writes none of it.

**Rollout and rollback.** The *rollout switch* `WAAV_SEGMENTED_STT` (`off`, `allowlist`, `on`) is read at start-up. A *control record*, one Redis key in the control plane of Bud (the platform that publishes deployments and prices), lists disabled deployments and capability rows (a *capability row* is the map's entry for one provider and model); read at session start, it narrows the switch without touching a live call. A session the engine does not cover gets one rule in the capability-map resolver (integration decisions A1, B2, B4). A voice agent with automatic turn detection on a model whose only client buffers audio until hang-up (OpenAI, Groq and four regional clients: Bhashini, FPT.AI, NAVER CLOVA `csr`, NECTEC `partii4`) is refused at setup with `stt_live_unsupported`: on `/v1/realtime` nothing can end its turns; on `/ws` the client's `audio_end` can, so there the refusal is a choice awaiting sign-off. Other sessions on such a model keep today's behaviour with the warning `stt_buffered_until_commit`. A *plain* session (`/ws`, no voice agent, no conversation loop) keeps today's client and the warning even where the switch covers it, unless it sets `transcription_mode: segmented` (from Release 2). Viettel (its client calls a retired domain) and NECTEC ids other than `partii4` are refused on every session (`client_not_implemented`), as is an OpenAI live-only model until Release 4.

**Indicators.** Transcript latency: share of turn-final uploads released within 2,500 ms, lost ones counted as misses. Segment success 99.5%. Lost turns at most 0.1%. Limiter queue wait, 95th percentile at most 100 ms. The first is measured, not asserted in CI: a 99th-percentile claim needs 3,000 samples with at most 20 misses.

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| What a segment costs | Uploaded seconds, on success only; the vendor-billed estimate beside it | One price owner (Bud); same rule as the file route; `core/voice_cost.rs:3-5` models no vendor minimum | Understates Groq's invoice 3.3 times per pause; a talkative caller is charged up to about 1.5 times the streamed seconds (padding per upload) |
| Analytics granularity | One root row per upload unit | Matches vendor request ids; same order of volume as today | Seven new columns, which Bud's registry must add first |
| Where the retry lives | In the transcriber layer's loop; this part is only its gate | Four drafts each had a retry: up to eight requests per utterance | This part gave up its own loop and deadline |
| Limiter scope | Process-local, reacts to refusals | The customer's vendor tier is unknown; a shared count of a guess buys nothing | Each replica backs off alone |
| When uploads start | At each pause while the key has headroom, else hold and join | Request count, not price, is the binding limit | Under pressure a turn waits for the turn decision before uploading |
| Overload | Warn on an estimate; refuse new sessions on measured overload | Refusing on a guess is worse than warning; lost uploads are evidence | A short lag; some callers are refused |
| Rollback | A control record in Bud's Redis and a per-deployment setting, read at session start | One vendor row can be disabled with no restart | One more key; a standalone gateway has only the process switch |
| Uncovered sessions | The resolver's rule above | On `/v1/realtime` such an agent stays silent; a plain client ends its own turns | This part's refuse-every-agent rule was withdrawn; a `/ws` agent sending `audio_end` is refused by choice |
| The 2.5 s criterion | An exit condition of Release 2 for Release 1 vendors; a label elsewhere | Vendors publish no median; CI runners are noisy | A failing vendor waits behind the allow-list |
| Hang-up during an upload | Dropped at once; an upload the vendor answered is charged, one in flight is not but its sent seconds and billed estimate are recorded | The transcript has no reader | One possibly billed request per hang-up |

## What changes in the code

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `core/stt/segmented/testkit.rs`, `clock.rs`; `gateway/Cargo.toml:45` | Scripted transcriber, detector, endpointer; test clock; `test-util` feature | Fakes and a clock a paused test can drive; the feature turns on tokio's test utilities (absent from `full`, `:138`) in development builds | new; modified | 0 |
| `gateway/tests/mock_providers/` | File-transcription vendor, WAV inspector | Request log, delays, enforced rate | modified | 0 |
| `gateway/Dockerfile:144-155`, `init.rs:36-58` | `init` stage, `run` | Fetch the Silero detector model at build time; today it downloads on first use (`Dockerfile:16-18`) | modified | 0 |
| `core/stt/openai/client.rs:902-903`; `core/stt/groq/client.rs:1085-1123` | `disconnect` | If a confirming test fails on that client: keep the callbacks across the finalize reconnect; Groq also logs a failed flush, clears its buffer and returns success like OpenAI, so `finalize_stt` stops skipping the reconnect (`core/voice_manager/manager.rs:1655-1668`) | modified | 0 |
| `core/metrics/bridge.rs:188-198`, `:248-256` | Bucket registration, `prime_series` | New series registered and primed at zero; emitters later | modified | 0, 1 |
| `core/stt/segmented/limiter.rs` | `KeyLimiter`, `SessionGate`, `LimiterRegistry` | Bucket, reserve, queue, back-off; long-window budgets and overload flag later | new | 1, 2 |
| `core/stt/segmented/upload.rs` | `LiveUpload` | Thin adapter: opens the row, calls the loop once | new | 1 |
| `core/stt/segmented/billing.rs` | `UploadRecord`, `billed_seconds_for_request` | Row contents as a pure function | new | 1 |
| `handlers/ws/bud_legs.rs:455-457`, `:482-496`, `:540-542` | `LegMeter` | A segmented mode: `stt_final` and `finish` write nothing; `stt_upload_open` writes one row per unit, with a `Drop` that records a failed state | modified | 1 |
| `handlers/ws/config_handler.rs:1804-1810`, `:1947` | `initialize_voice_manager` | Set the meter mode and hand the telemetry object over as the metering sink, before `VoiceManager::new` (`:1988`) | modified | 1 |
| `handlers/ws/config_handler.rs:425-437` | observer install | Skip the per-transcript billing observer in segmented mode | modified | 1 |
| `handlers/ws/segment_telemetry.rs` | `GatewayTelemetry` | Meter, metrics, this part's notices | new | 1 |
| `gateway/tests/segmented_stt_ws.rs`, `segmented_upload_span.rs`; `.github/workflows/ci.yml:282` | integration binaries | Real-socket and span tests on the CI line, built with `test-util`; real-model tests in a binary the accuracy job runs (`:269`) | new | 0, 1 |
| `bud-auth/src/runtime.rs:150-171` (the pattern) | control-record loader | Re-read on change, keep the last good value | modified | 2 |
| `observability/voice_attrs.rs` | `ALL`, span macro | Seven attributes, after Bud's registry change | modified | 3 |
| OpenAI and Groq clients | `FlushStrategy::OnSilence` | Remove unreachable code | modified | 3 |
| ElevenLabs and Deepgram URL builders | retention parameter | Canonical `data_retention`; default bytes unchanged | modified | 5 |

## What this part gives to and needs from the other parts

**Gives.** The limiter as the attempt loop's `AttemptGate`; `try_speculative` and the merge rules the engine applies; the metering sink that reads every `SegmentOutcome`; the cause group of a lost unit (`vendor_fault`, `capacity`, `auth`, `gateway`) that turn-taking uses so a capacity loss starts no language-model turn; the test kit; the release gates.

**Needs.** From the capability map: `SttLiveShared` (limiter registry, retry budgets), `SttLiveSession` (metering sink, outcome-sink slot) and `SegmentedStt::new_live(shared, session)`, which assembles the upload adapter, so the handler builds no transcriber; `ResolvedSttLive` limits and billing facts; a stable row identifier (metric label, allow-list, control record); the refusal rule with its session-kind and manual-mode inputs; the overload code `stt_overloaded`. From the engine: one `SegmentOutcome` per upload unit and the arrival stamp taken in `send_audio`, carried on `SpeechActivity`. From the transcriber layer: the loop, its ledger of what reached the vendor, a "never retry" policy, and vendor-reported usage on `SegmentTranscript` for token-priced models. From real-time performance: `SegmentDeadlines` and the latency histograms. From turn-taking: the dispatcher delivering each outcome to turn-taking, the wire and metering. From the customer contract: the codes `stt_segment_failed`, `stt_rate_limited`, `stt_capacity_low` and `stt_buffered_until_commit`, quoted from its code table.

## Tests to write first

Acceptance tests are lib tests needing no cargo feature but `test-util`, because CI runs lib tests in five feature sets and only six named integration binaries (`.github/workflows/ci.yml:76-81`, `:282`). Tests loading real Silero or SmartTurn model files (such as the SmartTurn confirming test) go in an integration binary in the accuracy job, the only one provisioning models (`:248-269`).

| Test name | Level | What it proves | Acceptance criterion | Release |
| --- | --- | --- | --- | --- |
| `a_session_on_a_buffering_model_gets_a_transcript_at_every_audio_end` | unit | Whether a second `audio_end` loses the OpenAI and Groq clients' callbacks (`core/stt/openai/client.rs:902-903`, `core/stt/groq/client.rs:1114-1115`); runs on both before the refusal merges | Honest refusal | 0 |
| `a_failed_upload_at_audio_end_does_not_reject_the_next_turn` | mock vendor over a real socket | Groq's first upload fails: no frame of the next turn draws an error, and the second `audio_end` yields a transcript of that turn's audio only | Honest refusal | 0 |
| `a_voice_agent_with_automatic_turn_detection_on_a_buffering_model_is_refused_over_a_real_socket` | mock vendor over a real socket | Refused with `stt_live_unsupported`; in a companion test a conversation-loop session on the same model is only warned (`stt_buffered_until_commit`) | Honest refusal | 0 |
| `an_elevenlabs_deployment_with_no_model_is_still_streamed` | unit | The refusal never catches a model that streams today | Streaming unchanged | 0 |
| `streaming_ws_messages_match_the_golden_recording_under_every_switch_value` | mock vendor over a real socket | Byte for byte, except `ready`'s additive `stt` key (covered sessions; all from Release 3) and three flagged changes: greeting fix and speech events (Release 2), Cartesia finalize (Release 4, re-recorded) | Streaming unchanged | 0, 1 |
| `the_production_image_carries_the_detector_model_files` | live image check in CI (no vendor, no key) | The image loads the detector with no network | Refusals are early and named | 0 |
| `a_segmented_session_bills_uploaded_seconds_not_streamed_seconds` | unit | 10 s streamed, 3.0 s uploaded: cost from 3.0 | Charged for what was uploaded | 1 |
| `a_failed_upload_record_has_an_error_class_a_billed_estimate_and_no_charged_seconds_or_cost` | unit | A failure never reads as a success | Same | 1 |
| `a_failing_vendor_receives_at_most_two_requests_per_segment` | mock vendor over a real socket | Requests counted on the wire, not on a fake | Ordering and one retry | 1 |
| `a_turn_final_request_never_waits_at_the_gate_past_the_queue_allowance` | unit | Refused 1,500 ms after hand-over, also for a held unit | Limits do not stall turns | 1 |
| `sixty_sessions_on_a_400_per_minute_key_lose_no_turns` | unit, paused clock | Smooth degradation to one upload per turn | Limits do not become lost turns | 1 |
| `two_hundred_sessions_on_a_400_per_minute_key_keep_the_admitted_sessions_working` | unit, paused clock | Oldest sessions lose nothing; new ones are refused | Same | 2 |
| `the_end_of_speech_anchor_is_the_arrival_time_of_the_chunk_with_the_last_voiced_sample` | unit | The gateway's own lag is not subtracted | End of speech to final | 1 |
| `ws_segmented_session_yields_a_transcript_and_a_reply` | mock vendor over a real socket | Whole chain on `/ws` | Live call yields transcript and reply | 1 |
| `realtime_cascade_segmented_session_yields_a_transcript_and_a_reply` | mock vendor over a real socket | Same on `/v1/realtime`; needs a Bud-mode fixture, since `test_support` is `cfg(test)` (`lib.rs:32-33`) | Same | 2 |
| `a_row_disabled_in_the_control_record_refuses_new_sessions_and_leaves_live_ones_alone` | unit | Rollback without restart | Rollout safety | 2 |
| `live_segmented_eos_to_final_distribution` | live, by hand | 3,000 turn-final utterances per vendor; at most 20 above 2,500 ms; once-per-turn rows exempt | 2.5 s criterion | 2 (exit) |

## What ships in which release

- **Release 0, groundwork and honest refusal.** The test kit and `test-util` feature, mock vendor and golden recordings of streaming sessions and of OpenAI and Groq push-to-talk sessions (Release 1 moves code out of those clients), taken after any conditional fix; the two confirming tests and the failed-flush test; metric series registered at zero; the detector model baked into the image. The resolver refuses sessions the map records as unable to work today, plus `/ws` voice agents with automatic turns on a buffering model, by choice; its withdrawal switch `WAAV_STT_FILE_ONLY_REFUSAL` (chapter 3), set to `off`, lifts those refusals and keeps the warning.
- **Release 1, first working calls** (allow-listed deployments). Entry: every replica reports the detector model loaded. The limiter with rate, concurrency, reserve and back-off; both upload policies; the upload adapter; uploaded-seconds metering on existing attributes; metric emitters; the rate-limited notice. Vendors: OpenAI file models, Groq, ElevenLabs `scribe_v2` and `scribe_v2_medical`, self-hosted OpenAI-compatible servers, WaaV Infer, Azure OpenAI. Conversation-loop and `/v1/realtime` calls also work, speech events still timed by the transcript. Dated item (addendum B6), due before 2027-02-26 and dependent on no later release: an empty OpenAI model becomes `gpt-transcribe` on every path, the REST file route included, sent `languages[]` and `keywords[]` instead of `language` and `prompt`; it may ship alone once that default is signed off. Exit: live `/ws` voice-agent calls on ElevenLabs `scribe_v2` and OpenAI `gpt-transcribe`; streaming sessions byte-identical to the golden recordings.
- **Release 2, dark launch complete.** Hourly and daily budgets; admission in warn mode, with the Azure OpenAI capacity warning `stt_capacity_low`; the control record; the session language vote (agree a language over the first segments, then pin it), moved up from Release 5; the `transcription_mode` wire field; the cascade's socket test; billing probes; one live SIP call; and the 2.5 s gate on 3,000 samples for the Release 1 vendors, both as exit conditions.
- **Release 3, default on.** Switch default `on` (a Release 1 vendor only once it passed the 2.5 s gate or its "slow" label was accepted; plain sessions on buffering models keep today's client unless they opt in); every session's `ready` gains the `stt` key, with a release note; refusal on measured overload and of rows whose behaviour today is unknown (Rev AI `human`, Sarvam `saaras:v2.5`, Yandex `deferred*`); seven analytics columns; dead flush code removed; second vendor wave (AssemblyAI synchronous endpoint, Deepgram hosted Whisper). Entry: a week of staging traffic at 99.5% segment success.
- **Release 4, live-only models and low latency.** The commit transport (the gateway's detector tells a vendor socket where an utterance ends) for OpenAI `gpt-live-transcribe`, `gpt-realtime-whisper` and `gpt-transcribe`, the two live-only models on Azure OpenAI (after a live probe) and Cartesia's manual finalize; a deployment's `latency_tier: low_latency` prefers it to a file transport. Behind `WAAV_STT_CARTESIA_MANUAL_FINALIZE` (off until a live probe passes), covered Cartesia sessions leave today's client: the third deliberate change to streaming sessions, with a release note and re-recorded goldens. The gate's rule for hedged requests (at most 5%, never on a vendor with a per-request minimum). Targets 2027-02-26; missing it takes nothing from customers.
- **Release 5, wider vendor coverage and hardening.** Fallback to a second vendor; retention and region options; the vendor minimum in the price; third vendor wave, each row enabled only after a live probe (Gnani, Alibaba, Tencent and Phonexia need new request code; Phonexia only on customer demand).
- **Release 6, interruption recovery and interim text.** Nothing new here: the lost-overlap outcome it reads exists from Release 1. The agent resumes after a false interruption on LiveKit and on `/ws` clients that declare support, not on `/v1/realtime`.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| A vendor limit binds at scale (one Groq key carries about 64 calls) | High | Lost turns | Reserve, joining, oldest-session-first, measured-overload admission; two load tests gate Release 3; fallback in Release 5 |
| The cost shown differs from the vendor invoice | High | Low to medium | Billed estimate on every row; billing probe before Release 3; price minimum in Release 5 |
| A streaming session changes | Low | Very high | Golden recordings; one test per touched code point; each deliberate change flagged, with a release note |
| The refusal blocks a `/ws` voice agent that sends `audio_end` and gets replies today | Low | High | A choice pending sign-off; plain sessions keep today's client; withdrawal switch |
| A `/v1/realtime` agent admitted in manual mode turns turn detection on mid-call on a buffering model (`handlers/openai_realtime/cascade.rs:619-625`, `:1399-1403`) | Low | A silent call | The change is rejected with `stt_live_unsupported`; the session stays in manual mode |
| The detector model is missing on a cluster with no outbound access | Medium | Every segmented session refused | Model baked into the image; CI check; entry condition of Release 1 |
| A rollback ends live calls | Medium | High | Control record first; process switch last |
| The OpenAI default change misses 2027-02-26 | Medium | Every OpenAI request naming no model fails, on the REST file route and uncovered live sessions | Release 1 or a standalone patch; needs no later release |
| Release 4 misses its 2027-02-26 target | Low | No OpenAI live-only model on calls; `gpt-transcribe` uploads still work | The commit transport needs only Release 1 and the live-audio tap; the hedge needs Release 2's measurements and is cut first |

## What is still open

Measurements needed:
- End of speech to final transcript per Release 1 vendor, once-per-turn rows excepted (3,000 samples; needs working keys and a gateway near the vendor).
- A billing probe on Groq, OpenAI and ElevenLabs: rounding, minimum, whether an abandoned request is billed.
- Uploads per call-minute and mean request time on real calls, to replace the admission seeds.
- Groq's daily limits and what its hourly audio cap counts.
- What ElevenLabs returns to a non-enterprise key that asks for no retention.

Choices that need a person, with the recommended default:
- Refuse a `/ws` voice agent with automatic turns on a buffering model, whose client can end turns with `audio_end`? Default: refuse; withdrawal switch available.
- Cost basis. Default: uploaded seconds.
- Refuse a new session on measured overload? Default: yes from Release 3.
- Seven analytics columns and a thirteenth error class. Default: seven; cancelled stays `internal`.
- Fallback consent. Default: listing a fallback is consent; off until Release 5.
- Accept a "slow" label for a Release 1 vendor failing the 2.5 s gate? Default: no; it stays behind the allow-list.
- Vendor limit tiers for the keys Bud operates; 200 calls on Groq need at least 1,250 requests a minute. Default: declare the real ceilings on the deployment.

## Where the detail is

Design: `/home/bud/ditto/waav/research/segmented-stt/design/W7-cost-observability-tests-rollout.md` and its two critiques beside it; it predates the integration decisions' Addendum B, which this chapter follows.
