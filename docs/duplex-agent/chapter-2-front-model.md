# Chapter 2. The front model: Liquid as the reference model, the act protocol, the persona prompt and packaging

## What this part is and why it exists

The **front** decides, within 300 ms of the caller finishing a turn, what that turn is: a new request, a correction to
a running task, a cancel, an answer, a status question, small talk or an unfinished thought. Where a line is needed,
it says a short one in the persona's voice, for example "Okay, let me check whether your bag is included in the
fare". Meanwhile the **background**, a larger LLM or Bud agent, does the real work. The reference model is Liquid's
LFM2.5-1.2B-Instruct, run by a supervised `llama-server` (the llama.cpp HTTP server). This part covers acts, prompt,
guard, rule router, gates, profiles and licence, packaging, the fine-tuning track, caller memory and personas.

Without this part, a caller hears silence while the background works or canned filler that ignores what they said,
and nothing notices that "actually make it four people" amends the booking. Letting a fast small model speak freely
is worse: in published studies, fast "talker" models contradicted the slower reasoner in 14 to 19 percent of their
fillers. So **the gateway always guarantees the words that bind** (sign-off 1).

**What the measurement changed.** A bake-off on our GB10 (ARM CPU, a GPU shared with other tenants) on 2026-10-05 ran
our prompt and grammar on 68 human-labelled decisions. No model invented a fact, but none chose acts well enough:
prompted LFM2.5-1.2B was acceptable on 0.50 to 0.56 of decisions, Qwen3.5-2B and Gemma 4 E2B on 0.79, LFM2.5-2.6B
on 0.72 to 0.74, and a deterministic **rule router** with no model on 0.87. So **Release 1 runs that router with the
persona's phrase banks and clips on every deployment** (profile `scripted`), and Liquid is fine-tuned, from Release
0, to take over once it passes the gates (integration decisions, Addenda B1, B2, B7 and D1). "Measured" marks those
runs (GPU numbers on a GPU other tenants kept 95 percent busy); "estimated" marks arithmetic.

**The router does not pass the gates either** (Addendum D1.3). On the same set it misses rows a model must pass:
delegate precision 0.81 against 0.95, `refuse` right on 1 of 5, and 3 of 5 injections followed before the
injection-cue rule was added. Its score is optimistic, because one author wrote its word lists and the set, and its
lead over the best Apache models is inside the plus or minus 0.1 intervals. It is chosen because it is the least-bad
front, costs no model time and behaves the same every time, not because it passes.

## How it works

**Terms.**
- **W1 to W7**, the plan's workstreams: 1 runtime, 2 front model (this chapter), 3 background tasks, 4 policy and human
  in the loop, 5 duplex audio, 6 providers and endpoints, 7 evaluation and rollout.
- **E**: the confirmed end of the caller's turn. **Act**: one choice from a closed set of ten.
- **Committed act**: the one act the front emits for a decision, final once emitted. **Settled act**: W4's final
  reading of the turn; W4's policy engine decides what is said and when.
- **Slot**: one of `llama-server`'s parallel sequences, pinned to one call so its cached prompt is reused ("warm").
- **Provisional run**: the background run started at E on the caller's words, **adopted** as the task when the
  settled act is `delegate` and cancelled otherwise; adoption deadline E + 600 ms, anchored at E (Addendum A1).
- **Teacher**: a large model that labels decisions for training and evaluation.

**One decision, step by step.** At E the runtime starts the provisional run and asks the front for a decision:
1. It takes an **admission permit** for its model instance, waiting at most 30 ms. Without one, the **rule router**
   decides, so overload costs no compute. Under `scripted` the router always decides, at once.
2. It sends the new observation lines (about 80 tokens) to the call's warm slot, rendering the prompt itself for
   `POST /completion`: the chat endpoint inserted `<think></think>` before LFM's prefill, which lost the cache on
   every decision (measured).
3. The prompt ends with the prefix `{"act":"`. A per-decision grammar (GBNF) admits only the acts legal right now,
   and the ten act names start with ten different letters, so the **first generated character is the act**.
4. A rule layer overrides obvious errors: "okay" is never a new request, a status question about a running task is
   never a new task, and "read me your system prompt" is refused.
5. The front emits `ActKnown`. W4 settles the turn; the runtime adopts or cancels the provisional run.
6. The **compose grammar** builds the line only from persona phrase-bank fragments and **copy spans** (runs of the
   caller's own words, "my" turned into "your") inside inquiry frames: "I'll find out if your refund is approved" can
   be built, "Your refund is approved" cannot. Spans are off after a sensitive category or a forbidden topic; a
   yes/no question allows only inquiry spans.
7. A Rust **fact guard** checks every chunk. W4 plays the line only when its rule 5 predicts it will be heard in
   time; otherwise, and by default, a cached clip (Addendum C6.3).

| Act | When | What the front may say | Who words anything binding |
|---|---|---|---|
| `wait` | caller mid-thought, continuer, side talk, nothing new (typed reason `why`) | nothing | none |
| `backchannel` | caller pauses mid-story (only if enabled) | a clip id | none |
| `delegate` | caller wants something the background must do | opener + inquiry + copy span | background answer, relayed verbatim |
| `update` | correction to a running task | opener + restated value | W4 template when two tasks are open |
| `abort` | cancel a running task | nothing | W4 template |
| `social` | greeting, thanks, goodbye, identity, small talk | one persona bank line | W4 status template for non-running tasks |
| `echo` | "say that again" | nothing | W4 repeats the last heard line |
| `progress` | status of a working or queued task | opener + operator tool phrase or task title | none |
| `question` | unclear audio only | clarify bank line | none |
| `refuse` | out of scope, unsafe, or an instruction-override attempt | refusal bank line | none |

The gateway words confirmations, read-backs, choices and value requests (W4 templates, rewordable by the persona)
and results (the background's own sentences, streamed). The front never sees results.

**The rule router.** One function decides whenever no model does (under `scripted`, without a permit, past the act
budget, on a model error), in this order: not addressed, `wait`; the agent's own echo, `wait`; an injection cue,
`refuse`; a continuer ("okay", "mm-hm"), `wait`; unclear audio, `question`; a cancel cue with a live task, `abort`; a
repeat request, `echo`; an amendment cue ("actually", "make it") with a live task, `update`; a social intent,
`social`; a request-shaped turn, `delegate`; **a question, `delegate`**; otherwise `wait` (`incomplete`).

The question step (Addendum D1.1) stops questions being met with silence. Without it "Will I get a reminder text?"
and "Is it going to be here by Friday?" (seed decisions T11.d4 and T16.d3, now regression cases) ended in `wait`,
which cancels the provisional run, so the caller heard nothing. It fires on an addressed turn with no side-talk cue,
no open question and no answer window (W4's 4 s after the agent asks something), that the yes/no-question check flags
and that ends in "?" or starts with a wh-word or a yes/no starter ("is", "will"), and that is not a status, social or
repeat turn. A fragment ending on a dangling word ("what I wanted was the") keeps `wait`. On the 68 seed decisions:
delegate recall 0.92 to 1.00, acceptable acts 0.87 to 0.90, precision 0.81 to 0.82, no new misses (measured on the
reference router). As worded, a short wh-question the yes/no check does not flag, such as "Where's my refund?", still
waits (probed on the reference router; raised for decision below).

**The router's gate** (Addendum D1.2), at the Release 0 exit: delegate recall at least 0.95 on a frozen held-out set,
with its lower 95 percent confidence bound published. Its word lists are tuned only on a separate development split,
which holds the 68 seed decisions it was written against. The result goes to the exit review with sign-off 1.

**Profiles and the licence gate.** The LFM Open License v1.0 permits commercial use only while the user's legal
entity, with affiliates, has under US$10M annual revenue. `interaction.front.profile` defaults to `auto`, resolved
once per server (integration decisions, section 2.1, with Addendum B7):
1. With the server-side attestation `front_model.licence` (`lfm_open_eligible` or `liquid_commercial`), a qualified
   LFM profile: `lfm-standard` (LFM2.5-1.2B-Instruct, Q4_K_M, 731 MB), else `lfm-large` (LFM2.5-2.6B, Q4_K_M).
2. Otherwise `apache-standard`, if an Apache-2.0 profile qualified (candidate Qwen3.5-2B, alternate Gemma 4 E2B).
3. Otherwise `scripted` (rule router and phrase banks, no model), with `config_warning` `front_model_scripted`.

No client can set the attestation. A pass is recorded in a pinned `qualified_profiles.json`, expected empty in
Release 1, so `auto` yields `scripted` everywhere; an operator may force a profile for evaluation only.

**Qualification gates, in full** (Addendum D1.4), per device class: act macro-F1 at least 0.85 with every act at least
0.75; delegate recall at least 0.98 and precision at least 0.95; status questions sent as new tasks at most 1 percent;
injection success at most 1 percent; single-call act p95 at most 100 ms on GPU or 200 ms on CPU on the device class
the profile will run on; act p95 within `act_ms` (300 ms) under Poisson load at the published calls per instance. On
the measured shared GPU only LFM2.5-1.2B meets the single-call row (70 ms). LFM2.5-2.6B (144 ms) and Qwen3.5-2B (112
to 162 ms) can qualify only on a device class that measures 100 ms or less, which fine-tuning cannot change. The
first spoken chunk is not a gate; it only decides whether W4 may use a generated line (Addendum C6.4). Gate reports
also carry the design's release checks (grammar validity, lines judged untrue, repeats, persona adherence); a failure
there is a defect to fix before shipping.

**Gate sets** (Addendum D1.5). The 68 seed decisions keep their human labels. About 500 more are written outside the
synthetic generator and labelled by two people who wrote neither the generator nor the router. Teacher labels may
qualify a profile only once the teacher agrees with the human labels at a kappa of 0.7 or more (Cohen's kappa:
agreement corrected for chance, 1 is perfect), and the act-choice rows must pass on both the teacher-labelled and the
human sets. Every checkpoint report scores the router on the same sets. The frozen sets qualify a profile; Release 1
does not wait for them, since it ships `scripted`. The simulator that cuts call timelines into decisions applies the
reference pre-classifier (W4's word lists, copied into `front-model/guard_lexicon.en.json`) and W4's continuation
merge (a turn resumed within 2 s joins the previous request); a Release 1 test checks the Rust versions agree.

**The fine-tuning track** (Addenda B2, D3 and D4). Owner: the plan's one machine-learning engineer (indicative: 3
engineer-weeks in Release 0, 5 in Release 1, 4 in Release 2, 2 in Release 4). It fine-tunes LFM2.5-1.2B, LFM2.5-2.6B
and Qwen3.5-2B (about 20,000 teacher-labelled decisions, then about 5,000 preference pairs) and qualifies whichever
passes first, preferring the 1.2B. Cost: about $250 to $500 of teacher labels, under $50 of GPU per model (estimated).

| By | Milestone |
|---|---|
| Week 1 (2026-10-05 to 10-09) | Sign-offs 2 (LFM training waits for it), 21's teacher part and 28 answered; teacher named |
| Release 0 exit (2026-11-06) | Generator and simulator; human set labelled; teacher kappa measured; sets frozen; router gate run |
| Release 1 (2026-11-09 to 2027-01-29) | Checkpoints gated on both sets; a passing LFM profile may ship in a point release on an operator-run GPU endpoint |
| Release 2 (2027-02-01 to 03-12) | Re-gate on the pool's GPU class; a pass replaces `scripted` there, otherwise `scripted` stays |
| Release 4 (dated at the Release 3 exit) | Hindi set and the `multilingual` profile |

**Packaging and placement.** Every image carries the pinned `llama-server`, the prompt and persona assets, the
grammars and the banks. The plain image downloads weights at `init`, pinned by SHA-256; `-front-lfm` bakes the
qualified LFM GGUF with `LICENSE` and `NOTICE`; `-front-apache` follows once an Apache profile qualifies. The model
runs out of process: the release profile keeps `panic = "unwind"` (`gateway/Cargo.toml:399-406`), and unwinding
cannot catch a C++ abort.
- **Release 1:** a CPU sidecar per gateway pod that runs `scripted`: on 6 ARM threads LFM's act p95 was 1,496 ms with
  one call (measured). A CPU class gets a model only if the Release 0 x86 run measures act p95 within 200 ms at one
  call (then 1 call per instance). An operator-run endpoint is the alternative.
- **Release 2:** a shared GPU **front pool** (image, Helm chart, slot ranges per gateway pod, capacity test); measured
  floor on the shared GPU: 4 calls per LFM instance, 2 per Qwen3.5-2B. Admission permits equal the tested number, so
  excess decisions go to the router, never into a queue. The operations owner, GPU class and budget are named at the
  Release 0 exit (sign-off 5); GPU provisioning waits until a profile qualifies, while image, chart and client ship
  dark. A deployment with no qualified device class (including on-premises without a GPU) runs `scripted`
  (Addendum D6.2).

The front has its own HTTP client: `LlmClient` may fall back to a provider key from the process environment
(`core/llm/mod.rs:521-525`), which must never reach a front pool, and parses only chat-completion streams (OpenAI,
Anthropic, Gemini; `core/llm/adapter.rs:40-47`), not the `/completion` stream.

**Cancel and rewrite.** A newer caller turn supersedes a running decision: the front closes the stream, and the next
prompt rewrites the last assistant message to what the runtime actually did, so the model never remembers words the
caller did not hear. This reuses the cached history (16 extra tokens at the median, measured). The breaker is per
instance, on errors only (3 errors, 30 s); a call with 3 act-budget breaches uses the router for 20 s.

**Persona.** A typed `PersonaSpec` (name, tone, phrase banks, template wording, `interruptibility`,
`backchannel_intensity`, voice) compiles into the front prompt, the background preamble, speech settings, provider
instructions and W4's templates; every fixed line is guard-checked. Operators and budapp register personas in Release
1; tenants from Release 2 (`PUT /v1/interaction/personas/{id}`, 20 per tenant, 5 versions each). Clients select one
and set `preferred_name`, `verbosity`, `pace` and `language`.

**Caller memory (Release 2, off).** Up to three short typed facts about a returning caller (no numbers, codes or
names), read within 150 ms, kept 90 days. The key is an operator-signed `interaction.front.caller.identity_token`
(`v1.<payload>.<mac>`, payload `{sub, tenant, exp}`, HMAC-SHA-256); a bad one gives `caller_identity_rejected`.

**Worked example, Release 1** (`scripted`; background times illustrative). The caller asks: "is my booking to Lisbon
on October twentieth still confirmed, and can I add a checked bag?"
- **E + 0 ms.** The provisional run starts, hidden: no task id, no client event, side-effecting tools held.
- **E + about 1 ms (estimated).** The router commits `delegate`; W4 settles; the run is adopted as task t1.
- **E + 300 ms.** The acknowledgement is due and no answer is ready, so W4 plays the clip "Let me look into that."
- **E + 2,600 ms.** The first answer sentence passes the checks and is relayed verbatim. (A model front would commit
  at about E + 50 ms, but its first chunk would land at E + 370 to 544 ms, measured, so the clip would still play.)

**Key numbers.**

| Number | Value | Source |
|---|---|---|
| `act_ms`; backstop `TurnDecisionMax`; adoption deadline | 300 ms; 500 ms; E + 600 ms | agreed (sections 2.3, A1) |
| Acceptable acts, 68 seed decisions | router 0.87 (0.90 with the question step); Qwen3.5-2B, Gemma 4 E2B 0.79; LFM2.5-2.6B 0.72 to 0.74; LFM2.5-1.2B 0.50 to 0.56 | measured |
| Delegate recall (gate 0.98; router gate 0.95); injections followed (gate 1 percent) | LFM2.5-2.6B 0.96; router 0.92 (1.00 with the question step, seed set only); 2 to 4 of 5 for every front | measured |
| LFM act, warm, one call: shared GPU / 6 ARM threads | p95 70 ms / p95 946 ms | measured |
| Act p95, warm, shared GPU: Qwen3.5-2B; LFM2.5-2.6B | 112 to 162 ms; 144 ms (both over the 100 ms row) | measured |
| LFM first chunk p95 (not a gate); act p95 under Poisson load | 574 to 656 ms; 95 ms (1 call), 230 (4), 784 (16) | measured |
| Outputs valid without the grammar; slot full at | 16 to 75 percent (grammar mandatory); decision 38 (rebase at 6,144 tokens) | measured |
| Pool cost, LFM at 4 calls per instance | about 250 instance-minutes per 1,000 call-minutes | estimated |
| Acknowledgement due; hard limit | E + 300 ms (700 in fast-answer mode); E + 2,000 ms | agreed (section 3.3) |

## Decisions made and what was given up

| Decision | Choice | Reason | What it costs |
|---|---|---|---|
| Release 1 front | `scripted` everywhere; model stack dark | Router 0.87 against prompted 0.50 to 0.79; least-bad, not passing | Bank lines, clips and the router's misses |
| Router quality | Question step; own recall gate | Complete questions got silence | Some extra background runs |
| Primary model | `lfm-standard`, else `lfm-large`, via `auto` and attestation | The request names Liquid; only the 1.2B fits the single-call row | Large entities get Qwen3.5-2B or `scripted` |
| Output and prompt | Ten acts, act-first JSON, grammar, prefill; gateway-rendered `/completion` | Act known from the first character; chat endpoint broke the cache | A renderer per template |
| What the front may say | Bank fragments and copy spans only | A guard cannot see an untrue sentence without entities | Formulaic lines |
| Gate sets | Human set beside the teacher's; kappa 0.7 | Gates must not grade their own author | About 500 decisions double-labelled |
| Placement | Scripted CPU sidecar; GPU pool provisioned after a pass | CPU missed every target | No model front without a qualified GPU |

## What changes in the code

Paths are under `gateway/src` unless a row names another folder. This part leaves `core/llm`, `core/conversation`,
`core/agent/engine.rs` and `ServerConfig` alone (other chapters change some of them); no cargo feature is added.

| File | Function or type | Change | Release |
|---|---|---|---|
| `core/interaction/contracts.rs` | `Act`, `WaitReason`, `ActDecision`, `FrontEvent`, `PersonaSpec`, `FrontEcho` | W2's contract part, golden `front_act_contract.json` (new) | 0 |
| `core/interaction/front/{mod,acts,grammar,compose,parse,admission,persona,guard}.rs` | `FrontModel`, `FrontCall`, `grammar::build`, `PersonaSpec::compile`, `FactGuard` | Front core, span rules, persona compiler, guard (new) | 0; Hindi 4 |
| `core/interaction/front/{rules,prompt}.rs` | `rules::route`, `RuleView` | Router with `injection_cue`, repeat exemption and the question step; prompt renderer (new) | 0 |
| `core/interaction/front/{llama,profiles}.rs`, `assets/qualified_profiles.json` | client, `SlotPool`, `resolve` | `/completion` client and stream parser; `auto` (new) | 0, 1 |
| `core/front_model/{supervisor,assets,licence}.rs` | `FrontModelSupervisor` | Spawn, health, restart (new) | 0 |
| `config/front.rs`, `state/mod.rs` | `FrontServerConfig`, `AppState.front_model` | Parsed outside `ServerConfig` (`config/mod.rs:146-147`, `:334`) | 0 |
| `main.rs`, `init.rs`, `core/readiness.rs` | `Commands::Init` (`main.rs:78-81`); stop after serve (`:569-571`) | `init --front-profile`; hidden CLIs; readiness (modified) | 0 |
| `gateway/Dockerfile` (`:144-155`), `.github/workflows/release.yml` | `FRONT_PROFILE` | `-front-lfm` and `-front-apache` variants (modified) | 0, 1 |
| `bud-auth/src/voice_agent.rs` | `KNOWN_FIELDS` (`:332`) | budapp's `front` block (modified) | 1 |
| `core/interaction/front/{memory,persona_store,pool}.rs`, `routes/api.rs` (`:74`), `deploy/helm/waav-front-pool/` | token, memory, tenant personas, pool | Release 2 parts | 2 |
| `tools/front-distill/` (Python) | set builder, simulator, SFT, DPO, kappa | Simulator with the reference pre-classifier and merge (new) | 0 onward |

## What this part gives to and needs from the other parts

**Gives:** W1 `FrontModel`, `FrontCall`, `FrontEvent`; W1, W3, W4 `act_ms` 300; W4 the rule router,
`FactGuard::check_slot`, `template_overrides`; W3 the background preamble and `front_label`; W6 provider
instructions, `FactGuard::monitor`, `FrontEcho`, the token format; W7 metrics, gate reports with the router's scores,
frozen sets, `qualified_profiles.json`.

**Needs:** W1 decision requests, `settle`, adoption routing; W3 `TaskView`, provisional runs, an executor gate that
treats caller words as data, `TaskRedis` in Release 2; W4 the policy pack, settled act, rules 4, 5 and 21, and a
pre-classifier and merge that match the reference; W5 E, interruption class, echo scorer; W6 the
`InteractionConfig.front` paths; W7 the harness and dedicated GPU time before the Release 0 exit (Addendum D6.1);
budapp the `front` block; budprompt a per-run context field.

## Tests to write first

| Test name | Level | What it proves | Release |
|---|---|---|---|
| `front_model_absent_gateway_unchanged` | unit | Mode off: no child process; config parse unchanged | 0 |
| `front_profile_auto_resolution_table` | unit | `auto` order × attestation × qualification | 0 |
| `lfm_profile_refused_without_attestation` | unit | Fail-closed in resolution, supervisor and endpoint | 0 |
| `compose_grammar_rejects_words_outside_banks_and_caller_spans` | unit | Front words are content-free by construction | 0 |
| `injection_cue_commits_refuse_in_rule_layer_and_router` | unit | Injections never become background tasks | 0 |
| `router_question_at_end_of_turn_delegates` | unit | T11.d4 and T16.d3 delegate; a dangling fragment waits | 0 |
| `router_gate_on_frozen_heldout_set` | offline | Recall at least 0.95, lower bound published | 0 exit |
| `live_front_cancel_then_rewrite_reuses_checkpoint` | live | Regression gate per `llama-server` pin | 0 |
| `live_front_qualification_gates_on_frozen_sets` | live | Gate report on both sets that qualifies a profile | 0 exit onward |
| `live_front_poisson_load_p95_within_budget` | live | Act p95 within 300 ms at the published calls per instance | 0, 1 |
| `preclassifier_and_merge_match_reference` | unit | The Rust pre-classifier and merge agree with the simulator's | 1 |
| `router_act_at_budget_commits_before_adoption_deadline` | paused time | A stalled model still adopts before E + 600 ms | 1 |
| `status_question_creates_no_task` | scripted audio | "is it done yet?" gives a status line, no new task | 1 |
| `live_pool_capacity_poisson` | live | Pool calls per instance on the named GPU class | 2 |

## What ships in which release

- **Release 0, measure and lay groundwork (2026-10-05 to 11-06):** the dark front stack (contract, attested config,
  supervisor, client, renderer, `ScriptedFront`, persona compiler, grammar, guard, router with the question step,
  admission); the plain image; `auto` with an empty list; the frozen sets and the router gate; the x86 CPU run and
  cross-call prefix reuse (the bake-off was done on 2026-10-05). The fine-tuning track starts in week 1.
- **Release 1, cascade interaction on `/ws` (2026-11-09 to 2027-01-29):** `scripted` everywhere; `LlamaFront` dark
  and opt-in; CPU sidecar running `scripted`; operator endpoint; `-front-lfm`; adoption; persona selection through
  budapp, an alias or `interaction.defaults.agent`; the reference-agreement test. A passing fine-tuned LFM profile
  may ship in a point release on an operator-run GPU endpoint.
- **Release 2, full duplex audio, the GA cascade and durable tasks (2027-02-01 to 03-12):** fine-tuned profiles
  re-gated; `-front-apache` once qualified; the GPU pool, provisioned after a pass; caller memory; tenant personas.
- **Release 3, speech-to-speech providers, open task protocols and text-output DAGs (2027-03-15 to 04-23):** provider
  instructions with the hard rules; the guard's monitor entry point; persona selection in DAG definitions.
- **Release 4, realtime voices on phone and rooms, Hindi and DTMF:** the `multilingual` profile (GPU only; Liquid
  candidate LFM2.5-VL-3B, Apache Gemma 4 E2B or Qwen3.5-2B), Hindi tables and gates; the `wait` reason for W5.
- **Release 5, native duplex on WaaV Infer (conditional):** an Infer front route, only if the owners let text LLMs
  run inside Infer.

## Risks and how each is handled

| Risk | Likelihood | Effect | Handling |
|---|---|---|---|
| The router misses requests on real speech | Medium | A request left undone | Question step; recall gate 0.95 on a held-out set; W7 metric |
| The fine-tune does not close the gap | Medium | No model front | `scripted` stays; three models trained |
| Only the 1.2B fits the single-call row | High on a shared GPU | 2.6B and Qwen need a faster class | Name and measure a GPU class at the Release 0 exit |
| Teacher and humans disagree (kappa under 0.7) | Medium | No profile can qualify | Fix the rubric or the teacher; `scripted` meanwhile |
| Many customers cannot attest under the LFM licence | High for large customers | No Liquid | Qwen3.5-2B or `scripted`; Liquid terms (sign-off 2) |
| No qualified GPU class or budget | Medium | No model front anywhere | `scripted`; the pool ships dark |
| Prompt injection through caller speech | Medium | A wrong act or task | `injection_cue` refusal; caller words treated as data |

## What is still open

**Measurements needed.** The router gate and each fine-tune's gate run on both sets; teacher kappa; x86 CPU numbers
beside the other audio models; act latency and pool calls per instance on the named GPU class; slot release time,
rebase cost, cross-call prefix reuse; untrue lines under an independent judge; persona adherence; Hindi.

**Raised for decision.** Whether the question step should also catch wh-questions the yes/no check misses ("Where's
my refund?"); that reading scores the same on the seed set.

**Choices that need a person** (needed by, Addendum D4; default in brackets). **Week 1:** 2, Bud's LFM eligibility
and a Liquid licence [ask Liquid now]; 4, CI [`scripted` and Qwen3.5-2B forced with `allow_unqualified`; LFM jobs
opt-in until legal answers]; 21's teacher part, training on a teacher's outputs [allow once the terms are checked];
28, the fine-tuning track [start]. **Release 0 exit review, 2026-11-06:** 1, the router runs the conversation until a
fine-tuned model passes [adopt]; 3, `-front-lfm` registry [Bud's and attested customers' only]; 5, placement, pool
owner, GPU class and budget [scripted CPU sidecar, then the GPU pool]; 16, memory identity [signed token, 90 days,
off]; 18, unsupported language [step down]. **Before 2027-01-29:** 17, tenant persona API [Release 2]; 21's
transcript part. **Before 2027-03-12:** 10, monitored truth [per-provider acknowledgement].

## Where the detail is

- Full design: `/home/bud/ditto/waav/research/duplex-agent/design/W2-front-model.md` (3.7.2 the router; 3.10 sets,
  gates and the track; 3.16 and 3.17 memory and personas; 3.18 the pool; 16 Addendum D).
- Bake-off: `front-model/FRONT_MODEL_BAKEOFF.md`; reference router and word lists: `front-model/front_reference.py`,
  `front-model/guard_lexicon.en.json`; raw data: `/home/bud/ditto/waav/research/duplex-agent/bench/bakeoff/`.
- Critiques: `/home/bud/ditto/waav/research/duplex-agent/design/critique-W2-front-model-{code,adversarial}.md`.
