# Chapter 3. The capability map, the resolver and Bud deployment integration

## What this part is and why it exists

Some speech-to-text models accept a live audio stream; others accept only a complete file. Today the gateway tells them apart by vendor name in two places and gets it wrong three ways: a self-hosted or Azure OpenAI deployment is refused, an ElevenLabs file model fails after admission, and an OpenAI or Groq file model under a voice agent returns no text until the client sends `audio_end` or hangs up. This part replaces those guesses with one table, the **capability map** (keyed by provider and model, it says how each model can be reached during a call), and one pure function, the **resolver**, which turns a session's provider, model and deployment into one answer that every other part reads.

Without it, each part would decide for itself whether a model streams, and they would disagree. Customers would need to know which models need the new segmenting engine; a wrong entry would make a dead call, not a named refusal; Bud's control plane (budapp) could not flag a deployment as slower on calls; and a streaming model could be rerouted by accident.

## How it works

**The map.** Its sources, already in schema version 2, are in `docs/segmented-stt/capability-map/`: a JSON file per provider in `rows/`, transport profiles a deployment may select (`profiles.json`), version and latency classes (`meta.json`). They stay in the repository with every source and note; the gateway embeds only the routing fields (those the resolver reads) generated from them, and validates them at start-up. A row matches a model id or a pattern. It carries a stable identifier, what happens when no transport is usable (with today's behaviour, read from the code), and ordered **transports** (a live socket, a socket the gateway commits on, or a file upload), each naming its code path (the **adapter**; `native` is today's client, unchanged), the release that enables it, limits, billing, dialect and latency seed. `resolve.py`, the reference resolver, is the executable specification of the Rust one; it checks lifecycle dates against the run date.

**The resolver.** It runs once per session, before anything is admitted or stored, so after a refusal the socket can take a corrected configuration. A session is **covered** when the build is at Release 1 or later and the rollout switch (`WAAV_SEGMENTED_STT`: `off`, `allowlist` or `on`), the control record (a Redis key that can switch a row or deployment off on a running gateway, from Release 2) and the deployment's own setting all permit the engine. Sessions are of three kinds: a voice agent with automatic turn detection; any other session with an agent, a conversation loop or a DAG pipeline; and a **plain** `/ws` session with none of these, whose client consumes transcripts itself.

| Step | What is decided |
| --- | --- |
| 1. Normalise | Provider aliases; the model string looked up and the one sent, which are the same or are rewritten together |
| 2. Find the row | Deployment override, then exact model, pattern, provider default, global default. A session naming no model gets the provider's declared answer |
| 3. Choose a transport | The first one enabled in this release whose adapter is built, whose language and region constraints admit the session, and which is today's client or is covered. From Release 4, `latency_tier: low_latency` with preference `auto` prefers a usable commit transport to a file transport, which stays the fallback |
| 4. Buffering clients | Where today's only client buffers until hang-up (OpenAI and Groq file models; Bhashini, FPT.AI, NAVER CLOVA `csr`, NECTEC `partii4`) and the engine does not take the session, a voice agent with automatic turns is refused by name and any other session keeps today's client with the warning `stt_buffered_until_commit`. A plain session (today one transcript per `audio_end`) keeps today's client even when covered, unless it sets `transcription_mode: segmented` (wire field from Release 2) |
| 5. Early refusals | No usable transport; retired model; detector model missing; audio format the engine cannot decode; base address or credential missing |
| 6. Finish | Trusted base address, features the adapter can apply, capacity estimate, warnings, then the language in the row's form, always sent when known; with no language or `auto`, the notice `stt_language_unset` |

**Refusal reasons.** Rows store `client_not_implemented`, never `not_covered_yet`; the resolver chooses at run time. It sends `not_covered_yet` only when the rollout switch does not cover the session and a covered session would get a transport in this release; only that text may say to ask the operator. Otherwise `client_not_implemented` says segmented speech-to-text for this provider is not available in this release; the map says whether a later release adds a client.

**The withdrawal switch.** `WAAV_STT_FILE_ONLY_REFUSAL` (default `on`) is read at start-up. With `off`, every session Release 0 newly refuses takes today's path again, keeping the warning `stt_buffered_until_commit`; sessions refused before Release 0 (self-hosted and Azure OpenAI deployments with today's two codes) stay refused. Release 3 removes it.

From Release 2 a **setup probe** (one real request with a silent clip during setup, verdict cached) checks a guessed row or a self-hosted server before the caller speaks.

**The hand-off.** `ResolvedSttLive` travels in the per-session `SttLiveSession`, beside the process-wide `SttLiveShared`, in the voice manager's configuration. A third factory, `create_stt_standard_live`, returns today's client for a native answer and otherwise calls `SegmentedStt::new_live(shared, session)`.

**Bud integration.** Both hard-coded refusals in `bud_legs.rs` become a resolver call. A deployment's base address travels in a type no client message can produce; its address rules apply at each connection. Two optional settings blocks carry tuning, a declared profile and the latency tier (`standard` by default, or `low_latency`). A background publisher writes each deployment's answer to Redis for budapp.

**Worked example.** A `/ws` voice agent (semantic turn detection) uses the deployment `stt-prod`: vendor `openai`, model `gpt-transcribe`.

- Today: no text unless the client sends `audio_end`. Release 0: refused before admission (`stt_live_unsupported`, reason `client_not_implemented`); the message offers a streaming model or manual turn detection.
- Release 1: off the allow-list, reason `not_covered_yet`; on it, adapter `openai_transcriptions`, transport `file_upload`, latency class `unknown`, and the call works.
- A Deepgram `nova-3` deployment gets today's client, byte for byte, in every release.

**Key numbers.**

| Number | Value | Source |
| --- | --- | --- |
| Map sources | 35 files, 412 rows, 12 transport profiles | `validate_rows.py --all`, 2026-10-04 |
| Embedded routing fields; resolver | about 1.1 MB before compression, parsed in under 10 ms; under 50 microseconds per session, no I/O | Estimates; measured in Release 0 |
| Latency seeds (99th percentile, end of speech to final) | 2,010 ms `gpt-4o-mini-transcribe` and `scribe_v2`; 1,540 ms Groq; 650 ms AssemblyAI synchronous; `gpt-transcribe` unmeasured | Third-party benchmark, fact-checked |
| Latency classes | realtime up to 600 ms, fast up to 1,200 ms | `meta.json`; to be recalibrated |
| Upload deadline | 6,000 ms, or 3,000 to 10,000 per deployment; raised here to the silence ceiling (1,500 ms default) plus 2,500 when lower, the only limit this part changes | Integration decisions 2 and 3, addendum A5 |
| Publisher | checks every 30 s, rewrites an unchanged record every 600 s, expiry 1,800 s | `handlers/voice_catalog.rs:33-37` |

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
| --- | --- | --- | --- |
| Form of the map | JSON sources per provider in the repository; the gateway embeds routing fields generated from them; closed vocabularies are Rust enums | Edited by people and jobs that read vendor pages; one set of sources serves gateway, Bud and tooling | Not checked by the compiler; start-up check, tests, staleness check |
| Unknown model id | Per provider: file upload where a file endpoint and adapter exist, today's client elsewhere | A wrong "file" guess is a slower call; a wrong "stream" guess is a dead call | A mistyped id is caught at setup only from Release 2 |
| A session the engine does not cover | On a buffering client, refuse only a voice agent with automatic turns; warn every other session | On `/v1/realtime` only a manual-mode commit ends a turn (`cascade.rs:415-423`); on `/ws` the client's `audio_end` can (`handlers/ws/audio_handler.rs:301-322`), so refusing there is a choice (plan sign-off item 3) | A `/ws` agent sending `audio_end` itself is refused; the withdrawal switch undoes it |
| Where refusals happen | In the resolver, before admission and before the session id is stored | The client can correct its configuration on the same socket; no slot is taken | The request must carry session kind, turn mode, audio format and language |
| Silero detector model cannot be loaded | Refuse, unless the operator sets `WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR=1` for the simpler energy detector | The fallback is materially worse | The production image must carry the model files |
| Trusted base address | A type with no deserializer; address rules applied at each connection | A check at setup does not protect later connections | A custom name resolver inside the HTTP client |
| Models today's client silently replaces | Logged and counted in Release 0 and when uncovered; a `ready.stt` notice when covered; from Release 3 a notice everywhere, or a refusal where the vendor never serves the model | No working call is broken on documentation alone | The named model is not the one running until Release 3 |
| How Bud learns the answer | One Redis record per deployment, plus a static route | budapp cannot call the gateway | Up to 30 s stale; two new key families need approval |

## What changes in the code

Paths are under `/home/bud/ditto/waav/WaaV/`.

| File | Function or type | Change | New or modified | Release |
| --- | --- | --- | --- | --- |
| `gateway/src/core/stt/capability/` | `resolve_stt_live`, `ResolvedSttLive`, `SttLiveRefusal`, `SttCapabilityRuntime` | Loader, validator, resolver; the runtime value holds the rollout and withdrawal switches | New | 0 |
| `gateway/src/core/stt/capability/routing_map.json` (from `assemble.py --routing`), `expected/release-<n>.json` (from `resolve.py --expected-json`); `docs/segmented-stt/capability-map/assemble.py`, `resolve.py` | Routing fields; expected resolutions as JSON | `assemble.py` writes the routing fields, `resolve.py` the expected tables; never edited by hand. Continuous integration runs `validate_rows.py --all`, `resolve.py --self-test` and `--check-release-0`, and fails on a stale generated file or table (`--expected`, `--matrix`) | Files new; scripts modified | 0, then each release |
| `gateway/src/state/mod.rs:302` | `AppState::try_new` | Holds the runtime value, read from the environment | Modified | 0 |
| `gateway/src/handlers/ws/config_handler.rs:161`, `:361-366`, `:1804`, `:1945-1983` | `handle_config_message`, `initialize_voice_manager`, `voice_manager_config_for` | Release 0: works out session kind and turn mode, resolves, refuses before the session id is stored. Release 1: resolution required; no second detector on a segmented session | Modified | 0, 1 |
| `gateway/src/handlers/ws/bud_legs.rs:93-134`, `:216-252`, `:654-664`, `:787-826` | `resolve_leg`, `resolve_agent_leg`, `LegRefusal`, `apply_stt`, `PreparedLegs`, `LegMeter` | Release 0: resolver beside the vendor-name tests. Release 1: those tests deleted; model and feature rules; resolution stored; deployment-changed mark | Modified | 0, 1 |
| `gateway/src/handlers/openai_realtime/session.rs:373-389` | `prepare` | Names the refusal for a file-only deployment used without a voice agent | Modified | 0 |
| `gateway/src/handlers/openai_realtime/cascade.rs:619-625`, `:1307-1321` | Cascade setup, `session.update` | Passes its manual flag to the resolver; rejects an update to automatic turns on an uncovered buffering model | Modified | 0 |
| `gateway/src/core/stt/capability/handoff.rs`, `trust.rs` | `SttLiveShared`, `SttLiveSession`, `EndpointTuning`, `TrustedSttEndpoint`, `egress_client_builder` | Hand-off types; trusted base and connect-time address rules | New | 1 |
| `gateway/src/core/stt/standard.rs` after `:523` | `create_stt_standard_live` | Third factory; existing two untouched | New | 1 |
| `gateway/src/core/voice_manager/config.rs:47`, `manager.rs:197-205` | `VoiceManagerConfig`, `VoiceManager::new` | One optional field carrying both types; dispatcher in the outcome-sink slot; three-arm match | Modified | 1 |
| `bud-auth/src/endpoint_config.rs:123-177`, `:388-410` | `SttSettings`, `SttSegmented`, `SttCapabilityOverride` | Two tolerant settings blocks; `latency_tier` read from Release 4 | Modified | 1 |
| `gateway/src/core/stt/capability/probe.rs`, `summary.rs` | `SetupProbe`, reduced view | Probe and verdict cache; view shared by route and record | New | 2 |
| `gateway/src/handlers/stt_capability.rs`, `routes/api.rs:126-135` | Publisher, `GET /capabilities/stt` | Per-deployment records; static matrix | New | 2 |
| `gateway/src/core/alias/mod.rs` | `AliasDefinition` | Server-side base for standalone self-hosted use | Modified | 2 |
| `gateway/scripts/stt_capability/drift_check.py`, `probe.py`, two workflows | Scheduled jobs | Documentation drift and live probes | New | 3 |

## What this part gives to and needs from the other parts

**Gives.**

- `ResolvedSttLive`: transport, adapter, engine, model and language to send, endpointing owner, interim kind, latency class and seed, limits, dialect, billing, segment profile, lifecycle, matched layer and stable row identifier.
- `SttLiveShared` and `SttLiveSession` with the members of integration decision 4 and addendum A3, including `EndpointTuning` and the outcome-sink slot the dispatcher fills before the factory runs.
- Refusal kinds and codes (`stt_live_unsupported`, `stt_not_streaming`, `stt_model_retired`, `stt_segmentation_unavailable`, `deployment_misconfigured`, `stt_overloaded`), the reason chosen for the session, and coded warnings marked "frame" or "ready only", all quoted from the customer-contract chapter, their single source.
- The withdrawal switch; the trusted base and the only sanctioned HTTP client builder; the per-deployment record and static matrix for Bud and SDKs.

**Needs.**

- Segmenter and adapter: `SegmentedStt::new_live(shared, session)`, the detector state, a pure "can decode this encoding" check.
- Transcriber layer: an adapter per id with its features, upload clients, file-breaker registry, drop-a-refused-field-and-retry, a "model not served" error (Release 1), the setup probe request (Release 2).
- Customer contract: `resolve_with_preference`, the `transcription_mode` wire field (Release 2), code strings, `ready.stt`, the plain-session mapping to `EndpointTuning`.
- Turn-taking: the agent mapping to `EndpointTuning`; the dispatcher that fans out `SegmentOutcome`.
- Real-time performance: the latency store and `segment_deadlines`, the only producer of time limits.
- Cost, observability, tests and rollout: rollout switch, control record, the limiter's overload flag, metering sink, Silero model files in the image, golden recordings.

## Tests to write first

All are library tests that need no detector model files, network or vendor key; chain tests use the gateway's `test-util` feature.

| Test name | Level | What it proves | Acceptance criterion | Release |
| --- | --- | --- | --- | --- |
| `the_shipped_map_resolves_as_the_release_table_says` | unit | The expected answer for every row, session kind and release | Mapper configuration | 0 |
| `release_0_refuses_only_what_the_map_justifies` | unit | The twin of `resolve.py --check-release-0` | Plain-terms refusal | 0 |
| `every_model_that_streams_today_resolves_to_the_native_engine` | unit | Config untouched in every release, except Cartesia from Release 4 with its flag on | Streaming unchanged | 0 |
| `a_streaming_session_receives_no_new_frame_before_ready` | unit | No warning frame on today's path | Streaming unchanged | 0 |
| `a_session_the_engine_does_not_cover_takes_todays_path_or_a_named_refusal` | unit | Switch off: today's path or today's two refusal codes | Streaming unchanged | 0 |
| `a_voice_agent_with_automatic_turns_on_a_buffering_client_is_refused_by_name` | unit | The six buffering clients of step 4 refuse it; Viettel and NECTEC ids other than `partii4` refuse every session | Plain-terms refusal | 0 |
| `the_refusal_reason_says_what_is_true_for_this_session` | unit | `not_covered_yet` only when the switch is the cause; only its text sends the customer to the operator | Plain-terms refusal | 0 |
| `the_withdrawal_switch_restores_todays_behaviour_for_the_newly_refused_sessions_only` | unit | Switch `off`: an OpenAI agent with automatic turns starts as today; a self-hosted deployment stays refused | Streaming unchanged | 0 |
| `every_other_session_on_a_buffering_client_keeps_todays_path_with_one_warning` | unit | Uncovered sessions of the other kinds, and covered plain ones not asking for `segmented`: today's client, warned | Streaming unchanged | 0 |
| `a_voice_agent_on_an_openai_file_model_is_refused_by_name` | in-process chain | Refused before admission; the slot is still free | Plain-terms refusal | 0 |
| `a_realtime_agent_in_manual_mode_cannot_turn_on_turn_detection_on_a_buffering_model` | in-process chain | The update is rejected; the session stays manual | Plain-terms refusal | 0 |
| `a_standalone_session_with_no_live_path_gets_the_coded_error_and_can_send_a_corrected_config` | in-process chain | Refusal leaves the socket usable | Plain-terms refusal | 0 |
| `a_file_only_model_resolves_to_the_segmented_engine` | unit | The Release 1 providers resolve segmented when covered | File-only models work on a call | 1 |
| `the_live_factory_returns_the_same_client_as_today_for_a_native_resolution` | unit | The new factory changes nothing for streaming | Streaming unchanged | 1 |
| `a_bud_session_on_a_file_only_deployment_reaches_the_voice_manager_as_segmented` | in-process chain | ElevenLabs `scribe_v2` and an OpenAI file model build the engine through the real handler | File-only models work on a call | 1 |
| `a_voice_agent_on_a_file_only_deployment_builds_one_detector_and_carries_its_endpoint_tuning` | in-process chain | One detector; a default `max_endpointing_ms` (3,000) counts as not set, so a 1,500 ms ceiling travels | File-only models work on a call | 1 |
| `a_missing_detector_model_refuses_unless_the_operator_allows_the_energy_detector` | unit | The early refusal and its switch | Plain-terms refusal | 1 |
| `a_name_that_moves_to_a_refused_address_after_the_check_is_not_dialled` | unit | Address rules hold at connect time | Bud integration: trusted base | 1 |
| `the_resolved_model_is_the_model_that_is_sent` | unit | Row applied and request sent cannot disagree | One interface for every model | 1 |
| `an_unknown_id_that_the_file_endpoint_rejects_is_refused_at_setup_not_mid_call` | unit | The probe turns a dead call into a refusal | One interface for every model | 2 |
| `the_published_record_equals_the_session_resolution` | unit | Bud sees what a session would get | Capability exposed per deployment | 2 |
| `a_low_latency_deployment_reaches_the_commit_socket_with_upload_as_fallback` | unit | `gpt-transcribe` with `low_latency`: the socket; with `standard`: the upload | One interface for every model | 4 |

## What ships in which release

- **Release 0, groundwork and honest refusal.** The generated routing file; loader, validator and resolver, used only to refuse and warn, at both `bud_legs` sites, on plain `/ws` and in the `/v1/realtime` cascade. It refuses sessions the map records as unable to work today, plus `/ws` voice agents with automatic turns on a buffering model, by choice. Named refusals: ElevenLabs file models, Deepgram `whisper*`, OpenAI live-only models, Viettel (its client posts to a retired domain), NECTEC ids other than `partii4`, and `/v1/realtime` without a voice agent. The withdrawal switch; counters; continuous-integration steps.
- **Release 1, first working calls.** Rows enabled for OpenAI file models, Groq, ElevenLabs `scribe_v2` and `scribe_v2_medical`, self-hosted servers, WaaV Infer and Azure OpenAI; the allow-list; the `transcription_mode` preference, no caller until Release 2; hand-off types and factory; vendor-name tests deleted; the trusted base; the static deployment override; early detector and audio-format refusals; `gpt-transcribe` as the map's OpenAI default (the client's own default changes under Addendum B6).
- **Release 2, dark launch complete.** The setup probe and cache; the Redis records and `GET /capabilities/stt`; the control record; the Azure OpenAI capacity warning `stt_capacity_low`; four more self-hosted profiles; the alias block for standalone self-hosted use.
- **Release 3, default on.** AssemblyAI synchronous and Deepgram hosted Whisper rows; refusal of models and languages only an asynchronous job serves, and of rows whose behaviour today is unknown (Rev AI `human`, Sarvam `saaras:v2.5`, Yandex `deferred*`), on today's client until then; substituted-model notices everywhere; the overload refusal; drift and live-probe jobs; Bud reads the records.
- **Release 4, live-only models and low latency.** Socket rows for the live-only models of OpenAI and Azure OpenAI (Azure address, `api-key` header), each after a live probe; Cartesia's finalize transport replacing today's client behind `WAAV_STT_CARTESIA_MANUAL_FINALIZE` (off until a live probe passes; a release note; the third deliberate change to streaming sessions); `latency_tier` read; the commit arm of the factory. It targets 2027-02-26; missing it takes nothing from customers.
- **Release 5, wider vendor coverage and hardening.** Third-wave rows after live probes (four need new request builders; Phonexia on demand; Viettel only if measured faster than the 15 to 30 s observed); self-hosted socket profiles on demand; fallback resolution; retention and region options; a hosted deployment's base address; re-resolving between turns.
- **Release 6, interruption recovery and interim text.** The deployment's interim-text setting on self-hosted profiles.
- **Refused in every release**, besides retired models and those only a slow job serves, with the row's reason: NAVER CLOVA Speech short, long and streaming; Huawei `chinese_16k_conversation`; Speechmatics `linden-1`; self-hosted Kyutai models; Deepgram Flux; unknown ElevenLabs realtime ids; Rev AI `human`; Sarvam `saaras:v2.5`; each is a candidate for later work.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
| --- | --- | --- | --- |
| A streaming session changes behaviour | Low | Very high | Native answers pass the config untouched; the tests above; recordings of streaming sessions; no new frame before `ready`; Cartesia's change has its own flag and new recordings |
| The embedded routing file differs from the sources | Medium | Wrong routing | Generated, never edited; continuous integration fails when it is stale; `--check-release-0` |
| A wrong row makes a dead call | Medium | High | Fail slow by provider; pattern rows for live-only families; setup probe from Release 2; a per-utterance repair and a session-ending error in Release 1 |
| The Release 0 refusal breaks a working customer | Low | Medium | Only what the map records as failing today, plus `/ws` agents by choice; rows whose behaviour today is unknown wait until Release 3; withdrawal switch |
| A tenant steers a self-hosted base address at an internal service | Low | High | Metadata and link-local addresses refused at every connection; no redirects; operator allow-list if tenants can edit the address |
| budapp does not accept the two blocks by Release 1 | Medium | Azure OpenAI stays below one call; whisper.cpp unreachable | Gateway parses tolerantly; no deployment needs an override to work |
| The map goes stale | High over time | Wrong routing or missed removals | Lifecycle dates warn; only a person retires a row; drift and probe jobs; named owner |
| An air-gapped cluster lacks the detector model | Medium | Every segmented session refused | Model files baked into the image in Release 0; operator switch |

## What is still open

**Measurements needed.**

- Resolver time, map parse time and embedded size against their budgets (Release 0).
- Name lookup times in the production cluster (before Release 1).
- Setup probe duration and cost per vendor and server family (before Release 2).
- A live probe with a real key for every Release 4 socket row and every Release 5 vendor.

**Choices that need a person, with the recommended default.**

- OpenAI's model when none is named: `gpt-transcribe`.
- A `/ws` agent with automatic turns whose client sends `audio_end` itself: refuse (sign-off item 3).
- A missing detector model: refuse by default.
- Whether Bud matches on `unsupported_deployment` or `stt_not_streaming`: until answered, uncovered self-hosted and Azure OpenAI deployments keep them (addendum A1).
- A billed setup probe on hosted vendors for unknown ids, and up to 2 s of setup wait: accept.
- Who may edit a self-hosted deployment's base address in budapp: operators only.
- Two new Redis key families written by the gateway: approve.

## Where the detail is

- Map sources, validator, reference resolver and generated tables: `/home/bud/ditto/waav/WaaV/docs/segmented-stt/capability-map/`.
- Full design: `/home/bud/ditto/waav/research/segmented-stt/design/W3-capability-map-and-resolution.md`, with its code and adversarial critiques beside it; `INTEGRATION_DECISIONS.md` in this directory overrides all three.
