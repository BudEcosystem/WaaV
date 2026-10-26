# Provider coverage: which regime, which experience, from which release

**How to regenerate.** Everything between the `BEGIN GENERATED` and `END GENERATED` markers is produced from
`provider_capabilities.json` by the executable specification in this folder:

```
cd /home/bud/ditto/waav/WaaV/docs/duplex-agent/providers
python3 regime_resolve.py --write-coverage   # rewrites the generated region in place
python3 regime_resolve.py --matrix           # prints the same region to standard output
python3 regime_resolve.py --validate         # checks the schemas, the examples and wire_events.md
python3 regime_resolve.py --self-test        # fails if this file's generated region is stale
```

Only the sections above the markers are written by hand. Release names are those of
`INTEGRATION_DECISIONS.md` section 1 (R1 = "Release 1, cascade interaction on `/ws`", R2 = "Release 2, full duplex
audio, the GA cascade and durable tasks", R3 = "Release 3, speech-to-speech providers, open task protocols and
text-output DAGs", R4 = "Release 4, realtime voices on phone and rooms, Hindi and DTMF", R5 = "Release 5, native duplex
on WaaV Infer (conditional)"). This folder follows `INTEGRATION_DECISIONS.md` sections 1 to 11 and Addenda A, B and C;
the detailed design is `/home/bud/ditto/waav/research/duplex-agent/design/W6-providers-endpoints.md` (section 14 lists
what the addenda changed).

## What the regimes mean for the caller

- **Regime A, the cascade** (`Cascade`). WaaV owns the turn and the voice. The front decides what each caller turn is
  and says acknowledgements, progress and small talk. In Release 1 that front is the deterministic rule router with
  the persona's phrase banks and clip bank (profile `scripted`) on every deployment, because no prompted small model
  passed the Release 1 front gates in the measured bake-off (Addendum B1); a fine-tuned LFM2.5 (`lfm-standard`, or
  `lfm-large`) takes over once it passes the gates on a server with the licence attestation, else a qualified Apache-2.0
  profile (Qwen3.5-2B is the candidate). The background starts at E in parallel and its answer is relayed verbatim,
  sentence by sentence, in the same WaaV voice. Confirmations are workstream 4 templates spoken exactly. Truth is
  **enforced**: WaaV writes every word. Cost: about 0.5 s more than a native interaction model (decisions section 3.4).
- **Regime B, provider speech** (`ProviderSpeech`). A speech-to-speech provider owns the turn and the voice and its
  own model answers ordinary turns. The gateway controls waits, progress, questions, confirmations and relays through
  the provider's verbs: exact words where the provider has a native verb (Grok, Inworld, Deepgram, Ultravox), guided
  speech with an exact-words check on the OpenAI family, gateway clips on ElevenLabs. Truth is **monitored**: the
  claim monitor detects completion claims and invented entities in what the provider says on its own and workstream 4
  corrects them. The OpenAI family alone can instead run as a **half-cascade** (`speech.voice_owner: gateway` or
  `speech.truth: enforced`): the provider only listens and transcribes, WaaV's front and background decide every word
  and WaaV's text-to-speech speaks them, so truth is enforced (Addendum A2 items 1 and 8).
- **Regime C, delegating** (`Delegating`). The provider is the front: it speaks its own fillers and listening sounds
  and hands work to WaaV, which is the background brain. Results are delivered whole and reworded by the provider
  (**verified paraphrase**); confirmations of irreversible actions go through the client UI, else a gateway clip in a
  different voice introduced by a template, else the question cannot be answered by voice. Closest to the Thinking
  Machines Lab experience (GPT-Live, Gemini 3.8 Live).
- **Regime D, native duplex** (`NativeDuplex`). A local native duplex model on WaaV Infer, driven like regime C once
  it has an injection channel and workstream 7's four criteria hold (Release 5).
- **Regimes B, C and D need the operator's acknowledgement.** They run only for providers listed in the server-side
  `interaction.speech.truth_ack` (default empty, sign-off 10); otherwise the session runs with interaction off and the
  client gets `config_warning` `truth_ack_required`.

## Problems this artefact found, and how the decisions settled them

This artefact raised ten problems; `INTEGRATION_DECISIONS.md` Addendum A2 answered each, and this folder, W6 and
chapter 6 now follow the answers.

1. **Ultravox and the half-cascade.** Ultravox has text output but no documented switch stopping its model wording its
   own replies. *Settled (A2 item 1):* `select_regime` tests `half_cascade_possible` (text output **and** a documented
   reply switch); only the OpenAI family qualifies. W6 renamed the field and narrowed its rule.
2. **Grok's default model.** *Settled (A2 item 2):* the descriptor follows the code, which sends `grok-realtime` when
   the model is empty (`core/realtime/grok/protocol.rs:49-53`); moving the default is a Lifecycle-track item behind a
   live probe.
3. **Caller-memory identity and personalisation switches.** *Settled (A2 item 3, C5):* the session path is
   `interaction.front.caller.identity_token` with `config_warning` `caller_identity_rejected` (W6's names), in the one
   token format W2 pins; the server settings are W2's `front_model.memory.*` (`enabled` false, `identity_key_file`,
   `retention_days` 90) and `front_model.tenant_personas.*` (at most 20 personas with 5 versions each per tenant). The
   schema and the server example moved them; `invalid-memory-under-interaction-key.json` shows the old paths refused.
4. **Where W3's operator catalog lives.** *Settled (A2 item 4):* under `interaction.tasks.*`; W3 renames its keys.
   The resolver refuses them from sessions.
5. **Where W5's compute pool lives.** *Settled (A2 item 5):* server-only `interaction.duplex.compute.*`, refused from
   sessions (`invalid-session-duplex-compute.json`).
6. **Bud deployments and Inworld, Ultravox, Yandex.** *Settled (A2 item 6, A3):* not reachable by Bud before Release 4,
   and then only if budapp can name such a deployment for `interaction.speech.realtime`; this is **sign-off 27**,
   revisited at the Release 3 exit review (section 6 of the generated region).
7. **"May only narrow".** *Settled (A2 item 7):* the resolver's reading is binding: a forced regime is honoured only
   when the provider can run it, the arms that force delegation are exclusive, `auto` owners resolve to `provider` in a
   forced delegating regime, and a cascade surface asked for another regime gets `regime_downgraded`.
8. **The half-cascade's owners and wording.** *Settled (A2 item 8):* WaaV's text-to-speech is the voice
   (`voice_owner: gateway`); owners resolve to filler `gateway`, listening sounds `none`, idle `gateway`.
9. **`question_closed` on `/ws`.** *Settled (A2 item 9):* the `/ws` `error` message gains an optional, additive `code`
   in Release 1 (`wire_events.md`, "error").
10. **Azure deployments hide the model.** *Settled (A2 item 10):* an optional `model_family` on the Azure realtime
    configuration (Release 3). Until it is set, fillers resolve to `gateway` and the session gets `config_warning`
    `model_family_unknown`; the deployment name is never read as a model (`realtime-azure-model-family.json`).

Two more rules from Addendum C are visible here: a caller who switches to an unsupported language mid-call is stepped
down only on `/ws` conversation and agent sessions (C7.1; last column of section 4), and Bud realtime deployments get
the operator default `interaction.defaults.realtime` in Release 3 (C7.3).

<!-- BEGIN GENERATED: python3 regime_resolve.py --matrix -->
Generated from provider_capabilities.json by `python3 regime_resolve.py --matrix` (regime rule evaluated at 2026-10-05; B and C rows assume the provider is listed in `interaction.speech.truth_ack`).

### 1. Default regime and experience per provider and model

Fillers, listening sounds and idle prompts name who makes them (`gateway`, `provider` or `none`). Confirmations are for an action that cannot be undone, on a session with a screen and on a phone call.

| Provider and model | Regime (truth_ack listed) | With the default empty truth_ack | Front | Fillers / listening sounds / idle | Voice | Truth and relay | Irreversible confirmation: screen / phone | First release: non-Bud / Bud / phone and rooms |
|---|---|---|---|---|---|---|---|---|
| Cascade: any speech-to-text, the local front model, any LLM or agent background, any text-to-speech | Cascade | same | local | gateway / gateway / gateway | WaaV TTS | enforced; relays verbatim (WaaV voice), streamed per sentence | exact words, WaaV voice / exact words, WaaV voice | R1 / R1 / R1 (R2 if the echo gates fail) |
| OpenAI Realtime, gpt-realtime-2 family (gpt-realtime-2, gpt-realtime-2.1, gpt-realtime-2.1-mini) | ProviderSpeech | interaction off: `truth_ack_required` (alternative: `speech.truth: enforced`) | narrator templates | provider / none / gateway | provider | monitored; relays guided, exact-words check, first sentence then one batch | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| OpenAI Realtime, older models (gpt-realtime default, gpt-realtime-mini, gpt-4o realtime previews, gpt-realtime-1.5) | ProviderSpeech | interaction off: `truth_ack_required` (alternative: `speech.truth: enforced`) | narrator templates | gateway / none / gateway | provider | monitored; relays guided, exact-words check, first sentence then one batch | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| Azure OpenAI Realtime (deployment-named; not Azure AI Voice Live) | ProviderSpeech | interaction off: `truth_ack_required` (alternative: `speech.truth: enforced`) | narrator templates | gateway / none / gateway | provider | monitored; relays guided, exact-words check, first sentence then one batch | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| Yandex Cloud AI Studio Realtime (OpenAI GA clone) | ProviderSpeech | interaction off: `truth_ack_required` (alternative: `speech.truth: enforced`) | narrator templates | gateway / none / gateway | provider | monitored; relays guided, exact-words check, first sentence then one batch | client UI / gateway clip, different voice (introduced) | R3 / R4 / R4 |
| xAI Grok Voice Agent (OpenAI GA compatible) | ProviderSpeech | interaction off: `truth_ack_required` | narrator templates | gateway / none / gateway | provider | monitored; relays exact words in the provider voice, first sentence then batches | native exact speech, provider voice / native exact speech, provider voice | R3 / R3 / R4 |
| Inworld Realtime (OpenAI GA compatible pipeline) | ProviderSpeech | interaction off: `truth_ack_required` | narrator templates | gateway / none / gateway | provider | monitored; relays exact words in the provider voice, first sentence then batches | native exact speech, provider voice / native exact speech, provider voice | R3 / R4 / R4 |
| Deepgram Voice Agent | ProviderSpeech | interaction off: `truth_ack_required` | narrator templates | gateway / none / gateway | provider | monitored; relays exact words in the provider voice, first sentence then batches | native exact speech, provider voice / native exact speech, provider voice | R3 / R3 / R4 |
| ElevenLabs Agents (agent id carried in the model field) | ProviderSpeech | interaction off: `truth_ack_required` | narrator templates | provider / none / provider | provider | monitored; relays as gateway clips in the agent voice | gateway clip, same voice / gateway clip, same voice | R3 / R3 / R4 |
| Ultravox | ProviderSpeech | interaction off: `truth_ack_required` | narrator templates | gateway / none / gateway | provider | monitored; relays exact words in the provider voice, first sentence then batches | native exact speech, provider voice / native exact speech, provider voice | R3 / R4 / R4 |
| Gemini 3.8 Live Extended Thinking | Delegating | interaction off: `truth_ack_required` | provider | provider / provider / provider | provider | monitored; results whole, reworded by the provider (verified paraphrase) | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| Gemini 3.8 Live (WaaV's default Gemini model) | Delegating | interaction off: `truth_ack_required` | provider | provider / provider / provider | provider | monitored; results whole, reworded by the provider (verified paraphrase) | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| Gemini 3.1 Flash Live and older (degraded) | Delegating | interaction off: `truth_ack_required` | provider | provider / provider / provider | provider | monitored; results whole, reworded by the provider (verified paraphrase) | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| Amazon Nova 2 Sonic | Delegating | interaction off: `truth_ack_required` | provider | provider / none / provider | provider | monitored; results whole, reworded by the provider (verified paraphrase) | client UI / gateway clip, different voice (introduced) | R3 / R3 / R4 |
| OpenAI GPT-Live (gpt-live-1, v1/live/sessions; new adapter) | Delegating | interaction off: `truth_ack_required` | provider | provider / provider / provider | provider | monitored; results whole, reworded by the provider (verified paraphrase) | client UI / gateway clip, different voice (introduced) | R3 / R4 / R4 |
| Hume EVI (EVI 3, EVI 4-mini) - retiring | refused: `interaction_unsupported` | refused | - | - | - | - | - | never / never / never |
| WaaV Infer native full-duplex model (built, not registered) | refused: `interaction_unsupported` | refused | - | - | - | - | - | R5 / R5 / R5 |

### 2. Operator and client overrides

| Provider and model | `speech.voice_owner: gateway` | `speech.truth: enforced` | `regime: cascade` | `regime: provider_speech` | `regime: delegating` |
|---|---|---|---|---|---|
| OpenAI Realtime, gpt-realtime-2 family (gpt-realtime-2, gpt-realtime-2.1, gpt-realtime-2.1-mini) | Cascade (half_cascade) | Cascade (half_cascade) | Cascade (half_cascade) | ProviderSpeech | Delegating |
| OpenAI Realtime, older models (gpt-realtime default, gpt-realtime-mini, gpt-4o realtime previews, gpt-realtime-1.5) | Cascade (half_cascade) | Cascade (half_cascade) | Cascade (half_cascade) | ProviderSpeech | Delegating |
| Azure OpenAI Realtime (deployment-named; not Azure AI Voice Live) | Cascade (half_cascade) | Cascade (half_cascade) | Cascade (half_cascade) | ProviderSpeech | Delegating |
| Yandex Cloud AI Studio Realtime (OpenAI GA clone) | Cascade (half_cascade) | Cascade (half_cascade) | Cascade (half_cascade) | ProviderSpeech | Delegating |
| xAI Grok Voice Agent (OpenAI GA compatible) | ProviderSpeech + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | ProviderSpeech + `regime_downgraded` | ProviderSpeech | Delegating |
| Inworld Realtime (OpenAI GA compatible pipeline) | ProviderSpeech + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | ProviderSpeech + `regime_downgraded` | ProviderSpeech | Delegating |
| Deepgram Voice Agent | ProviderSpeech + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | ProviderSpeech + `regime_downgraded` | ProviderSpeech | Delegating |
| ElevenLabs Agents (agent id carried in the model field) | ProviderSpeech + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | ProviderSpeech + `regime_downgraded` | ProviderSpeech | ProviderSpeech + `regime_downgraded` |
| Ultravox | ProviderSpeech + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | ProviderSpeech + `regime_downgraded` | ProviderSpeech | Delegating |
| Gemini 3.8 Live Extended Thinking | Delegating + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | Delegating + `regime_downgraded` | Delegating + `regime_downgraded` | Delegating |
| Gemini 3.8 Live (WaaV's default Gemini model) | Delegating + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | Delegating + `regime_downgraded` | Delegating + `regime_downgraded` | Delegating |
| Gemini 3.1 Flash Live and older (degraded) | Delegating + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | Delegating + `regime_downgraded` | Delegating + `regime_downgraded` | Delegating |
| Amazon Nova 2 Sonic | Delegating + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | Delegating + `regime_downgraded` | Delegating + `regime_downgraded` | Delegating |
| OpenAI GPT-Live (gpt-live-1, v1/live/sessions; new adapter) | Delegating + `voice_owner_unavailable` | refused: `truth_enforcement_unavailable` | Delegating + `regime_downgraded` | Delegating + `regime_downgraded` | Delegating |
| Hume EVI (EVI 3, EVI 4-mini) - retiring | refused: `interaction_unsupported` (+`voice_owner_unavailable`) | refused: `truth_enforcement_unavailable` | refused: `interaction_unsupported` | refused: `interaction_unsupported` | refused: `interaction_unsupported` |
| WaaV Infer native full-duplex model (built, not registered) | refused: `interaction_unsupported` (+`voice_owner_unavailable`) | refused: `truth_enforcement_unavailable` | refused: `interaction_unsupported` | refused: `interaction_unsupported` | refused: `interaction_unsupported` |

### 3. Owner values: what a requested owner becomes (default regime)

`x` means the request is overridden to `x` with `<kind>_owner_overridden`.

| Provider and model | filler: gateway / provider / none | listening sounds: gateway / provider / none | idle: gateway / provider / none |
|---|---|---|---|
| Cascade: any speech-to-text, the local front model, any LLM or agent background, any text-to-speech | gateway / `gateway` / none | gateway / `gateway` / none | gateway / `gateway` / none |
| OpenAI Realtime, gpt-realtime-2 family (gpt-realtime-2, gpt-realtime-2.1, gpt-realtime-2.1-mini) | gateway / provider / none | `none` / `none` / none | gateway / provider / none |
| OpenAI Realtime, older models (gpt-realtime default, gpt-realtime-mini, gpt-4o realtime previews, gpt-realtime-1.5) | gateway / provider / none | `none` / `none` / none | gateway / provider / none |
| Azure OpenAI Realtime (deployment-named; not Azure AI Voice Live) | gateway / provider / none | `none` / `none` / none | gateway / provider / none |
| Yandex Cloud AI Studio Realtime (OpenAI GA clone) | gateway / provider / none | `none` / `none` / none | gateway / provider / none |
| xAI Grok Voice Agent (OpenAI GA compatible) | gateway / provider / none | `none` / `none` / none | gateway / provider / none |
| Inworld Realtime (OpenAI GA compatible pipeline) | gateway / provider / none | `none` / provider / none | gateway / provider / none |
| Deepgram Voice Agent | gateway / `none` / none | `none` / `none` / none | gateway / `none` / none |
| ElevenLabs Agents (agent id carried in the model field) | `provider` / provider / `provider` | `none` / `none` / none | gateway / provider / `provider` |
| Ultravox | gateway / `none` / none | `none` / `none` / none | gateway / provider / none |
| Gemini 3.8 Live Extended Thinking | `provider` / provider / `provider` | `provider` / provider / `provider` | `provider` / provider / `provider` |
| Gemini 3.8 Live (WaaV's default Gemini model) | `provider` / provider / `provider` | `provider` / provider / `provider` | `provider` / provider / `provider` |
| Gemini 3.1 Flash Live and older (degraded) | `provider` / provider / `provider` | `provider` / provider / `provider` | `provider` / provider / `provider` |
| Amazon Nova 2 Sonic | `provider` / provider / `provider` | `none` / `none` / none | `provider` / provider / `provider` |
| OpenAI GPT-Live (gpt-live-1, v1/live/sessions; new adapter) | `provider` / provider / `provider` | `provider` / provider / none | `provider` / provider / `provider` |

### 4. Surfaces

| Surface | Kind | First release with interaction | Bud mode | Reaches | Egress | Caller switches to an unsupported language mid-call |
|---|---|---|---|---|---|---|
| /ws conversation session (conversation_config) | cascade | R1 | both | cascade | pump | steps down to the plain conversation path |
| /ws Bud agent session (operator default, interaction-only alias or budapp entry) | cascade | R1 | only | cascade | pump | steps down to the plain conversation path |
| raw /ws session with a background target | cascade | R1 | both | cascade | pump | repeat-request template, continues in English |
| LiveKit room driven by a /ws session | cascade | R1 if the echo gates pass, else R2 | both | cascade | pump | as the /ws session that drives it |
| SIP call through LiveKit | cascade | R1 if the echo gates pass, else R2 | both | cascade | pump | as the /ws session that drives it |
| /v1/realtime?model=prompt:<agent> (the GA cascade) | cascade | R2 | only | cascade | pump | repeat-request template, continues in English |
| /ws DAG session whose graph outputs text | cascade | R3 | both | cascade | pump | repeat-request template, continues in English |
| /ws DAG session whose graph outputs audio (AudioOutput, TtsProvider or RealtimeProvider node) | refused | refused: `interaction_dag_audio_output` | both | - | none | - |
| DAG realtime node (the serial turn-by-turn node LiveKit and SIP use today) | refused | refused: `interaction_dag_audio_output` | both | - | none | - |
| /realtime, WaaV's native realtime protocol (non-Bud gateways only) | provider | R3 | refused | all | pump | repeat-request template, continues in English |
| /v1/realtime relay (OpenAI GA protocol, Bud deployments only) | provider | R3 | only | openai, azure, grok | stop_only | repeat-request template, continues in English |
| /v1/realtime translate facade (Bud deployments only) | provider | R3 | only | gemini, nova_sonic, deepgram, elevenlabs, hume | stop_only | repeat-request template, continues in English |
| /ws session whose voice is a realtime provider (interaction.speech.realtime), including LiveKit rooms and SIP calls | provider | R4 | both | all | pump | repeat-request template, continues in English |

### 5. Lifecycle dates (Lifecycle track; evaluated against an injected clock)

| Provider or model | Warning from | Shutdown (refused from) | Replacement | Codes |
|---|---|---|---|---|
| OpenAI text-to-speech tts-1, tts-1-hd, gpt-4o-mini-tts | 2026-10-08 | 2027-01-06 | gpt-realtime-2.1-mini (a realtime model; no silent synthesis through it, Decision 20) | `model_retiring`, `model_retired` |
| OpenAI transcription whisper-1, gpt-4o-transcribe, gpt-4o-mini-transcribe, gpt-4o-transcribe-diarize | 2026-11-28 | 2027-02-26 | gpt-live-transcribe or gpt-transcribe | `model_retiring`, `model_retired` |
| Hume text-to-speech (core/tts/hume/) | first lifecycle patch | 2026-11-13 | none | `provider_sunset`, `provider_retired` |
| OpenAI Realtime, older models (gpt-realtime default, gpt-realtime-mini, gpt-4o realtime previews, gpt-realtime-1.5), models matching `^(gpt-realtime\|gpt-4o-realtime.*)$` | 2026-10-22 | 2027-01-20 | gpt-realtime-2.1 | `model_retiring`, `model_retired` |
| OpenAI Realtime, older models (gpt-realtime default, gpt-realtime-mini, gpt-4o realtime previews, gpt-realtime-1.5), models matching `^gpt-realtime-mini$` | 2026-10-22 | 2027-01-20 | gpt-realtime-2.1-mini | `model_retiring`, `model_retired` |
| Gemini 3.1 Flash Live and older (degraded), models matching `^gemini-2\.0-flash-live-001$` | already past | 2025-12-09 | gemini-3.8-live | `model_retiring`, `model_retired` |
| Amazon Nova 2 Sonic, models matching `^amazon\.nova-sonic-v1:0$` | already past | 2026-09-14 | amazon.nova-2-sonic-v1:0 | `model_retiring`, `model_retired` |
| Hume EVI (EVI 3, EVI 4-mini) - retiring, models matching `.*` | first lifecycle patch | 2026-11-13 | none | `provider_sunset`, `provider_retired` |

### 6. Exceptions that need sign-off

| Exception | Release or refusal | Sign-off (INTEGRATION_DECISIONS.md section 11) |
|---|---|---|
| GPT-Live is refused on /v1/realtime (unsupported_vendor) and on the DAG realtime node (provider_not_turn_based) | refused; reachable on /realtime from Release 3 and through /ws interaction.speech.realtime from Release 4 | coverage exception (decisions section 6) |
| Speech-to-speech providers reach LiveKit rooms and SIP calls only through /ws interaction.speech.realtime | Release 4 | coverage exception (decisions section 6) |
| Bud deployments reach GPT-Live only in Release 4 (native /realtime refuses Bud mode; /v1/realtime refuses GPT-Live) | Release 4 | coverage exception (decisions section 6) |
| Bud deployments cannot reach Inworld, Ultravox or Yandex on /v1/realtime (neither relay nor facade lists them, and native /realtime refuses Bud mode); only /ws interaction.speech.realtime from Release 4, if budapp can name such a deployment | Release 4 at the earliest; revisited at the Release 3 exit review | 27 |
| Hume EVI is refused (retiring 2026-11-13) | refused; removed from the registry in the first release after 2026-11-13 | 8 (early standalone patch) |
| WaaV Infer native duplex is refused until it has an injection channel | Release 5 (conditional on W7's four criteria) | coverage exception (decisions section 6) |
| Audio-output DAGs, including any graph with a realtime node, are refused in interaction mode | refused (interaction_dag_audio_output); duplex DAG output listed under Later, owner W5 | coverage exception (decisions section 6) |
| Regimes B and C run only for providers listed in the server-side speech.truth_ack; truth there is monitored, and regime C relays are a verified paraphrase | Release 3; default list empty | 7 and 10 |
| Languages other than English: a session language without a pack runs with interaction off; a caller who switches to an unsupported language mid-call is stepped down to the plain conversation path only on /ws conversation and agent sessions, while raw /ws, the GA cascade and regimes B and C play a repeat-request template and continue in English (Hindi and code-switched packs in Release 4) | interaction_locale_unsupported / interaction_language_step_down | 18 (and 26 for the Hindi packs) |
| On SIP: agent backchannels off by default, sensitive values declined until DTMF (Release 4), no duck on a bare voice onset until echo cancellation (Release 2) | Releases 2 and 4 | 19 (backchannel defaults), 12 (SIP and LiveKit in Release 1 only if the echo gates pass) |
| Confirmations on providers without exact speech or a reproducible voice use the client UI, else a different-voice gateway clip (speech.voice_mismatch: confirmations_only), else the question is unanswerable by voice | Release 3 | no number (W6 section 10 item 2) |
| /realtime clients cannot call the verbs directly; the gateway runtime drives them from Release 3 | Later | 23 |
<!-- END GENERATED -->
