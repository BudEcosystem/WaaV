# Chapter 5. The provider-independent customer contract

## What this part is and why it exists

The customer contract is everything a customer's code can see: the configuration it sends, the messages it receives on `/ws` and on `/v1/realtime` (the OpenAI Realtime-compatible front door for a Bud voice agent), the warning and error codes, and what the SDKs do with them. This part fixes it so that the same client code works with a model that streams text during speech and with a file-only model, which the gateway serves by segmented transcription: it cuts the caller's audio into utterances with its own voice detector, uploads each to the vendor's file endpoint, and returns the text. The client is told what it got. A live DAG session (`dag_config`) gets the same contract.

Without this part the engine would work and customers would still be hurt. Today a file-only model fails at setup with an uncoded error or starts and stays silent until hang-up. A lost utterance would reach the client as `error`, and deployed widget builds show "Disconnected" after any `error` (`clients_sdk/widget/src/websocket.ts:653-655`, `clients_sdk/widget/src/widget.ts:536-556`). One refusal would keep two codes depending on the path (`handlers/ws/bud_legs.rs:122-134`, `:815-826`). And no client could tell whether text arrives during speech or after each pause, or how long to wait.

## How it works

The speech-to-text configuration is the same for every model. At setup the capability-map resolver (chapter 3) says how the model is reached, and the gateway reports the answer in one new object, `stt`, inside the existing `ready` message. Everything else is additive. `docs/segmented-stt/customer-contract-reference.md` defines every field and code.

| Piece | What it carries | When a client sees it |
| --- | --- | --- |
| `ready.stt` | `transcription_mode`, `interim_results` (`live`, `per_segment`, `none`), `endpointing`, `speech_events`, `detector`, `confidence_source`, latency figures, `final_deadline_ms`, `streaming_alternatives`, `notices` (facts reported without a message) | Once, on every audio-enabled session the rollout switch covers |
| `stt_result` (shape unchanged) | On a segmented session: interim results that each hold the whole turn so far, then exactly one result that is both final and end-of-turn | Each caller turn |
| `vad_event` (new) | `speech_start`, `speech_end` (what the detector heard); `turn_start`, `turn_end`, `turn_closed` (what the gateway decided) | When the client set `features.vad_events`, and always on voice-agent sessions. `turn_closed` for a turn with no text is always sent |
| `transcription_mode` preference | Optional: `auto`, `streaming`, `segmented`. Request beats deployment beats gateway default. Only `streaming` in the request can refuse a session; `segmented` moves a plain session off a buffering client | Request field, from Release 2 |

The rollout switch, the server setting `WAAV_SEGMENTED_STT`, covers every session when `on` (the default from Release 3) and the deployments it names when `allowlist`. *Gateway-endpointed* means `endpointing` is `gateway`: the gateway decides where turns end. A *plain* session is a `/ws` session with no voice agent, no conversation loop (`conversation_config`, the built-in language-model loop) and no DAG. A *buffering model* is one whose only gateway client holds the audio until `audio_end` or hang-up: OpenAI and Groq file models, Bhashini, FPT.AI, NAVER CLOVA `csr`, NECTEC `partii4`.

**Sessions the switch does not cover** get today's bytes and no `ready.stt`, except as follows. A model that fails today (an ElevenLabs file model; Viettel, whose client posts to a retired domain) is refused with `stt_live_unsupported`. On a buffering model so is a voice agent with automatic turn detection: on `/v1/realtime` nothing can end its turns, as a commit acts only in manual mode; on `/ws` its client's `audio_end` can, so the refusal there is a choice awaiting sign-off. Other sessions on a buffering model get `stt_buffered_until_commit`, which says text arrives when the client sends `audio_end` (not yet at every one; see chapter 7) or at hang-up.

**Covered sessions** on a buffering model use the engine where it serves that model, except a plain session: it keeps today's client and that warning unless the request sets `transcription_mode: "segmented"`, since today it gets one transcript per `audio_end` and the engine could end a turn mid-press.

**Worked example.** A plain `/ws` client sends its usual configuration with `"provider":"elevenlabs","model":"scribe_v2","language":"en"` and `vad_events: true`. From Release 1, if allow-listed, it gets `config_warning` `stt_segmented_mode`, then (abridged):

```json
{"type":"ready",…,"stt":{"provider":"elevenlabs","model":"scribe_v2",
 "transcription_mode":"segmented","interim_results":"per_segment","endpointing":"gateway",
 "confidence_source":"none","final_latency_slow_ms":2010,"final_latency_slow_percentile":99,
 "latency_basis":"seed","final_deadline_ms":6000,"streaming_alternatives":["scribe_v2_realtime"],…}}
```

The caller speaks for 2,400 ms, timed from speech start.

| Time | Message |
| --- | --- |
| 224 ms | `vad_event` `speech_start` |
| 512 ms | `vad_event` `turn_start`: the safe moment to stop local playback |
| 2,624 ms | `vad_event` `speech_end`: the cut; the upload and `final_deadline_ms` start here |
| After 416 to 1,500 ms of silence | `vad_event` `turn_end`: the gateway decided the turn is over |
| The later of that decision and the vendor's answer | `stt_result`, final and end-of-turn, with the whole turn's text |
| Immediately after | `vad_event` `turn_closed`, `had_transcript: true` |

If the upload fails after its one allowed retry, or no answer comes within 6,000 ms of `speech_end`, the client receives `stt_warning` `stt_segment_failed` and the call continues. A commit (`audio_end`) always gets exactly one end-of-turn result, empty if nothing was recognised.

**Existing settings on a segmented session.** Each canonical setting is honoured, mapped, ignored or surcharged; `stt_setting_not_applied` says which. `endpointing_ms` sets `min_end_silence_ms`, the silence that ends a turn when no end-of-turn model decides (500 ms by default); `utterance_end_ms` sets `silence_ceiling_ms` (800 to 3,000 ms). Neither changes `cut_pause_ms`, the 224 ms pause that cuts a segment, so a value tuned for a streaming vendor cannot multiply uploads. `turn_detection.threshold` (fixed at 0.7) and diarization are ignored; two channels are mixed down. A known language is always sent; with none or `auto`, `ready.stt` carries `stt_language_unset` (short segments make detection unreliable) and a row taking a candidate list (`gpt-transcribe` `languages[]`, AssemblyAI `language_codes`) gets the deployment's expected languages.

**Code table.** Every customer-visible code, by message; other chapters quote it. A code ships in Release 1 unless marked.

| Message | Codes |
| --- | --- |
| `config_warning`, before `ready`; none on a streaming session that used no new field | `stt_buffered_until_commit` (Release 0), `stt_segmented_mode`, `stt_setting_not_applied`, `stt_min_billed_duration`, `stt_detector_fallback`, `stt_transport_fallback`; from Release 2: `stt_capacity_low`, `stt_latency_slow`, `stt_fields_reduced`, `stt_transcription_mode_invalid`, `stt_transcription_mode_ignored`, `stt_mode_unavailable`; existing `deployment_setting_not_applied`, also for a raised deadline |
| `ready.stt` notices | On today's path: `stt_model_substituted`, `stt_client_unverified`, `stt_capability_assumed`, `stt_model_deprecated`, `stt_placeholder_model_ignored` (the last three as `config_warning` on a segmented session). On a segmented session: `stt_narrowband_audio`, `stt_language_unset` |
| `stt_warning`, during the call; not fatal | `stt_segment_failed`, `stt_degraded`, `stt_rate_limited`, `stt_audio_dropped`, `stt_fields_reduced`, `stt_detector_fallback`, `stt_fallback_engaged` (Release 5) |
| `error` at setup, with additive `code`, `recoverable`, `details` | `stt_live_unsupported`, `stt_model_retired` (both Release 0), `stt_segmentation_unavailable` (reason `detector_refused` or `audio_format`), `stt_overloaded` (Release 3); existing `stt_not_streaming` (strict preference) and `deployment_misconfigured`; on an uncovered self-hosted or Azure OpenAI deployment, today's `unsupported_deployment` or `stt_not_streaming` until Bud confirms nothing matches them |
| `error`, once during the call | `stt_unavailable`, when transcription stops for good; `deployment_changed`, when an edit to the deployment mid-call made the next request fail |
| Reasons of `stt_live_unsupported` | `not_covered_yet`: the switch does not cover the session, and a covered one would be served in this release; only this text may point to the operator. `client_not_implemented`: no gateway client serves this model live in this release (the map says whether a later release adds one); its text never points to the operator. Also `async_only`, `language_not_live` (Release 3), `provider_not_built`, `disabled`, `model_not_served` (Release 2), `realtime_needs_agent` |

`stt_model_substituted` is only logged and counted in Release 0 and on uncovered sessions, a notice on covered ones, and sent on every session from Release 3. A refusal decided before the session id is stored has `recoverable: true`: the client can send a corrected `config` on the same socket (`handlers/ws/config_handler.rs:179-190`, `:361-370`).

**Key numbers.**

- 224 ms to confirm speech and to cut a segment, 512 ms of sustained speech for `turn_start`, 416 ms, the shortest silence that can end a turn (when an end-of-turn model says finished): the engine's design values (chapter 1), not yet measured on call audio.
- 1,500 ms end-of-turn silence ceiling (integration decision 3), as on the streaming path. An agent's `max_endpointing_ms` is honoured when set; until bud-auth makes it optional (Release 1), exactly 3,000 reads as unset.
- 6,000 ms `final_deadline_ms`, counted from the turn's newest cut (`Stopped`, sent as `speech_end`), so without the 224 ms cut pause; 3,000 to 10,000 ms per deployment, raised to at least the ceiling plus 2,500 ms. Chapter 6 sets it, the engine reports it, this part copies it.
- 2,010 ms: Pipecat's 99th percentile from end of speech to final for ElevenLabs' file endpoint (`capability-map/rows/elevenlabs.json`), a seed until Release 2 measurements report a 95th percentile; `gpt-transcribe` has none.
- A talkative caller is charged up to about 1.5 times the streamed seconds, as each upload is padded (chapter 7).

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| Result sequence | Cumulative interims, then one final end-of-turn result | A final that is not end-of-turn arms the existing 600 ms and 1,500 ms timers (`core/voice_manager/stt_result.rs:170-173`, `:236-251`), which would split turns | Interim text is not revised word by word |
| Who gets `ready.stt` | Only sessions the rollout switch covers | Customers off the allow-list must notice nothing in Releases 1 and 2 | From Release 3 every `ready` has one more key |
| A lost utterance | The new `stt_warning` type, code `stt_segment_failed`, never `error` | Deployed widgets treat any `error` as the end of the session | A third message type |
| Lost words in the transcript | No marker such as `[inaudible]`; the warning carries the text offset | Customer text holds only words the vendor returned | A client must read the warning to show a gap |
| Uncovered sessions on a buffering model | Refuse only a voice agent with automatic turn detection; warn the rest | It cannot end a turn on `/v1/realtime`; on `/ws` it is refused by choice | A `/ws` agent whose client commits is refused |
| Commit (`audio_end`) | Always answered by one end-of-turn result; unconfirmed audio that is not silence is uploaded | A push-to-talk "no" must still return "No." | An occasional upload of noise |
| Preference | `transcription_mode`; only a request-level `streaming` is strict | A strict deployment setting would turn a model swap into an outage | A deployment owner cannot force a refusal |
| Missing detector model | Refuse with `stt_segmentation_unavailable` unless the operator allows the energy (loudness-based) detector | The fallback is much worse and would become the silent default where downloads are blocked | The image must carry the model files |
| `confidence` | The vendor's number, else the wire family's existing derived number, else 1.0; `confidence_source` says which | Clients that threshold today keep working | Not comparable across models |
| Speech events on streaming sessions | Derived from transcripts, for clients that asked, behind `WAAV_STT_TRANSCRIPT_SPEECH_EVENTS` | The same client code on every model | One of three flagged changes to streaming sessions |

## What changes in the code

Paths are under `gateway/src` unless they start with another directory.

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `handlers/ws/messages.rs` | `OutgoingMessage::Error` (`:372-376`); `Ready` (`:277-328`), `ReadyStt`, `VadEvent`, `SttWarning` | Optional `code`, `recoverable`, `details`, constructors `error`, `coded_error`; then `stt` as the last field of `Ready`, two new variants, value `notice` of `AgentResponseStarted.kind` (`:422-430`) | Modified, new | 0; 1 |
| `handlers/ws/stt_contract.rs` | `refusal_to_error`, the buffered warning; `ready_stt`, `setting_outcomes`, the other warnings, `segment_outcome_to_wire`, `engine_backstop_to_error`; `resolve_with_preference` | Pure functions; the only producer of customer-visible text | New | 0; 1; 2 |
| `handlers/ws/speech_events.rs` | `SpeechEventEgress`, `send_transcript` | One sender of `vad_event`; all four transcript emitters send through one function | New | 1 |
| `handlers/ws/config_handler.rs` | `handle_config_message`; `initialize_voice_manager` | Coded leg refusal (`:320-337`); resolution, with the session kind, before the session id is stored (`:361-370`); warnings; `ready.stt` (`:776-790`); then the engine's two late refusals get the resolver's code (`:1988-1994`, `:2053-2057`) | Modified | 0; 1 |
| `handlers/ws/config_handler.rs`, `agent.rs` | `register_stt_error_callback` (`:2103-2134`); emitters (`:2077-2090`, `:1605-1617`, `:3477-3489`; `agent.rs:312-324`) | One `stt_unavailable` on a gateway-endpointed session; emitters call `send_transcript` | Modified | 1 |
| `handlers/ws/bud_legs.rs` | `LegRefusal` (`:55-61`) | Optional `details` | Modified | 0 |
| `handlers/ws/config.rs` | `STTWebSocketConfig` (`:382`) | `transcription_mode: Option<String>` | Modified | 2 |
| `handlers/openai_realtime/cascade.rs` | `error_kind` (`:238-248`), error mapping (`:1114-1130`), `session.update` (`:619-625`), `core` | Prefer the `code` field; four `server_error` codes; pass the manual flag to the resolver, refuse turning turn detection on; then `bud.session.stt` and `bud.session.warning` | Modified | 0; 1 |
| `handlers/openai_realtime/session.rs` | Handshake (`:374-376`), `not_found` (`:167-173`) | A session without a voice agent that names a file-only deployment is refused by name | Modified | 0 |
| `bud-auth/src/endpoint_config.rs` | `SttSettings`, `KNOWN_STT` (`:388`) | Deployment-level `transcription_mode` | Modified | 2 |
| `clients_sdk` (TypeScript, Python, widget, dashboard) | Config types, parsers, pipelines | The new field, `ready.stt`, two message types; stop gating audio when `endpointing` is `gateway`; Python stops sending `nova-3` for every provider (`python/bud_waav/ws/session.py:942`) | Modified | 2 |
| `gateway/docs/` | `segmented-stt.md`, `websocket.md`, `api-reference.md`, `openai-stt.md`, `openapi.yaml` | Customer documentation; regenerated specification | New, modified | 0 to 3 |

## What this part gives to and needs from the other parts

**Gives.** To every part: the code table and the wire shapes; no other part writes customer-visible text. To chapter 3 (capability map and resolver): `resolve_with_preference`, called from both resolution sites, and one wire reason per `SttLiveRefusal` reason. To chapter 4 (turn-taking): `SpeechEventEgress`, the `/ws` messages its Realtime translation consumes, and the response kind `notice`. To chapter 7 (cost, observability, tests and rollout): the codes its limiter raises.

**Needs.**

- From chapter 3: `ResolvedSttLive`, `SttLiveRefusal`, whether the rollout switch covers the session, the session kind, and the fact behind `stt_buffered_until_commit`. `SttLiveShared` holds this part's three process-wide settings; `SttLiveSession` carries `EndpointTuning`, which this part's settings mapping fills when there is no agent.
- From chapter 1 (the engine): the result contract; `SpeechActivity`, including whether a result follows a turn's close; `SttLiveFacts` (detector, `final_deadline_ms`); typed notices (`AudioDropped` becomes `stt_audio_dropped`; `NoiseThresholdRaised` is only logged and counted); and an error callback raised only when transcription stops.
- From chapter 4: each `SegmentOutcome` through the voice manager's outcome dispatcher, which accepts a registration made after the turn-taking loops are set up.
- From chapter 2 (the transcriber layer): typed planning warnings and the four session-ending error kinds.
- From chapter 6 (real-time performance): a latency estimate with a typical value, a slow value and a basis.
- From chapter 7: the golden recordings (recorded message lists of today's streaming sessions), and the confirming test that every `audio_end` returns a transcript on the OpenAI and Groq clients, which clear their callbacks on disconnect (`core/stt/openai/client.rs:902-903`, `core/stt/groq/client.rs:1114-1115`).

## Tests to write first

All are library tests that run without detector features. "Chain" is an in-process chain of a scripted client, the real handler and a fake transcriber. Criteria are the brief's, as corrected: 1, ordered transcripts and exactly one end-of-turn result per turn; 3, a transcript per utterance and a reply on `/ws` and `/v1/realtime`; 5, streaming models take today's path: byte-identical where the switch does not cover the session; where it covers, only the additive `stt` key in `ready` differs. The only other changes are flagged: transcript-derived speech events and the greeting fix in Release 2, and Cartesia's gateway-driven finalize (`WAAV_STT_CARTESIA_MANUAL_FINALIZE`) in Release 4. "One interface" is the planning goal that client code never depends on the provider or model.

| Test name | Level | What it proves | Acceptance criterion | Release |
| --- | --- | --- | --- | --- |
| `the_recorded_streaming_baseline_matches_this_build` | Chain | The harness reproduces today's bytes | 5 | 0 |
| `an_uncoded_error_serializes_exactly_as_before` | Unit | The new error fields change no uncoded error | 5 | 0 |
| `a_voice_agent_with_automatic_turn_detection_on_a_buffering_model_is_refused_and_every_other_session_is_warned` | Chain | Switch off: such an agent is refused; plain, conversation-loop and manual-mode sessions get today's messages plus `stt_buffered_until_commit`, and `audio_end` still returns text | The brief's plain refusal until this ships | 0 |
| `with_the_rollout_switch_off_a_streaming_session_gets_no_ready_stt_and_no_new_message` | Chain | An uncovered streaming session is today's, byte for byte | 5 | 1 |
| `a_streaming_session_emits_the_recorded_messages_plus_ready_stt` | Chain | A covered streaming session differs by one key | 5 | 1 |
| `the_same_config_starts_a_session_for_every_example_model` | Unit | One configuration for Deepgram, ElevenLabs, OpenAI and self-hosted models | One interface; 3 | 1 |
| `one_scripted_client_sees_the_same_message_types_and_turn_text_on_every_wire_family` | Chain | The central promise, for each wire family (one vendor request format, such as OpenAI-compatible multipart) | One interface | 1; rows added in 3, 4, 5 |
| `a_segmented_turn_sends_cumulative_interims_then_one_final_end_of_turn_result` | Chain | The result sequence | 1; 3 | 1 |
| `a_second_segment_returning_after_600_ms_does_not_split_the_turn` | Chain | The transcript timers never arm | 1 | 1 |
| `a_lost_segment_sends_stt_warning_and_no_error_through_the_real_error_callback` | Chain | A lost utterance is not a lost call | 3 | 1 |
| `an_explicit_commit_of_short_or_unconfirmed_audio_still_returns_exactly_one_answer` | Chain | The commit rule | 3 | 1 |
| `no_marker_for_lost_words_appears_in_any_client_transcript` | Chain | The gateway adds no text | One interface | 1 |
| `ready_stt_becomes_one_bud_session_stt_event_on_a_gateway_endpointed_session` | Unit | The Realtime surface is told | 3 | 1 |
| `streaming_preference_on_a_file_only_model_is_refused_naming_the_model_and_the_fix` | Unit | Only the strict preference refuses | One interface | 2 |
| `transcript_derived_speech_events_need_their_flag` | Chain | The streaming-session exception is switchable | 5 | 2 |
| `the pipeline stops gating audio when endpointing is gateway` | Unit (TypeScript) | The SDK follows the report, not a provider name | One interface | 2 |

Socket tests (integration binary `segmented_stt_ws`) and live tests are in chapter 7 and assert these codes.

## What ships in which release

- **Release 0, groundwork and honest refusal.** The test harness and recorded baseline; `code`, `recoverable` and `details` on `error`; the uncovered-session rule (refusal, warning, rejected `session.update`); `stt_live_unsupported`, on every session for OpenAI's live-only models and Viettel, and on `/v1/realtime` without a voice agent. Refused: what the map records as unable to work today, plus `/ws` agents with automatic turns on a buffering model, by choice.
- **Release 1, first working calls.** `ready.stt` and its notices (`stt_language_unset` among them) on covered sessions; the setup warnings; `stt_warning`; `stt_unavailable`, also for a mistyped model (`model_not_served`) until the setup probe; `stt_segmentation_unavailable`; detector-timed `vad_event`; the commit rule; the settings mapping; `bud.session.stt` and `bud.session.warning`.
- **Release 2, dark launch complete.** The `transcription_mode` preference with the SDK updates (together, or the SDK drift test fails, `clients_sdk/typescript/tests/unit/config-drift.test.ts:1-20`); the two flagged streaming changes, with release notes; measured latency, `stt_latency_slow`; `stt_capacity_low`; a language agreed per session, then pinned; the setup probe's refusal.
- **Release 3, default on.** `ready.stt` on every session, with a release note; refusals for a silently substituted file-only model (AssemblyAI `universal-2`, Gladia `solaria-3`), a batch-only language (`language_not_live`), and Rev AI `human`, Sarvam `saaras:v2.5` and Yandex `deferred*`, whose behaviour today is unknown; `stt_overloaded`; Deepgram and AssemblyAI file routes join the same-request test; Bud documentation.
- **Release 4, live-only models and low latency.** `endpointing: "gateway"` on a streaming transport, so OpenAI's live-only models stop being refused, and Azure OpenAI's after a live probe; `latency_tier: "low_latency"` on a deployment reaches `gpt-transcribe` on OpenAI's socket; covered Cartesia sessions move to gateway-driven finalize behind its flag, with a release note.
- **Release 5, wider vendor coverage and hardening.** `stt_fallback_engaged`; retention and region options documented; codes for the remaining construction failures.
- **Release 6, interruption recovery and interim text.** `agent_paused` and `agent_resumed`; re-decoded interim text reported as `live`.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| A deployed client breaks on the extra `ready` key or an unknown message type | Low | High | First-party clients ignore both (read in code); allow-list first; release note at Release 3 |
| A Realtime client library fails on a `bud.*` event | Unmeasured | Medium | Sent only on sessions that could not run before; `WAAV_REALTIME_BUD_SESSION_EVENTS` turns it off; check with real client libraries |
| A `/ws` agent with automatic turn detection whose client sends `audio_end` itself may get replies today (`handlers/ws/audio_handler.rs:301-322`) and is refused | Low | Medium | Sign-off before Release 0; the refusal names a remedy; `WAAV_STT_FILE_ONLY_REFUSAL` (chapter 3) withdraws it, keeping the warning |
| A slow client loses a whole turn's text, because transcripts are shed after 500 ms (`handlers/ws/messages.rs:520`, `:570-582`) | Low | Medium | Counted; turn-taking still receives the text; documented |

## What is still open

**Measurements needed.**

- End of speech to text for each file model, with medians; until then the report shows seeds.
- How often a commit of unconfirmed audio returns invented text. Default until measured: keep the rule, tighten the silence floor if needed.

**Choices that need a person, with the recommended default.**

- Refusing a `/ws` agent with automatic turn detection whose client sends `audio_end` itself: refuse.
- The 1,500 ms ceiling against the agent contract's published 3,000 ms: 1,500 ms.
- A missing detector model: refuse, unless the operator sets `WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1`.
- No lost-words marker in customer text: none.
- Transcript-derived speech events on streaming sessions: on in Release 2.
- The protocol version stays `"1.0"`, against the comment at `handlers/ws/messages.rs:30-38`, as a bump makes every deployed TypeScript SDK warn on every session: yes.
- Whether `ready.stt` may name the vendor and model to a voice agent's end user: yes.
- Refusing new sessions on measured overload: refuse from Release 3.

## Where the detail is

In `docs/segmented-stt/`: `customer-contract-reference.md`, `INTEGRATION_DECISIONS.md` (Addenda A and B win over this chapter), `capability-map/EXPECTED_RESOLUTION.md`.
