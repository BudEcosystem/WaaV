# Segmented speech-to-text: implementation status

What of `SEGMENTED_STT_PLAN.md` is built, where, how it was verified, and what is not built yet.
Branch `feat/segmented-stt` (WaaV) and the Bud-side commits on `claude/streaming-update-plan-analysis-3b232f`
(bud-runtime) and `feat/scribe-v2-realtime` (BudModelCatalog-SDK).

## Where the code is

| Part | Code |
| --- | --- |
| Engine: front end, detectors, segmenter, end-of-turn ladder, sequencer, result contract | `segmented-stt/src/{audio,detector,segmenter,endpointer,sequencer,turn,engine}.rs` |
| Transcriber layer: attempt loop, limiter, long-window budgets, upload breaker, quality filter, setup probe, language vote | `segmented-stt/src/transcriber/{attempts,gate,breaker,quality,probe,language_vote}.rs` |
| Wire families (file uploads) | `segmented-stt/src/transcriber/wire/{openai_compat,elevenlabs,deepgram,assemblyai,azure_fast,google_recognize}.rs` |
| Commit transports (vendor sockets the gateway commits on) | `segmented-stt/src/transcriber/wire/{openai_realtime,cartesia_finalize}.rs` |
| Capability map, resolver, rollout switch, control record | `segmented-stt/src/{map,resolve,rollout,control}.rs`, data `segmented-stt/data/stt_live_routing.json` |
| Fallback to a second vendor | `segmented-stt/src/fallback.rs`; targets, admission and metering in `gateway/src/core/stt/segmented/live.rs` and `gateway/src/handlers/ws/bud_legs.rs` |
| Data region and retention | `gateway/src/core/stt/data_settings.rs` (streaming and upload clients), `segmented-stt/src/live.rs` (`unapplied_data_settings`); vendor switches in `gateway/src/core/stt/{deepgram,elevenlabs,prerecorded}.rs` |
| Session planning (row to transcriber, profile, limits) | `segmented-stt/src/live.rs` |
| Gateway: the `BaseSTT` adapter, detector models, session resolution | `gateway/src/core/stt/segmented/{adapter,models,live}.rs` |
| Gateway: voice manager dispatch, turn signals, agent and conversation wiring | `gateway/src/core/voice_manager/{manager,segmented}.rs`, `gateway/src/core/turn/`, `gateway/src/handlers/ws/{agent,config_handler,processor,segmented_session,stt_contract}.rs` |
| `/v1/realtime` | `gateway/src/handlers/openai_realtime/{cascade,session,handshake}.rs` |
| Bud: capability record and `GET /capabilities/stt` | `gateway/src/handlers/stt_capability.rs` |
| budapp, budadmin, budplayground, chart | `services/budapp/budapp/endpoint_ops/{audio_config,voice_publisher,services,endpoint_routes}.py`, `services/budapp/budapp/prompt_ops/voice_agent.py`, `services/budadmin/src/pages/home/deployments/[slug]/settings/AudioSettings.tsx`, `services/budplayground/app/lib/realtime/client.ts`, `infra/charts/bud/values.yaml` |

## Switches

| Variable | Default | Effect |
| --- | --- | --- |
| `WAAV_SEGMENTED_STT` | `allowlist` | `off`, `allowlist` (with `WAAV_SEGMENTED_STT_ALLOWLIST`), `on` |
| `WAAV_STT_LIVE_RELEASE` | 6 | The release in force, 0 to 6 |
| `WAAV_STT_SEGMENT_ALLOW_ENERGY_DETECTOR` | off | Use the loudness detector when Silero cannot load |
| `WAAV_STT_FILE_ONLY_REFUSAL` | on | `off` withdraws the refusal of agents on buffering models (warning stays) |
| `WAAV_STT_SETUP_PROBE` | on | Probe guessed rows and self-hosted servers at setup |
| `WAAV_STT_COMMIT_TRANSPORT` | off | Build OpenAI's realtime-socket commit transport (set after a live probe) |
| `WAAV_STT_CARTESIA_MANUAL_FINALIZE` | off | Cartesia's gateway-driven finalize; the map must also record the probe |
| `WAAV_PER_UTTERANCE_INTERRUPTIBILITY` | `segmented` | The greeting fix: `segmented`, `all`, `off` |
| `WAAV_STT_REDECODE_INTERIMS` | on | Re-decode self-hosted models while the caller speaks (Release 6) |
| `WAAV_REALTIME_BUD_SESSION_EVENTS` | on | `bud.session.stt` and `bud.session.warning` on `/v1/realtime` |
| `stt.data_region`, `stt.data_retention` (deployment) | absent | EU processing, no retention; refused where the vendor cannot be asked |
| `fallback_models` (deployment, FRD-022) | absent | Fallback deployments, live calls and uploads |
| `waav:stt_live:control` (Redis) | absent | Control record: deployments and rows switched off at session start |

## By release

**Release 0** — done: test kit (fake transcriber, scripted detector, paused clock, mock vendors over real sockets);
the capability map, schema, resolver (12,822-case parity grid against `resolve.py`) and named refusals; the
B5 confirming tests and fix (a transcript at every `audio_end`, a failed Groq flush); the SmartTurn confirming
test (`packets_of_20_ms_never_accumulate_a_model_window`, not fixed, as planned); the greeting-defect
confirming test; Silero and SmartTurn baked into the image (`waav-gateway init`, `$CACHE_PATH/models`);
catalog `scribe_v2_realtime` (BudModelCatalog-SDK); playground setup-refusal message.
Golden recording of a streaming session (`gateway/tests/segmented_stt_golden.rs`): covered or not, it matches
the committed recording apart from `ready.stt`.

**Release 1** — done: the engine; OpenAI-compatible, ElevenLabs, Deepgram, AssemblyAI, Azure fast and Google
transcribers with one attempt loop, breakers, limiter and quality filter; both `bud_legs` refusal sites on
the resolver; `ready.stt`, notices, `stt_warning`, coded errors, `vad_event`; voice agents with speech-time
admission, detector barge-in, echo and backchannel filtering, the lost-turn rule, the commit wait; uploaded-
seconds metering; budapp accepts `stt.segmented`, `stt.capability_override`, `stt.expected_languages`; the
dated B6 change (`gpt-transcribe` default with `languages[]`/`keywords[]` on every path); the greeting fix on
segmented sessions.

**Release 2** — done: setup probe and cache (`model_not_served`, `stt_fields_reduced`); per-deployment
capability record `voice_capability:{id}` and `GET /capabilities/stt`; budapp reads it (agent save-time
check, `GET /endpoints/{id}/audio-stt-capability`); conversation loop and `/v1/realtime` timed by the
detector (findings 3 and 4: detector barge-in, deltas from each segment's text); control record; hourly and
daily budgets with admission (`stt_capacity_low`, `stt_overloaded` from Release 3), `stt_rate_limited`,
`stt_degraded`; latency store fed by measurements; the language vote; the `transcription_mode` field; the
greeting fix for streaming sessions behind its flag; the dropped-frame rule test; SDK updates (TypeScript,
Python).
Not done: benchmark harness with live cells; LiveKit chain test and the live SIP call (exit criteria that
need a live room and a phone line).

**Release 3** — done: AssemblyAI synchronous and Deepgram hosted Whisper adapters; the "slower on calls"
flag in the capability record and budapp's API.
budadmin shows a Live calls card with the Slower on calls badge on a transcription deployment's Audio
settings page.
Not done: switching the default to `on` (a rollout decision, after the Release 2 exit measurements); the
seven analytics columns (budmetrics); drift and live-probe jobs; rewiring the OpenAI and Groq clients onto
the shared code and removing their unreachable `OnSilence` strategy (it changes the REST route's error
classes and needs sign-off).

**Release 4** — done: the commit transport on OpenAI's realtime transcription socket (live-only models, and
`gpt-transcribe` on the low-latency tier) and Cartesia's gateway-driven finalize, both behind switches until a
live probe; the hedge and the low-latency tier.

**Release 5** — done: the vendor minimum in price (each outcome's billed seconds follow the row's minimum
and increment; from Release 5 a unit is priced at that).

Fallback to a second vendor. The list is the deployment's `fallback_models` (FRD-022), the same list
`/v1/audio/transcriptions` already walks; no new setting. A unit the active vendor loses for a vendor-side
reason (5xx, a network or protocol failure, 401/403, a model not served, 429, an open breaker, a spent
budget, a session already ended by a fatal answer) goes again to the next fallback while its deadline
allows. The session stays there; vendors are not alternated turn by turn. `stt_fallback_engaged` is
sent once. A loss the next vendor would share (a refused request, a unit outside the vendor's bounds) or
a single timeout moves nothing; a run of timeouts opens the breaker, which does.

Each fallback has the following properties:
- It is resolved through the capability map in segmented mode, with this build's adapters.
- It is skipped if it cannot carry the data settings of both deployments.
- It is admitted against its own limits when it first serves, and the admission is held for the call.
- The units it serves are billed by its row's rule, at its own price, and metered on its deployment
  (`served_by` on the outcome).

Retention and region options. The deployment settings are `stt.data_region: eu` and
`stt.data_retention: none` (`vendor_default` or absent means today). They reach the streaming socket,
the upload client and the segmented engine:
- Deepgram: EU host, and `mip_opt_out`.
- ElevenLabs: EU residency host, and `enable_logging=false`.
- AssemblyAI uploads: EU host.
- OpenAI uploads on the segmented path: `eu.api.openai.com`.
- A deployment's own address (self-hosted, a resource endpoint): that address decides.

A vendor whose client cannot carry a setting is never used for that deployment. The live session is
refused at setup, and the upload before any vendor call, with `stt_data_setting_unavailable` and
details `{provider, settings: [{setting, reason}]}`. budapp accepts both settings, and budadmin shows
them as a "Data handling" card.

**Release 6** — done: interim text by re-decoding for self-hosted models (a second, interim-only entry on
the attempt loop; stale answers dropped at each cut; `interim_results: live`). Not done: pause, then commit
or resume after a false interruption — the contract reserves `agent_paused` and `agent_resumed` but defines
neither them nor how a client declares support, and launching without it is a sign-off item of the plan.

## Defects found while building it

| Defect | Where | Status |
| --- | --- | --- |
| Silero v5 got each 512-sample chunk without the 64-sample context the model reads; real speech scored at most 0.011. Affects the turn-detection ensemble too | `core/silero_vad/detector.rs` | Fixed, unit and live tests |
| OpenAI and Groq clients lost their callbacks at the first `audio_end`; a failed Groq flush left the session disconnected and re-uploaded the lost turn | `core/voice_manager/manager.rs`, `core/stt/groq/client.rs` | Fixed (B5) |
| A reply after a protected greeting could never be cleared | `core/voice_manager/{manager,state}.rs` | Fixed behind `WAAV_PER_UTTERANCE_INTERRUPTIBILITY` |
| A `/ws` client could name a private `base_url` for uploads (SSRF) | `segmented-stt/src/transcriber/http.rs` | Fixed: public-only resolver, plan-time check |
| Two deployments behind one host on different paths shared a setup-probe verdict and a breaker: a healthy one's cached "served" hid an outage at the other (found by the Bud-mode end-to-end run) | `gateway/src/core/stt/segmented/live.rs` | Fixed: keyed by the target's address, tagged; unit test |
| Continuous SmartTurn never runs on live packets | `core/smart_turn/mel_extractor.rs` | Confirmed by test, not fixed (plan) |
| `today's refusal text` for an unsupported deployment carries an 18-space run | `handlers/ws/bud_legs.rs` | Kept byte-identical for uncovered sessions |

## Verification

- `segmented-stt`: `cargo test` (the engine scenarios, every wire family against a mock vendor, both
  commit transports against mock sockets, the resolver parity grid), `cargo clippy --all-targets -D warnings`.
- Gateway: `cargo test --lib --features silero-vad,smart-turn,turn-detect` (all pass; ONNX tests need
  `ORT_DYLIB_PATH`), `cargo test --features dag-routing --test segmented_stt_ws` (a real `/ws` socket against
  a mock file-only vendor), clippy clean except a pre-existing lint in `core/stt/iflytek/client.rs`.
- End to end, standalone (38 checks): the gateway binary with the real Silero and SmartTurn models,
  recorded speech streamed at real time over `/ws`, and an OpenAI-compatible mock vendor (file uploads,
  realtime socket, chat, speech). Eight scenarios:
  - two turns with one upload and one final each (end of speech to final about 0.6 s);
  - a commit mid-speech;
  - a coded refusal, then a corrected config;
  - a plain session on today's buffering client, with a transcript at every `audio_end` and
    `gpt-transcribe` fields;
  - a conversation loop whose reply stops about 0.44 s into the caller's speech (detector barge-in; a
    control run without barge-in delivers 15 s more of reply);
  - a live-only model on the realtime socket (about 0.43 s to final);
  - a mistyped model refused by the setup probe;
  - a no-retention request refused before any upload.
- End to end, Bud mode (14 checks, run twice on one gateway process): the binary reads its deployments
  from a Redis control plane, the caller uses a Bud key, and vendor credentials are RSA-encrypted. Four
  scenarios:
  - a deployment whose vendor is down hands the call to its fallback deployment: every utterance is
    transcribed, `stt_fallback_engaged` is sent once, and each unit is metered with the fallback as
    `served_endpoint_id`;
  - a deployment asking for no retention on OpenAI is refused live and on `/v1/audio/transcriptions`
    before any upload;
  - a self-hosted deployment returns re-decoded live interims (growing partial clips, then one final).
- budapp `tests/test_audio_config.py`, `test_voice_agent_config.py`, `test_audio_voices.py`; budadmin
  (full suite); budplayground `app/lib/realtime`; chart `tests/test_waav_segmented_stt_chart.py`;
  BudModelCatalog-SDK scraper tests; the TypeScript (328) and Python (444) SDK unit suites, green apart
  from three config-drift guard failures that predate this work (the `agent` envelope key and two stale
  required-field sets, from the voice-agent and per-deployment config commits).
