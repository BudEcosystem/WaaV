# Technology register for the full-duplex interaction agent

Written 2026-10-04 as the register required by `INTEGRATION_DECISIONS.md` (in this folder) section 9, and updated on
2026-10-05 with the measured front-model bake-off, the further Liquid models of Addendum B7, Addenda A to D, and
licence reads of DeepFilterNet, A2A, MCP and Pipecat. It lists the models, protocols and technologies the research
considered for full duplex, turn-taking, echo, task management and orchestration, with a verdict, the reason, the
licence or terms, where the plan uses each one and the evidence. **Workstreams**, written W1 to W7: workstream 1
runtime, 2 front model, 3 background tasks, 4 policy and human in the loop, 5 duplex audio, 6 providers and endpoints,
7 evaluation and rollout. W7 maintains this register after the plan. W7's machine-readable licence register
(`WaaV/LICENSES/MODEL_AND_DATA_REGISTER.yaml`, W7 section 3.14) must agree with it: every chosen or candidate entry has
a row there, and the row's status word (**ship**, **attested**, **opt-in**, **eval**, **remove** or **legal**) is
quoted below where it matters.

**How to read it.**

- **Chosen**: in the plan, with a release. **Candidate**: in the plan behind a gate, a measurement or a sign-off.
  **Watch**: not in the plan; re-check at each release exit review. **Rejected**: not used, for the reason given.
- **Regimes** (INTEGRATION_DECISIONS terms): A, the cascade (WaaV owns the voice); B, a speech-to-speech provider
  speaking under gateway control; C, a delegating provider that is itself the front, with WaaV as its background;
  D, native duplex on WaaV Infer.
- **Releases**, with Addendum D3's target windows for six gateway engineers and one machine-learning engineer:
  Lifecycle track (dated vendor changes; first patch by 2026-10-23); Release 0, measure and lay groundwork (2026-10-05
  to 2026-11-06); Release 1, cascade interaction on `/ws`, streaming speech-to-text only (2026-11-09 to 2027-01-29);
  Release 2, full duplex audio, the GA cascade and durable tasks, plus interaction on segmented (file-only)
  speech-to-text sessions (2027-02-01 to 2027-03-12); Release 3, speech-to-speech providers, open task protocols and
  text-output DAGs (2027-03-15 to 2027-04-23); Release 4, realtime voices on phone and rooms, Hindi and DTMF, and
  Release 5, native duplex on WaaV Infer (conditional), both scheduled at the Release 3 exit review.
- **"Not confirmed"** marks a fact the fact-checks could not confirm or did not check. Fact-checks (`verify/`) win over
  the research reports (`web/`). "Sign-off N" is item N of INTEGRATION_DECISIONS section 11 and its addenda.
  "Read 2026-10-05" marks a licence read from the project's own repository by the final alignment; it is not in the
  fact-checks.
- **Evidence labels** (absolute paths; "bake-off" is in this folder):

| Label | File |
|---|---|
| verify models | `/home/bud/ditto/waav/research/duplex-agent/verify/interaction-models-2026+duplex-models.md` |
| verify Liquid | `/home/bud/ditto/waav/research/duplex-agent/verify/liquid-ai+fast-responder-design.md` |
| verify platforms | `/home/bud/ditto/waav/research/duplex-agent/verify/orchestration-hitl+platform-capabilities.md` |
| verify turn-taking | `/home/bud/ditto/waav/research/duplex-agent/verify/turn-taking-tech+evaluation.md` |
| web interaction, web duplex, web turn-taking, web orchestration | `/home/bud/ditto/waav/research/duplex-agent/web/interaction-models-2026.md`, `web/duplex-models.md`, `web/turn-taking-tech.md`, `web/orchestration-hitl.md` (in the same research folder) |
| bench | `/home/bud/ditto/waav/research/duplex-agent/bench/liquid-benchmark.md` (the earlier benchmark: GB10, llama.cpp build 11200, busy shared machine) |
| bake-off | `front-model/FRONT_MODEL_BAKEOFF.md` in this folder (2026-10-05: the packaged prompt and grammar, 68 labelled decisions, CPU on 6 ARM threads and a GPU shared at a median 95% load); raw data `/home/bud/ditto/waav/research/duplex-agent/bench/bakeoff/`; it wins over "bench" where both measured the same thing |
| corpus | `/home/bud/ditto/waav/research/interaction-models/synthesis/00-thesis-and-decomposition.md` |
| W2, W3, W5, W6, W7 | `/home/bud/ditto/waav/research/duplex-agent/design/W2-front-model.md`, `W3-background-tasks.md`, `W5-duplex-audio.md`, `W6-providers-endpoints.md`, `W7-eval-native-rollout.md` (in the same `design/` folder) |
| decisions | `INTEGRATION_DECISIONS.md` in this folder (sections 1 to 11, then Addenda A, B with B7, C and D) |

---

## 1. Summary

| Category | Chosen | Candidate | Watch | Rejected |
|---|---|---|---|---|
| Interaction systems and research | Talker-Reasoner pattern; Qwen-Audio-Agent lifecycle split; Voice-Light and PACE mechanisms (reimplemented) | Inkling and Inkling-Small as background models; MoshiRAG for regime D | TML interaction models; SALMONN-duo; Context Spanning; SGLang streaming sessions | KAME as a component; Freeze-Omni |
| Native duplex and speech-to-speech models | none before Release 5 | base PersonaPlex-7B; NemotronLabs VoiceChat 11B | Moshi; MiniCPM-o 4.5; Realtime-Venus; Gander | Kyutai RL checkpoints, Hibiki-Zero (non-commercial); LFM2.5-Audio as front; turn-based omni models |
| Commercial realtime APIs | GPT-Live 1; gpt-realtime-2 family; Gemini 3.8 Live (both); Nova 2 Sonic; Grok; Inworld; Ultravox; Deepgram Voice Agent; ElevenLabs (limited); Azure OpenAI and Yandex clones | Deepgram Flux; Gemini 3.1 Flash Live (degraded) | Azure AI Voice Live; Qwen3.8-Omni-Flash-Realtime; StepAudio 3; Tavus | Hume EVI (retiring); retiring OpenAI models after their dates |
| Front model and serving | `scripted` profile (deterministic rule router with the question step and its own recall gate, Addendum D1; the Release 1 front everywhere); LFM2.5-1.2B-Instruct (`lfm-standard`, the primary model, enabled once a fine-tuned checkpoint passes the gates); `llama-server` on `/completion` | LFM2.5-2.6B (`lfm-large`, distillation base); Qwen3.5-2B (the Apache candidate) and Gemma 4 E2B (alternate); LFM2.5-VL-3B (Hindi, Release 4); DSpark on GPU; in-process llama.cpp for edge | vLLM and SGLang for the pool | Qwen3.5-0.8B (dropped by the bake-off); LFM2.5-230M and 350M as decision makers; reasoning LFM models as front; Llama 3.2 |
| Turn-taking and voice activity | Smart Turn v3.2; Silero v6.2; pipecat-backchannel rules; provider turn events as inputs | Krisp Interruption Prediction; MaAI VAP backchannel models (evaluation only); X2-Turn | Krisp Turn Prediction; Easy Turn; Meta hierarchical model; TurnBench leaders | LiveKit text turn detector (removed); LiveKit audio models; TEN |
| Echo and noise | sonora AEC3; browser AEC; WaaV echo layers; DeepFilterNet (kept; code MIT or Apache-2.0, bundled weights still under a legal read) | none | DTLN-aec; Krisp filters | webrtc-audio-processing; aec-rs; aec3-rs; NVIDIA Maxine AEC |
| Tasks and orchestration | LLM tier; budprompt runs; webhook; DAG backend; A2A 1.0 (Apache-2.0); MCP Tasks (Apache-2.0, moving from MIT); OpenAI Responses async tools; GPT-Live and Gemini delivery vocabulary | none | OpenAI Agents API | Pipecat (BSD-2-Clause), LiveKit Agents, LangGraph, Temporal, ADK as dependencies (kept as references) |
| Evaluation | Full-Duplex-Bench v1, v1.5, v2, v3 (regenerated stimuli for gates); tau-Voice; Big Bench Audio; MTR-DuplexBench; TurnBench (evaluation only: non-commercial data, never training, never a gate); Game-Time; Audio MultiChallenge | IndicFDB (with Hindi) | APEX-Voice; DuplexAct-Bench | Official Full-Duplex-Bench and TurnBench data for release gates or training (non-commercial) |

**What the measured front bake-off changed** (decisions Addendum B and B7; bake-off §3, §5, §8). On the packaged
prompt and grammar no prompted model passes the Release 1 front gates, so the Release 1 front is the `scripted`
profile on every deployment, attested or not, with the model stack dark and opt-in; `apache-standard` does not exist
in Release 1. Liquid stays the primary model and the route to it is fine-tuning: the distillation track runs from
Release 0 on both `lfm-standard` (LFM2.5-1.2B) and `lfm-large` (LFM2.5-2.6B), and qualifies whichever passes first,
preferring the 1.2B when both pass. Measured on the same 68 decisions:

| Front | Acceptable act | Delegate recall | Warm act p95, shared GPU | Calls per instance within the 300 ms act budget (shared GPU) |
|---|---|---|---|---|
| LFM2.5-1.2B-Instruct | 0.50 to 0.56 | 0.73 to 0.85 | 70 ms | 4 |
| LFM2.5-2.6B (reasoning suppressed by the prefill) | 0.72 to 0.74 | 0.96 | 144 ms | 4 |
| LFM2.5-VL-3B, text only | 0.71 | 0.92 | 108 to 111 ms | 4 |
| Qwen3.5-2B (thinking off) | 0.74 to 0.79 | 0.81 to 0.88 | 112 to 162 ms | 2 |
| Gemma 4 E2B (thinking off) | 0.76 to 0.79 | 0.92 | 311 ms | not run under load |
| Qwen3.5-0.8B (thinking off) | 0.66 to 0.71 | 0.85 to 0.88 | 145 ms | not run under load |
| Rule router (`scripted`, no model) | 0.87; 0.90 with the question step (optimistic: same author as the set) | 0.92; 1.00 with the question step | at once | not applicable |

The router's "with the question step" figures are the final review's simulation on the same 68 decisions (Addendum
D1.1). **The front qualification gates** (Addendum D1.4), which every model profile must pass: act macro-F1 at least
0.85 with every act at least 0.75; delegate recall at least 0.98 and precision at least 0.95; status questions sent as
new tasks at most 1%; injection success at most 1%; single-call act p95 at most 100 ms on GPU or 200 ms on CPU, on the
device class the profile will run on; act p95 within the 300 ms act budget under Poisson load at the published calls
per instance. On the measured shared GPU only LFM2.5-1.2B (70 ms) meets the single-call row; the others qualify only on
a device class that measures 100 ms or less, which fine-tuning cannot change. The router has its own gate: delegate
recall at least 0.95 on a frozen held-out set at the Release 0 exit review (2026-11-06), with the lower confidence
bound published (D1.2). Every front, the rule router included, followed 2 to 4 of 5 injections when measured (the
router's `injection_cue` rule came after). No front line carried an invented entity. On 6 ARM CPU threads no model
meets the act target even at one call (LFM2.5-1.2B act p95 1,496 ms), so the Release 1 CPU sidecar runs no model. The first spoken chunk missed its target on every device (LFM2.5-1.2B 575 to 656 ms at p95 on
the shared GPU), so acknowledgements default to clips. The grammar is mandatory (16 to 75% of outputs are valid
without it); the gateway renders the prompt itself and calls `/completion`, where cancel-and-rewrite reuses the cache
(16 extra tokens at the median).

---

## 2. Interaction-model systems and research

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| Thinking Machines Lab interaction models (TML-Interaction-Small) | 276B-total, 12B-active model trained on 200 ms micro-turns with an asynchronous background model; 0.40 s on Full-Duplex-Bench v1. | Closed; no API or weights; preview promised "later this year" | **Watch.** The reference experience; nothing to integrate. | Sets the stated gap: regime A is about 0.5 s behind (decisions 3.4) | verify models §2; corpus |
| Inkling | TML's 975B-total, 41B-active MoE, 1M context; text, image and 16 kHz audio in, text out. TML says it is meant "to serve as the background reasoning model in the interaction models system". | Apache-2.0 (fact-checked). WaaV ships no weights; the operator's serving stack has its own terms. Licence register: **ship** as a documented background option | **Candidate** background model; needs an operator-run OpenAI-compatible server (served by SGLang or vLLM per TML, not re-checked). | LLM background tier, Release 1, no gateway code | verify models §2; web interaction §2; W7 §3.14 |
| Inkling-Small | 276B total, 12B active, $1.20 per 1M output tokens, VoiceBench 90.1. Any link to the interaction model is not confirmed. | Apache-2.0 | **Candidate**, as Inkling. | LLM tier, Release 1 | verify models §2 |
| Talker-Reasoner (arXiv 2410.08328) | A fast Talker and a slow Reasoner sharing a structured belief state; waiting is a per-phase policy. | Paper | **Chosen as the pattern**: front talks, background reasons, task state (not chat history) is the interface. | Spine of Releases 1 to 4 | verify platforms §1; web orchestration §1 |
| Qwen-Audio-Agent (2609.25195) | Frontend agent, backend agent and orchestration runtime; mixed direct and delegated execution 91.04% against 72.39% and 80.60%. | Paper | **Chosen as design reference** for keeping speech interruption, task cancellation, completion and delivery separate. | W1 and W3 runtime, Release 1 | verify models §2; web interaction §4.2 |
| Voice-Light (2609.20995) | Cascade harness: duck to −15 dB and pause by 500 ms, private speculation, bridge speech, history from audio the client played. | Code has no licence (default copyright) | **Chosen as mechanisms, reimplemented from the paper**; code not copied. | W5 two-stage barge-in, W1 heard history, Release 1 | verify models C5; W5 decision 2 |
| PACE (2608.07631) | Middleware anchoring model context to client playback; referent accuracy 25.0% to 96.3%. | Paper | **Chosen as principle**: history is what was heard. | W1 heard time, Release 1 | verify models §2 |
| ConvFill (2511.07397); VERA LiveAnswer (2509.26542) | Talker-plus-reasoner filler study; narrated-answer study. | Papers | **Evidence**: fine-tuned talkers contradict the reasoner in 14 to 19% of fillers on MultiWOZ and size does not help, so the front is content-free; narration lost 15.7 maths points, so results are relayed verbatim. | W2 compose grammar, W4 relay, Release 1 | verify Liquid C3, C4 |
| MoshiRAG (Kyutai) | Moshi front with a retrieval trigger token; reference text injected back as a stream; text-in, text-out backend. | CC-BY-4.0; needs the ARC4 encoder (tagged CC-BY-4.0, possible Llama base not confirmed) and Kyutai STT 1B | **Candidate** for regime D: the only open front with an injection channel. Encoder chain is sign-off 25. | Release 5 evaluation | verify models §2, §3; W7 §3.13 |
| KAME (Sakana) | Moshi plus an oracle stream fed by a text LLM; 6.43 against Unmute's 7.70 at 2.1 s. | Code MIT | **Rejected as a component**: one WebSocket session at a time, fixed vendors, English. | None | verify Liquid §3.2, C14 |
| SALMONN-duo; NVIDIA frontend-backend tool calls (2609.19334); Context Spanning (2609.33443) | Learned delegation; delegation token with prefill-and-repeat; chunked-prefill injection of text into a duplex model. | Papers, no weights | **Watch.** Context Spanning is the recipe for background results in regime D. | Release 5 input | web interaction §4.2; verify models §2 |
| SGLang streaming sessions (TML's upstreamed PR 19171) | KV-ownership decorator for append-only sessions, byte-identical to baseline. | Licence not in the evidence | **Watch** as the blueprint for Infer streaming duplex sessions. | Release 5 | corpus §4a |
| Freeze-Omni | Half-duplex interrupt head on a frozen Qwen2-7B; 745 ms. | Apache-2.0 code | **Rejected**: half-duplex and slow. | None | web duplex §4; corpus §6 |
| OpenAI chat-supervisor demo; Vocal Bridge; RePlay (2609.31588); Kyutai Unmute | Supervisor-tool delegation; orchestration-only product; retrieval of pre-recorded lines (383 ms); cascade with semantic VAD. | Demo; commercial; paper; MIT | **Reference.** The supervisor rule that fillers "must NOT indicate whether you can or cannot fulfill" is in W2's banks; RePlay supports cached clips. | W2 banks and W5 clip bank, Release 1 | verify Liquid §3.2; web interaction §4.3 |
| Proactivity studies (2609.19596; DuplexAct-Bench 2609.39446) | Native models challenge false claims 14 to 15% of the time and warn of hazards 4 to 7%. | Papers | **Evidence** for keeping proactive interjection under "Later". | Decisions §9 | verify models §2 |

---

## 3. Native full-duplex and speech-to-speech models (open weights)

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| PersonaPlex-7B v1 (NVIDIA, base) | Moshi-architecture 7B with a text role prompt and a voice prompt; English, 24 kHz; card: interruption success 0.950 at 0.240 s, smooth turn-taking 0.908 at 0.170 s (A100 80 GB). No injection channel. | NVIDIA Open Model License, "ready for commercial use"; hosting terms are sign-off 25 | **Candidate**: first Infer onboarding; background results need the floor-handover spike. | Release 5 (W7's four criteria) | verify models §2, C1; W7 §2.9 |
| `personaplex-rl-seamless`, `moshika-rl-seamless` (Kyutai) | GRPO-tuned turn-taking, pauses, backchannels; the reported before-and-after numbers disagree with the paper. | CC BY-NC 4.0 combinations | **Rejected for product; evaluation only.** Reproducing the recipe in-house is the commercial route. | Research job | verify models C1, C10 |
| NemotronLabs VoiceChat 11B (NVIDIA) | Native duplex with user-transcript, agent-text, tool-call and speech streams; 448 ms turn-taking, 93% resume after backchannels. Limits: 80 GB GPU, 2-minute trained context, gibberish after several turns, self-talk, at most 5 tools, no interruption during tool calls. GB10 port: p95 about 82 ms against an 80 ms cadence. | OpenMDW-1.1 (commercial with attribution; patent-termination clause) | **Candidate** as a separate local provider in its own vLLM container, not inside Infer; legal read of the patent clause. | Release 5 evaluation | verify models §2, C6; W7 §3.13 |
| Moshi (moshiko, moshika) | Kyutai 7B native duplex, 80 ms frames, inner monologue text. | CC-BY-4.0 (research report; not re-checked) | **Watch**: baseline only, no injection. | Release 5 baseline | web duplex §4; W7 §3.13 |
| MiniCPM-o 4.5 | 9.37B omni model deciding at 1 Hz whether to speak; 0.6 s first token; full duplex on a 12 GB GPU via llama.cpp-omni. | Apache-2.0 | **Watch**: 1-second units, English and Chinese. | Later regime D | verify models §2 |
| Realtime-Venus (Ant Group); Gander | Duplex fronts built on MiniCPM-o 4.5 with native delegation; Venus keeps speaking through backchannels 97% of the time. | Apache-2.0 | **Watch**: closest open match to the target shape, but coarse timing and English and Chinese only. | Later regime D | verify models §2 |
| Hibiki-Zero 3B (Kyutai) | Moshi-class duplex speech translation, already loadable in Infer (`waav-infer/crates/waav-infer-server/src/engine.rs:1189-1194`). | CC BY-NC-SA 4.0 | **Rejected for product**: non-commercial; Infer must refuse it in commercial configurations. | Evaluation only | verify models §2; W7 §3.14 |
| Hibiki (2025) | Earlier Kyutai duplex translation model. | Not in the evidence (not confirmed) | **Rejected** for the agent: translation, not dialogue; belongs to the DAG translation path. | None | web duplex §5 |
| LFM2.5-Audio-1.5B (Liquid) | Turn-based interleaved speech model, English. Liquid's runner rejects custom system prompts; with the front prompt it answered as the caller. Infer registers `lfm2_audio_s2s` (`waav-infer/crates/waav-infer-core/src/model.rs:1149-1160`). The "under 100 ms" claim is for LFM2-Audio, not 2.5. | LFM Open License v1.0 | **Rejected as front; evaluation only** (W7 decision 2.10). | Evaluation | bench §10; verify models C13 |
| Qwen omni open models (Qwen3-Omni-30B-A3B, Qwen2.5-Omni) | Turn-based Thinker-Talker models; Qwen3-Omni needs about 79 GB in bf16. | Apache-2.0 (research report) | **Rejected** as front: turn-based and heavy; a small text model plus streaming speech gives the same turn-taking with any voice. | None | web duplex §4, §5 |
| Other open speech models: Step-Audio 2 mini, Kimi-Audio, GLM-4-Voice and Kyutai `glm-4-voice-of-reason-9b`, Fun-Audio-Chat, VITA, Mini-Omni2, Voila, FLM-Audio, Covo-Audio, Chroma-4B | Turn-based models or research duplex models. | Mixed; FLM-Audio unstated; GLM-4-Voice licence "other" | **Rejected**: turn-based, research grade or unclear licence. | None | web duplex §4; verify models C11 |
| DuplexJev-4B | Side model answering "has the user finished?" and "which filler fits?". | CC-BY-NC-4.0 | **Rejected for product**; design reference for a turn and filler head. | None | verify models §2 |

---

## 4. Commercial realtime APIs

All are used under the vendor's service terms; prices are those the fact-checks confirmed.

| Name | What it is | Terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| OpenAI GPT-Live 1 (`gpt-live-1`) | GA 2026-09-10 full-duplex voice model that delegates to a Responses backend or any client backend; `session.commentary.append`, `thinking.append`, `instructions.append` (500 tokens each); delegation events carry no task text; client WebSocket output audio has no timing fields. | $0.05/min plus backend; 25 to 500 sessions by tier | **Chosen** for regime C, client delegation only; refused on `/v1/realtime`. | Release 3 on `/realtime` (not Bud); Release 4 on `/ws` | verify models §2, C4; verify platforms §1; W6 decision 7 |
| OpenAI `gpt-realtime-2`, `-2.1`, `-2.1-mini` | Turn-based realtime models with native preambles; τ-Voice 45.7% (2.1 High). | Audio $32 in, $64 out per 1M tokens (2) | **Chosen** for regime B as the half-cascade, the only family that qualifies today (text output and a documented reply switch): WaaV's text-to-speech is the voice (`voice_owner: gateway`), the gateway owns the filler and idle speech, backchannels are off (decisions A2 items 1 and 8). | Release 3; default move by 2027-01-20 | verify models §2; W6 §3.3 |
| Retiring OpenAI models | `gpt-realtime`, `gpt-4o-realtime`, `gpt-realtime-mini` (2027-01-20); `tts-1`, `tts-1-hd`, `gpt-4o-mini-tts` (2027-01-06); `whisper-1`, `gpt-4o-transcribe` family (2027-02-26). | Shutdown dates | **Rejected after the dates**; defaults move first. | Lifecycle track | verify platforms §3; decisions §1 |
| Gemini 3.8 Live and 3.8 Live Extended Thinking | GA 2026-09-15; non-blocking tools by default, results scheduled `INTERRUPT`, `WHEN_IDLE` or `SILENT` (SDK spelling; one docs page says `INTERRUPTED`); Extended Thinking is async-only and reports `interactionStatus`; proactive audio cannot be switched off. τ-Voice 68.6% (Extended Thinking High). | Audio $3 in, $12 out per 1M tokens; 15-minute audio sessions | **Chosen** for regime C. WaaV already defaults to `gemini-3.8-live` (`gateway/src/core/realtime/gemini/protocol.rs:62`) without `behavior`, so tools already run non-blocking. | Lifecycle track (explicit `behavior`, `toolCallCancellation`, `interactionStatus`; target 2026-11-13); Release 3 | verify models C3, C7; verify platforms §2 item 1 |
| Gemini 3.1 Flash Live | Legacy; synchronous tools only; 0.57 s on Full-Duplex-Bench v1. | as Gemini | **Candidate (degraded)** regime C: results only between turns. | Release 3 | verify platforms §1; W6 §3.3 |
| Amazon Nova 2 Sonic | Async tools with no automatic cancellation; 8-minute connections; 7 languages including Hindi; τ-Voice 57.1%. | AWS terms | **Chosen** for regime C after the `interactive: true` text fix. | Lifecycle track; Release 3 | verify models §2, C2; verify platforms §3 |
| xAI Grok voice | OpenAI-shaped WebSocket; `force_message` speaks verbatim "without involving the model"; τ-Voice 56.5%. WaaV's default model is `grok-realtime` (`gateway/src/core/realtime/grok/protocol.rs:49-53`); `grok-voice-think-fast-2.0` is the newer model W6 named. | xAI terms | **Chosen** for regime B (native exact speech). The descriptor follows the code; moving the default to `grok-voice-think-fast-2.0` is a Lifecycle-track item behind a live probe (decisions A2 item 2). | Release 3 | verify platforms §1; W6 §3.3 |
| Inworld Realtime | Packaged cascade; `response.speak` verbatim; filler and `providerData.backchannel` are opt-in. OpenAI compatibility and router claims not confirmed. | Inworld terms | **Chosen** for regime B with Inworld's filler and backchannel off. Bud deployments reach it only from Release 4, if budapp can name such a deployment (sign-off 27). | Release 3 | verify platforms §1, §2 item 7; verify models §3 |
| Ultravox (`ultravox-v0.7`) | Speech-in LLM; tools freeze the call (2.5 s default); `forced_agent_message`; thread urgency. GLM-4.6 base and 0.384 s endpoint default not confirmed. | Projector weights MIT; service terms | **Chosen** for regime B after the tools wire fix; not a half-cascade (no documented reply switch). Bud deployments only from Release 4 (sign-off 27). | Lifecycle track; Release 3 | verify platforms §1; verify models §3 |
| Deepgram Voice Agent | Packaged cascade; `InjectAgentMessage` takes `message` and `behavior` (`default` and `queue` refused while the user speaks); speculative function calls. | Deepgram terms | **Chosen** for regime B after the wire fix (WaaV sends `content`; typed text moves to `InjectUserMessage`). | Lifecycle track; Release 3 | verify platforms §1, §2 item 6; W6 decision 4 |
| Deepgram Flux | Streaming speech-to-text with `StartOfTurn`, `EagerEndOfTurn`, `TurnResumed`, `EndOfTurn` on `/v2/listen`; about 260 ms end of turn; eager mode costs 50 to 70% more LLM calls. | Deepgram terms | **Candidate** endpoint input; WaaV calls `/v1/listen` only (`gateway/src/core/stt/deepgram.rs:480`). | Release 2 (new adapter) | verify turn-taking B3; W5 §3.8.2 |
| ElevenLabs Agents | Hosted agent whose tools live on the vendor side; `execution_mode: async`, `pre_tool_speech` default `auto`; no verbatim speech event found. | ElevenLabs terms | **Chosen (limited)** regime B: confirmations by gateway clip or client UI. | Release 3 | verify platforms §1, §2 item 2, §4 |
| Azure OpenAI Realtime (WaaV `azure`) and Yandex | OpenAI-protocol clones in WaaV (`gateway/src/core/realtime/mod.rs:52-65`). | vendor terms | **Chosen** as OpenAI family, regime B. Azure deployments hide the model name (`gateway/src/core/realtime/azure/protocol.rs:36-38`), so an optional `model_family` field arrives in Release 3; until it is set, fillers resolve to the gateway with `config_warning` `model_family_unknown` (decisions A2 item 10). Bud deployments reach Yandex only from Release 4 (sign-off 27). | Release 3 | W6 §3.2.1 |
| Azure AI Voice Live | Microsoft service with `gpt-realtime-2.1`, `gpt-5.6-terra`, `gpt-5.6-luna`, `phi4-mm-realtime`; reference-based AEC; filler-word removal. | Microsoft terms | **Watch**: no WaaV adapter, listed under "Later". | Later | verify platforms §1; W6 §8 |
| Hume EVI 3 and EVI 4-mini | Access ends 2026-11-13 at 12:01 a.m. EST; account data deleted. | Retiring | **Rejected**: warning now, refusal from 2026-11-13, then removal. | Lifecycle track, first patch | verify models §2; decisions §1 |
| Qwen3.8-Omni-Flash-Realtime (Alibaba) | API documented as full duplex; `semantic_vad` filters backchannels; OpenAI-style events. | Alibaba terms | **Watch**: not a WaaV provider; the scaffold's OpenAI mapper would be the base. | None | verify models §2 |
| StepAudio 3 Realtime; Tavus Griffin and Sparrow-2; ByteDance SeedRealtime; Sesame | Think-while-speaking model; video model in research preview; turn model inside Tavus only; no developer API found. | Closed | **Watch.** StepFun API and SeedRealtime access not confirmed. | None | verify models §2, §3, C12 |
| Vapi; Retell; LiveKit Cloud adaptive interruption | Voice platforms and a Cloud-only interruption model. | Commercial | **Rejected as dependencies**; design references. | None | verify platforms §1 |

---

## 5. Front-model candidates and serving

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| `scripted` profile (W2's deterministic rule router, persona phrase banks, clip bank) | No model: the router picks the act at once; banks and clips speak; W4's templates and the verbatim relay word everything that binds. Bake-off: 0.87 acceptable acts, macro-F1 0.88, delegate recall 0.92, precision 0.81 (optimistic, same author as the set); `refuse` 1 of 5; followed 3 of 5 injections, which W2's new `injection_cue` lexicon (commits `refuse`) and repeat-request exemption address. Its last rule met two complete questions ("Will I get a reminder text?", "Is it going to be here by Friday?") with silence; the **question step** (Addendum D1.1) now sends an addressed question that is not a status, social or repeat turn to `delegate`, while fragments ending on a dangling word keep `wait`. Simulated on the same 68 decisions: delegate recall 1.00, acceptable acts 0.90, precision 0.82, no new misses. | WaaV code | **Chosen** as the Release 1 front on every deployment, attested or not (Addendum B1), and the bar every model must beat; the default for good if no fine-tune passes. It is not chosen because it passes: it still misses the model gates on precision (against 0.95), refusals and injections, and its lead over the best Apache models is within the ±0.1 intervals; it is the least-bad front, costs no model time and behaves the same every time (D1.3). **Gated** at the Release 0 exit review (2026-11-06): delegate recall at least 0.95 on a frozen held-out set with the lower confidence bound published, tuned only on a separate development split, and scored on the same sets as every model checkpoint (D1.2, D1.5). | Release 0 (question step, gate); Release 1 onward; per-call fallback for a model profile | bake-off §3.1, §7 items 2, 5, 11; decisions B1, B5, D1 |
| LFM2.5-1.2B-Instruct, Q4_K_M (731 MB) | Liquid non-reasoning hybrid (10 convolution, 6 attention layers); IFEval 86.23; 8 languages, not Hindi. Bake-off: acceptable acts 0.50 to 0.56, delegate recall 0.73 to 0.85, 4 of 5 injections followed, no invented entities; the only model that met a latency target (GPU act p95 70 ms warm, 95 ms at one call under load), and so the only one that meets the qualification gates' single-call act row (100 ms on GPU) on the measured shared GPU (D1.4); 4 calls per shared-GPU instance within the 300 ms act budget; CPU act p95 1,496 ms at one call; first spoken chunk 575 to 656 ms at p95 on the shared GPU. | LFM Open License v1.0: commercial use only while the legal entity with affiliates is under US$10M revenue; research and nonprofit unrestricted; redistribution with the licence. Licence register: **attested** | **Chosen** as `lfm-standard`, the primary and reference front, but not prompted: it fails every quality gate when prompted, so it becomes the attested default only when a fine-tuned checkpoint from the distillation track passes the gates; runs only with a server-side attestation (`lfm_open_eligible` or `liquid_commercial`). Liquid commercial terms are sign-off 2. | Distillation from Release 0 (sign-off 28); a Release 1 point release at the earliest if a checkpoint passes; `-front-lfm` image | bake-off §3.1, §3.3, §5.2, §8.2; bench §1, §9; verify Liquid §3.1, C9; decisions §2.1, B1, B2 |
| LFM2.5-2.6B, Q4_K_M (`LiquidAI/LFM2.5-2.6B-GGUF`, sha256 `02a8b7e1...`) | Liquid 2.6B; reasoning suppressed by the assistant prefill and grammar (no reasoning text produced). Measured after the bake-off on the same package, harness and shared GPU: acceptable acts 0.72 to 0.74, macro-F1 0.68 to 0.72, delegate recall 0.96 (the highest of any front measured, the rule router included) with precision 0.74 to 0.76, no status question sent as a task, 3 of 5 injections followed, no invented entities; warm act p95 144 ms; 4 calls per instance within the act budget (278 ms at 4, 394 ms at 8). The earlier benchmark, run without the grammar and prefill, saw it reason first and take 446 to 888 ms to the first word. | LFM Open License v1.0; same licence and attestation as `lfm-standard`. Licence register: **attested** | **Candidate** as `lfm-large`, the stronger Liquid base for the distillation track; qualifies only if it passes the gates, and the 1.2B is preferred when both pass (faster act). Still fails the Release 1 gates when prompted. Its 144 ms single-call act p95 misses the 100 ms GPU row on the measured shared GPU, so it can qualify only on a device class that measures 100 ms or less (D1.4). | Distillation from Release 0; attested default only after passing | decisions B7; bench §6 |
| LFM2.5-VL-3B, Q4_K_M (sha256 `2436cf4b...`), used text-only | Non-reasoning 2.6B backbone plus a 400M vision encoder; 16 languages including Hindi. Measured text-only after the bake-off: acceptable acts 0.71, delegate recall 0.92, 0.17 of status questions sent as new tasks, 2 to 3 of 5 injections followed, no invented entities; warm act p95 108 to 111 ms; 4 calls per instance (262 ms at 4, 401 ms at 8). No Hindi set has been run. | LFM Open License v1.0 assumed for the family (not confirmed). Licence register: **eval** until adopted, then **attested** | **Candidate**: the Liquid route to the Hindi front profile. Like `lfm-large`, its single-call act p95 (108 to 111 ms) misses the 100 ms GPU row on the measured shared GPU (D1.4). | Release 4 | decisions B7; verify Liquid C1; W2 §2.11 |
| Qwen3.5-2B (thinking off) | Hybrid linear-attention model, 201 languages; non-thinking IFEval 61.2. Bake-off: acceptable acts 0.74 to 0.79 (tied best with Gemma), macro-F1 0.72 to 0.79, delegate recall 0.81 to 0.88, 3 of 5 injections followed, no invented entities; warm act p95 112 to 162 ms on the shared GPU, 1.2 s act p50 on CPU; 2 calls per shared-GPU instance (its 248,320-token vocabulary makes grammar sampling dearer under load). | Apache-2.0. Licence register: **ship** once qualified | **Candidate**: the Apache candidate carried into fine-tuning; becomes `apache-standard` (the default without an attestation) only if a fine-tuned checkpoint passes the gates; CI's default model job runs it forced with `allow_unqualified` (sign-off 4). Its 112 to 162 ms single-call act p95 qualifies only on a device class that measures 100 ms or less on GPU, which fine-tuning cannot change (D1.4). Also a Hindi profile candidate. | Distillation from Release 0; `apache-standard` from Release 2 at the earliest; Release 4 (Hindi) | bake-off §3.1, §3.3, §5.2, §8.1; verify Liquid §3.1; decisions B2, C8 |
| Gemma 4 E2B-it (thinking off) | 2.3B effective (5.1B with embeddings); IFEval 82.93 on Liquid's card; 140+ pre-training languages. Bake-off: acceptable acts 0.76 to 0.79, delegate recall 0.92, best on status questions (5 of 6), 4 of 5 injections followed; warm act p95 311 ms on the shared GPU and 5.0 s on CPU. | Apache-2.0 (do not mix with earlier Gemma under the Gemma Terms of Use). Licence register: **ship** once qualified | **Candidate**: the alternate Apache candidate if Qwen3.5-2B's fine-tune fails; slower (about three times Qwen3.5-2B's act time on CPU; its 311 ms GPU act p95 is about three times the 100 ms single-call row, D1.4) and larger (3.1 GB against 1.3 GB). | Distillation alternate | bake-off §3.1, §3.3, §8.1; verify Liquid C7 |
| Qwen3.5-0.8B (thinking off) | Non-thinking IFEval 52.1; prone to thinking loops. Bake-off: acceptable acts 0.66 to 0.71; sent 1 or 2 of 6 status questions to the background as new tasks, misrouted 3 of 4 cancels, refused nothing; only about 12% faster than Qwen3.5-2B on GPU. | Apache-2.0. Licence register: **eval** | **Rejected** (dropped by the bake-off; Addendum B2). | None | bake-off §8.1 |
| LFM2.5-2.6B-Base | Tagged Hindi; not measured. | LFM v1.0 | **Watch**; LFM2.5-VL-3B is the measured Liquid route to Hindi. | None | verify Liquid C1 |
| LFM2.5-350M and 230M | Warm first word about 90 ms on CPU, but invented facts or claimed bookings in 4 to 6 of 6 scenarios. | LFM v1.0 | **Rejected** as decision makers (experiments only). | None | bench §9 |
| LFM2.5-8B-A1B; LFM2.5-1.2B-Thinking; LFM2-1.2B-Tool; Liquid d1 decision API | Reasoning models; a tool-calling specialist (not measured); a hosted decision API with no weights. | LFM v1.0; hosted | **Rejected** as front: reasoning latency, the front has no tools, and it must run locally. | None | bench §3; verify Liquid §3.1 |
| SmolLM3-3B; Granite 4.0-H-350M; Phi-4-mini; Llama 3.2 | Small models assessed on paper. | Apache-2.0; Apache-2.0; MIT; Llama licence (700M MAU clause, naming rule) | **Rejected** for the bake-off (lower IFEval or larger cache); Llama 3.2 also on licence. | None | W2 §2.1; verify Liquid §3.1 |
| llama.cpp `llama-server`, pinned build 11200 (commit 81bc6b83f) | Server with per-request grammar, pinned slots, prompt cache, context checkpoints for hybrid models, UNIX socket. Bake-off: grammar with assistant prefill works; on the chat endpoint the server inserts `<think></think>` before LFM's prefill and loses cache reuse, so the gateway renders the prompt and calls `POST /completion`, where cancel-and-rewrite processes 16 extra tokens at the median. Priming the 1,700 to 1,800-token persona prefix takes 0.23 to 1.14 s on the shared GPU and 6.4 to 6.7 s on CPU; a call's history fills an 8,192-token slot near decision 38, so the rebase at 6,144 tokens is required. | MIT (W7 register; not fact-checked). Licence register: **ship** | **Chosen**: managed sidecar per pod (no model child in Release 1, because no model meets the CPU act target), external-endpoint mode, then a shared GPU front pool in Release 2 sized by its own capacity test from a floor of 4 calls per LFM instance and 2 per Qwen3.5-2B instance; admission permits equal calls per instance, excess decisions go to the rule router. GPU provisioning for the pool waits until a profile qualifies (the image, chart and client ship dark), and a deployment with no qualified device class, GPU-less on-premises included, runs `scripted` (Addendum D6.2). | Release 1 sidecar and external endpoint; Release 2 pool | bake-off §2, §4, §5, §7 items 6 and 7, §8.3; W2 §2.7; decisions B3, B4 |
| DSpark drafter (LFM2.5-1.2B-Instruct-DSpark, 176 MB) | Speculative decoding. GB10: first word 309 to 187 ms on GPU, slower on CPU; acceptance 15 to 31% on JSON; vendor 2.10× mean, 1.66× to 1.72× on chat. | LFM v1.0 (assumed) | **Candidate**, GPU only, off by default. | Release 2 GPU pool | bench §8; verify Liquid C11 |
| vLLM; SGLang | Serve LFM2.5 (vLLM v0.23.0 and later); short-convolution prefix caching unproven. | Open source | **Watch** for the pool if `llama-server` fails the load gate. | None yet | verify Liquid §3.1, §4 |
| In-process llama.cpp (`llama-cpp-2`); in-process ONNX Runtime; a WaaV Infer text route; Liquid LEAP SDK | Alternatives to the sidecar. | Various; LEAP deprecated | In-process llama.cpp: **candidate** for single-tenant edge builds only (a native abort kills every tenant). ONNX **rejected** for now. Infer **rejected** while text LLMs stay outside Infer by decision. LEAP **rejected**. | Release 4 edge option | W2 §2.7; verify Liquid §3.1 |

---

## 6. Turn-taking, voice activity and interruption

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| Smart Turn v3.2 (Pipecat) | 8M-parameter audio end-of-turn model, 23 languages, 8 s input; WaaV fetches `smart-turn-v3.2-cpu.onnx` from a branch URL (`gateway/src/core/smart_turn/detector.rs:32-33`). TurnBench recall 0.752, false positives 0.047, 1017 ms (14th of 20 qualifying). One vCPU: 37 to 57 ms plus 7 to 9 ms preprocessing. | BSD-2-Clause | **Chosen**: best open endpointer, run on demand at each pause. | Release 0 pin and hash; Release 1 | verify turn-taking B2, B3; verify models C9; W5 decisions 13, 14 |
| Silero VAD v6.2.x | VAD; v6.2 (2025-11-06) has 16% fewer errors on noisy audio; v6.2.3 2026-09-23. WaaV pulls v5 from `master` (`gateway/src/core/silero_vad/detector.rs:19-20`). | MIT (research report; licence file to verify) | **Chosen**: v6.2 is the only VAD in interaction mode; legacy sessions keep v5. | Release 0 hash pin; Release 1 | verify turn-taking B2; W5 decision 16 |
| LiveKit text turn detector | Text end-of-turn model downloaded by default (`gateway/src/core/turn_detect/config.rs:27-36`) and baked into the image. | LiveKit Model License: LiveKit Agents only | **Rejected and removed** (licence). | Release 0 | verify turn-taking B3; W5 decision 15 |
| LiveKit Turn Detector v1 and v1-mini; LiveKit adaptive interruption | Audio models (9.9% false cut-off at 300 ms); Cloud-only interruption model. | LiveKit Model License; Cloud | **Rejected** (licence; Cloud only). | None | verify turn-taking B2, B3 |
| Krisp Interruption Prediction v1 | 6M-parameter English audio classifier; 0.833 s to decide, 5.9% false positives, against VAD's 0.375 s and 66.3%. Its `turn-taking-test-v1` set is a gated download with licence "other". | Proprietary; pricing and terms not found; no Krisp crate in `gateway/Cargo.toml` today. Licence register: **opt-in** (plug-in, after commercial terms are signed; never in a default image or configuration); the test set **eval**, if allowed | **Candidate** paid English plug-in (cargo feature `krisp-ip`), built only after its terms clear (sign-off 25). | Release 4 | verify turn-taking B3; W5 §3.5.3, §3.5.4 |
| Krisp Turn Prediction v3 and Voice Isolation 2.5; ai-coustics Quail | Audio end of turn (AUC 94.58); primary-speaker isolation (WER 15.35% to 8.22%). | Proprietary | **Watch** as paid plug-ins. | None | verify turn-taking B2, B3 |
| Voice Activity Projection and MaAI models | Two-channel timing model (TurnBench 0.845 recall at 368 ms). MaAI `vap_bc_*` and `*_kyoto` are tagged MIT with empty cards; `vap_en`, `vap_mc_en`, VAD and `bc_det_jp/ch` are CC-BY-NC-ND. | MIT tag with unverified provenance; non-commercial | **Candidate, evaluation only** for backchannel timing until provenance clears (sign-off 25). | A/B after Release 2 | verify turn-taking B1; W5 decision 10 |
| pipecat-backchannel | Rules: turn incomplete, at least 0.7 s of speech, 2.5 s cooldown, probability 0.8, cached clips in the agent's voice, never in model context. | BSD-2-Clause | **Chosen** as the backchannel rules. | Clip bank Release 1; emitter Release 2 | verify turn-taking B3; W5 decisions 10 to 12 |
| X2-Turn-4B | Joint streaming ASR and five turn states on Voxtral Mini Realtime; English 225 ms; 24 GB GPU. | Weights Apache-2.0; base licence to verify | **Candidate** GPU tier on Infer after a legal read. | Later | verify turn-taking B3; W5 decision 6 |
| Easy Turn | Audio-plus-text turn model (about 850M parameters, 3.45 GB checkpoint), GPU. | Repository Apache-2.0; dataset licence unresolved | **Watch**; its training set is considered for a WaaV-trained classifier. | Later | verify turn-taking B2; W5 §3.5.4 |
| Meta hierarchical end-of-turn model (2603.13379); FD-VAD; HiThink Turn; Vox Maru; Ooma | 1.14M stereo model at 36 ms median; TurnBench leaders at 0.93 to 0.96 recall. | No code, or closed | **Watch**; Meta's design guides the later WaaV-trained six-class model. | Later | verify turn-taking B3; verify models §2 |
| Provider turn signals (OpenAI `semantic_vad`, Gemini VAD, AssemblyAI) | OpenAI semantic VAD 0.303 recall on TurnBench; AssemblyAI's confidence threshold is deprecated on Universal Streaming. | Vendor | **Chosen as inputs** to the endpoint policy, never sole deciders; AssemblyAI moves to Universal-3.6 Pro (W5 lifecycle item). | Releases 1 to 3 | verify platforms §2 item 8; verify turn-taking B2 |
| TEN VAD; TEN Turn Detection | Agora models. | Non-compete; end-user device ban | **Rejected** (licence). | None | verify turn-taking B2, B3 |
| Parakeet realtime EOU 120M; Kyutai STT 1B with semantic VAD | ASR with end-of-utterance; semantic VAD only in Kyutai's Rust server. | NVIDIA open model licence; CC-BY 4.0 | **Watch** (Infer-hosted only; Parakeet needs a legal read). | None | verify turn-taking B2, B3 |

---

## 7. Echo and noise

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| sonora 0.2.0 | Pure-Rust port of WebRTC AEC3 (M145); 4.2 µs per 16 kHz frame; reference tests pass. | BSD-3-Clause | **Chosen** behind cargo feature `aec`: automatic on phone legs and on `/ws` clients without echo cancellation. | Release 2 | verify turn-taking B3; W5 decisions 7, 8 |
| Browser WebRTC echo cancellation | Client-side AEC in browsers and LiveKit web clients. | n/a | **Chosen as relied on**: gateway AEC stays off there (double processing hurts). | Release 1 | web turn-taking §6.1; W5 §3.6.2 |
| WaaV echo layers | Transcript echo filter, correlation probe and gate against the exact delivered audio. | WaaV code | **Chosen**: always on in interaction mode, before AEC exists. | Release 1 | W5 decision 7 |
| DeepFilterNet (`deep_filter` at git revision `f1d19bf`, features `tract` and `default-model-ll`) | Noise suppression already in the default image: dependency at `gateway/Cargo.toml:251`, feature `noise-filter` (`:81`) built by `gateway/Dockerfile:19`, used at `gateway/src/utils/noise_filter.rs:4`; it embeds the DeepFilterNet3 weights at compile time. Removes noise, not voices or echo. | Code: MIT or Apache-2.0 at the user's option (README at the pinned revision, read 2026-10-05). The bundled weights have no separate licence statement there. Licence register: code **ship**; weights **legal** until read in Release 0 | **Chosen (kept)**; on LiveKit, AEC runs before it. If the weights' licence is restrictive, replace them behind the `noise-filter` feature. | Existing; licence read Release 0; ordering in Release 2 | web turn-taking §2; W5 §3.6.4; W7 §3.14 |
| webrtc-audio-processing 2.1.0 | Bindings to the C++ WebRTC audio processing module. | crates.io reports a "non-standard" licence file | **Rejected**: C++ build and a licence needing a legal read. | None | verify turn-taking B2 |
| aec-rs (SpeexDSP); aec3-rs; NVIDIA Maxine AEC | Linear echo filter; unfinished Rust port; deprecated GPU effect. | BSD/MIT; unclear; NVIDIA terms | **Rejected**: weaker on non-linear echo, unfinished, deprecated. | None | web turn-taking §6.2; verify turn-taking B3 |
| DTLN-aec; Krisp VIVA speech-detection filter | Neural AEC in TFLite only; pass-through until bot speech clears. | MIT; proprietary | **Watch.** | None | web turn-taking §6.2 |

---

## 8. Task and orchestration protocols

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| WaaV LLM background tier | A separate `LlmClient` per session with per-task history keys and its own tool loop; the final answer streams sentence by sentence after the `ANSWER:` marker, cut by the conversation path's sentence aggregator; drafts before the marker are never relayed. | WaaV code | **Chosen**: first backend; speculative start at end of turn on by default. | Release 1 | W3 §2.4, §2.5, §2.20; decisions §3, C1 |
| budprompt runs (Bud agent runtime) | `background: true` runs with SSE resume, status poll and approval re-submission when `durable_engine_enabled`; runs serialise per conversation. An answer streams only if the agent declares `background.answer_marker`; otherwise it is relayed whole at completion. | Bud internal | **Chosen**: foreground runs first, then background runs; side runs opt-in (sign-off 20); speculative start only for agents declared free of side effects. | Release 1; Release 2 | W3 §2.6; decisions §6, C1, C2 |
| Webhook with callback; DAG task backend | Generic customer backend; DAG executor with cancel and per-node progress. | WaaV-defined | **Chosen.** | Release 2; Release 3 | W3 §2.4; decisions §6 |
| A2A 1.0 (v1.0.0 2026-03-12, v1.0.1 2026-05-28) | Eight task states including `INPUT_REQUIRED` and `AUTH_REQUIRED`; `return_immediately`; push notifications. | Apache-2.0 (the `a2aproject/A2A` repository licence, read 2026-10-05); WaaV writes its own client, so no SDK licence applies. Licence register: **ship** | **Chosen** backend; its states shape WaaV's wire states. | Release 3 | verify platforms §1; W7 §3.14 |
| MCP 2026-07-28 with the Tasks extension | Stateless MCP; Tasks in `io.modelcontextprotocol/tasks` (`tasks/get`, `tasks/update`, cooperative cancellation); URL-mode elicitation required for secrets. | Specification and new code Apache-2.0, in transition from MIT (older contributions may remain MIT); documentation CC-BY-4.0 (the `modelcontextprotocol` repository licence, read 2026-10-05). Licence register: **ship** | **Chosen** backend; elicitation answers map to W4's outcomes. | Release 3 | verify platforms §1; W7 §3.14 |
| OpenAI Responses async tool calling (2026-09-03) | `async: true` tools; `wait_for_tasks` and `request_user_input_async` are application patterns; steering is GPT-6 only and undoes nothing. | OpenAI terms | **Chosen** backend. | Release 3 | verify platforms §1 |
| GPT-Live delegation and Gemini result scheduling | Vendor delegation and three delivery modes: speak now, speak when idle, know silently. | Vendor | **Chosen** as the regime C task protocol and the shape of WaaV's delivery policy. GPT-Live has no "when idle" knob. | Release 3 | verify platforms §1, §2 item 5 |
| OpenAI Agents API (public beta 2026-09-10) | Durable agent sessions with turn and `requires_action` events. | OpenAI terms | **Watch.** | None | verify platforms §1 |
| Pipecat async function results | Non-blocking calls, `is_final=False` progress, deferred delivery, `cancellable_by_llm` (1.8.0, 2026-08-26). | BSD-2-Clause (the `pipecat-ai/pipecat` repository licence, read 2026-10-05); no code copied, so no obligation today. Licence register: framework reference only | **Rejected as a dependency**; reference for progress and late results. | None | verify platforms §1; W7 §3.14 |
| LiveKit Agents | Python framework: `ctx.update`, `with_filler`, opt-in `CANCELLABLE`, `AgentTask`; preemptive generation now under `turn_handling`. | Code Apache-2.0 | **Rejected as a dependency** (WaaV uses LiveKit rooms as transport); reference for opt-in cancellation and fillers. | None | verify platforms §1; verify Liquid C13 |
| LangGraph interrupts; Temporal; Google ADK; Ultravox threads | Interrupt and resume; signals and heartbeats; confirmations; urgency levels. | Various | **Rejected as dependencies; reference**: re-run before an interrupt must be idempotent; cancellation arrives only on heartbeat. | None | verify platforms §1 |

---

## 9. Evaluation suites

| Name | What it is | Licence or terms | Verdict and reason | Plan use | Evidence |
|---|---|---|---|---|---|
| Full-Duplex-Bench v1 and v1.5 | Pause, backchannel, turn-taking, interruption; v1.5 adds side talk and background speech. | Repository CC BY-NC 4.0. Licence register: official data **eval** | **Chosen** first, through a new WaaV WebSocket adapter (shipped clients use WebRTC to a hard-coded OpenAI host); release gates use WaaV-regenerated stimuli; the official data runs only in the research job (sign-off 25). | Release 0 baseline; Release 2 | verify turn-taking A1.3, A3; W7 §2.8 |
| Full-Duplex-Bench v2 | Multi-turn live dialogue with a GPT-Realtime examiner. | CC BY-NC 4.0 | **Chosen**: Node adapter over `/v1/realtime`. | Release 3 | W7 §3.8 |
| Full-Duplex-Bench v3 | Tool use under disfluency in a LiveKit worker with 12 mock tools; TML pass@1 68.0. | CC BY-NC 4.0 | **Chosen**: port the tools and latency injector into the harness. | Release 3 | verify turn-taking A1.4, A3 |
| tau-Voice (tau2-bench voice mode) | 278 customer-service tasks with database checks; voice agents 31 to 51% in clean audio. | MIT | **Chosen** through a new tau2 provider and an MCP server wrapping the tools (the cascade refuses client tools). | Release 3 | verify turn-taking A1.1, A1.2, A3 |
| Big Bench Audio | Spoken reasoning; TML 75.7, 96.5 with background agent. | MIT | **Chosen**, with and without the background. | Release 2 | verify turn-taking A3 |
| MTR-DuplexBench | Ten-round drift, quality and safety. | Apache-2.0 | **Chosen.** | Release 2 | verify turn-taking A3 |
| TurnBench (Sesame) | 30 h of two-channel talk; end-of-turn recall, false positives and latency; above 0.15 false positives does not qualify. | Code MIT; data non-commercial ("fine for internal evaluation, not for training a product model"). Licence register: **eval** | **Chosen, evaluation only**: run in the research job for detector comparison; never training data, never a release gate; whether internal product evaluation is allowed is sign-off 25. | Release 2 (report only) | verify models C9; web interaction §4.4; W7 §2.11, §3.14 |
| Game-Time | Time awareness and simultaneous speech. | CC BY 4.0 | **Chosen** once time features exist. | Release 3 or later | verify turn-taking A3 |
| Audio MultiChallenge; SID-Bench; HumDial-FDBench | Multi-turn memory and repair; interruption detection; duplex dialogue. | MIT; not stated; Apache-2.0 | **Chosen**: report only (Release 3); detector sets (Release 2). SID-Bench licence needs a legal read. | Releases 2, 3 | verify turn-taking A2, A3; W7 §3.8 |
| IndicFDB (2609.31967) | Full-Duplex-Bench across 10 Indian languages; finds commercial APIs "either fast or robust to pauses, never both". | Not checked | **Candidate** once a Hindi front exists. | Release 4 | verify models §2; W7 §3.8 |
| APEX-Voice (2609.34973); DuplexAct-Bench; GCM-Bench; JarvisBench | Professional workflows (every system at or below 25% Pass@1); proactivity; references after interruption; human-in-the-loop attention. | Not in the evidence | **Watch.** | None | verify models §2; web interaction §4.2, §5 |
| VoiceAgentBench; FD-Bench | Indic spoken tool calling; noise recipes. | Krutrim Community Licence; custom licence | **Legal read** (VoiceAgentBench); recipe only (FD-Bench). | None | W7 §3.14 |
| Artificial Analysis speech-to-speech index | τ-Voice and Full-Duplex-Bench subsets across vendors. | Public site | **Reference** for vendor comparison (non-delegating leader is 11.5 points behind the delegating leaders). | None | verify models C2 |

---

## 10. Facts the fact-checks could not confirm

Treat these as open until a probe or a legal read settles them: the ARC4 encoder's licence chain behind MoshiRAG;
whether `INTERRUPTED` is rejected on Gemini's wire; Ultravox's GLM-4.6 base and 0.384 s endpoint default; Inworld's
OpenAI compatibility; StepFun and SeedRealtime API access; the Kyutai RL before-and-after numbers; the hardware behind
any Liquid audio latency claim; vLLM prefix caching for LFM short-convolution state; whether `llama-server` streams a
tool call before trailing text; Liquid commercial terms and pricing; Krisp pricing; the provenance of MaAI's
MIT-tagged models; the licence of the X2-Turn base model, Easy Turn's dataset, SID-Bench and IndicFDB; the licence of
DeepFilterNet's bundled weights; the licence of LFM2.5-VL-3B and the DSpark drafter (assumed to be the LFM family
licence); the licence of the original Hibiki; and the absence claims (no open full-duplex model beyond English and
Chinese, no opened TML preview), which cannot be proved. The licences of DeepFilterNet's code, A2A, MCP and Pipecat were
read from each project's own repository on 2026-10-05 and are recorded in sections 7 and 8; no fact-check covered
them, so the licence register keeps the source of each read.

The bake-off's numbers are forecasts on a 68-decision seed set written by the package's author, measured on an ARM
CPU and a GPU shared with other tenants. The gates run on frozen Release 0 sets that are not self-referential
(Addendum D1.5): the 68 seed decisions keep their human labels, and about 500 more are written outside the synthetic
generator and labelled by two people who wrote neither the generator nor the router; teacher labels may qualify a
profile only once the teacher agrees with the human labels at kappa 0.7 or more, and act choice must pass on both the
teacher-labelled and the human sets. The Release 0 x86 run and the Release 2 pool's capacity test replace the
bake-off's sizing floors.
