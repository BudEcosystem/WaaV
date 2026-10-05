# Segmented speech-to-text: integration decisions

Written 2026-10-03 by the orchestrator after the seven workstream designs were critiqued, revised and
cross-checked. The two cross-check reports (`cross-check-consistency.md`, `cross-check-coverage.md`)
found about twenty places where the designs disagree with each other. This file gives one binding
answer for each. Where a workstream design says something else, this file wins and the design must be
changed to match.

Decisions marked **needs sign-off** are choices a person should confirm. Each has a recommended
default so the work can proceed; the plan lists them together for review.

Workstream names used below: W1 engine, W2 transcriber layer, W3 capability map and resolution,
W4 turn-taking and interruption, W5 customer contract, W6 real-time performance, W7 cost,
observability, tests and rollout.

## 1. One release list

Seven releases, used by the plan and by every design. Use these names; do not use "first", "second",
"dark launch" or a design's private numbering without the release name beside it.

| Release | Name | What it delivers |
| --- | --- | --- |
| 0 | Groundwork and honest refusal | Test kit (fake transcriber, scripted detector, deterministic clock, mock vendor server); the two confirming tests (SmartTurn never runs on small packets; a second `audio_end` on the OpenAI client); golden recordings of streaming sessions; the capability map, its schema, the resolver and its tests (used in this release only to produce refusals); the named refusal of section 5; the Silero model files baked into the production image; Bud: catalog entry for `scribe_v2_realtime`, playground message |
| 1 | First working calls | Behind an allow-list. The engine (Silero and scripted detectors, segmenter, endpoint ladder, sequencer, result contract, per-pause and per-turn upload policies); the OpenAI-compatible and ElevenLabs transcribers with the one attempt loop and file breakers; resolver with static deployment override and trusted base URL; both `bud_legs` refusal sites replaced by the resolver; voice-agent loop: detector barge-in and the lost-turn ladder; `ready.stt` and the lost-segment warning; uploaded-seconds metering; limiter with request rate and concurrency; time limits from seeded latency values |
| 2 | Dark launch complete | Setup probe with its cache; per-deployment capability record published to Redis; conversation loop changes; `/v1/realtime` cascade events timed by the detector; control record; long-window budgets and admission; latency store fed by measurements; benchmark harness with live cells; software development kit updates; self-hosted server profiles; greeting fix (separately flagged, see section 14); LiveKit and SIP checks |
| 3 | Default on | Switch default becomes on; Bud "slower on calls" label and analytics columns; drift and live-probe jobs; removal of the dead `OnSilence` code; second vendor wave: AssemblyAI synchronous endpoint and Deepgram hosted Whisper |
| 4 | Live-only models and low latency | Gateway-driven commit on vendor sockets (OpenAI `gpt-live-transcribe`, `gpt-realtime-whisper`, `gpt-transcribe` on the socket; Cartesia manual finalize); hedged second request; the low-latency tier. **Must ship before 2027-02-26**, when OpenAI removes `whisper-1` and the `gpt-4o-*transcribe*` models |
| 5 | Wider vendor coverage and hardening | Third vendor wave (section 12); fallback to a second vendor; canonical retention and region options; vendor minimum reflected in price |
| 6 | Interruption recovery and interim text | Pause, then commit or resume after a false interruption; interim text by re-decoding for self-hosted models |

Crosswalk to the designs' own words: W7's release 1 is Release 0; W7's release 2 and every other
design's "first" is Release 1 plus Release 2 (split as in the table; a design must say which of its
first-release items are Release 1 and which are Release 2); W7's release 3 is Release 3; W7's release 4
is Release 5; W7's release 5 is split between Release 4 (commit transport, hedging) and Release 6
(interruption recovery, interim text).

## 2. Time limits of one upload

W6 owns every value. `segment_deadlines(...)` in W6 is the only producer.

- `SegmentRequest` (W2) carries W6's `SegmentDeadlines` whole. W2 deletes its own stall formula, its
  250 ms and 800 ms room constants and the "twice the stall timeout" recommendation. The request limit
  applies per request.
- Deadline form in Release 1: a fixed instant, computed by the sequencer as the newest cut of the
  caller turn at hand-over plus the deadline (6,000 ms default; a deployment may set 3,000 to 10,000).
  The movable deadline is a later refinement with its own test.
- The queue allowance counts from hand-over to the upload component, not from the cut.
- Nothing times out while a unit is held in the engine.
- A timeout is recorded in the latency store as a rank-only entry, not as an invented sample.
- W1 removes its own 2,762 ms figure and anchor rule and takes every limit from W6. W4 reads the
  bound within which a turn closes from the engine's reported `resolution_deadline_ms` and does not
  restate a formula. W5 quotes the engine's value in its examples.

## 3. End-of-turn ceiling

The silence ceiling is 1,500 ms by default (W1's revised ladder: on-demand audio model at the pause,
then the existing text end-of-turn model when the transcript is back, then the ceiling). This matches
the streaming path. A voice agent's `max_endpointing_ms` is honoured when the agent sets it, clamped
to 800 to 3,000 ms. W5 and W6 correct every mention of a 3 s ceiling. **Needs sign-off** (the agent
contract publishes 3,000 ms as its default).

## 4. Building a segmented session

W3 defines two types and no other design invents its own.

- `SttLiveShared`: process-wide, built once at start-up and held in application state. The capability
  map, the latency store, the limiter registry, the file-breaker registry, the upload HTTP clients and
  the detector model pool.
- `SttLiveSession`: per session, built in `initialize_voice_manager`. The `ResolvedSttLive`, the
  deployment identity and credential scope, the endpoint tuning (section 7), and the sinks for
  metering and outcomes.

`VoiceManagerConfig` carries both. `SegmentedStt::new_live(shared, session)` is the only place that
plans the transcriber. The handler does not construct transcribers (W7 changes its text).

## 5. Sessions the engine does not cover yet

One rule, implemented once in W3's resolver and used by both `bud_legs` sites and the plain `/ws` path.
It applies from Release 0, and afterwards to any session outside the allow-list or with the switch off.

- A session where the gateway decides turns (a voice agent, the conversation loop, or any session
  whose turn detection is not manual) on a model whose only client buffers until hang-up: refuse at
  setup with the code `stt_live_unsupported`, naming the model and a remedy.
- A push-to-talk session (the client ends each turn with `audio_end` or a commit): keep today's
  behaviour, which works.
- This rule covers OpenAI and Groq, and the regional buffering clients (Bhashini, FPT.AI, NAVER,
  NECTEC, Viettel). Yandex and SberDevices keep today's blind uploader until Release 5 and are
  reported as known broken in `ready.stt`.

The confirming test for a second `audio_end` runs first; if it shows the OpenAI client loses its
callbacks after the first flush, that defect is fixed in Release 0 so push-to-talk keeps working.
**Needs sign-off**: whether any customer relies on one transcript at disconnect from OpenAI or Groq on
a gateway-decided-turn session today.

## 6. Deepgram, AssemblyAI and the per-turn upload policy

- Deepgram's hosted Whisper models and AssemblyAI's synchronous endpoint ship in Release 3. Until
  then a live session on a Deepgram `whisper*` id is refused with `stt_live_unsupported`; AssemblyAI
  `universal-3-5-pro` keeps streaming as today.
- The per-turn upload policy (hold the turn's audio and upload once) ships in Release 1, because
  Azure OpenAI at default quota and Groq's request limits need it.

## 7. Agent turn-detection settings

W4's mapping wins. The audio model's threshold is fixed at 0.7; an agent's eagerness maps to the
silence ceiling, not to the threshold. `SttLiveSession` carries an `EndpointTuning` value with the
short pause, the silence ceiling and the policy. W3 removes the eagerness-as-threshold mapping.

## 8. What a client commit does

W5's rule. On `audio_end` or a realtime commit the engine cuts at once, uploads unconfirmed
non-silent audio, and always emits exactly one final result for that commit, empty if nothing was
recognised. W1 marks the flush outcome with whether a result follows. W4's wrapper keeps its record
until that result arrives and does not drop an empty final after a commit.

## 9. Delivering segment outcomes

W4's dispatcher in the voice manager owns the fan-out. It has one registration point that accepts
late registration and delivers each `SegmentOutcome` to turn-taking, to the wire and to metering.
A lost segment reaches the client as the warning `stt_segment_failed` through the outcome, not through
the notice hook (W7 changes its route). W5 adds any codes the dispatcher needs.

## 10. Capability map fields

W3 adds: a stable identifier on every row (used in the breaker key, metric labels and the rollback
list); per transport, the release from which it is enabled; an overload refusal kind and code;
surcharges and billing increments; a ramp flag; a deployment deadline override; billing per transport;
language and region constraints on a transport; the audio format a socket transport requires; a
provider flag saying the model string is not a model and must not be logged (Reverie stores the
customer's application identifier there); and key-normalisation exceptions (Bhashini ids begin with
the provider name; Huawei ids may carry a region suffix). The row files are regenerated with the final
adapter identifiers and profile names. The map shipped in each release is checked against an expected
resolution table for that release.

## 11. Upload client and connection warming

W2 owns the client: HTTP/1.1 by default with a per-host switch to HTTP/2 after measurement, a 1.5 s
connect timeout, separate shared clients for public and trusted hosts. W6's pool-depth rule decides
warming: keep as many idle connections per host as the session may have uploads in flight, topped up
at session start and at speech start. W2 withdraws its host-recency rule and cold flag. One
configuration, read from the environment with the prefix `WAAV_STT_SEGMENT_`.

## 12. Every vendor has an outcome

**Needs sign-off** on scope. The plan places every vendor that needs segmentation.

| Release | Vendors and models |
| --- | --- |
| 1 | OpenAI file models, Groq, ElevenLabs `scribe_v2` and `scribe_v2_medical`, self-hosted OpenAI-compatible servers, WaaV Infer, Azure OpenAI |
| 3 | AssemblyAI synchronous endpoint (`universal-3-5-pro`), Deepgram hosted Whisper |
| 4 | OpenAI live-only models and Cartesia manual finalize, through the commit transport |
| 5 | Azure Speech fast transcription (`MAI-Transcribe-*`), Google `Recognize` (including `chirp_2` in languages it cannot stream), Speechmatics `melia-1` and `oak-1` (only if a measurement shows they answer within the deadline), and the regional REST vendors through one generic adapter over their existing single-request functions: Yandex, SberDevices, NAVER CLOVA, Bhashini, FPT.AI, Gnani (REST), NECTEC `partii4` (evaluation only), Alibaba flash models, Tencent flash, Huawei short audio, Baidu, Phonexia. Each is enabled only after a live probe with a real key. Viettel is enabled only if a measurement contradicts the 15 to 30 s the audit observed |

Vendors that cannot be segmented (the only file interface is a slow asynchronous job): Amazon
Transcribe, Rev AI, iFlytek, Gladia `solaria-3`, AssemblyAI `universal-2`. Their streaming models keep
today's path. A file-only model or a batch-only language on these vendors is refused at setup with
`stt_live_unsupported`. Where today's client silently sends a different model (AssemblyAI
`universal-2`, Gladia `solaria-3`), Releases 1 and 2 report it in `ready.stt` with the warning
`stt_model_substituted`, and Release 3 turns it into a refusal with a release note.

W2 adds the generic regional REST family (it must cover Gnani, Alibaba, Tencent, Huawei, Baidu and
Phonexia, which its current text omits) or states for each why it cannot.

## 13. Refusals happen early and carry codes

The resolver refuses at admission, before a voice manager is built, for an audio format the engine
cannot decode and for a missing detector. The production image carries the Silero model files (a
Release 0 task owned by W7). If the model cannot be loaded on a build that includes Silero, the session
is refused with `stt_segmentation_unavailable`. The operator switch
`WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1` permits the energy detector instead; builds without the
Silero feature (development and CI) use the energy or scripted detector and report the detector kind.
W3 and W5 adopt this. **Needs sign-off.**

## 14. Deliberate exceptions to "streaming sessions are unchanged"

Two changes touch streaming sessions. Both ship separately from the engine, each behind its own flag
and with a release note, in Release 2: the greeting fix in shared agent code (W4), and
transcript-derived speech events for streaming clients that asked for `vad_events` (W5). Release 1
leaves streaming sessions byte-identical, and the golden recordings prove it.

## 15. Smaller answers

| Question | Answer |
| --- | --- |
| A marker such as `[inaudible]` for lost words | Not in customer-visible transcript text. It reaches only the language model's input, using the offset on the outcome. **Needs sign-off** |
| Uploads in flight when the call ends | Dropped at once. Seconds already uploaded are metered. One possibly billed request per hang-up is accepted |
| OpenAI's model when the session names none | `gpt-transcribe`. An explicit `whisper-1` still works and carries a deprecation warning. **Needs sign-off** |
| ElevenLabs retention | Send nothing by default on both paths (the vendor default applies). The canonical retention option arrives in Release 5 |
| Self-hosted default and `language` | Send it; the setup probe decides. The probe's verdict type gains "try the minimal request" |
| The brief's 2.5 s criterion | A release gate for the Release 1 vendors (99th percentile from end of speech to final at or under 2.5 s, 3,000 samples, measured from a gateway near the vendor) and a label on every other row. A vendor measured slower is offered with the "slow" label and a warning, not refused. **Needs sign-off** |
| Native clients the audits found broken | Reported in `ready.stt` and in the map; not refused; listed as separate defects |
| `/v1/realtime` without a voice agent | Cannot address a file-only deployment. Named refusal `stt_live_unsupported` and one documentation line |
| Resume after a false interruption on `/v1/realtime` | Not offered; documented limitation. **Needs sign-off** to launch without it |
| A DAG speech-to-text node run as a library call | Out of the live path; unchanged; said once in the plan |
| Sample count for a 99th-percentile claim | 3,000 samples with at most 20 exceedances (W6); W7 corrects its 1,000 |
| Soft split limit | W1 derives it as the smaller of 20 s and the row's maximum minus 5 s |
| OpenAI's 2,010 ms seed | Applies to `gpt-4o-mini-transcribe` only; `gpt-transcribe` starts unmeasured |
| Bud deployment settings | One schema. budapp accepts `stt.segmented` and `stt.capability_override` by Release 1; the remaining fields by Release 3 and Release 5 |
| LiveKit and SIP | W1 adds a dropped-frame test through `receive_audio`; W4 adds a LiveKit chain test; W7 adds a SIP live check as a Release 2 exit criterion |
| The acceptance table | W7 regenerates it from section 6 of `cross-check-coverage.md` |

---

# Addendum A: answers to what the alignment pass left open

Written 2026-10-04 after the seven designs were aligned. The aligners reported points where a decision
above could not be applied as written, or where two aligned designs still used different names. Each
gets one answer here. This addendum wins over the sections above and over any design or chapter.

## A1. Section 5 is narrowed: refuse only what cannot work today

The rule in section 5 would have refused a conversation-loop client that ends its own turns with
`audio_end` on an OpenAI or Groq model, which works today. Plain `/ws` has no way to declare
push-to-talk (`handlers/ws/config.rs:448-461`). The rule becomes:

- **Refused** with `stt_live_unsupported`: a voice-agent session whose turn detection is not manual, on
  a model whose only client buffers until hang-up. Nothing on such a session can end a turn, so it
  cannot work today. Also refused, on every kind of session: an OpenAI live-only model
  (`gpt-live-transcribe`, `gpt-realtime-whisper`) before Release 4, because today's client posts it to
  the file endpoint, which rejects it.
- **Warned, not refused**: every other session on a buffering model receives the existing
  `config_warning` message with code `stt_buffered_until_commit`, saying the model returns text only
  when the client sends `audio_end` or hangs up. Its behaviour is otherwise unchanged.
- **A `/v1/realtime` voice-agent session admitted in manual mode** that later sends a
  `session.update` turning turn detection on, while on an uncovered buffering model: the update is
  rejected with the same code and the session continues in manual mode
  (`handlers/openai_realtime/cascade.rs:619-625`, `:1399-1403`). The cascade passes its manual flag to
  the resolver.
- Self-hosted and Azure OpenAI deployments that are not yet covered keep today's two codes
  (`unsupported_deployment`, `stt_not_streaming`) until the Bud-side check on those strings is answered.

Sessions on streaming models are untouched by this rule and stay byte-identical.

## A2. Agent `max_endpointing_ms`: set or default

The field is a plain number with a default of 3,000 (`bud-auth/src/voice_agent.rs:57-78`), so "set"
cannot be told from "default". Until bud-auth makes it optional and Bud stops publishing the default,
the gateway reads exactly 3,000 as "not set" and applies the 1,500 ms ceiling scaled by eagerness.
Making the field optional is a Release 1 task for bud-auth and a Bud-side task.

## A3. What the two session values carry

- `SttLiveShared` also holds the retry budgets.
- `SttLiveSession` also holds the session's standardized speech-to-text configuration (language, wire
  audio format), the leg credential, and the slot for the outcome sink. `VoiceManager::new` installs
  its dispatcher in that slot before it calls the factory.
- `SegmentDeadlines` lives in a neutral leaf module, `core/stt/segment_limits.rs`, importable by both
  the engine and the transcriber layer.

## A4. Endpoint tuning has three named fields

`cut_pause_ms` (224, an engine constant, not settable by an agent in Release 1);
`min_end_silence_ms` (an agent's `silence_ms`, or the canonical `endpointing_ms`); and
`silence_ceiling_ms`. No design uses the phrase "the short pause" for anything but the cut.

## A5. A deployment's deadline cannot undercut the ceiling

The resolver raises a deployment deadline that is below the silence ceiling plus 2,500 ms to that
value and reports the change. Otherwise audio held until the end-of-turn decision could be handed
over with almost no time left.

## A6. The greeting fix and other changes outside segmented sessions

- The greeting fix lands in Release 1, active only on segmented sessions, because detector-driven
  interruption cannot work without it when an agent uses the default non-interruptible greeting
  (`core/voice_manager/manager.rs:708-711`, `:907-911`). Streaming sessions get it in Release 2 behind
  `WAAV_PER_UTTERANCE_INTERRUPTIBILITY`, with a release note.
- The repair to the existing breaker's half-open state affects nine existing clients. It is withheld
  from Release 1 and proposed for Release 2 behind its own switch. The new upload breakers have the
  repair from the start.
- From Release 3, when the switch default is on, every session's `ready` message gains the `stt` key.
  That is an additive change to streaming sessions and gets a release note. In Releases 1 and 2 only
  sessions the switch covers receive it; a substituted model on an uncovered native session is recorded
  in the log and a counter only.
- New metric series appear on `/metrics` at zero from Release 0.

## A7. Release placement of items the decisions did not name

| Item | Release |
| --- | --- |
| A conversation-loop or `/v1/realtime` call on an allow-listed deployment | Works from Release 1 with interruption and speech events still timed by the transcript; detector timing arrives in Release 2 |
| The holding phrase spoken before a late transcript | 2 |
| The resolver understands the `transcription_mode` preference | 1 (no caller yet); the wire field and SDK support arrive in Release 2 |
| The setup probe | 2. In Release 1 a mistyped model on an allow-listed deployment is admitted and fails at the first utterance with the "model not served" class |
| Refusing batch-only languages on Amazon Transcribe, Rev AI and iFlytek | 3 |
| The seam for the live-audio tap | 4, with the commit transport |
| A deadline that moves when the caller speaks again | 5, only if a counter shows it is needed |
| Clients for self-hosted server sockets (vLLM, NVIDIA NIM, speaches) | 5, only on demand |
| Text streamed by a vendor for an uploaded file | 6 |
| Interim re-decoding | 6, through a second entry point on the attempt loop (`run_interim`), so the transcriber still has one caller |

## A8. The third vendor wave needs new request code for four vendors

Gnani, Alibaba, Tencent and Phonexia have no existing single-request function to wrap: their clients
are socket-only. Each needs a small new request builder in the generic regional family (Gnani's REST
call, Alibaba's synchronous short-file models, Tencent's flash recognition, Phonexia's unary gRPC or
fast-polled task). They stay in Release 5, each sized separately and enabled only after a live probe.
Phonexia is on-premises and is built only when a customer needs it.

## A9. The 2.5-second gate

It is an exit condition of Release 2 for the Release 1 vendors. A vendor that fails it stays behind
the allow-list until it passes or product accepts the "slow" label for it. Rows that upload once per
turn (Azure OpenAI at default quota) are exempt and labelled, because their upload cannot start before
the end-of-turn decision and their quota makes 3,000 samples impractical.

## A10. One name for each thing

| Thing | Final name |
| --- | --- |
| Warning for a lost upload | `stt_segment_failed` (not `stt_segment_lost`) |
| Refusal of new sessions on measured overload | `stt_overloaded` |
| Reasons inside `stt_live_unsupported` | `not_covered_yet` (the engine does not cover the session), `client_not_implemented` (the gateway's client for the model ships in a later release), `async_only` (the vendor's only file interface is a slow job), `language_not_live`, `provider_not_built`, `disabled`, `model_not_served`, `realtime_needs_agent` |
| Code table | The customer-contract chapter is the single source; other chapters quote it |
| Binary for tests over a real socket | A new integration binary, `segmented_stt_ws`, built with the `test-util` feature and added to the named list in continuous integration |
| Idle timeout and warm-up timeout of the upload client | 50 s and 2,000 ms |
| When connections are topped up | Session start, speech start, and after any cancelled request |

## A11. Smaller answers

- "Seconds already uploaded are metered" means: an upload the vendor already answered is charged as a
  success even if the call then ends; an upload still in flight at hang-up is not charged, and its sent
  seconds and the estimate of the vendor's bill are recorded.
- The map's pricing pointer is informational. Prices stay with the deployment's price in Bud. The stale
  table in `config/pricing.rs` is a separate defect.
- Limits the schema does not yet carry (per-day limits as rate-limit entries, vendor limit-header
  names, a retention parameter per transport) are added when the release that reads them is built.
- The engine's `AudioDropped` notice maps to the warning `stt_audio_dropped`; `NoiseThresholdRaised`
  is logged and counted, not sent to the client.

---

# Addendum B: answers to the final independent review

Written 2026-10-04 after a five-lens independent review whose serious findings were each checked by a
refuter. This addendum wins over everything above it, over every chapter and over the top-level plan.
The copy of this file in `WaaV/docs/segmented-stt/` is authoritative.

## B1. The two deadline facts

- `final_deadline_ms` is the deadline in force: 6,000 ms by default, or the deployment's value after
  the raise in A5. It is counted from the turn's newest cut, which is when the engine reports
  `Stopped` and the wire's `speech_end` is sent. It does not include the 224 ms cut pause. The engine
  reports it and the customer contract copies it unchanged. Chapter 1's figure of 6,224 is withdrawn.
- `resolution_deadline_ms` is the bound within which a turn closes, 6,922 ms by default, counted from
  the last voiced sample (224 cut pause + 6,000 deadline + 250 guard + 448 onset window).

## B2. The premise of A1, restated

The A1 rule stands. Its stated reason is corrected. On `/v1/realtime` nothing on a voice-agent session
with automatic turn detection can end a turn on a buffering model, because a commit acts only in manual
mode (`handlers/openai_realtime/cascade.rs:415-423`). On `/ws` the client's own `audio_end` can end one
(`handlers/ws/audio_handler.rs:301-322`), so such a `/ws` agent is refused by deliberate choice, not
because it cannot work; plan sign-off item 3 covers it. "Release 0 refuses only sessions that cannot
work today" is replaced everywhere by "Release 0 refuses sessions the map records as unable to work
today, plus `/ws` voice agents with automatic turns on a buffering model, by choice".

## B3. Which regional clients buffer

The buffering regional clients covered by section 5 and A1 are Bhashini, FPT.AI, NAVER CLOVA (`csr`)
and NECTEC `partii4`: four, not five. Viettel's client also buffers but posts to a retired domain
(`core/stt/viettel_ai/config.rs:40`), so the map records it as failing today and refuses it on every
session with `client_not_implemented`; NECTEC ids other than `partii4` are refused the same way.

## B4. A plain `/ws` client that ends its own turns keeps today's client

A *plain* session is a `/ws` session with no voice agent and no conversation loop: the client consumes
transcripts itself. On a model whose today client buffers until hang-up (OpenAI and Groq file models,
Bhashini, FPT.AI, NAVER `csr`, NECTEC `partii4`), a plain session stays on today's client with the
warning `stt_buffered_until_commit` even when the rollout switch covers it, unless the request sets
`transcription_mode: segmented` (wire field from Release 2). Reason: today such a client gets one
transcript per `audio_end`; moving it to the engine without its consent would let the gateway end a
turn on a pause inside one press. Voice agents in either turn mode, conversation-loop sessions and DAG
sessions go to the engine when covered. The reference resolver gains a third session kind, `plain`.

## B5. Groq has the same defects as OpenAI at `audio_end`, and one more

Groq's `disconnect()` clears both callbacks (`core/stt/groq/client.rs:1113-1115`), so every `audio_end`
after the first delivers nothing. When the flush upload fails, `disconnect()` returns an error after
marking the client disconnected, `finalize_stt` stops at its `?` before reconnecting
(`core/voice_manager/manager.rs:1655-1668`), and every frame of the next turn produces an error message.
The Release 0 confirming test covers both clients (`a_session_on_a_buffering_model_gets_a_transcript_at_every_audio_end`,
plus a failed-flush case for Groq), and the conditional fix covers both: keep the callbacks across the
finalize reconnect, and make Groq's `disconnect()` log a failed flush and return success as OpenAI's
does. Until the fix ships, the `stt_buffered_until_commit` text says text arrives "when the client
sends `audio_end`", without promising every one.

## B6. The OpenAI default model is the date-critical item

What fails on 2027-02-26 is the code default `whisper-1` (`core/stt/openai/config.rs:63-67`,
`:669-674`; `core/stt/batch.rs:668-669`), which serves uncovered live sessions and the REST file route.
A separately dated item ships in Release 1, or as a standalone patch as soon as sign-off 5 is given:
an empty OpenAI model becomes `gpt-transcribe`; for `gpt-transcribe` the client sends `languages[]` and
`keywords[]` instead of `language` and `prompt`; the plugin metadata lists `gpt-transcribe`. It
**must ship before 2027-02-26** and depends on no later release. Release 4 (the commit transport)
**targets** that date; missing it takes nothing away from customers.

## B7. Language on segmented sessions

From Release 1 the session's language is always sent when known. When a segmented session has no
language or `auto`, `ready.stt` carries the notice `stt_language_unset`, saying detection on short
segments is unreliable and recommending a language; where a row takes a candidate list
(`gpt-transcribe` `languages[]`, AssemblyAI `language_codes`) the deployment's expected languages are
sent. The session-level language vote (agree a language across the first segments, then pin it)
moves from Release 5 to Release 2, before default-on.

## B8. Cartesia from Release 4 is a third deliberate change to streaming sessions

Every Cartesia row lists the gateway-driven finalize transport ahead of today's client, because
today's client ends an utterance more than once. From Release 4 a covered Cartesia session therefore
leaves today's client. This is the third deliberate change to streaming sessions (with the two in
section 14). It sits behind its own flag, `WAAV_STT_CARTESIA_MANUAL_FINALIZE`, off until a live probe
passes, with a release note, and the Cartesia golden recordings are re-recorded.

## B9. The latency tier is a real input

The deployment override gains `latency_tier`: `standard` (default) or `low_latency`, read from
Release 4. With `low_latency` and preference `auto`, a usable commit transport is preferred over a file
transport when a row has both; the file transport remains the fallback. This is how a session reaches
`gpt-transcribe` on OpenAI's socket. The schema and the reference resolver (`--latency-tier`) carry it.

## B10. Refusal reasons say what is true for this session

- `not_covered_yet` only when the rollout switch does not cover the session and a covered session would
  get a transport in this release. Its text may say "ask the operator to enable segmented
  speech-to-text".
- `client_not_implemented`: the gateway has no client that can serve this model on a live call in this
  release; the capability map says whether a later release adds one. Its text never tells a covered
  session to ask the operator; it says segmented speech-to-text for this provider is not available in
  this release.
- The resolver chooses between the two at run time; rows store `client_not_implemented`.

## B11. Azure OpenAI live-only models

Azure OpenAI `gpt-live-transcribe` and `gpt-realtime-whisper` join Release 4 with OpenAI's, through the
same commit transport with the Azure address and `api-key` header, each enabled only after a live probe.

## B12. Models this plan does not serve, named

Beyond retired models and models whose only file interface is a slow job, these stay refused in every
release, with the reason given: NAVER CLOVA Speech short, long and streaming (no gateway client for
those products; `csr` is served from Release 5); Huawei `chinese_16k_conversation`; Speechmatics
`linden-1`; self-hosted Kyutai models (no client for their socket); Deepgram Flux (no client for
`/v2/listen`); unknown ElevenLabs realtime ids; Rev AI `human`; Sarvam `saaras:v2.5` (translation
interfaces only). Each is a candidate for later work, not part of this plan.

## B13. Smaller answers

- Rows whose today behaviour is unknown and that are not retired (Rev AI `human`, Sarvam
  `saaras:v2.5`, Yandex `deferred*`) are refused from Release 3, not Release 0.
- Release 0 adds a gateway `test-util` feature that enables tokio's test utilities for dev builds;
  tests that need the real Silero or SmartTurn model files run in an integration binary in the accuracy
  job, not as feature-free library tests.
- The withdrawal switch `WAAV_STT_FILE_ONLY_REFUSAL` belongs to the resolver (chapter 3): off removes
  the Release 0 refusals and keeps the warning.
- The Azure OpenAI capacity warning (`stt_capacity_low`) ships in Release 2.
- Substituted-model warnings: logged and counted in Release 0 and on uncovered sessions; in
  `ready.stt` on covered sessions; on every session from Release 3.
- Cost wording: a talkative caller is charged up to about 1.5 times the streamed seconds (padding per
  upload), not "slightly more".
- `transcribe_self_hosted` and `transcribe_azure_openai` (`handlers/transcribe.rs:769-1027`) are
  reusable single-request functions; the upload adapters for those vendors wrap or lift them.
- DAG sessions are a surface of the customer promise: a DAG pipeline on a live session uses the same
  construction site and result contract; the DAG library-call speech-to-text node is unchanged.

---

# Addendum C: loose ends after applying Addendum B

Written 2026-10-04. Wins over everything above it.

- **C1. Conversation loops without turn detection.** A conversation-loop session whose turn detection
  is disabled is ended by its own client's `audio_end`, like a plain session, so B4 applies to it: on a
  buffering model it stays on today's client unless it sets `transcription_mode: segmented`. A
  conversation loop with turn detection enabled goes to the engine when covered.
- **C2. Risk ratings.** "Release 4 misses 2027-02-26": Medium, with no loss to customers. "The OpenAI
  default change misses 2027-02-26": Low when scheduled as a standalone change.
- **C3. Two kinds of substitution.** Where today's client substitutes another model because the
  requested one is retired, served only by a slow job, or of unknown behaviour, the row is refused from
  Release 3 (`refuse_from_release: 3`). Where the requested model is live-capable but today's client
  serves another (for example AssemblyAI `universal-3-6-pro`), the session is warned with
  `stt_model_substituted` on every session from Release 3 and not refused.
- **C4. Wire values for a covered plain session on a buffering client.** `ready.stt.transcription_mode`
  is `buffered` and `endpointing` is `client`, as the contract reference defines.
- **C5. Generated artifacts.** `assemble.py --routing PATH` writes the routing map the gateway embeds
  (the map without reviewer prose; about 1.0 MB, 45 KB compressed). `resolve.py --expected-json DIR`
  writes `release-<n>.json`, which the gateway's test `the_shipped_map_resolves_as_the_release_table_says`
  compares against. Both exist now.
- **C6. Expected languages.** The deployment's speech-to-text settings gain `expected_languages` (a list),
  accepted by budapp by Release 1. Until it exists, the single `language` is sent.
- **C7. Cartesia.** Its finalize transport needs a live probe (`enable_requires`) as well as the flag.
- **C8. The withdrawal switch** `WAAV_STT_FILE_ONLY_REFUSAL` stays until Release 5 at the earliest; it is
  not removed at default-on.
- **C9. NAVER CLOVA Speech short, long and streaming.** A voice agent with automatic turns is refused;
  other sessions keep today's client, which sends them to `csr`, and are warned of both the substitution
  and the buffering.
- **C10. Test time in continuous integration.** Release 0 enables the gateway `test-util` feature in the
  continuous-integration library-test job, so paused-clock tests run there.
- **C11. The greeting fix** is signed off before Release 2, when it ships behind its flag.
- **C12. The low-latency tier** both prefers the commit transport and enables the early hedged request.
