# Chapter 6. Providers and endpoints: capabilities, regimes, verbs, configuration and wire events

## What this part is and why it exists

WaaV reaches callers through many endpoints: the `/ws` WebSocket (conversation, Bud agent, raw and DAG modes, and
LiveKit rooms and SIP calls controlled through it), the native `/realtime` speech-to-speech endpoint, and the
OpenAI-compatible `/v1/realtime` (relay, translating facade and the "GA cascade" that runs a Bud agent). It reaches
many providers: any speech-to-text and text-to-speech pair in the cascade, eleven registered speech-to-speech
providers (`core/realtime/mod.rs:227-241`) and GPT-Live as a twelfth. This part makes the interaction agent work on all
of them. It describes what each provider can do (a **capability descriptor**), picks one of four arrangements (a
**regime**) per session, gives the gateway a small set of **verbs** to steer a speech-to-speech provider, defines the
one `interaction` configuration block, every client-visible event and how each task event maps onto them, fixes
provider wires that are wrong today, and absorbs dated vendor retirements.

Without it, the runtime and policy of the other chapters would run only as a cascade on `/ws`. Speech-to-speech
sessions would talk over callers (six of eleven providers send no caller speech-stop event; only the shared OpenAI
mapper does, `core/realtime/openai/protocol.rs:335`), say acknowledgements twice, lose results across reconnects,
and let the provider claim "your refund went through" unchecked (in the ConvFill study, fast talkers contradicted the
slower reasoner in 14 to 19% of fillers). Screens would show "cancelled" for a task that had booked a table, Hume
sessions would break on 2026-11-13, and OpenAI defaults would point at retired models.

## How it works

Terms: workstreams 1 to 7 (W1 to W7 in the artefacts) are the seven chapters: 1 runtime, 2 front model,
3 background tasks, 4 policy and human in the loop, 5 duplex audio, 6 providers and endpoints, 7 evaluation and
rollout. **E** is the confirmed end of the caller's turn. Regime **A** is the cascade (WaaV owns the voice), **B** a
speech-to-speech provider speaking under gateway control, **C** a provider that is itself the front with WaaV as its
background, **D** native duplex on WaaV Infer. A **provisional run** is a background run started before anyone has
decided the turn is a task; **adoption** makes it the task without restarting it. The **half-cascade** is an
OpenAI-family session whose own replies are switched off: the provider only listens, WaaV decides every word and
WaaV's text-to-speech speaks them.

**Capability descriptor.** Each provider protocol computes `ProviderCapabilities` from the session configuration,
because support differs by model. It records who owns turns and voice; `half_cascade_possible` (text output **and** a
documented reply switch: only the OpenAI family); for each verb (`say_exact`, `say_guided`, `add_context`,
`request_reply`, `stop_output`, `hold_floor`, plus plain user text) whether it is native, emulated at a stated cost,
or absent; how background work can return; its fillers and listening sounds and how to switch them off; activity
signals; session limits; lifecycle. The model is the one the code sends: Grok's empty model is
`grok-realtime` (`core/realtime/grok/protocol.rs:49-53`). An Azure deployment name hides the model
(`core/realtime/azure/protocol.rs:36-38`), so Azure gains an optional `model_family` field.

**Regime rule.** A pure function `select_regime(surface, caps, config, server, today)` picks the regime. An operator
may only narrow it: a forced regime is honoured only when the provider can run it; GPT-Live, Gemini 3.8 Live and Nova 2
Sonic can only delegate; `auto` owners become `provider` in a forced delegating regime; anything not honoured keeps the
automatic regime with `config_warning` `regime_downgraded`. Regimes B and C run only for providers the operator lists
in the server-side `speech.truth_ack` (empty by default); otherwise the session runs exactly as today with
`truth_ack_required`.

| Provider (default model) | Regime | Confirmations | Truth |
|---|---|---|---|
| Any STT and TTS on `/ws`, GA cascade, text-output DAG | A | exact, WaaV TTS | enforced |
| OpenAI family (OpenAI, Azure OpenAI, Yandex) | B, or half-cascade (`voice_owner: gateway`) | client UI, else gateway clip, else guided and checked (reversible only) | monitored (enforced as half-cascade) |
| Grok (`grok-realtime`), Inworld, Deepgram Voice Agent, Ultravox | B | native exact speech | monitored |
| ElevenLabs Agents | B, limited (no exact speech) | gateway clip in the agent's voice when reproducible | monitored |
| Gemini 3.8 Live (both), Gemini 3.1 (degraded), Nova 2 Sonic, GPT-Live | C | client UI, else introduced different-voice clip, else guided (reversible only) | monitored |
| Hume (retiring), Infer native duplex (no injection channel) | refused | | |

**Verbs and the executor.** One executor task per live provider session turns verbs into wire frames through a
cloneable handle, never a lock on the provider, so audio frames never wait for verbs. It keeps an ordered outbox across
reconnects (32 entries), one speech slot, connection epochs (a result for an old call id takes a call-free path), an
**output gate** before the event tap that drops provider audio on providers with no cancel (held at most 8 s), and
activity state fed first by the gateway's own voice-activity detection.

**Provider voice and truth.** `ProviderVoice` implements workstream 1's `SpeechPort` over the handle and chooses, per
line (by `SpeakPurpose`, such as `Ack`, `Confirmation` or `Relay`), native exact speech, a gateway clip (gateway TTS
while the gate holds the provider), guided speech with an **exact-words check** (normalised entity sets: "Saturday the
10th at 7 p.m." matches "2026-10-10 19:00"), or the client UI. Irreversible confirmations are never guided. A
`ClaimMonitor` reads the provider's output transcript and sends one `ClaimVerdict` per completion claim or unsourced
entity to workstream 4, which alone words the correction; this part emits `unverified_claim`. In B and C the caller's
answer comes from the forced-on input transcript, joined to workstream 5's local voice segment for attribution.

**Background at E and streamed answers, per regime.** In A, no client event exists for a provisional run;
`task_created` (with `speculative: true`) is sent at adoption, and each relayed sentence (workstream 3's
`AnswerSentence`) becomes one `assistant_transcript` with `verbatim: true` and `sentence {attempt, seq}`. A Bud agent
streams only when `background.answer_marker` is declared (by budapp, or by the operator for an agent known to emit
it); otherwise its answer is relayed whole at completion, about 0.4 to 1 s later than today (Addendum D5). In B and C
the provider decides by its own call whether a turn is a task. Deepgram's eager-window calls and Gemini's withdrawable
calls become provisional runs (`start_provisional` with origin `ProviderCall { adopt_by }`), adopted when the
provider's turn ends (Deepgram `AgentThinking`; Gemini `turnComplete`) and discarded silently on withdrawal; Bud and
other costly backends hold the call until adoption. In B the first sentence goes out alone (on Grok as one
`force_message`), then every sentence released since (Deepgram queues two); C delivers whole results because the
provider rewords them anyway.

**Acknowledgement timing.** Workstream 4's rule: due at E + 300 ms (E + 700 ms and skipped if the first answer
sentence is ready, on fast backends); hard limit E + 2,000 ms; cached clips by default. In B this part defines E (the
gateway VAD's speech stop) and skips when no delegate call came or the provider spoke after E. In C the provider owns
acknowledgements.

**Configuration.** One `interaction` block (`true` or an object; absent means off) is carried as raw JSON in every
home: the `/ws` envelope, `/realtime` config, aliases, Bud's `VoiceAgentEntry`, Bud realtime deployments, DAG
definitions. One lenient parser drops each bad field with a `config_warning` naming its path; a typed block would make
one unknown value fail the whole `/ws` config (`handlers/ws/handler.rs:713-727`). Server fields live in one
`interaction` YAML key: `speech.truth_ack`, `defaults.agent` and `defaults.realtime` (operator defaults for Bud agents
in Release 1 and Bud realtime deployments in Release 3, budapp's field winning when present), workstream 3's task
catalog under `interaction.tasks.*` and workstream 5's server-only `interaction.duplex.compute.*`, each refused from
sessions. Caller memory and tenant personas live under workstream 2's `front_model` key; a session carries only the
operator-signed `interaction.front.caller.identity_token` (workstream 2's format; a bad one gives
`caller_identity_rejected`). The licence attestation is server-only (`front_model.licence`). The operator may declare
an answer marker as `background.answer_marker` in `interaction.defaults.agent` (agents in `defaults.agent_ids`) or in
an agent alias's `interaction` (an alias names no agent: only if every agent used with it emits the marker); budapp's
own field wins; a session sending `background` gets `interaction_override_refused` (Addendum D5). This part owns the
`config_warning` list; a caller who switches to an unsupported language is stepped down only on `/ws` conversation
and agent sessions.

**Events and the task-event mapping.** Identical JSON bodies on `/ws`, LiveKit (topic `waav.interaction`) and
`/realtime`, prefixed `bud.` on `/v1/realtime`, protocol 1.1: `task_created`, `task_progress`, `task_question`,
`task_answer_received`, `task_result`, `task_cancelled`, `floor_state`, `unverified_claim`, `agent_audio_stopped`,
`audio_segment` (not on `/realtime`), opt-in `playback_control` and `overlap`; client messages `task_answer`,
`task_cancel`, `interaction_update`, `playback_position`. The `/ws` `error` gains an optional `code`
(`question_closed`; `handlers/ws/messages.rs:372-376` has only `message` today). Every workstream 3 task event maps to
one client event: progress, revisions and stopped answers are `task_progress`; a cancel that is requested or
unconfirmed is `task_progress` with `cancelling: true` and the state unchanged; only a confirmed `Cancelled` is
`task_cancelled`; `Completed`, `Failed`, `Rejected`, `Unknown` and `StaleOutcome` (with `superseded: true`) are
`task_result`. Results and cancellations carry `effects`, so no screen shows "cancelled" for a task that committed
something. The session echo carries the resolved settings, `policy_hash` among them.

**Worked example (Release 1, `/ws` cascade, LLM background; times are estimates).**
1. 0 ms, E. Workstream 1 starts a provisional run and asks the front for its act (in Release 1 the rule router
   with the persona's phrase banks, `scripted`).
2. +5 ms. The act is `delegate`; the run is adopted: `task_created{task_ref:"t1", speculative:true, state:"working"}`.
3. +300 ms. No sentence is ready, so the cached acknowledgement clip plays:
   `audio_segment{phase:"start", kind:"ack"}`, `assistant_transcript{source:"gateway", kind:"ack"}`.
4. +900 ms. The first answer sentence passes workstream 3's checks; workstream 4 opens the relay behind the clip.
5. +1,000 ms. The clip ends; the relay plays: `assistant_transcript{source:"background", kind:"relay",
   verbatim:true, sentence:{attempt:0, seq:0}}`. +1,250 ms, sentence 1. +1,600 ms, `task_result{state:"completed",
   streamed:true, effects:"none_committed"}`.
With a 600 ms median time to the first sentence, the acknowledgement would be skipped.

**Key numbers.** OpenAI shuts down `tts-1` and its family on 2027-01-06,
`gpt-realtime` on 2027-01-20, `whisper-1` and the `gpt-4o-transcribe` family on 2027-02-26 (fact-checked); WaaV's
defaults are exactly those models (`core/tts/openai/config.rs:23-25`, `core/realtime/openai/config.rs:75-78`,
`core/stt/openai/config.rs:61-67`). Deepgram Flux ends a turn in about 260 ms; its eager mode costs 50 to 70% more LLM
calls. GPT-Live costs $0.05 per minute. Front model (measured, GB10): LFM2.5-1.2B act p95 70 ms on a shared GPU; no
model meets the act target on 6 ARM CPU threads; first spoken chunk 575 to 656 ms p95, so acknowledgements are clips.
Regimes C and D are the routes to Thinking Machines Lab's 0.40 s; the cascade costs about 0.5 s more. Outbox, slot,
gate and `adopt_by` values (3,000 ms Deepgram, 10,000 ms Gemini) are estimates to be measured.

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| Where capabilities live | computed per protocol and model | support differs by model inside one provider | one required method on 12 protocols plus test protocols |
| Regime selection | deterministic rule; operator may only narrow | deciding facts are static and fact-checked; no voice changes mid-call | no per-turn switching |
| Half-cascade | only where text output and a reply switch exist (OpenAI family) | Ultravox's model would still word its own replies | Ultravox stays monitored |
| Verb access | executor task behind a cloneable handle | surfaces own the provider in four incompatible ways; no lock on the audio path | a second task per session |
| Truth in B and C | monitored, one pipeline, per-provider operator acknowledgement | only WaaV-written words can be guaranteed | the "never untrue" goal is weaker there (sign-offs 7, 10) |
| Configuration | raw JSON in every home, one lenient parser | `bud-auth` cannot name gateway types; one bad value must not kill a session | hand-written parsing |
| Memory and persona settings | under `front_model`; the session token path is this part's | one owner per setting | two keys to read |
| Bud agents in Release 1 | server default plus interaction-only alias; both may declare the answer marker | budapp is the single writer of agent entries | narrower `agent_config_refusal`; agents without a marker answer later (sign-off 13, Addendum D5) |
| Provider speculation | Deepgram and Gemini calls become provisional runs | one side-effect gate; withdrawn calls stay invisible | adoption timers per provider |
| Unconfirmed cancels | `task_progress`, `cancelling: true` | no "cancelled" before it is true | clients wait for the final event |
| Azure model | optional `model_family`; unknown: gateway fillers | the deployment name is arbitrary | one more field |
| GPT-Live reach | `/realtime` (non-Bud) in Release 3, `/ws` in Release 4 | GA clients need `response.done` per utterance | Bud waits for Release 4 |
| OpenAI TTS after 2027-01-06 | probe; move the default or refuse; never a realtime model | a realtime model paraphrases | tenants may need another TTS (sign-off 22) |

## What changes in the code

Paths are under `WaaV/gateway/src` unless they start with `bud-auth/` or `clients_sdk/`.

| File | Function or type | Change | New or modified | Release |
|---|---|---|---|---|
| `core/realtime/capabilities.rs` | `ProviderCapabilities`, lifecycle table, `select_regime` | descriptor with `half_cascade_possible`, narrowing rule, dated lifecycle with injected clock | new | Lifecycle (table), 1 (dark) |
| `core/realtime/hume/*`, `core/tts/hume/*` | lifecycle | `provider_sunset` now, `provider_retired` from 2026-11-13 | modified | Lifecycle, first patch |
| `core/realtime/{gemini,deepgram,nova_sonic,ultravox}/protocol.rs` | wire fixes | tool `behavior`, cancellation, busy, `InjectUserMessage`, `interactive: true`, tools; escape flags | modified | Lifecycle, wire-fix patch |
| `core/{tts,realtime,stt}/openai/config.rs`, `grok/protocol.rs` | defaults | dated default moves; Grok's after a probe | modified | Lifecycle, dated |
| `core/interaction/contracts.rs` | regimes, event bodies, task-event mapping, warning codes, paths | contract entries and two goldens | new (shared) | 0 |
| `core/interaction/config.rs`, `server_config.rs` | `InteractionConfig`, `InteractionServerConfig` | lenient parse, precedence, server-only refusals, echo | new | 0 (dark), 1 |
| `handlers/ws/{messages,processor,config_handler}.rs` | `Config.interaction`, events, `Error.code`, `agent_config_refusal` | `/ws` home, events, 1.1, alias change | modified | 1 |
| `bud-auth/src/voice_agent.rs`, `core/alias/mod.rs` | `VoiceAgentEntry`, `AliasDefinition` | `interaction: Option<Value>`; marker precedence: entry, then alias, then server | modified | 1 |
| `core/realtime/verbs.rs`, `scaffold/{executor,session,protocol}.rs` | `RealtimeCommand`, `RealtimeVerbHandle` | verbs, outbox, slot, epochs, gate, shared config | new and modified | 1 (dark) |
| `clients_sdk/{typescript,python,widget}` | config, events, task UI | `/ws` events, answer, cancel, "stopping" state | modified | 1; `bud.*` helper 2 |
| `handlers/openai_realtime/cascade.rs` | GA mapper | `bud.*` arms before `_ => {}` (`:1134`) | modified | 2 |
| `routes/api.rs`, `handlers/interaction_personas.rs` | persona route | `PUT/GET/DELETE /v1/interaction/personas/{id}` | new | 2 |
| `core/stt/deepgram_flux.rs`, `core/stt/mod.rs`, `plugin/builtin/mod.rs` | `deepgram-flux` | `/v2/listen` client, turn events | new and modified | 2 |
| `core/realtime/base.rs`, `azure/protocol.rs`, `bud-auth/src/endpoint_config.rs` | `RealtimeConfig.model_family` | Azure model family | modified | 3 |
| `core/interaction/{provider_voice,claim_monitor,provider_prompts,provider_provisional}.rs` | `ProviderVoice`, `ClaimMonitor` | regimes B and C | new | 3 |
| `core/realtime/gpt_live/*`, `core/realtime/delegate_tool.rs` | GPT-Live adapter, `waav_delegate` | new provider; delegate tool per family | new | 3 |
| `handlers/realtime/handler.rs`, `handlers/openai_realtime/*` | interaction branches | `/realtime`, `/v1/realtime`, `bud.*` interception, relay executor | modified and new | 3 |
| `dag/definition.rs`, `dag/context.rs` | `has_audio_output` (new), `DagOutput::Event` | refusal scan; text-output DAG events | modified | 1 (scan), 3 |

## What this part gives to and needs from the other parts

**Gives**: the four regimes; `ProviderCapabilities`; `RealtimeVerbHandle` and `ProviderVoice` as the `SpeechPort`
for regimes B and C; `ClipRenderer`; `ClaimVerdict` through `PolicyInput::ProviderSpeech`; `InteractionConfig`, the
path table and every `config_warning` code; the task-event mapping, the wire-names table and the echo; E for regimes B
and C; the Flux adapter.

**Needs**: from workstream 1, `SpeechPort` (`mark_stopped -> PumpStopReport`, `set_stream_open`),
`SpeakTag.purpose` and `.task`, heard time, the one open relay item, the server-key loader; from workstream 2, the
provider-speech guard, entity normalisation, `PersonaSpec::compile`, `FrontEcho`, profile resolution, the token format
and verifier, `CallerMemory`, the persona registry; from workstream 3, the task events (`AnswerSentence`,
`CancelUnconfirmed`, `StaleOutcome`, `effects`) and `start_provisional(origin, words, adopt_by)` with
`ProviderCall { adopt_by }`; from workstream 4, the question types and signals, `DeliveryPolicy`, correction templates
and the published `PolicyConfig`; from workstream 5, the VAD feed and attribution in B and C, `ClipBank`, the barge-in
block; from workstream 7, the measurements below.

**Other teams (requested dates; Addendum D5 and the plan).** budapp: `VoiceAgentEntry.interaction`,
`background.answer_marker` and the `front` block (Release 1) by 2026-11-02; per-tool confirmation templates,
`background.voice_approvals` and `side_runs` (Release 2) by 2026-11-30; `RealtimeSettings.interaction` and
`model_family` (Release 3) by 2027-03-12. Operator defaults and aliases keep budapp off the critical path.

## Tests to write first

| Test name | Level | What it proves | Release |
|---|---|---|---|
| `realtime_provider_wire_goldens_match_main_except_listed_exceptions` | unit | interaction-off wires unchanged except approved, escape-flagged fixes | Lifecycle |
| `regime_refuses_hume_after_2026_11_13_and_warns_before` | unit | dated lifecycle with injected clock | Lifecycle |
| `wire_names_match_contract_golden` | unit | one set of names, values and task-event mapping | 0 |
| `forced_regime_can_only_narrow_and_warns_regime_downgraded` | unit | the narrowing rule | 1 |
| `half_cascade_needs_text_output_and_a_reply_switch_and_ultravox_is_not_offered_one` | unit | enforced truth only where WaaV decides every word | 1 |
| `bud_agent_interaction_from_server_default_alias_and_entry_precedence` | unit | Bud agents and operator or alias answer markers work before budapp ships | 1 |
| `session_duplex_compute_and_server_task_catalog_are_refused_as_server_only` | unit | one owner per setting | 1 |
| `provisional_run_emits_no_client_event_and_task_created_is_sent_at_adoption` | in-process paused time | speculation is invisible until adopted | 1 |
| `cancel_unconfirmed_is_task_progress_with_cancelling_and_state_unchanged_never_task_cancelled` | unit | truthful screens | 1 |
| `ws_error_carries_optional_code_only_on_interaction_errors_and_old_errors_are_unchanged` | unit | additive error code | 1 |
| `caller_identity_token_with_bad_mac_expiry_or_tenant_disables_memory_only` | unit | memory cannot be read with a forged identity | 2 |
| `azure_without_model_family_resolves_fillers_to_gateway_with_model_family_unknown` | unit | no guessing from deployment names | 3 |
| `claim_monitor_sends_one_provider_speech_verdict_for_completion_claim_without_task` | scripted audio | one detector, one correction | 3 |
| `provider_speculative_call_starts_provisional_run_with_provider_call_origin_and_adopt_by` | in-process paused time | provider speculation on the same gate | 3 |

The regime, owner, warning and wire rules already run in `providers/regime_resolve.py --self-test` (54 cases).

## What ships in which release

Target windows are Addendum D3's (six gateway engineers and one machine-learning engineer assumed).

- **Lifecycle track.** First standalone patch by 2026-10-23: wire goldens recorded on `main`, the lifecycle table,
  Hume warning and refusal, OpenAI retiring warnings, escape-flag plumbing. Wire-fix patch targeted for 2026-11-13, each
  fix when its live probe passes: Gemini tool `behavior`, cancellation and busy events; Deepgram, Nova 2 Sonic and
  Ultravox fixes. Dated defaults moved by each OpenAI shutdown date in the key numbers; Grok's default only after a
  probe.
- **Release 0, measure and lay groundwork (2026-10-05 to 2026-11-06).** Contract entries, goldens and tables; the
  parser built dark.
- **Release 1, cascade interaction on `/ws` (2026-11-09 to 2027-01-29).** Streaming speech-to-text only (Addendum D2):
  a file-only (segmented) speech-to-text session runs with interaction off (`interaction_surface_unavailable`,
  `release: 2`). `interaction` on `/ws`, aliases and Bud agent entries; Bud operator defaults and alias answer markers;
  validation, server-only refusals and warnings; echo (front `scripted` on every deployment); protocol 1.1; `/ws` and
  LiveKit events and `error.code`; DAG audio-output refusal; SDKs and widget. Capabilities, verbs and executor as a
  dark parallel track.
- **Release 2, full duplex audio, the GA cascade and durable tasks (2027-02-01 to 2027-03-12).** Interaction on
  segmented speech-to-text sessions; GA cascade `bud.*` events; `playback_position`; tenant persona route and memory
  identity token (under `front_model`); Deepgram Flux adapter.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs (2027-03-15 to 2027-04-23).**
  Regimes B and C on `/realtime` and `/v1/realtime`: delegate tool, `ProviderVoice`, claim monitor, exact-words check,
  provider speculation, streamed relay in B, GPT-Live on `/realtime` (non-Bud), `truth_ack`,
  `interaction.defaults.realtime`, Azure `model_family`, text-output DAG events.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF (scheduled at the Release 3 exit).**
  `interaction.speech.realtime` brings B and C to LiveKit and SIP, GPT-Live to Bud, and Inworld, Ultravox and Yandex to
  Bud if budapp can name them; `dtmf` channel.
- **Release 5, native duplex on WaaV Infer (conditional).** `NativeDuplex` once Infer can take injected context.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| A wire fix is wrong for a vendor | medium | sessions fail after the patch | each fix gated by a recorded live probe; escape flag restores the old frames |
| Provider claims an outcome it never checked | high on some providers | caller misled | claim monitor, workstream 4 correction, per-provider claim-rate gates, `speech.truth: enforced` |
| Guided speech changes a number | medium on the OpenAI family | wrong detail heard | entity-set check; never for irreversible confirmations |
| Caller's "yes" lost in B or C | medium | question times out | forced input transcription, heard anchor, corroborating `answer` call, attribution join |
| Two acknowledgements | medium | sounds broken | owners per provider; skip when the provider spoke after E |
| Withdrawn provider call shows a task | medium | false "task created" | `task_created` only at adoption |
| A screen says "cancelled" too early | medium | caller misled | unconfirmed cancels stay `task_progress` with `cancelling: true` |
| budapp late | high | Bud sessions blocked | operator defaults for agents and their answer marker (Release 1) and realtime deployments (Release 3) |
| Forged caller identity | low | another caller's facts read | signed token, tenant check, keyed hash, memory off by default |

## What is still open

**Measurements needed.**
- Live probes before code: Deepgram inject fields; Gemini `BLOCKING`, `INTERRUPT`, `name`, call ids after
  resumption; Nova mid-session text; GPT-Live event names; Grok, Inworld, Ultravox exact speech; whether WaaV's xAI
  endpoint serves `grok-voice-think-fast-2.0`; Azure's own retirement dates; Flux messages; OpenAI's speech model list
  by 2026-11-30.
- Exact-words rate of guided speech; how far each output transcript leads its audio; claim-monitor rates.
- Injected-line time to first audio and second-line queueing; provider discard rate and call-to-adoption time.

**Choices for a person (recommended default; sign-off number from decisions section 11 and Addendum A3).**
Needed by (Addendum D4): week 1 (from 2026-10-05), 8 and the first-patch part of 9; before 2026-11-13, the wire-fix
parts of 9; the Release 0 exit review (2026-11-06), 7, 12, 13, 16; before 2026-12-15, 22; before 2027-01-29, 17;
before 2027-03-12, 10, 23, 27.
- Interaction-off changes in the provider wires and protocol 1.1: approve with escape flags (9).
- Early standalone Hume and wire-fix patch: adopt (8).
- Monitored truth in B and C with per-provider `speech.truth_ack`: adopt (7, 10).
- Bud agents in Release 1 through operator defaults and interaction-only aliases, accepting that agents without a
  declared marker answer 0.4 to 1 s later (Addendum D5): yes (13).
- Bud deployments reach Inworld, Ultravox and Yandex only from Release 4, if budapp can name them: accept (27).
- `/realtime` client verbs: gateway runtime only in Release 3, client verbs later (23).
- OpenAI TTS after 2027-01-06: after the 2026-11-30 probe, move the default or refuse; never a realtime model (22).
- Tenant persona route: Release 2, off unless enabled, authenticated only (17).
- Caller-memory identity and retention: memory off; workstream 2's operator-signed token, 90 days when on (16).
- SIP and LiveKit in Release 1 only if the echo gates pass (12).
- Different-voice confirmation clips: `confirmations_only`, introduced (no number).

## Where the detail is

- Design: `/home/bud/ditto/waav/research/duplex-agent/design/W6-providers-endpoints.md` (3.12.4 task-event mapping
  and wire names; 3.19 Bud agents; 13 and 14 alignment with Addenda A to C) and its two critiques beside it.
- Artefacts: `/home/bud/ditto/waav/WaaV/docs/duplex-agent/providers/` (capabilities, resolver, wire events, coverage,
  schemas and examples).
