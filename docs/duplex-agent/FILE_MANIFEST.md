# File manifest: building the full-duplex interaction agent

Every file the plan creates or changes, merged from the "file-by-file change list" of the seven workstream designs
(`research/duplex-agent/design/W1-...md` to `W7-...md`, section 4) after Addenda A to E and the final review. Written
2026-10-05 against WaaV `main` at `1b26ec84`. No file here has been written yet.

How to read it:

- **New or modified** was decided by checking the repository on disk: a path that exists today is "modified". Paths
  are relative to the WaaV repository root, except `waav-infer/`, which is the separate WaaV Infer repository.
- **Workstreams**: Runtime (chapter 1), Front model (2), Tasks (3), Policy and HITL (4), Duplex audio (5), Providers
  and endpoints (6), Evaluation and rollout (7). A file touched by several workstreams lists each, and its change text
  joins their descriptions with " / ".
- **Releases**: Lifecycle track, R0 (measure and lay groundwork) to R5 (native duplex, conditional), Later. A file
  that lands in one release and grows in later ones lists every release.
- **Interaction-off safety**: every change to an existing file is additive, dark (not reached with the interaction
  mode off) or behind its own escape flag; golden wire traces prove mode-off sessions unchanged.
- Corrections from the final review are applied: interaction mode never builds the legacy Bud agent engine, so the
  duplex-audio and policy edits to `core/agent/engine.rs` and `handlers/ws/agent.rs` are dropped (the engine keeps only
  additive telemetry), and `core/smart_turn/processor.rs` is unchanged.

**Totals: 342 files: 225 new, 117 modified.**

| Area | Files | New | Modified |
|---|---|---|---|
| Gateway: the new interaction module | 111 | 111 | 0 |
| Gateway: interaction module tests | 6 | 6 | 0 |
| Gateway: the front-model supervisor | 4 | 4 | 0 |
| Gateway: realtime (speech-to-speech) providers | 30 | 7 | 23 |
| Gateway: speech-to-text and text-to-speech providers | 15 | 1 | 14 |
| Gateway: voice manager, audio, turn detection | 12 | 2 | 10 |
| Gateway: LLM, agent, conversation, DAG | 11 | 1 | 10 |
| Gateway: handlers and routes | 23 | 4 | 19 |
| Gateway: configuration, state, startup, observability, API docs and other | 22 | 2 | 20 |
| Gateway: tests and fixtures | 11 | 6 | 5 |
| Gateway: build, packaging, deployment and CI | 7 | 3 | 4 |
| bud-auth crate (Bud agent projection) | 2 | 0 | 2 |
| Client SDKs | 4 | 0 | 4 |
| Evaluation harness (new project, outside the gateway crate) | 73 | 73 | 0 |
| Front-model distillation (new tool, outside the gateway) | 1 | 1 | 0 |
| Licence register and scripts | 2 | 2 | 0 |
| WaaV Infer (separate repository) | 8 | 2 | 6 |

Files with work in each release (a file can count in several): Lifecycle 21, R0 145, R1 119, R2 37, R3 53, R4 11, R5 11, Later 1.

## Gateway: the new interaction module

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/interaction/actor.rs`<br>`gateway/src/core/interaction/clock.rs`<br>`gateway/src/core/interaction/events.rs`<br>`gateway/src/core/interaction/executor.rs`<br>`gateway/src/core/interaction/floor.rs`<br>`gateway/src/core/interaction/front_sched.rs`<br>`gateway/src/core/interaction/host.rs`<br>`gateway/src/core/interaction/ids.rs`<br>`gateway/src/core/interaction/mod.rs`<br>`gateway/src/core/interaction/msg.rs`<br>`gateway/src/core/interaction/perceive.rs`<br>`gateway/src/core/interaction/ports.rs`<br>`gateway/src/core/interaction/speculate.rs`<br>`gateway/src/core/interaction/speech.rs`<br>`gateway/src/core/interaction/supervisor.rs`<br>`gateway/src/core/interaction/timeline.rs`<br>`gateway/src/core/interaction/views.rs` | new | Runtime | R1 | The runtime (section 3). |
| `gateway/src/core/interaction/claim_monitor.rs` | new | Providers and endpoints | R3 | `ClaimMonitor` and `ExactWordsVerifier` (both over W2's guard and normalisation) |
| `gateway/src/core/interaction/config.rs` | new | Runtime, Providers and endpoints | R1 | The runtime (section 3). / `InteractionConfig`, `from_json_lenient`, precedence merge (server default, alias, agent entry, client), validation and every `config_warning` code of section 3.10.4, echo with `source`, `front`, `policy_hash`, the effective `policy` and `barge_in` blocks and Azure's `model_family`; carries `front.caller.identity_token` to W2's verifier (section 3.20.2) |
| `gateway/src/core/interaction/contracts.rs` | new | Runtime, Front model, Tasks, Policy and HITL, Duplex audio, Providers and endpoints | R0 | W1's sections of the contract file and the golden test `runtime_contract_golden`. / W2's part: `Act`, `WaitReason`, `FrontDecisionId`, `ActDecision`, `FrontEvent`, `DelegationKind`, `TaskBinding`, `PersonaSpec` and its enums, `FrontEcho`; the golden test against `front_act_contract.json` / this design's shared types: `TaskId`, the `TaskRef(pub String)` newtype (one definition, Addendum C4), `Revision`, `TaskState` and its wire strings, `EffectsKnowledge`, `TaskView`, `TaskBoard`, `TaskEvent`, `TaskEventKind` (with `AnswerSentence { seq }` and `AnswerStopReason`), `ProvisionalKey`, `ProvisionalOrigin`, `ProvisionalWords`, `DiscardReason`, the `TaskRegistry` trait with `start_provisional`, `confirm_provisional` and `discard_provisional` (Addendum C2.1); golden file `task_contract.json` / this design's part: `DeliveryPolicy`, `Utterance`, `UtteranceKind` (with `Greeting`), `HistoryCategory`, `PolicyInput`, `PolicyAction` (with `TurnSettled.adopt`), `SettledAct`, `Utterance.stream: Option<StreamSpan>` (workstream 1's type), `HitlRequest`, `HitlAnswer`, `HitlKind`, `HitlOutcome`, `Binding`, `ActionProposal`, `InteractionSignal`; golden test `policy_contract_golden` / duplex section: `InterruptionClass`, `DuplexEvent`, `OverlapNote`, `AgentAnchor`, `Attribution`, `EagerEndpoint`, `EagerWithdrawal`, `EndOfTurnReport`, `EndpointSource`, `EagerMatch`, `GateCommand`, `GateState`, `StopTarget`, `StopReason`, `PumpStopReport`, `FloorSnapshot`, `ClipBank` types; golden test over `duplex_contract.json` / `Regime`, `RegimeRefusal`, wire names of `RegimeDecision`, client event bodies, `config_warning` codes, configuration path constants; goldens `wire_names_match_contract_golden` and `config_paths_match_contract_golden` |
| `gateway/src/core/interaction/duplex/analyser.rs` | new | Duplex audio | R1 | `IngressAnalyser`, drop-oldest frame ring, `VoiceActivitySource` (Silero v6.2 and segmented tap), chunk counter, feature tracker with first-turn baseline, utterance buffer, reflex duck, bounded event channel with drop counter |
| `gateway/src/core/interaction/duplex/backchannel/clips.rs` | new | Policy and HITL, Duplex audio | R1 | `ClipBank` gains this design's groups (acknowledgement, holding, check-in, bridge, follow-up, notice, withdrawal); rendered by workstream 1's `capture_clip` on the session's own TTS provider, never a second TTS instance; only in interaction mode / `ClipBank`, `ClipGroup`, `ClipRender`, trim, fade, loudness, per-`VoiceKey` cache in memory and under `CACHE_PATH` |
| `gateway/src/core/interaction/duplex/backchannel/mod.rs` | new | Duplex audio | R2 | `BackchannelEmitter` with an inline seeded generator, `AskFront` path for `backchannel_via_model`, yield at E |
| `gateway/src/core/interaction/duplex/barge_in.rs` | new | Duplex audio | R1 | `BargeInFsm` with utterance-scoped `Stop` outputs |
| `gateway/src/core/interaction/duplex/classifier/attribution.rs`<br>`gateway/src/core/interaction/duplex/classifier/heuristic.rs`<br>`gateway/src/core/interaction/duplex/classifier/lexicon.rs`<br>`gateway/src/core/interaction/duplex/classifier/mod.rs` | new | Duplex audio | R1, R4 | trait, stack, rules 1-15 (rule 5 reads W4's `answer_expected()`), English lexicons, attribution |
| `gateway/src/core/interaction/duplex/classifier/krisp.rs` | new | Duplex audio | R4 | Krisp plug-in behind feature `krisp-ip` |
| `gateway/src/core/interaction/duplex/classifier/model.rs` | new | Duplex audio | Later | ONNX six-class two-channel model behind feature `duplex-model` |
| `gateway/src/core/interaction/duplex/config.rs` | new | Duplex audio | R1 | `DuplexConfig` with defaults and `from_json_lenient`, voice-agent knob mapping, `longest_episode_ms()`, validation; parser for the server-only `interaction.duplex.compute.*` (a session `compute` block is refused) |
| `gateway/src/core/interaction/duplex/echo/aec.rs` | new | Duplex audio | R2 | `sonora` AEC3 wrapper behind feature `aec`; continuous render feed |
| `gateway/src/core/interaction/duplex/echo/delay.rs`<br>`gateway/src/core/interaction/duplex/echo/probe.rs` | new | Duplex audio | R1 | bulk-delay estimator, echo probe, correlation gate (uses `realfft`) |
| `gateway/src/core/interaction/duplex/echo/reference.rs`<br>`gateway/src/core/interaction/duplex/echo/transcript_filter.rs` | new | Duplex audio | R1 | render reference on playout time; transcript echo filter with the single-word rule (replaces the segmented plan's `is_probable_echo`) |
| `gateway/src/core/interaction/duplex/endpoint.rs` | new | Duplex audio | R1, R2, R4 | `EndpointPolicy`, segment claim, Smart Turn judge, provider event mapping, lexical hold, late-final merge, eager end of turn and withdrawal, `EndOfTurnReport` with `source`, `confident`, `attribution`, `suspected_echo` and `eager: EagerMatch`; optional veto |
| `gateway/src/core/interaction/duplex/ingress_tap.rs` | new | Duplex audio | R1, R3 | `IngressTap` trait and per-endpoint implementations; primary-participant filter |
| `gateway/src/core/interaction/duplex/mod.rs` | new | Duplex audio | R1 | `DuplexEngine` owned by W1's `InteractionRuntime`; `check_build()`; `echo_defence_active()`; wiring |
| `gateway/src/core/interaction/duplex/overlap.rs` | new | Duplex audio | R1 | `OverlapNote`, anchor, attribution, routing, note data |
| `gateway/src/core/interaction/duplex/pool.rs` | new | Duplex audio | R1 | duplex compute pool, per-session ordered jobs, Smart Turn rate limit and global queue, lag metric |
| `gateway/src/core/interaction/front/acts.rs` | new | Front model | R0 | `Act`, `AllowedActs`, legality table, first-character map, the golden contract file loader |
| `gateway/src/core/interaction/front/admission.rs` | new | Front model | R0 | priority permit queue, per-call latency hysteresis, error breaker |
| `gateway/src/core/interaction/front/assets/qualified_profiles.json`<br>`gateway/src/core/interaction/front/profiles.rs` | new | Front model | R1 | profile registry (`lfm-standard`, `lfm-large`, `apache-standard`, `scripted`, `multilingual`, `experimental-light`), pinned hashes, licence requirements, `tokens_per_word`, assistant-turn opener; `resolve` (`auto`) and the qualified-profile list |
| `gateway/src/core/interaction/front/background_preamble.txt`<br>`gateway/src/core/interaction/front/persona.rs`<br>`gateway/src/core/interaction/front/persona/front_system.txt`<br>`gateway/src/core/interaction/front/provider_instructions.txt` | new | Front model | R0, R3 | `PersonaSpec` (with `interruptibility`, `utc_offset`, four-level `backchannel_intensity`, `template_overrides`), `PersonaBanks`, `CallerProfile`, `CompiledPersona`, operator registry, compile targets |
| `gateway/src/core/interaction/front/compose.rs` | new | Front model | R0 | recovering opener, inquiry, span and closer from a finished line; the W3 label |
| `gateway/src/core/interaction/front/grammar.rs` | new | Front model | R0 | GBNF builder per decision: compose chains, pronoun turn, restate stops, polar filtering, `wait` reasons, prefill-aware, say modes |
| `gateway/src/core/interaction/front/guard.rs`<br>`gateway/src/core/interaction/front/guard/lang_en.rs`<br>`gateway/src/core/interaction/front/guard/lang_hi.rs` | new | Front model | R0, R4 | `FactGuard` (check, bank lines, slots, monitor), normalisation, language tables and word lists |
| `gateway/src/core/interaction/front/llama.rs` | new | Front model | R0, R1 | front HTTP client (socket or pinned TCP, no redirects, per-request timeouts, key file) on `POST /completion` with the native stream parser, request bodies, `SlotPool` and `SlotLease`, janitor, prime, erase, save, restore, `timings` capture, `/apply-template` check, classification slot; token-exact prompt only if section 3.8.7 requires it |
| `gateway/src/core/interaction/front/memory.rs` | new | Front model | R2 | `CallerMemory`, `RedisCallerMemory` on W3's `TaskRedis`, `BudappCallerMemory`, identity-token verification (`verify_identity_token`, the pinned `v1` format, derived keys) and write validation |
| `gateway/src/core/interaction/front/mod.rs` | new | Front model | R0, R1 | `FrontModel` (with `supports_speculation`, `echo`), `FrontCall` (sequencer, settle, promote, discard), `DecisionHandle`, `DecisionContext`, `LlamaFront`, `ScriptedFront` |
| `gateway/src/core/interaction/front/parse.rs` | new | Front model | R0 | streaming act parser and validation (reuses `core/agent/text.rs` `SpeechChunker` and a prefill-seeded `SpeakFieldExtractor`); act final after `ActKnown` |
| `gateway/src/core/interaction/front/persona_store.rs` | new | Front model | R2 | tenant persona store and cache; the persona and memory routes added to `create_api_router` (`routes/api.rs:74`) only when enabled |
| `gateway/src/core/interaction/front/pool.rs` | new | Front model | R2 | pool instance list, slot-range leases on the task Redis, per-range admission share |
| `gateway/src/core/interaction/front/prompt.rs` | new | Front model | R0 | record, observation and call-header renderers, W1 line rendering, `TaskView` rendering in coarse buckets, sanitiser; the full-prompt renderer in each profile's chat template with its assistant-turn opener (section 3.2.4) |
| `gateway/src/core/interaction/front/rules.rs` | new | Front model | R0 | `RuleView` (with `injection_cue`, `sensitive_context`, `forbidden_topic`), overrides including `injection_cue_is_refuse` and the repeat exemption, the shared `route` function (uses W4's pack lexicons) with the question step 11 of section 3.7.2 (Addendum D1 item 1); the Release 1 front |
| `gateway/src/core/interaction/hitl/mod.rs` | new | Policy and HITL | R1, R2 | `HitlDialog` state machine, heard check, attribution rule |
| `gateway/src/core/interaction/hitl/parse.rs` | new | Policy and HITL | R1 | three-stage answer parser with signal-quality, echo, negation, coverage and residue checks |
| `gateway/src/core/interaction/hitl/sensitive.rs` | new | Policy and HITL | R1 | screening and the always-on detector (Release 1); channel choice and capture-window actions (Release 2); DTMF collection (Release 4) |
| `gateway/src/core/interaction/metrics.rs` | new | Runtime, Evaluation and rollout | R1 | `InteractionMeter` subscribed to the bus. / `InteractionMeter`, one `interaction.wait` span per wait |
| `gateway/src/core/interaction/policy/config.rs` | new | Policy and HITL | R1 | `PolicyConfig`, defaults and clamps of section 3.11 (`ack_due_ms`, `fast_answer_ms`, `answer_window_ms`, `agreed_wait_max_ms` and the rest), precedence of the configuration homes, parsing of the agent's raw JSON override block, `config_warning` reporting, `policy_hash` |
| `gateway/src/core/interaction/policy/delivery.rs` | new | Policy and HITL | R1 | `DeliveryPolicy` resolution, `DeliveryQueue` (ranks, pre-emption, re-ranking, staleness, most-recent-wins, adjacency) |
| `gateway/src/core/interaction/policy/interject.rs` | new | Policy and HITL | R1 | pre-classifier lexicons, rule layer over workstream 2's act, amendment binding, routing table, the settled act and `TurnSettled.adopt` (section 3.5.5) |
| `gateway/src/core/interaction/policy/locale.rs` | new | Policy and HITL | R1 | the locale gate; the language step-down where `step_down_available`, else the repeat-request template (section 3.13.1) |
| `gateway/src/core/interaction/policy/mod.rs` | new | Policy and HITL | R1 | `PolicyEngine`, `step`, `next_wakeup`, `answer_expected`, `utterance_meta`, `holds_session_open`, `policy_hash` |
| `gateway/src/core/interaction/policy/pack.rs`<br>`gateway/src/core/interaction/policy/packs/en.rs` | new | Policy and HITL | R0 | `PolicyPack` format and validator (Release 0); English lexicons, templates, banks and withdrawal lines (Release 1); the persona-override rules of section 3.13.4 |
| `gateway/src/core/interaction/policy/regimes.rs` | new | Policy and HITL | R3 | regime B and C mappings, the regime C confirmation segment, `ProviderSpeech` corrections (section 3.5.4) |
| `gateway/src/core/interaction/policy/session_end.rs` | new | Policy and HITL | R1 | finish-or-stop question, `SessionEnding` resolution |
| `gateway/src/core/interaction/policy/speak_value.rs` | new | Policy and HITL | R1 | `SpeakValue` rendering (codes, money, dates and times with the `time` crate, card digits, emails) |
| `gateway/src/core/interaction/policy/stream.rs` | new | Policy and HITL | R1 | `AnswerStream`: one open relay item per answer attempt, sentence spans appended one ahead, the release check at append, a stop row for every `AnswerStopReason`, pagination by closing the item, the stall clip, completion and the explicit close (section 3.6.1) |
| `gateway/src/core/interaction/policy/timing.rs` | new | Policy and HITL | R1 | `WaitState`, the acknowledgement due time with fast mode and the per-backend rolling median, rules 6 to 12 and 19 to 22 as pure functions |
| `gateway/src/core/interaction/policy/truth.rs` | new | Policy and HITL | R1, R3 | completion, cancellation, execution and outcome rules; trusted-entity check for progress; calls workstream 2's fact guard; exports the completion and outcome lexicons for workstream 6's `ClaimMonitor` |
| `gateway/src/core/interaction/policy/weave.rs` | new | Policy and HITL | R1 | relay planning: bridge, pagination, partial resume, relay questions, relevance, offers, restatement-mismatch backstop |
| `gateway/src/core/interaction/provider_prompts.rs` | new | Providers and endpoints | R3 | the regime sections of section 3.11 and the closed directive list |
| `gateway/src/core/interaction/provider_provisional.rs` | new | Providers and endpoints | R3 | provider-call provisional runs: adoption triggers, `adopt_by` timers, the call-held path for backends without speculation (section 3.16) |
| `gateway/src/core/interaction/provider_voice.rs` | new | Providers and endpoints | R3 | `ProviderVoice` (W1's `SpeechPort` over a verb handle), `ProviderPlayout`, `OpenQuestionGate`, `ClipRenderer`, streamed-relay batching (section 3.17), per-regime E and acknowledgement skips (section 3.18) |
| `gateway/src/core/interaction/server_config.rs` | new | Runtime, Providers and endpoints | R0 | `InteractionServerConfig`, the one server key from the same YAML file or from `WAAV_INTERACTION_*` variables, and the recorded configuration source (section 3.17). / `InteractionServerConfig` fields owned here (`speech.truth_ack`, `defaults` with `agent` and `realtime`, `allow_client_override`, `realtime_legacy_wire`), parsed by W1's loader of the one `interaction` key; W3's `tasks` and W5's `duplex.compute` sub-blocks parsed by their owners (Addendum A2 items 4 and 5) |
| `gateway/src/core/interaction/tasks/admission.rs` | new | Tasks | R1 | `BackendAdmission`, shared `BackendSet` per backend configuration |
| `gateway/src/core/interaction/tasks/answer.rs` | new | Tasks | R1 | `AnswerFilter`, the four release rules, marker detection, the two text pipelines, `AnswerRelease` (section 3.17) |
| `gateway/src/core/interaction/tasks/backend.rs` | new | Tasks | R1 | `TaskBackend`, `BackendCaps`, `BackendEvent`, `StartRequest`, handles, errors |
| `gateway/src/core/interaction/tasks/backends/a2a.rs` | new | Tasks | R3 | section 3.7.5 |
| `gateway/src/core/interaction/tasks/backends/bud_agent.rs` | new | Tasks | R1, R2 | section 3.7.2: foreground runs |
| `gateway/src/core/interaction/tasks/backends/dag.rs` | new | Tasks | R3 | section 3.7.9 |
| `gateway/src/core/interaction/tasks/backends/llm_tier.rs` | new | Tasks | R1 | section 3.7.1, the task loop around `execute_batch_in_task` with a streaming answer filter |
| `gateway/src/core/interaction/tasks/backends/mcp.rs` | new | Tasks | R3 | section 3.7.6 |
| `gateway/src/core/interaction/tasks/backends/openai_responses.rs` | new | Tasks | R3 | section 3.7.7 |
| `gateway/src/core/interaction/tasks/backends/tool.rs` | new | Tasks | R1 | section 3.7.3 single-tool tasks |
| `gateway/src/core/interaction/tasks/backends/webhook.rs` | new | Tasks | R2 | section 3.7.4 |
| `gateway/src/core/interaction/tasks/config.rs` | new | Tasks | R1 | `TaskConfig` (session `interaction.tasks.*`), `TaskServerConfig` (server `interaction.tasks.*`: `task_backends`, `allow_client_*`, `phone_continuation`, `speech_policy`, `speculation`; Addendum A2.4), defaults, validation against W1's, W2's, W4's and W5's values |
| `gateway/src/core/interaction/tasks/core.rs` | new | Tasks | R1 | `TaskRegistryCore`: the state machine, limits, queue, duplicate policy, revision strategy (two-phase restart), gate reservations, timers, retry |
| `gateway/src/core/interaction/tasks/driver.rs` | new | Tasks | R1 | `TaskDriverPool`, per-run event pump and sequential command worker, retry rules |
| `gateway/src/core/interaction/tasks/host.rs` | new | Tasks | R1 | `TaskHost` implementing the `TaskRegistry` trait: lock, publish, dispatch, inbox pump, tick, event rings, `TaskGate` |
| `gateway/src/core/interaction/tasks/input.rs` | new | Tasks | R1 | `BackgroundInput` rendering, utterance-class filter, sensitive-field detection, provenance tagging |
| `gateway/src/core/interaction/tasks/lexicon.rs` | new | Tasks | R1, R4 | loads W4's lexicon classes from the policy pack, plus the thanks list, per language |
| `gateway/src/core/interaction/tasks/mod.rs` | new | Tasks | R1 | module root, re-exports `TaskHost`, `Task`, `TaskBackend` |
| `gateway/src/core/interaction/tasks/model.rs` | new | Tasks | R1 | task, revision record, operation, committed effects, result, stale outcome, error, records (section 3.2) |
| `gateway/src/core/interaction/tasks/progress.rs` | new | Tasks | R1 | progress filter, materiality, coalescing |
| `gateway/src/core/interaction/tasks/prompt.rs` | new | Tasks | R1 | the background prompt template with the answer-marker rule (section 3.7.1); no front prompt text |
| `gateway/src/core/interaction/tasks/provisional.rs` | new | Tasks | R1, R3 | provisional runs: key minting, the three origins, `confirm_provisional`, adoption, discard, the deadline anchored at E and the eager ceiling (Addendum A1), held sentences, the `speculative_start_unsafe_backend` refusal (section 3.16) |
| `gateway/src/core/interaction/tasks/push.rs` | new | Tasks | R2 | signed push delivery with retries (reuses the WaaV signature scheme) |
| `gateway/src/core/interaction/tasks/speech_policy.rs` | new | Tasks | R1 | `OutboundSpeechPolicy` (section 3.11, rule 5), applied to results, statuses and each answer sentence |
| `gateway/src/core/interaction/tasks/store.rs` | new | Tasks | R1, R2 | `TaskStore` trait, in-memory store, Redis store (own connection plus a pub/sub connection), store writer, the shared `TaskRedis` handle |
| `gateway/src/core/interaction/tasks/supervisor.rs` | new | Tasks | R1, R2 | `TaskSupervisor` (created lazily): detached hosts, orphans, relay routing, adoption sweeper, phone-line index |
| `gateway/src/core/interaction/tasks/view.rs` | new | Tasks | R1 | `TaskView` and `TaskBoard` building, coarse elapsed buckets, `version` (section 3.5.1) |

## Gateway: interaction module tests

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/interaction/policy/tests/*.rs` | new | Policy and HITL | R0 | virtual-clock harness and the tests of section 5 |
| `gateway/src/core/interaction/tests/*.rs` | new | Runtime | R0 | Unit and property tests with paused time and fakes. |
| `gateway/src/core/interaction/tests/clock_rule.rs`<br>`gateway/src/core/interaction/tests/fakes.rs`<br>`gateway/src/core/interaction/tests/guard_conformance.rs`<br>`gateway/src/core/interaction/tests/scenarios.rs` | new | Evaluation and rollout | R0 | in-process scenario runner over the event form; guard conformance against W2's guard API; tokio-only time rule |

## Gateway: the front-model supervisor

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/front_model/assets.rs`<br>`gateway/src/core/front_model/licence.rs`<br>`gateway/src/core/front_model/mod.rs`<br>`gateway/src/core/front_model/supervisor.rs` | new | Front model | R0 | process supervisor (run directory, pid files, `pre_exec`, stop), streaming asset download and verification, licence attestation gate |

## Gateway: realtime (speech-to-speech) providers

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/realtime/*/mod.rs` | modified | Providers and endpoints | R3 | forward `capabilities` and `verb_handle` |
| `gateway/src/core/realtime/azure/protocol.rs`<br>`gateway/src/core/realtime/yandex/protocol.rs` | modified | Providers and endpoints | R3 | delegate `capabilities` and `map_command` to the inner OpenAI protocol with their own ids; azure's `capabilities` reads `model_family` (never the deployment name, `:104-110`) for the preamble rule and reports `model_family: None` when it is unset |
| `gateway/src/core/realtime/base.rs` | modified | Providers and endpoints | R3 | `capabilities` and `verb_handle` on `BaseRealtime` (`:686-857`); `ReplayConversationItem` gains `kind` and `heard` (`:495-499`); `RealtimeResponseOverride` gains `input` and `metadata`; `ToolDefinition` (`:391-397`) gains `behavior: Option<ToolBehavior>` (skip if none); `RealtimeConfig` (`:175`) gains `provider_data` passthrough and an optional `model_family` (Azure, Release 3, Addendum A2 item 10) |
| `gateway/src/core/realtime/capabilities.rs` | new | Providers and endpoints | Lifecycle, R1 | `ProviderCapabilities` and its enums, `Regime`, `RegimeDecision`, `select_regime`, lifecycle table |
| `gateway/src/core/realtime/deepgram/protocol.rs` | modified | Providers and endpoints | Lifecycle | `send_text` sends `InjectUserMessage` (escape flag); `InjectAgentMessage{message, behavior}`; tools in `Settings`; new events; `session_update_plan` with `Update*` |
| `gateway/src/core/realtime/delegate_tool.rs` | new | Providers and endpoints | R3 | `waav_delegate` declaration per provider family, argument parsing (including `answer`), `mcp_call` rewrite helpers |
| `gateway/src/core/realtime/elevenlabs/protocol.rs` | modified | Providers and endpoints | R3 | `contextual_update`; capabilities (`OutputGap` responding end) |
| `gateway/src/core/realtime/gemini/protocol.rs` | modified | Tasks, Providers and endpoints | Lifecycle, R3 | (W6) map `toolCallCancellation` to an event instead of `Ignore` (`:347-349`); the task semantics are in sections 3.7.8 and 3.16.4 / explicit `behavior`; `scheduling`, `willContinue`, `name` on responses; `toolCallCancellation`; `interactionStatus` to `ProviderBusy` (with `ResponseDone` kept on `turnComplete`); `audio` input field; compression only in interaction mode; `session_update_plan` refusal; forced input transcription in interaction mode |
| `gateway/src/core/realtime/gpt_live/mod.rs`<br>`gateway/src/core/realtime/gpt_live/prompt.rs`<br>`gateway/src/core/realtime/gpt_live/protocol.rs` | new | Providers and endpoints | R3 | GPT-Live adapter (section 3.7) |
| `gateway/src/core/realtime/grok/protocol.rs` | modified | Providers and endpoints | Lifecycle | `force_message`; `idle_timeout_ms`; capabilities with the model as sent (`grok-realtime` when empty, `:49-53`); the default model moves only after a live probe (Lifecycle track) |
| `gateway/src/core/realtime/hume/*.rs` | modified | Providers and endpoints | Lifecycle | lifecycle `Retiring`/`Retired`; `capabilities`; no new verbs |
| `gateway/src/core/realtime/infer/mod.rs` | modified | Providers and endpoints | R3 | `TestInfer::capabilities` (`:310`); capabilities (native duplex, no injection) |
| `gateway/src/core/realtime/infer/protocol.rs` | modified | Providers and endpoints, Evaluation and rollout | R5 | `TestInfer::capabilities` (`:310`); capabilities (native duplex, no injection) / default endpoint to Infer's real path and port; role prompt when the capability descriptor says the model accepts one; new fields only when the engine advertises them |
| `gateway/src/core/realtime/inworld/protocol.rs` | modified | Providers and endpoints | R3 | `response.speak`; `providerData` toggles; capabilities |
| `gateway/src/core/realtime/mod.rs` | modified | Providers and endpoints, Evaluation and rollout | R5 | `gpt_live` module, `RealtimeProvider::GptLive`, parse aliases; registry list; count pin 11 to 12 (`:308`) / `infer` in `get_supported_realtime_providers` and the `RealtimeProvider` enum |
| `gateway/src/core/realtime/nova_sonic/protocol.rs` | modified | Providers and endpoints | Lifecycle | `send_text` with `interactive: true` (escape flag); `session_update_plan` refusal; context items folded into the next connection's system prompt |
| `gateway/src/core/realtime/openai/config.rs` | modified | Providers and endpoints | Lifecycle | lifecycle entries; dated default switch |
| `gateway/src/core/realtime/openai/messages.rs` | modified | Providers and endpoints | R1 | `force_message` item type and `interruptible`, `response.speak`, `metadata`, `providerData`, all `skip_serializing_if` (`ConversationItem` at `:246`) |
| `gateway/src/core/realtime/openai/protocol.rs` | modified | Providers and endpoints | R3 | `capabilities` (model-aware: preambles on gpt-realtime-2.x); `map_command` (system items, guided verbatim, hold with whole `turn_detection`, `input` and `metadata` on `response.create`); map `response.created` to `ResponseStarted`; `replay_item` for `kind` and `heard` through one helper (`:170-182`); session `output_modalities` from config for the half-cascade |
| `gateway/src/core/realtime/scaffold/event.rs` | modified | Providers and endpoints | R3 | new `S2sEvent` variants (section 3.4.1) |
| `gateway/src/core/realtime/scaffold/executor.rs` | new | Providers and endpoints | R3 | the verb executor task: outbox, speech slot, epoch handling, call-free path, re-apply on reconnect |
| `gateway/src/core/realtime/scaffold/mock.rs` | modified | Providers and endpoints | R3 | `MockProtocol::capabilities` (`:197`); scripted activity for tests |
| `gateway/src/core/realtime/scaffold/protocol.rs` | modified | Providers and endpoints | R3 | `capabilities`, `map_command` (default of section 3.4.1), `session_update_plan`, `build_session_config_with_history`, `tick_interval`, `on_tick`, `on_close` |
| `gateway/src/core/realtime/scaffold/session.rs` | modified | Providers and endpoints | R3 | shared `Arc<StdRwLock<RealtimeConfig>>` read at each (re)connect (`:413`, `:512`, `:838-851`); `ActivityState` written by the supervisor instead of the local `in_response` (`:550`, `:593-601`); output gate before the tap (`:586-592`); `OutputGate` and `OutputGap` taps; epoch bump after replay (`:530-537`); planned rotations wait for open delegate calls in interaction mode (`:542-571`); tick arm; close classification (`:707-742`); `update_session` asks `session_update_plan` before merging (`:961-965`); executor spawned in `connect`; `verb_handle`; `send_text` unchanged (`:890-892`) |
| `gateway/src/core/realtime/scaffold/transport.rs` | modified | Providers and endpoints | R3 | keep the close frame's code and reason (`:93`) |
| `gateway/src/core/realtime/ultravox/protocol.rs` | modified | Providers and endpoints | R3 | tools in create-call body; `forced_agent_message`; `urgency`; `agentReaction`; capabilities |
| `gateway/src/core/realtime/verbs.rs` | new | Providers and endpoints | R1 | `RealtimeCommand`, `TaskWire`, `ProviderCallRef`, `VerbOutcome`, `CommandPlan`, `SessionUpdatePlan`, `ActivitySnapshot`, `ActivityState`, `RealtimeVerbHandle`, envelope and neutralisation helpers |

## Gateway: speech-to-text and text-to-speech providers

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/stt/assemblyai/` | modified | Duplex audio | Lifecycle | `turn_event` from `end_of_turn`, informational (`is_speech_final` already set, `client.rs:517-523`) (Release 1); Universal-3.6 Pro model, no confidence threshold for it, `mode` and `interruption_delay` extras, default moved after the probe behind an escape flag (Lifecycle) |
| `gateway/src/core/stt/base.rs` | modified | Duplex audio | R1 | `STTResult.turn_event`; `BaseSTT::finalize_segment()` defaulting to `Unsupported` |
| `gateway/src/core/stt/cartesia/client.rs` | modified | Duplex audio | R1 | `finalize_segment` through the existing `finalize` (`:592-608`) |
| `gateway/src/core/stt/deepgram.rs` | modified | Duplex audio | R1 | `finalize_segment` sends the finalize request (ack parsed at `:694`) |
| `gateway/src/core/stt/deepgram_flux.rs` | new | Providers and endpoints | R2 | the Flux adapter (section 3.21) |
| `gateway/src/core/stt/mod.rs` | modified | Providers and endpoints | R2 | `STTProvider::DeepgramFlux`, parse `deepgram-flux` (`core/stt/mod.rs:291`, `:397`), the supported-provider list and its tests; metadata and constructor beside `deepgram` (`plugin/builtin/mod.rs:63`, `:1666`) |
| `gateway/src/core/stt/openai/config.rs`<br>`gateway/src/core/tts/openai/config.rs` | modified | Providers and endpoints | Lifecycle | lifecycle entries; dated default switch |
| `gateway/src/core/tts/aws_polly/provider.rs`<br>`gateway/src/core/tts/google/provider.rs` | modified | Runtime | R0 | Declare `InsideSpeak` (or leave the `CompletionOnly` default). |
| `gateway/src/core/tts/base.rs` | modified | Runtime | R0 | Default no-op `on_utterance_start(context_id)` on `AudioCallback`; default `boundaries() -> CompletionOnly` on the provider trait. |
| `gateway/src/core/tts/deepgram_aura.rs` | modified | Runtime | R0 | Declare `PerRequest`. |
| `gateway/src/core/tts/hume/*.rs` | modified | Providers and endpoints | Lifecycle | lifecycle `Retiring`/`Retired`; `capabilities`; no new verbs |
| `gateway/src/core/tts/provider.rs` | modified | Runtime | R0 | Context id in `RequestEntry`; `on_utterance_start(Some(id))` when the dispatcher pops a request. |
| `gateway/src/core/tts/websocket.rs` | modified | Runtime | R0 | `on_utterance_start(Some(id))` on the first frame after the lock is released; a `Cleared` notify. |

## Gateway: voice manager, audio, turn detection

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/audio/playback_pump.rs` | modified | Runtime, Duplex audio | R0, R1 | `spawn_metered`: playout-clock pacing, 20 ms frames, gates, cursor, render tap, rewind, events with `heard_at`, gap causes, marker callback. The existing `spawn` is unchanged. / requirements of section 3.3.1 on W1's `spawn_metered` only: playout clock, 20 ms re-chunking without inherited durations, per-lane gates, `DeliveryOutcome`, delivered cursor, render tap, `Underrun` versus `StreamGap`, stopped set; the legacy `spawn` untouched |
| `gateway/src/core/audio/playback_queue.rs` | modified | Runtime, Duplex audio | R0, R1 | Opaque `PlaybackTag`; `clear_utterance`; per-utterance open flag; W5's per-lane sub-queues for the metered queue. / metered queue: per-lane sub-queues with arrival sequence; gate-aware pop with the clip-yield rule; utterance-scoped purge returning `PumpStopped`s; counters still under the lock (`:80-111`); the legacy FIFO untouched |
| `gateway/src/core/audio/playout_gate.rs` | new | Runtime, Duplex audio | R1 | `PlayoutControl` implementation on the metered pump. / `PlayoutControl`, `GateCommand`, `StopTarget`, `PumpStopReport`, `FloorSnapshot` with `agent_holds_floor()`, gain ramps for linear PCM16 only, fades, rewind ring |
| `gateway/src/core/silero_vad/detector.rs` | modified | Duplex audio | Lifecycle | versioned paths under `CACHE_PATH` first; verify on every load; release-tag URL for the pinned v5 file (`:18-20`, `:182-256`) (Lifecycle); v6.2 file and interface assertion for the analyser (1) |
| `gateway/src/core/smart_turn/detector.rs` | modified | Duplex audio | Lifecycle | versioned path under `CACHE_PATH` first; SHA-256 pin by revision; verify on every load (`:29-33`, `:424-440`) |
| `gateway/src/core/turn/controller.rs` | modified | Duplex audio | R1 | new signals pass the user-input mute rule like other user input; no new `TurnEvent` |
| `gateway/src/core/turn/signal.rs` | modified | Duplex audio | R1 | `VadStart`, `VadStop`, `ProviderTurn`, `OverlapClassified`, `EndpointDecided` |
| `gateway/src/core/turn/strategies/classified.rs` | new | Duplex audio | R1 | `ClassifiedStart` (probe `agent_holds_floor()`), `EndpointStop` (never `Speculate`) |
| `gateway/src/core/turn_detect/config.rs` | modified | Duplex audio | Lifecycle | no default LiveKit URLs (`:27-36`); licence acknowledgement |
| `gateway/src/core/voice_manager/callbacks.rs` | modified | Runtime | R0 | `on_utterance_start`; completion counter; new fields at all four construction sites. |
| `gateway/src/core/voice_manager/manager.rs` | modified | Runtime, Policy and HITL, Duplex audio, Evaluation and rollout | Lifecycle, R0, R1 | Section 3.6 VoiceManager changes 1 to 7. / always-on detector before the user callback (Release 1); capture gate in the STT wrapper (`:1034-1077`: user callback at `:1073` suppressed, empty transcript to the final observer at `:1045`, `CapturedSpeech` to the runtime) and silence instead of caller audio to the STT vendor during a capture (`:636-639`) (Release 2). One atomic read per result when no capture is open / interaction mode: no `SmartTurnProcessor` (`:364-382`); tap hook before the STT send (`:564-641`); pump built once with `playback_enqueuer()` (`:1187-1205`); `clear_tts` not called in interaction sessions (client clear goes to W1's `ClientClear`); `claim_stt_segment`, `finalize_stt_segment`; warning text at `:384-401` (Lifecycle) / VAD and bot-speaking edge emission after `notify_smart_turn` (`:592-600`) and from the playout estimate (`:1481-1483`); `last_vad_speech_start_ns` and `last_vad_speech_end_ns` getters. Shared with W5; whichever lands first owns it |
| `gateway/src/core/voice_manager/stt_result.rs` | modified | Duplex audio | R1 | `claim_segment(generation)` reusing the forced-fire claim (`:504-535`); interaction-mode duplicate window of 1,500 ms covering `is_final` fragments (`:582-611`); expose the current generation |

## Gateway: LLM, agent, conversation, DAG

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/core/agent/brain.rs` | modified | Tasks | R1, R2 | new `task_events()` decoder, `TaskStreamFrame`, `TaskStreamEvent` (reusing `SseDecoder`, `:250-311`, and `map_event`, `:341-441`); `AgentEvent` and `events()` unchanged; `AgentTaskRequest` body with `background`, optional `conversation`, input items, task metadata (beside `:217-242`); `open_task`, `resume(id, after)`, `get(id)`, `submit_approval`, all lifecycle calls sending `x-model-name`; a second, shared HTTP client without the 120 s read timeout (beside `:463-477`) |
| `gateway/src/core/agent/engine.rs` | modified | Tasks, Evaluation and rollout | R0 | none for the interaction mode. Separately proposed (its own change, its own test, changes interaction-off behaviour on purpose): map a streamed `requires_action` to `TurnFailure::ApprovalRequired` and the existing `approval_message` path (`:1217-1219`) with explicit arms in the event match (`:895-952`) / barge-in stop time in `barge_in` (`:613-630`) and speech-end latency beside `RESPONSE_LATENCY_MS` (`:1049`) |
| `gateway/src/core/agent/mod.rs` | modified | Runtime | R1 | Re-exports of the reusable text and ledger pieces. |
| `gateway/src/core/conversation/legs.rs` | new | Runtime | R0 | `resolve_llm_legs` (Bud rewrite or URL check on both URLs; attribution; fatal handler). |
| `gateway/src/core/conversation/mod.rs` | modified | Runtime, Tasks, Policy and HITL, Duplex audio, Evaluation and rollout | Lifecycle, R0, R1 | `validate_llm_url` becomes `pub(crate)`; legacy `spawn_turns` (decision 12). / new `pub(crate) fn background_client_config` (beside `to_client_config`, `:532`); `validate_llm_url` (`:2342`) made `pub(crate)`; the existing sink keeps ignoring non-final results (`:1023-1025`) / in interaction mode the conversation orchestrator is not constructed and the STT callback only does `try_send` (W1 decision 3), so `/ws` conversation sessions are in Release 1 with agent sessions (integration decisions section 5, "Conversation sessions"); its latency filler, `followup_allowed` gate and idle re-engagement are untouched for mode off; the legacy inline-turn fix is workstream 1's separate mode-off flag `spawn_turns`, default off / fix the comment at `:1342` (Lifecycle); in interaction mode the orchestrator is not built (W1), so `handle_barge_in` is never reached (1) / the same two attributes on the orchestrator's turn span (`:1235`) |
| `gateway/src/core/llm/functions.rs` | modified | Tasks, Policy and HITL | R1 | new `FunctionCallParams` fields (`:44-54`; the two literals in `execute_one`, `:466-472`, `:506-512`, get `None` defaults); new `RegistryItem` fields with no-op defaults in `RegistryItem::new` (`:110-119`) and a new constructor for operator tools defaulting to `Possible`; private `execute_one_with(.., Option<&TaskExec>)` (`:417-527`), with `execute_one` calling it with `None`; new `pub(crate) execute_batch_in_task`; the `ToolGate` trait, `TaskExec`, `GateDecision` with `AwaitAdoption`; `cancel_async` stays synchronous, cancels a stored token and spawns grace-then-abort (`:246-254`); cancelled calls skip `deliver_async`; new `cancel_all_async()`. `ToolLoopOptions` and `execute_batch` are unchanged (so its `Copy` derive, `:290`, and the test literal at `:1000-1004` are untouched) / progress path (only finals today, `:486`); built-in tools; the confirmation gate in a private `execute_one_with`, binding `(task, revision, tool, canonical args)`, and the `AwaitAdoption` answer before adoption, with `execute_one` unchanged for interaction-off paths (W3 sections 2.19 and 3.7.3) |
| `gateway/src/core/llm/mod.rs` | modified | Runtime, Tasks | R0 | `stream_messages` factored out of `complete_streaming`. / none: the task loop uses the existing streaming `continue_from_history` with a token callback (`:1102-1117`) |
| `gateway/src/dag/context.rs` | modified | Tasks, Providers and endpoints | R3 | optional `progress_tx` on `DAGContext` beside `output_tx` (`dag/context.rs:73`), fed by `record_node_start` and `record_node_end` (`:265-280`) and by the streaming forwarder on a node's first chunk (`dag/executor.rs:200-260`); `None` everywhere today, so DAG sessions without interaction are unchanged / `DagOutput::Event` (`:25-30`) (Release 3, text-output graphs) |
| `gateway/src/dag/definition.rs` | modified | Providers and endpoints | R1, R3 | definition-level `interaction: Option<Value>`; a pure helper `has_audio_output()` over `AudioOutput`, `TtsProvider` and `RealtimeProvider` nodes (`:522`, `:543`, `:552`) |
| `gateway/src/dag/executor.rs` | modified | Tasks | R3 | optional `progress_tx` on `DAGContext` beside `output_tx` (`dag/context.rs:73`), fed by `record_node_start` and `record_node_end` (`:265-280`) and by the streaming forwarder on a node's first chunk (`dag/executor.rs:200-260`); `None` everywhere today, so DAG sessions without interaction are unchanged |
| `gateway/src/dag/nodes/provider.rs` | modified | Providers and endpoints | R3 | refuse `gpt_live` (`provider_not_turn_based`) until the duplex DAG output listed under "Later" exists; no interaction support on the node (a graph with a realtime node is an audio-output DAG, Decision 22); with interaction off, unchanged |

## Gateway: handlers and routes

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/src/handlers/api.rs` | modified | Front model | R0, R2 | optional `front_model` readiness object; handler passes supervisor state; schema regenerated (and again for the Release 2 routes) |
| `gateway/src/handlers/interaction_events.rs` | new | Providers and endpoints | R3 | the task-event mapping of section 3.12.4 (`TaskEvent` and W4 signals to client events) and `InteractionEvent` to `/ws`, `/realtime`, `bud.*`, LiveKit data and DAG output mappers |
| `gateway/src/handlers/interaction_personas.rs` | new | Front model, Providers and endpoints | R2 | tenant persona store and cache; the persona and memory routes added to `create_api_router` (`routes/api.rs:74`) only when enabled / the persona route of section 3.20.1 (tenant from `Auth.id`, `403 tenant_required`, errors), mounted only when `front_model.tenant_personas.enabled`, calling W2's registry and limits |
| `gateway/src/handlers/livekit/webhook.rs` | modified | Tasks | R2 | move `generate_webhook_signature` (`:548-580`) to a shared `utils/signing.rs`, re-imported in `webhook.rs` so its tests (`:1031-1100`) still compile |
| `gateway/src/handlers/openai_realtime/cascade.rs` | modified | Runtime, Duplex audio, Providers and endpoints | R2 | Segment responses, audio attribution, `speech_started` gating, `start_core` and `apply` routing. / interaction mode only: hold an open episode's transcripts; `speech_started` on the floor change with the onset on the cascade clock (`:776-795`, `:963-971`); W1's segment markers / `bud.task.*`, `bud.floor.state`, `bud.claim.unverified`, `bud.interaction.*` arms before `_ => {}` (`:1134`); interaction turns get their own GA response instead of being dropped (`:1139-1144`); `handle_config_message` call site gains the `interaction` argument (`:1307-1321`) |
| `gateway/src/handlers/openai_realtime/facade.rs` | modified | Providers and endpoints | R3 | `Act::Verb(RealtimeCommand)` (`:365-390`), executed through the handle in `send_now` (`:1556-1598`); system items to `AddContext` only with interaction on (`:861-864`); `OutputGate` ends the open GA response; explicit arms for new vendor events (`:1261-1389`); tap fan-out to the runtime (`:1487-1492`) |
| `gateway/src/handlers/openai_realtime/policy.rs` | modified | Providers and endpoints | R3 | intercept `bud.*` client events before `client_event` (`:240`); merge `waav_delegate` into `session.update` tools only with interaction on, strip it from `session.updated`; taps for activity (`:334-389`); `mcp_call` rewrite on every event carrying the item |
| `gateway/src/handlers/openai_realtime/relay_verbs.rs` | new | Providers and endpoints | R3 | `RelayVerbExecutor` |
| `gateway/src/handlers/openai_realtime/session.rs` | modified | Providers and endpoints | R3 | relay verb arm next to the select loop (`:1022`); relay output gate; GPT-Live vendor refusal naming `/ws`; deployment interaction settings |
| `gateway/src/handlers/realtime/handler.rs` | modified | Duplex audio, Providers and endpoints | R3 | ingress tap before `provider.send_audio` (`:396-403`); regime B provider audio into the pump with a `RealtimeMessageRoute::Audio` sink (`:832-840`); regime C ungated render tap / interaction branch after connect (runtime host with the verb handle and the event tap); verb messages through the handle; `session_update_plan` errors (`:993-1046`); refuse `interaction` inside `update_session` (`:1013-1026`); `waav_delegate` interception forwards, never executes, in the callback (`:859-877`); `behavior` in `map_tools` (`:1156`); forced transcription in interaction mode (`:1089-1095`); GPT-Live key lookup and canonical name (`:692-729`, `:1055-1069`) |
| `gateway/src/handlers/realtime/messages.rs` | modified | Providers and endpoints | R1 | `RealtimeSessionConfig.interaction: Option<Value>` and `model_family` (Azure); new incoming verbs and task messages; new outgoing task, floor, claim, `verb_result`, `warning`, `function_call_cancelled`; `protocol_version` and `interaction` on `session_created` |
| `gateway/src/handlers/tasks.rs` | new | Tasks | R2, R3 | `POST /v1/tasks/{id}/callback`, `POST /v1/tasks/{id}/a2a-push`, `GET /v1/tasks/{id}`, `GET /v1/tasks?resume_key=` |
| `gateway/src/handlers/ws/agent.rs` | modified | Runtime, Providers and endpoints | R1 | Interaction branch building W3's agent backend; session limit sends `Shutdown` to the runtime; step-down re-entry. / map new runtime signals next to `agent_signal_message` (`:50-121`) |
| `gateway/src/handlers/ws/audio_handler.rs` | modified | Runtime | R1 | `speak` and `clear` route to the runtime when `interaction` is set. |
| `gateway/src/handlers/ws/command_handler.rs` | modified | Providers and endpoints | R3 | LiveKit `waav.interaction` topic through the send-message operation (`:90-98`) |
| `gateway/src/handlers/ws/config.rs` | modified | Tasks | R1 | (W6) `InteractionConfig.tasks` uses `TaskConfig` from this workstream |
| `gateway/src/handlers/ws/config_handler.rs` | modified | Runtime, Tasks, Duplex audio, Providers and endpoints, Evaluation and rollout | R0, R1, R3 | `initialize_conversation_loop` calls `resolve_llm_legs` (`bud_llm_leg` moves into it); one branch per brain to `host::install_ws`; metered pump handed over before the egress callbacks; completion flush moved to the pump's idle signal in interaction sessions; raw-mode and DAG refusals; step-down re-entry. / `find_post_stt_node` (`:3412-3428`) moved to a shared DAG helper so the backend and the `/ws` path use one rule; behaviour unchanged / `deliver_tts_audio` returns `DeliveryOutcome` (`:2517-2610`); the LiveKit forwarder carries the participant identity (`:2930-2974`); build `DuplexEngine`; `/ws` config fields; the DAG drain (`:3318-3346`) is not touched (audio-output DAGs are refused) / accept, parse leniently, validate and resolve `interaction`; Bud-mode refusals extended (`:851-898`); `agent_config_refusal` refuses only an alias that sets `stt`, `tts`, `llm` or `dag_template` together with `agent` (`:840-842`), and stays unchanged for `interaction` (`:811-849`); server `defaults.agent` merge; DAG audio-output scan; echo on `ready` (`:774-790`) / pass `stream_id` to `TurnProfiler::new` (`:2007-2010`) |
| `gateway/src/handlers/ws/handler.rs` | modified | Runtime | R1 | Teardown sends `Shutdown`. |
| `gateway/src/handlers/ws/messages.rs` | modified | Runtime, Policy and HITL, Duplex audio, Providers and endpoints | R1, R2 | Segment marker and additive events; an audio send variant that reports acceptance; the optional `code` field on `error` (Addendum A2 item 9, used for `interaction_failed`); protocol 1.1 (W6). / workstream 6's events (`task_question`, `task_answer_received`, `floor_state` and the task events) out, `task_answer` and `task_cancel` in; optional `assistant_transcript` fields; an optional `code` on the `error` message (`:372-376` has only `message` today; Addendum A2 item 9); `ToSchema` derives; spec regenerated; protocol 1.1 / playback control, playback position, overlap marking (names from W6's table); `send_with_policy` reports the `DroppableAudio` outcome (`:557-568`) / `PROTOCOL_VERSION = "1.1"` (`:37`) and its pin test (`:796`); `Config.interaction: Option<Value>` (`:41-109`); new incoming (`:110-235`) and outgoing (`:276-495`) variants; size checks (`:663-730`); `Ready.interaction`; `AssistantTranscript` optional fields including `sentence`; `Error` gains `code: Option<String>` with `skip_serializing_if` (`:372-376`, Addendum A2 item 9); critical and coalescing egress classes (`:530-589`) |
| `gateway/src/handlers/ws/processor.rs` | modified | Runtime, Providers and endpoints | R1 | `clear`, `truncate`, `agent_input`, `audio_end`, and W6's `task_answer`, `task_cancel`, `interaction_update` route to the runtime when `interaction` is set. / dispatch `task_answer`, `task_cancel`, `interaction_update` (`:80-187`); `Config` destructure gains `interaction` (`:86-96`) |
| `gateway/src/handlers/ws/state.rs` | modified | Runtime | R0 | `interaction: Option<Arc<InteractionRuntime>>`. |
| `gateway/src/routes/api.rs` | modified | Front model, Tasks, Providers and endpoints | R2 | tenant persona store and cache; the persona and memory routes added to `create_api_router` (`routes/api.rs:74`) only when enabled / the callback, A2A push and `GET /v1/tasks` routes are built in a separate router merged in `main.rs` only when the interaction mode is on (the always-mounted routers at `routes/webhooks.rs:13-17` and `routes/api.rs:74`, `:146` are not changed); the merge sits inside `limited_routes` so the governor layer applies (`main.rs:481-488`) / the persona routes beside the protected routes in `create_api_router` (`:74`), mounted only when the `interaction` switch and `front_model.tenant_personas.enabled` are on |
| `gateway/src/routes/webhooks.rs` | modified | Tasks | R2 | the callback, A2A push and `GET /v1/tasks` routes are built in a separate router merged in `main.rs` only when the interaction mode is on (the always-mounted routers at `routes/webhooks.rs:13-17` and `routes/api.rs:74`, `:146` are not changed); the merge sits inside `limited_routes` so the governor layer applies (`main.rs:481-488`) |

## Gateway: configuration, state, startup, observability, API docs and other

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/docs/openapi.yaml` | modified | Front model, Policy and HITL, Providers and endpoints | R0, R1, R2 | optional `front_model` readiness object; handler passes supervisor state; schema regenerated (and again for the Release 2 routes) / workstream 6's events (`task_question`, `task_answer_received`, `floor_state` and the task events) out, `task_answer` and `task_cancel` in; optional `assistant_transcript` fields; an optional `code` on the `error` message (`:372-376` has only `message` today; Addendum A2 item 9); `ToSchema` derives; spec regenerated; protocol 1.1 / register `InteractionConfig` and new message schemas; regenerate with `BLESS_OPENAPI=1` |
| `gateway/src/config/env.rs` | modified | Evaluation and rollout | R5 | `("infer", "INFER_REALTIME_URL")` in `REALTIME_ENDPOINT_OVERRIDE_ENVS` |
| `gateway/src/config/front.rs` | new | Front model | R0, R2 | `FrontServerConfig`, parsing the `front_model` YAML key and `WAAV_FRONT_*` env (including `WAAV_FRONT_LICENCE`), startup validation (endpoints, slot ranges, attestation, budgets); `memory`, `tenant_personas` and `pool` blocks |
| `gateway/src/config/mod.rs` | modified | Front model | R0 | `pub mod front;` only; `ServerConfig` unchanged |
| `gateway/src/core/alias/mod.rs` | modified | Providers and endpoints | R1 | `AliasDefinition.interaction: Option<Value>` (`:153`) |
| `gateway/src/core/metrics/bridge.rs` | modified | Tasks | R1 | task metrics, including the speculative-start and streamed-answer counters (beside `record_async_tool`, `:555-557`) |
| `gateway/src/core/mod.rs` | modified | Runtime | R0 | `pub mod interaction;` |
| `gateway/src/core/observability/profiler.rs` | modified | Evaluation and rollout | R0 | one new histogram series in `record_turn` (`:412-440`) |
| `gateway/src/core/observability/turn_profile.rs` | modified | Evaluation and rollout | R0 | `pre_vad_speech_end_ns`, snapshot in `open_turn`, `TurnTrace::vad_speech_end_ns`, `response_latency_from_speech_end_ns` |
| `gateway/src/core/readiness.rs` | modified | Front model | R0, R2 | optional `front_model` readiness object; handler passes supervisor state; schema regenerated (and again for the Release 2 routes) |
| `gateway/src/core/state.rs` | modified | Duplex audio | Lifecycle | text detector only when configured (`:145-146`) |
| `gateway/src/docs/openapi.rs` | modified | Front model, Providers and endpoints | R0, R2 | optional `front_model` readiness object; handler passes supervisor state; schema regenerated (and again for the Release 2 routes) / register `InteractionConfig` and new message schemas; regenerate with `BLESS_OPENAPI=1` |
| `gateway/src/init.rs` | modified | Front model, Duplex audio | Lifecycle, R0 | `InitOptions`; per-asset steps; front step independent of `turn-detect`; `LICENSE` and `NOTICE` beside LFM weights / gated on `any(turn-detect, smart-turn, silero-vad)`; pre-bakes enabled models under versioned names with pinned hashes; no LiveKit assets by default |
| `gateway/src/livekit/client/events.rs` | modified | Duplex audio | R1, R2 | per-participant tap before the noise filter (`:199-228`); callback carries the participant identity; participant kind *(inferred SDK support)* |
| `gateway/src/main.rs` | modified | Runtime, Front model, Tasks | R0, R2 | Records the configuration source chosen at `:155-160` before `AppState::try_new` (`:208`). / parse front config; start the supervisor before serving; stop it after `axum::serve` returns (next to `policies.shutdown()`, `:569-571`); `Init` arguments; hidden `front-guard` and `front-render` subcommands / the callback, A2A push and `GET /v1/tasks` routes are built in a separate router merged in `main.rs` only when the interaction mode is on (the always-mounted routers at `routes/webhooks.rs:13-17` and `routes/api.rs:74`, `:146` are not changed); the merge sits inside `limited_routes` so the governor layer applies (`main.rs:481-488`) |
| `gateway/src/observability/interaction_attrs.rs` | new | Evaluation and rollout | R0 | attribution names outside `voice_attrs::ALL` (turn, wait and session groups, including time to answer, speculative outcome and waste, acknowledgement due time and skip, resolved front profile and attestation), `interaction_wait_span!`, declare-every-name tests |
| `gateway/src/observability/mod.rs` | modified | Evaluation and rollout | R0 | attribution names outside `voice_attrs::ALL` (turn, wait and session groups, including time to answer, speculative outcome and waste, acknowledgement due time and skip, resolved front profile and attestation), `interaction_wait_span!`, declare-every-name tests |
| `gateway/src/observability/voice_attrs.rs` | modified | Evaluation and rollout | R0 | `voice_turn_span!` field list gains the two turn attribution names as `Empty` fields; `ALL` unchanged |
| `gateway/src/plugin/builtin/mod.rs` | modified | Providers and endpoints | R2 | register `gpt_live`; Hume removal after the date (test at `:2059` extended) / `STTProvider::DeepgramFlux`, parse `deepgram-flux` (`core/stt/mod.rs:291`, `:397`), the supported-provider list and its tests; metadata and constructor beside `deepgram` (`plugin/builtin/mod.rs:63`, `:1666`) |
| `gateway/src/plugin/dispatch.rs` | modified | Evaluation and rollout | R5 | `BuiltinRealtimeProvider` variant and `REALTIME_PROVIDER_MAP` entry |
| `gateway/src/plugin/ffi_adapters.rs` | modified | Providers and endpoints | R3 | report `ProviderCapabilities::unknown` (`:646`) |
| `gateway/src/state/mod.rs` | modified | Runtime, Front model, Tasks | R0, R1, R2 | `AppState.interaction: Arc<InteractionServerConfig>`, filled in `AppState::try_new` (`:302`, built at `:552`; `new` at `:296` wraps it). / `AppState.front_model: OnceLock<Arc<FrontService>>` (one constructor) / `AppState.tasks: Arc<OnceCell<Arc<TaskSupervisor>>>` (beside `batch_jobs`, `:114`), filled on the first task; empty on interaction-off pods; `AppState.task_redis: Option<TaskRedis>` handed to W2's memory port |

## Gateway: tests and fixtures

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `gateway/tests/agent_barge_in_span.rs` | new | Evaluation and rollout | R0 | the barge-in attribute lands on the agent turn span |
| `gateway/tests/agent_turn_span.rs` | modified | Evaluation and rollout | R0 | the barge-in attribute lands on the agent turn span |
| `gateway/tests/fixtures/front/` | new | Front model | R0 | the benchmark's verbatim outputs as guard and rule fixtures; the red-team items; the frozen offline sets; `front_act_contract.json` |
| `gateway/tests/fixtures/mod.rs` | modified | Front model | R0 | the benchmark's verbatim outputs as guard and rule fixtures; the red-team items; the frozen offline sets; `front_act_contract.json` |
| `gateway/tests/interaction_mode_off_golden.rs` | new | Runtime | R0 | Golden traces: conversation, agent, and conversation with the pump switch on; captured on `main` first. |
| `gateway/tests/interaction_runtime_e2e.rs` | new | Runtime | R1 | End-to-end with mock STT, TTS (one per boundary class), a streaming mock LLM and a scripted front. |
| `gateway/tests/interaction_tasks_*.rs` | new | Tasks | R1, R2, R3 | integration tests (section 5) |
| `gateway/tests/openapi_drift.rs` | modified | Providers and endpoints | R3 | pin `"1.1"` (`:128-137`) |
| `gateway/tests/realtime_full_integration.rs`<br>`gateway/tests/realtime_provider_matrix.rs` | modified | Evaluation and rollout | R5 | fix the 12-versus-11 provider drift (with W6, which also removes Hume) / provider lists bumped |
| `gateway/tests/turn_profile_attribution.rs` | new | Evaluation and rollout | R0 | telemetry tests |

## Gateway: build, packaging, deployment and CI

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `.github/workflows/ci.yml` | modified | Duplex audio, Evaluation and rollout | Lifecycle | accuracy job provisions with `turn-ensemble`; `turn_detect_test` in its own `turn-detect` job; matrix comment (`:76-81`, `:263-269`) (Lifecycle); duplex unit tests in every feature set including `""` (1) / add `voice_span_contract`, `turn_profile_attribution`, `agent_turn_span`, `agent_barge_in_span`, `realtime_provider_matrix`, `realtime_full_integration` to `integration-mock` (`:282`); new jobs `interaction-eval-pr-scripted` (Redis service, `working-directory` override; the gateway's own `scripted` profile), `interaction-eval-pr-local-front` (on the `[self-hosted, gb10]` GPU runner: the Qwen3.5-2B candidate forced with `allow_unqualified`, no attestation; sign-off 4 as reworded by Addendum C8), `model-register-check`, all added to `required.needs` (`:375-385`); the reference job `interaction-eval-lfm` (`lfm-standard` and `lfm-large`, prompted and fine-tuned checkpoints) is opt-in, runs only when `WAAV_EVAL_FRONT_LICENCE` is set, and stays out of `required.needs` until legal answers sign-off 4 |
| `.github/workflows/interaction-eval-nightly.yml` | new | Evaluation and rollout | R1 | nightly stress (one run per candidate checkpoint; LFM checkpoints only where attested), load on each front placement, live matrix with the floor and speculative experiments, and SIP on a `[self-hosted, gb10]` runner, reusing waav-infer's pattern (`waav-infer/.github/workflows/live-matrix.yml:13`, `:30`); weekly full matrix |
| `.github/workflows/release.yml` | modified | Front model | R0, R1 | pinned llama-server stage; `FRONT_PROFILE` build argument for `init`; `-front-lfm` and `-front-apache` variants pushed beside the plain tag |
| `deploy/front-pool/`<br>`deploy/helm/waav-front-pool/` | new | Front model | R2 | pool image Dockerfile and entrypoint; StatefulSet, headless Service and Helm chart |
| `gateway/Cargo.toml` | modified | Front model, Duplex audio | Lifecycle, R0, R4 | `libc` as a direct dependency; in Release 4 only, an optional edge feature `front-model-inproc` (`llama-cpp-2`), added to the CI matrix and clippy when it lands / `turn-ensemble` without `turn-detect` (`:113`) (Lifecycle); features `aec` with dependency `sonora` (2), `krisp-ip` (4), `duplex-model` (Later) |
| `gateway/Dockerfile` | modified | Front model, Duplex audio | Lifecycle, R0, R1 | pinned llama-server stage; `FRONT_PROFILE` build argument for `init`; `-front-lfm` and `-front-apache` variants pushed beside the plain tag / production features unchanged in name, now without the text detector; comment at `:14-18` updated; `init` pre-bakes versioned Smart Turn and Silero files |

## bud-auth crate (Bud agent projection)

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `bud-auth/src/endpoint_config.rs` | modified | Providers and endpoints | R3 | `RealtimeSettings.interaction: Option<Value>` and `model_family: Option<String>` (`:298-309`); `KNOWN_REALTIME` gains both (`:414`); `RealtimePolicy` gains `legacy_wire: Vec<String>` (section 9.2) |
| `bud-auth/src/voice_agent.rs` | modified | Front model, Tasks, Policy and HITL, Providers and endpoints | R1, R2 | `FrontPersonaEntry`; `front` in `KNOWN_FIELDS`; parsed with `block(..)`; the shared budapp fixture updated / `background` block (with `side_runs`, `voice_approvals`, `answer_marker`), parsed leniently, added to `KNOWN_FIELDS` (`:330-352`) / optional `interaction` block (raw JSON, parsed in the gateway; Release 1 sessions may instead use the server's `interaction.defaults.agent` or an alias) and per-tool `confirmation_templates` (Release 2), added to `KNOWN_FIELDS` (`:330-354`) and parsed leniently like other blocks; the fixture is shared byte for byte with budapp's publisher (`:455-461`), so budapp must publish the fields (cross-team list) / `VoiceAgentEntry.interaction: Option<Value>`; `parse_voice_agent_blob` fills it (`:360-410`); `KNOWN_FIELDS` gains `interaction` (`:332`) |

## Client SDKs

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `clients_sdk/python/`<br>`clients_sdk/typescript/`<br>`clients_sdk/widget/` | modified | Providers and endpoints | R1 | section 3.13 |
| `clients_sdk/python/bud_waav/ws/session.py` | modified | Evaluation and rollout | R0 | typed `agent_*` events, a `truncate` sender, and a documented measurement mode (no keepalive, no reconnection, fail on disconnect) |

## Evaluation harness (new project, outside the gateway crate)

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `eval/config/*.toml`<br>`eval/matrix/core.yaml`<br>`eval/matrix/full.yaml`<br>`eval/parity/intentional_diffs.yaml`<br>`eval/slo.yaml` | new | Evaluation and rollout | R0 | service levels, run modes, live-matrix definitions, parity ledger |
| `eval/duplex_eval/adapters/ga_realtime.py`<br>`eval/duplex_eval/adapters/livekit.py`<br>`eval/duplex_eval/adapters/native_realtime.py`<br>`eval/duplex_eval/adapters/sip.py`<br>`eval/duplex_eval/adapters/ws.py` | new | Evaluation and rollout | R0 | transport adapters (section 3.3), flushing on `agent_audio_stopped` in protocol 1.1 sessions |
| `eval/duplex_eval/analysis/ack_rule.py`<br>`eval/duplex_eval/analysis/acts.py`<br>`eval/duplex_eval/analysis/answer_window.py`<br>`eval/duplex_eval/analysis/asr.py`<br>`eval/duplex_eval/analysis/binding.py`<br>`eval/duplex_eval/analysis/entities.py`<br>`eval/duplex_eval/analysis/evidence.py`<br>`eval/duplex_eval/analysis/judges.py`<br>`eval/duplex_eval/analysis/metrics.py`<br>`eval/duplex_eval/analysis/report.py`<br>`eval/duplex_eval/analysis/routing.py`<br>`eval/duplex_eval/analysis/segments.py`<br>`eval/duplex_eval/analysis/speculative.py`<br>`eval/duplex_eval/analysis/stats.py`<br>`eval/duplex_eval/analysis/streamed.py`<br>`eval/duplex_eval/analysis/tone_decode.py`<br>`eval/duplex_eval/analysis/vad.py`<br>`eval/duplex_eval/analysis/verdict.py`<br>`eval/duplex_eval/analysis/voice.py`<br>`eval/duplex_eval/analysis/waits.py` | new | Evaluation and rollout | R0 | scoring (section 3.6), including speculative waste and side effects before adoption, streamed-relay conformance, draft leak and the `no_marker`, `draft_dropped` and marker-then-tool counts, the acknowledgement rule recomputed from the timeline, first sound in fast mode, dialogue-control routing and answer onsets |
| `eval/duplex_eval/clock.py`<br>`eval/duplex_eval/echo.py`<br>`eval/duplex_eval/interaction_echo.py`<br>`eval/duplex_eval/pacer.py`<br>`eval/duplex_eval/playout.py`<br>`eval/duplex_eval/recorder.py` | new | Evaluation and rollout | R0 | clock, pacing, reference playout, stereo recording, echo profiles; reading W6's `interaction` echo (effective `PolicyConfig` and `policy_hash`, resolved front profile, attestation and placement, effective `duplex.barge_in` values) and verifying the hash |
| `eval/duplex_eval/experiments/ack_floor.py`<br>`eval/duplex_eval/experiments/answer_window.py`<br>`eval/duplex_eval/experiments/speculative_ab.py` | new | Evaluation and rollout | R1 | the Release 1 experiments of decisions 2.19, 2.21 and 2.24 (speculative start, acknowledgement floor, fast-mode arms, answer window), each with its pre-registered decision rule |
| `eval/duplex_eval/mocks/bud_plane.py`<br>`eval/duplex_eval/mocks/canned_tts.py`<br>`eval/duplex_eval/mocks/front_mock.py`<br>`eval/duplex_eval/mocks/mcp_tool_server.py`<br>`eval/duplex_eval/mocks/oracle_stt.py`<br>`eval/duplex_eval/mocks/otel-collector.yaml`<br>`eval/duplex_eval/mocks/push_receiver.py`<br>`eval/duplex_eval/mocks/s2s_ga.py`<br>`eval/duplex_eval/mocks/s2s_gemini.py`<br>`eval/duplex_eval/mocks/task_server.py`<br>`eval/duplex_eval/mocks/tone_tts.py` | new | Evaluation and rollout | R0 | mocks (section 3.5); task faces write the answer marker, stream final answers, carry the draft canary and play the `no_marker`, draft-then-tool and marker-then-tool variants; the Bud plane seeder writes all three interaction sources; caller-memory seeder and persona-registration client are Release 2; the MCP tool server is nightly only |
| `eval/duplex_eval/mocks/caller_memory.py`<br>`eval/duplex_eval/mocks/persona_api.py` | new | Evaluation and rollout | R2 | mocks (section 3.5); task faces write the answer marker, stream final answers, carry the draft canary and play the `no_marker`, draft-then-tool and marker-then-tool variants; the Bud plane seeder writes all three interaction sources; caller-memory seeder and persona-registration client are Release 2; the MCP tool server is nightly only |
| `eval/duplex_eval/stimuli/align.py`<br>`eval/duplex_eval/stimuli/build.py`<br>`eval/duplex_eval/stimuli/clips.py` | new | Evaluation and rollout | R0 | render, align and label stimuli and canned agent clips; provenance manifest |
| `eval/duplex_eval/stress/` | new | Evaluation and rollout | R0 | front stress job (decision 2.17), one report per distillation checkpoint against the rule router and the prompted baseline (decision 2.22) |
| `eval/judges/calibration/v1.jsonl`<br>`eval/judges/prompts/*.txt` | new | Evaluation and rollout | R0 | prompts (section 3.10) and human-labelled calibration sets |
| `eval/loadgen/` | new | Evaluation and rollout | R0 | load generator (section 3.16) |
| `eval/pyproject.toml`<br>`eval/uv.lock` | new | Evaluation and rollout | R0 | new `duplex_eval` project; depends on `bud_waav` by path |
| `eval/registers/technology.yaml` | new | Evaluation and rollout | R0 | licence register, technology register (section 3.17) and their joint check, including the image-variant rules |
| `eval/scenarios/*.yaml`<br>`eval/stimuli/**` | new | Evaluation and rollout | R0 | scenario catalogue (audio form; event form generated), stimuli, canned clips |
| `eval/suites/bba.py`<br>`eval/suites/fdb_v1.py`<br>`eval/suites/fdb_v2/waav_adapter.js`<br>`eval/suites/fdb_v3/`<br>`eval/suites/gametime.py`<br>`eval/suites/mtr.py`<br>`eval/suites/tau_voice/waav_provider.py` | new | Evaluation and rollout | R0 | public-suite adapters (section 3.8) |
| `eval/tests/*.py`<br>`eval/tests/gates/*.py` | new | Evaluation and rollout | R0 | harness self-tests, metric tests, audio gates (section 5) |

## Front-model distillation (new tool, outside the gateway)

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `tools/front-distill/` | new | Front model | R0 | scenario generator, simulator with the Release 0 reference pre-classifier and continuation merge (section 3.10.1 item 2), teacher labelling and validation, the human gate set with its development and held-out splits, teacher kappa (section 3.10.2); SFT and DPO scripts, export; a parallel track from Release 0 (Addenda B2 and D3) |

## Licence register and scripts

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `LICENSES/MODEL_AND_DATA_REGISTER.yaml`<br>`scripts/check_registers.py` | new | Evaluation and rollout | R0 | licence register, technology register (section 3.17) and their joint check, including the image-variant rules |

## WaaV Infer (separate repository)

| File | New or modified | Workstreams | Releases | What changes |
|---|---|---|---|---|
| `waav-infer/.github/workflows/ci.yml` | modified | Evaluation and rollout | R0 | run the register check |
| `waav-infer/LICENSES/MODEL_AND_DATA_REGISTER.yaml` | new | Evaluation and rollout | R0 | the shared register with its version stamp |
| `waav-infer/ci/fetch_models.py` | modified | Evaluation and rollout | R0 | a revision or hash for every pin (current pin format **to verify**) |
| `waav-infer/crates/waav-infer-backend-torch/src/hibiki.rs` | modified | Evaluation and rollout | R5 | a streaming per-frame codec path at the native rate (today `encode_user_audio` takes the whole buffer, `hibiki.rs:1115`) |
| `waav-infer/crates/waav-infer-backend-torch/src/personaplex.rs` | new | Evaluation and rollout | R5 | PersonaPlex arm reusing Hibiki's Moshi components |
| `waav-infer/crates/waav-infer-protocol/src/session.rs` | modified | Evaluation and rollout | R5 | optional `duplex` and `role_prompt` fields, protocol minor bump, new server frames (section 3.13) |
| `waav-infer/crates/waav-infer-server/src/engine.rs` | modified | Evaluation and rollout | R5 | per-model duplex step worker over `as_duplex()` and `SlotBatch`; `personaplex` arm; non-commercial refusal flag |
| `waav-infer/crates/waav-infer-server/src/ws.rs` | modified | Evaluation and rollout | R5 | streaming duplex session beside `finalize_s2s`, holding an admission permit and registered with the watchdog spine |

## Outside this repository

Cross-team changes the plan depends on (section 17 of the plan): budapp publishes `VoiceAgentEntry.interaction`, the
`front` and `background` blocks (including `background.answer_marker`), per-tool confirmation templates,
`RealtimeSettings.interaction` and Azure `model_family`; budprompt adds `durable_engine_enabled` on target
deployments, a per-run context field, approval pauses on the resume tail, push instead of resume tails and a
final-answer marker; budgateway proxies the status and SSE resume `GET`s and returns 429 before creating a run.
