# Chapter 2. The per-utterance transcriber layer

Paths are under `/home/bud/ditto/waav/WaaV/gateway/src`. Other parts of the plan are named with their chapter: the segmenting engine (1), the capability map and resolver (3), turn-taking (4), the customer contract (5), real-time performance (6), and cost, observability and rollout (7).

## What this part is and why it exists

Some speech-to-text models accept only whole files. To use one on a live call, the gateway cuts the caller's audio into utterances and uploads each. This part is everything between "here is one utterance of 16 kHz audio" and "here is its transcript, or a failure with a named cause". It has one interface, `SegmentTranscriber`, implemented once per wire family (a group of vendors whose file endpoints take the same request shape), and one attempt loop that decides whether an utterance gets a second request. It also owns the HTTP clients for these uploads, their circuit breakers (a circuit breaker is a shared counter that stops calls to a vendor for a few seconds after a run of failures), and the filter that drops text a model invents on silence.

Without it, three things go wrong. Of the four places with file-transcription code, only the REST file route's self-hosted and Azure OpenAI functions (`handlers/transcribe.rs:769-1027`) can run two uploads at once, and this part reuses them. The OpenAI, Groq and prerecorded clients each need exclusive access to their client (`core/stt/base.rs:695`), wait 30, 120 or 300 seconds and open a connection per session (`core/stt/openai/client.rs:52-58`, `core/stt/groq/client.rs:80-88`, `core/stt/prerecorded.rs:72`), so one dead connection would freeze a caller's turn. And four first-draft designs each added a retry; stacked, a failing vendor would have received every utterance four to eight times, while the one existing breaker per vendor (shared with streaming calls and all tenants) counts a rate-limit answer as an outage (`core/stt/http_resilience.rs:136-141`).

## How it works

1. **Plan once per session.** `plan_segment_transcriber` reads the session's resolved capability row (the capability map is the table that says how each model is reached) and fixes the URL, credential header, request fields and shared state. It does no network I/O, because session construction is synchronous (`core/voice_manager/manager.rs:178-205`). The session's language is sent on every request when known. With none or `auto`, the plan step returns the `ready.stt` notice `stt_language_unset` (detection on short segments is unreliable; set a language), and a row that takes candidate languages (`gpt-transcribe` `languages[]`, AssemblyAI `language_codes`) gets the deployment's expected ones.
2. **One attempt is one request.** `transcribe(&self, audio, context)` sends one request and returns a transcript or an error carrying its own verdict: what the breaker should record, whether a second request makes sense, what to tell the rate limiter. It never retries.
3. **One loop decides.** `SegmentAttempts::run` is the only caller of `transcribe`. For each utterance it takes a breaker permit, waits at the limiter gate (the object that says when a request may be sent on a credential), and sends. A vendor gets at most two requests per utterance. The second is sent in four cases: a quick failure worth retrying (connection error, HTTP 500, 502, 503, 504, 408); a stall (no response headers in time: the first request keeps running, the second uses a fresh connection, the first answer wins); a request field the vendor refused (resent without it, remembered for ten minutes); an expired rotating token. A retry and a stall request each spend a token of a retry budget, so a vendor-wide slowdown cannot double the load.
4. **It computes no time limit.** Chapter 6 computes all five and they arrive with each utterance in one value, `SegmentDeadlines`.
5. **Upload breakers are separate** from streaming breakers, keyed by provider, host and capability row. They count one outcome per utterance, never count a rate-limit or credential answer, and open early when a host stops answering.
6. **Connections are kept warm** to the number of uploads that may be in flight, topped up at session start, when the caller starts speaking, and after any cancelled request.
7. **A quality verdict** is computed when the transcript is released in order.

| Time limit | Default | Counted from | Effect |
| --- | --- | --- | --- |
| Queue allowance | 1,500 ms | hand-over to this layer | the gate stops holding the first request |
| Stall timeout | about 2,500 ms for hosted vendors; never under 1,500 ms | first byte of the first request | one second request may start beside the first |
| Request limit | the row's value, at most 10,000 ms | each request's own start | that one request is cancelled |
| Room for a second request | the larger of 500 ms and the median round trip plus 100 ms | when a second request would start | with less room, none is sent |
| Deadline | 6,000 ms. A deployment may set 3,000 to 10,000; the resolver raises a value below the silence ceiling (the silence after which the gateway always ends a turn, 1,500 ms by default) plus 2,500 ms | the caller turn's newest cut, fixed at hand-over | the utterance is given up and the turn moves on |

**Worked example: a dead pooled connection.** Times in milliseconds from the caller's last speech sample, ElevenLabs `scribe_v2`.

```
    0  caller stops speaking
  224  the engine cuts the utterance and hands it over; the deadline (6,000 after the cut) is 6224
  224  breaker permit, gate pass, request 1 sent on a pooled connection; no answer
 2736  stall timeout (224 + 2512): retry budget token, gate pass, 3488 ms of room left;
       request 2 starts on a fresh connection; request 1 keeps running
 2936  handshake done, audio sent
 4136  request 2 answers (an assumed round trip of 1200): transcript; request 1 is dropped and
       its connection replaced; the breaker records one success for the utterance
```

Without the second request the utterance would have been lost at 6,224 ms.

**What one request looks like** (OpenAI `gpt-transcribe`; the capability row decides the fields):

```
POST https://api.openai.com/v1/audio/transcriptions      multipart, Authorization: Bearer <key>
  file=audio.wav (audio/wav, 16 kHz mono)   model=gpt-transcribe   response_format=json   languages[]=en
200  {"text": "I'd like to change my booking.", "usage": {...}}
```

Key numbers and where they come from:

- 2,512 ms stall timeout: computed by chapter 6 from a 99th-percentile time from end of speech to final text of 2.01 s, published by Pipecat (an open-source voice framework) for OpenAI and ElevenLabs file endpoints. It applies to ElevenLabs `scribe_v2` and OpenAI `gpt-4o-mini-transcribe`; OpenAI `gpt-transcribe` is unmeasured and starts from an estimated default.
- 224 ms from last speech to the cut: the engine's constant `cut_pause_ms`.
- 100 to 440 ms for a cold connection, one name lookup of 5.17 s: measured in the research probe. This is why clients are shared and warmed.
- 1.5 s connect timeout; retry budget of 10 % of requests plus a burst of 10, per provider and credential: estimated, the second from published retry-budget guidance.
- Upload client: 50 s idle and 2,000 ms warm-up timeouts, fixed by the integration decisions until vendor idle timeouts are measured.
- Breaker for uploads: opens at a 50 % failure rate over at least 10 of the last 50 utterances, sustained for 1 s; or after five stalled requests with no headers seen on the key; 5 s cooldown; 4,096 keys at most. Estimated first values.
- Published 99th percentiles for the other vendors (Pipecat): Groq 1.54 s, AssemblyAI synchronous 0.65 s. No vendor publishes a median.

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| Where the retry lives | One loop above a single-request `transcribe` | Stacked retries multiplied uploads and skipped the limiter | Other parts delete their own retries and reach vendors only through it |
| A stalled request | Send a second beside it; keep the first | A dead connection would otherwise cost the whole turn; cancelling loses the work if the vendor is only slow | Possibly a second billed request; capped by the retry budget |
| Rate-limit answers (HTTP 429) | Not counted by the breaker; reported to the limiter | Limits belong to a credential, the breaker is shared by tenants | A limiter must always be present; a minimal one is the floor |
| Breaker key and home | Provider, host and row identifier, in a separate bounded registry; a client-chosen host gets a breaker owned by its session | Separates uploads from streaming, hosts from each other, models on one host | Each row learns of a host outage by itself, in about 1 s at load |
| How the breaker counts | One outcome per utterance; sustained for 1 s; early opening on a silent host | At 50 uploads a second a 300 ms blip would open the default breaker for 5 s, about 250 lost utterances | First values, unmeasured |
| HTTP version | HTTP/1.1, with a per-host switch to HTTP/2 after measurement | HTTP/2 puts every upload on one connection with a 65,535-byte initial window | One connection per upload in flight |
| When to warm | By pool depth (chapter 6's rule), not by how recently the host was used | The pool reuses its newest connection and a cancelled request closes its connection | Small extra requests; whether vendors count them against limits is unknown |
| Which quality rules drop text | Only rules with measured support; the rest count and log | Dropping a genuine "okay" leaves an agent waiting for an answer it already got | Some invented "thank you" passes until the rules are measured |
| Existing file clients | Lift pure functions and wrap the self-hosted and Azure OpenAI request functions in Release 1; rewire the OpenAI and Groq clients in Release 3; change OpenAI's default separately | Rewiring changes the error classes of the REST route and needs sign-off; the default has its own date, 2027-02-26 | Two field builders coexist until Release 3, pinned equal by tests |
| Regional vendors | Wrap the existing one-request function where one exists (nine vendors); a small new request builder each for Gnani, Alibaba, Tencent and Phonexia | Reuse without copying. The four have only socket clients (`core/stt/gnani/grpc.rs:30`, `core/stt/alibaba_cloud/client.rs:39-40`, `core/stt/tencent/client.rs:41`, `core/stt/phonexia/client.rs:335`) | The nine keep the old breaker rules and private connections; the four are sized separately |
| Address safety | Plan without name lookups; the client's resolver checks the address at every new connection | A once-per-session check lets a name change its address mid-call | Needs chapter 3's client builder in Release 1 |

## What changes in the code

New files are under `core/stt/segment_transcriber/`. By the end of Release 1 this part has added thirteen files and changed twelve existing ones; later releases add one file per vendor family.

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `mod.rs`, `error.rs` | `SegmentTranscriber`, `SegmentAudio`, `SegmentRequest`, `SegmentTranscript`, `SegmentError`; then `plan_segment_transcriber` and the adapter registry | Interface and types first, with no HTTP | new | 0, 1 |
| `testing.rs` | `FakeTranscriber`, `ScriptedVendor` | Test kit: a scripted fake and a mock vendor on a loopback socket | new | 0 |
| `attempts.rs`, `gate.rs`, `ledger.rs` | `SegmentAttempts::run`, `AttemptGate`, `GateHint`, `PauseGate`, `RetryBudget`, `AttemptLedger`, `AttemptObserver` | The one loop, its gate seam, the record of what reached the vendor | new | 1 |
| `exchange.rs`, `http.rs` | `exchange`, `SegmentClients`, warming | One HTTP exchange; shared clients; settings from `WAAV_STT_SEGMENT_` variables | new | 1 |
| `wire/openai_compat.rs`, `wire/elevenlabs.rs` | `dialect_fields`, parsers | OpenAI, Groq, self-hosted servers, WaaV Infer (the project's own inference server), Azure OpenAI; ElevenLabs | new | 1 |
| `quality/` | `evaluate`, phrase data; `language_vote.rs` | The verdict function; the session language vote (two agreeing segments pin the language for the rest of the call) | new | 1; 2 |
| `core/resilience/circuit_breaker.rs` | `try_acquire`, `record_outcome`, `abandon_probe`, `force_open` | Additive; defaults keep today's behaviour | modified | 1 |
| `core/stt/http_resilience.rs` | `FileBreakers`, `FileBreaker`, `BreakerPermit` | Registry and permit for uploads | modified | 1 |
| `core/stt/prerecorded.rs`, `core/stt/openai/client.rs`, `handlers/transcribe.rs` | status helpers, parsers, event-stream reader, multipart builder; `transcribe_self_hosted`, `transcribe_azure_openai` | Lifted to shared code; the two functions wrapped by their adapters; wire bytes unchanged | modified | 1 |
| `core/state.rs` | this part's fields of `SttLiveShared`: upload breakers, clients, retry budgets | Built once at start-up | modified | 1 |
| `core/stt/openai/config.rs:63-67`, `:669-674`; `core/stt/batch.rs:668-669`; `plugin/builtin/mod.rs:110` | default model; request fields | An empty model becomes `gpt-transcribe`, which gets `languages[]` and `keywords[]` instead of `language` and `prompt`; the plugin metadata lists it. Must ship before 2027-02-26 | modified | 1, or a standalone patch |
| `probe.rs` | setup probe request | One silent clip to test a model and its fields | new | 2 |
| `wire/deepgram.rs`, `wire/assemblyai_sync.rs`; `core/stt/batch.rs` | adapters; `deepgram_listen_url` | Second vendor wave | new; modified | 3 |
| `core/stt/openai/client.rs`, `core/stt/groq/client.rs`, `core/stt/openai/config.rs` | `flush_buffer`, `send_request` | Rewired onto shared code, keeping the Release 0 `disconnect` fix; Groq's three-attempt loop and the dead `OnSilence` code removed | modified | 3 |
| `attempts.rs` | hedge; fallback hop, token refresh; `run_interim`, early text | Switched on per release | modified | 4; 5; 6 |
| `wire/azure_fast.rs`, `google_recognize.rs`, `speechmatics_jobs.rs`, `regional.rs`, `regional_new.rs`; nine regional `client.rs` files | adapters; `OneShotRecognize` | Third vendor wave | new; modified | 5 |

## What this part gives to and needs from the other parts

Gives:

- `SegmentTranscriber`, `SegmentTranscript` and `SegmentError`, the contract types. The engine reaches them only through the loop.
- `SegmentAttempts`, called once per utterance by the upload adapter that chapter 7 supplies; that adapter creates the `AttemptLedger`, the record chapter 7 meters from.
- `AttemptGate` and `GateHint`, which chapter 7's limiter implements; `AttemptObserver`, which chapter 6's latency store implements.
- The quality verdict, which the engine's sequencer calls and writes into `SegmentOutcome`.
- The adapter registry and each adapter's list of settings it sends, read by chapter 3's resolver; the setup probe's request; the test kit.

Needs:

- `ResolvedSttLive` (chapter 3): adapter, model, language as sent, request dialect (which fields the model accepts), limits, base URL and egress policy (which addresses a client may connect to), the row's stable identifier.
- `SttLiveShared` and `SttLiveSession` (chapter 3): the only hand-off types. The second holds the leg credential and the session's standardized speech-to-text configuration, which the plan step reads. `SegmentedStt::new_live(shared, session)` is the only caller of the plan step.
- `SegmentDeadlines` (chapter 6) with every utterance, imported from the leaf module `core/stt/segment_limits.rs` so that this part never depends on the engine, and the fixed deadline instant from the engine's sequencer; a call to `prewarm` at session start and on `SpeechActivity::Started`.
- A limiter gate from Release 1 and fallback targets from Release 5 (chapter 7).
- Delivery of `SegmentOutcome` by turn-taking's dispatcher (chapter 4), and warning and error codes, quoted from chapter 5's code table, the single source; a lost utterance reaches the client as `stt_segment_failed`.

## Tests to write first

The design names about 160 tests; this table lists those that carry an acceptance criterion or guard a decision. Acceptance criteria of the brief, as corrected: (1) padding uploaded, order kept, no early end of turn; (2) interruption behaviour; (3) a transcript per utterance on ElevenLabs `scribe_v2` and an OpenAI file model; (4) end of speech to final text within 2.5 s at the 99th percentile; (5) streaming models take today's path.

| Test name | Level | What it proves | Criterion | Release |
| --- | --- | --- | --- | --- |
| `fake_transcriber_scripts_delay_failure_and_order` | unit | The fake the engine's tests build on behaves as scripted | 1 | 0 |
| `uploaded_wav_is_exactly_the_audio_handed_in` | mock vendor over a real socket | Padding and pre-roll reach the vendor unchanged | 1 | 1 |
| `two_transcribe_calls_overlap_on_one_transcriber` | mock vendor | Two uploads can be in flight | 1 | 1 |
| `a_failing_vendor_receives_at_most_two_requests_per_segment` | in-process chain | Engine, adapter, loop and transcriber together send at most two | 1 | 1 |
| `there_is_one_attempt_loop` | unit (reads the source) | `transcribe` has one production caller and nothing wraps the loop in a timer | 1 | 1 |
| `a_stalled_attempt_gets_one_second_request_on_a_fresh_connection` | mock vendor | Stall recovery | 4 | 1 |
| `the_loop_applies_the_limits_it_is_given` | mock vendor | No time limit is computed in this part | 4 | 1 |
| `a_300_ms_blip_at_50_uploads_a_second_does_not_open_the_breaker` | mock vendor | A short vendor blip costs no utterance | none; resilience | 1 |
| `file_and_streaming_breakers_are_different_objects` | unit | An upload outage cannot stop streaming sessions | 5 | 1 |
| `a_refused_optional_field_is_dropped_and_the_segment_retried_once` | mock vendor | A wrong capability row does not produce a dead call | 3 | 1 |
| `scribe_v2_round_trip_from_plan_to_transcript`, `gpt_transcribe_round_trip_from_plan_to_transcript` | mock vendor | Plan to transcript against a recorded real response | 3 | 1 |
| `a_short_okay_with_clean_vendor_signals_is_kept` | unit | The filter keeps genuine short answers | 2 | 1 |
| `first_upload_after_prewarm_is_not_first_on_its_connection` | mock vendor, real time | No upload pays a handshake | 4 | 1 |
| `an_empty_openai_model_sends_gpt_transcribe_on_every_route` | unit | No route sends `whisper-1` by default | none; 2027-02-26 | 1 |
| `a_session_without_a_language_gets_stt_language_unset` | unit | The notice; a candidate-list row gets the expected languages | none | 1 |
| Existing wire tests of the OpenAI, Groq and ElevenLabs clients; `a_live_session_still_gets_the_streaming_client` | unit | Unedited and green after the lifts | 5 | 1 |
| `the_setup_probe_maps_answers_to_verdicts` | mock vendor | The six probe verdicts | 3 | 2 |
| `live_openai_gpt_transcribe_segment`, `live_groq_whisper_segment`, `live_elevenlabs_scribe_v2_segment` | live, needs a key | The real endpoint accepts the request | 3 | 2 |
| `assemblyai_sync_always_sends_language_codes_and_the_model_header` | mock vendor | That endpoint never falls back to English silently | none; vendor coverage | 3 |
| `regional_adapter_calls_the_existing_one_shot_function` | unit | Existing clients are wrapped, not copied | none; vendor coverage | 5 |

## What ships in which release

- **Release 0, groundwork and honest refusal.** The interface and types without HTTP; the test kit; a registry with no adapter built, so nothing is uploaded. Chapter 3's resolver refuses with `stt_live_unsupported` the sessions the map records as unable to work today (such as OpenAI's live-only models), plus, by choice, `/ws` voice agents with automatic turns on a model whose only client buffers until hang-up. Other sessions on a buffering model keep today's client with the warning `stt_buffered_until_commit`; self-hosted and Azure OpenAI keep today's refusals. Chapter 7's confirming test and conditional fix cover both the OpenAI and Groq clients. The fix keeps their callbacks across the reconnect after `audio_end` and makes Groq's `disconnect()` log a failed upload, drop that audio and return success, as OpenAI's does, so a failed upload no longer skips the reconnect and fails the next turn (`core/voice_manager/manager.rs:1655-1668`).
- **Release 1, first working calls.** The loop, upload breakers, clients and warming; OpenAI file models, Groq, self-hosted OpenAI-compatible servers, WaaV Infer, Azure OpenAI and ElevenLabs `scribe_v2` and `scribe_v2_medical`; field repair; the quality verdict; the lifts; the language rule of step 1. The OpenAI default-model change ships here, or earlier as a standalone patch once signed off, and must ship before 2027-02-26. Streaming sessions match the golden recordings except for `ready.stt` where the rollout switch covers them. Without the setup probe, a mistyped model fails at its first utterance as "model not served".
- **Release 2, dark launch complete.** The setup probe's request; request timings fed to the latency store; self-hosted server profiles; live checks; the half-open repair for existing clients, behind its own switch; the session language vote, before default-on. Exit condition: the 2.5-second gate (chapter 6) for the Release 1 vendors; one that fails stays behind the allow-list, and rows that upload once per turn are exempt.
- **Release 3, default on.** Deepgram hosted Whisper and the AssemblyAI synchronous endpoint; the OpenAI and Groq clients rewired, with sign-off, and the dead `OnSilence` code removed; ElevenLabs headerless uploads after a live check.
- **Release 4, live-only models and low latency.** The hedged second request (sent early, before the first is known to be stuck); registry entries for the commit transports (vendor sockets on which the gateway says where an utterance ends) of OpenAI, including `gpt-transcribe` for the low-latency tier, Azure OpenAI and Cartesia, the last behind its own flag. It targets 2027-02-26; missing that takes nothing away from customers.
- **Release 5, wider vendor coverage and hardening.** Azure fast transcription, Google `Recognize`, twelve regional vendors (Phonexia only on demand), and Speechmatics and Viettel only if measured within the deadline, each enabled after a live probe with a real key; fallback to a second vendor; token refresh; retention and region options; promotion of the counting-only quality rules after measurement; HTTP/2 for hosts where it was measured.
- **Release 6, interruption recovery and interim text.** Streamed text of an uploaded file switched on; `run_interim` for interim re-decoding.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| Retries multiply across layers again | Low | Vendor overload, double billing | One caller of `transcribe`, enforced by a source-reading test and a request count at the mock vendor |
| Vendors bill abandoned or duplicate requests | Unknown; undocumented | Up to about 10 % more requests billed | Retry budget; a measurement decides whether to cancel the first request at a stall |
| A capability row names a field the model refuses | Medium for new and self-hosted models | Every utterance fails | Repair and remember; three refusals in a row end the session with a named error; the setup probe from Release 2 |
| Breaker values wrong | Medium | Lost utterances, or an outage noticed late | Configurable; load test against the mock vendor |
| The lifts change today's file route or sessions kept on buffering clients | Low | Regression for existing customers | Pure moves only; existing tests unedited; Release 0 records today's sessions; later releases reproduce them, apart from `ready.stt` where the switch covers them |
| The filter drops real words | Low in Release 1 | The agent waits for an answer it already got | Unproven rules only count; promotion after measurement |
| Regional vendors slip | High | Release 5 is late for some vendors | Each is enabled alone; four are sized as new adapters; none starts without a key |
| OpenAI's default-model change misses 2027-02-26, when `whisper-1` is removed | Medium | Every OpenAI request naming no model fails: the REST file routes and live sessions the switch does not cover | Ship it in Release 1 or as its own patch, not with the Release 3 rewire |

## What is still open

Measurements needed:

- Median, 95th and 99th percentile time to response headers for OpenAI, Groq and ElevenLabs by audio length; none exists for `gpt-transcribe`.
- Whether each vendor bills a request the gateway abandons, or the loser of two.
- Breaker values at 50 uploads a second.
- Per vendor host: the HTTP/2 stream limit and window, the idle timeout, and whether a warm request counts against a rate limit.
- False-drop rate of the counting-only quality rules on labelled calls, including 8 kHz telephone audio.
- A live probe for every Release 5 vendor; for Speechmatics, Phonexia and Viettel it must show an answer within the deadline.

Choices that need a person, with the recommended default:

- When to turn on the half-open repair for the nine existing clients (Release 2, own switch). A half-open breaker (after its cooldown, waiting for one trial request) stays stuck today if that trial gets a client error. Default: off until signed off.
- Ceiling on the share of utterances that time out on a healthy vendor. Default: under 0.5 %.
- Error-class changes on the file route in Release 3. Default: approve, with a release note.
- ElevenLabs bills 20 % more when key terms are sent. Default: map vocabulary hints to key terms and warn.

## Where the detail is

In `/home/bud/ditto/waav/research/segmented-stt/design/`: the full design, `W2-transcriber-layer.md`, and its code-fit and adversarial critiques, `critique-W2-transcriber-layer-code.md` and `critique-W2-transcriber-layer-adversarial.md`.
