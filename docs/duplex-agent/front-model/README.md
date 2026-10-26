# Front model assets: prompt, persona, grammar, banks, templates, guard tables, evaluation set

Written 2026-10-04 as part of the integration phase of the full-duplex interaction agent plan, and updated on
2026-10-05 after the measured bake-off and Addenda A, B and C of the integration decisions, and again the same day
after the final independent review (Addendum D1.1: the rule router's addressed-question step). These are planning
artefacts, **not gateway code**: they are the files the gateway would ship for its front model, in a form a reviewer
can read and a benchmark can run, plus an executable reference implementation that pins down the behaviour the
gateway's Rust modules must have. This folder is the plan's copy of
`/home/bud/ditto/waav/research/duplex-agent/plan/front-model/`. No gateway or WaaV Infer source was changed.

Terms. The **front** (or front model) is the small local model that answers at once and keeps the call going. The
**background** is the larger LLM or agent that does the actual task. A **decision** is one call to the front, made at
the end of a caller turn, after a task status change, or at a caller pause. An **act** is the front's choice for a
decision, from ten: `wait`, `backchannel`, `delegate`, `update`, `abort`, `social`, `echo`, `progress`, `question`,
`refuse` (W2 section 3.2.1). A **copy span** is a run of the caller's own words, with "my" turned into "your" and so
on, that the front may repeat. **Compose mode** means the front can only speak phrase-bank fragments and copy spans.
The **fact guard** is the deterministic check on anything the front could say. The **rule router** is the
deterministic function that chooses an act with no model at all; the profile that uses only the rule router, the
banks and the templates is called `scripted`. **E** is the confirmed end of the caller's turn. W1 to W7 name the
plan's workstreams: workstream 1 runtime, 2 front model, 3 background tasks, 4 policy and human in the loop, 5 duplex
audio, 6 providers and endpoints, 7 evaluation and rollout.

The binding source is `../INTEGRATION_DECISIONS.md` (sections 2 and 7, then Addenda B, C and D; later addenda win
over the earlier sections), then the front-model design
`/home/bud/ditto/waav/research/duplex-agent/design/W2-front-model.md` and the policy design
`/home/bud/ditto/waav/research/duplex-agent/design/W4-policy-hitl.md` (policy, templates, banks). The measured results
are in `FRONT_MODEL_BAKEOFF.md` in this folder.

Which model these files serve. The reference model is LFM2.5-1.2B-Instruct, Q4_K_M, profile `lfm-standard`; the
stronger Liquid base LFM2.5-2.6B (profile `lfm-large`) and the Apache-2.0 candidate Qwen3.5-2B use the same files
unchanged. Measured on the GB10 with these files, **no prompted model passes the Release 1 front gates** (prompted
LFM2.5-1.2B chose an acceptable act in 0.50 to 0.56 of 68 decisions, LFM2.5-2.6B 0.72 to 0.74, Qwen3.5-2B 0.74 to
0.79, the rule router 0.87 as measured and 0.90 after the review fix, all on the same author-labelled seed set; the
router too misses the model gates), so **Release 1 runs `scripted` by default on every deployment**, and the model stack
ships dark, for evaluation only (Addendum B1). The files therefore serve two purposes now: the banks, templates and
rule lexicons are what Release 1 speaks with, and the prompt, grammar and guard are what the distillation track
trains and gates the fine-tuned models on, from Release 0 (Addendum B2).

## What each file is

| File | What it is | Owner in the designs | Gateway home (planned) |
|---|---|---|---|
| `front_system_prompt.md` | The stable-prefix system prompt with `{{slots}}`, the per-call header, the per-decision observation and the background preamble (plus provider instructions for regimes B and C), each with where it is appended and why. The four fenced blocks are read directly by `front_reference.py` | W2 owns all prompt text | `core/interaction/front/prompt.rs` and its text assets |
| `persona.schema.json` | JSON Schema (draft 2020-12) for `PersonaSpec`: every field the designs use, with a description and a safe default | W2 (`PersonaSpec`), W4 (template and bank format), W5 (`interruptibility`, `backchannel_intensity`) | `core/interaction/front/persona.rs`; `contracts.rs` |
| `personas/travel_northwind.json` | Travel booking agent "Ava" for Northwind Travel (phone) | example | operator YAML or budapp `front` block |
| `personas/clinic_harbourview.json` | Clinic appointment scheduler "Nora": refuses medical advice; date of birth, insurance and health-card numbers are sensitive and never taken or read back by voice | example | same |
| `personas/retail_larkspur.json` | Retail order-support agent "Sam" (web) | example | same |
| `act_grammar.gbnf` | One instance of the per-decision GBNF grammar (W2's own example turn): the continuation after the assistant prefill `{"act":"`, so the first generated character names the act. It is the gateway's grammar (`front-act-2`: the `control` act of chapter 9, `follow_up` as `follow-up-act`, no `echo` or `backchannel`), generated by `front_reference.py --write-grammars`; the gateway's test `the_bag_question_grammar_is_the_packaged_one` builds the same turn and compares every rule | W2 | built per decision by `core/interaction/front_model.rs` |
| `compose_line.gbnf` | The compose-mode line grammar alone (openers, inquiry phrases, copy spans, restating spans, progress lines, social and refusal lines) for the same turn | W2 | same |
| `phrase_banks.en.json` | 19 groups, 102 lines: openers, inquiry fragments, acknowledgement clips, progress and status openers, clarification, refusal, social lines by intent, holding lines, check-ins, wait-agreed lines, bridges, withdrawals, the stall clip, backchannel clips and the lines said when a service fails (chapter 10: stall, hearing, exit, exit with a follow-up, back). Each group is tagged with the acts that may use it and its situation; each line may carry `polar`, `intent`, `signed` | W2 groups feed the grammar; W4 groups are clips W4 plays without a model call | W4's policy pack (`PolicyPack`), merged with persona overrides by the compiler |
| `templates.en.json` | 65 W4 templates with 11 typed slot kinds: confirmation read-back (separate clauses), choice, value, status, presence, offers, which-task, finish-or-stop, cancellation truth, failures, approvals, sensitive values, sign-in links. Templates that report an outcome name the task event they require | W4 | W4's policy pack |
| `guard_lexicon.en.json` | The fact-guard tables: the entity rule and its source tiers, number, money, date and code normalisation, completion claims, outcome predictions, ETA promises, channel promises, unbacked action narration, role leak, disfluency, polar first words; plus the copy-span rules, the sanitiser lists, the function words a copy span writes in lower case (another language is the front model's reading, r14), the sensitive categories and the rule-layer cue lexicons | W2 (guard), W4 (lexicons) | `core/interaction/front/guard.rs`; W4's pack |
| `eval_scenarios.json` | 23 call timelines, 68 labelled decisions across the three personas, 25 labelled bad outputs and 17 template checks; the benchmark's six scenarios converted to the act protocol. It was the bake-off's evaluation set and seeds the frozen sets and the preference pairs of the distillation track | W2 supplies, W7 runs | seed of the frozen evaluation sets at the Release 0 exit |
| `front_reference.py` | Executable specification, Python 3 standard library only: persona compiler, renderers, legality table, grammar builder (the gateway's `front-act-2`, and with `contract="bakeoff"` the bake-off's `front-act-1`, which the eval set is scored against), a GBNF parser and recogniser that mirrors llama.cpp build 11200, stream parser, rule layer, shared rule router, fact guard, template renderer, scorer, `--self-test` (47 cases), and an optional check through llama.cpp's own grammar code | W2 | the Rust modules above; the hidden `waav-gateway front-render` and `front-guard` subcommands (W2 section 3.6.8) do the same for training data |
| `FRONT_MODEL_BAKEOFF.md` | The measured bake-off: six models on the GB10 with these files, quality, latency, cancel-and-rewrite, grammar cost and a Poisson load test, and what each result decided | orchestrator, for integration decisions section 8 | none (evidence) |

## How a persona is personalised

Four sources, each typed, none of them free text from a client (integration decisions section 7 and Addendum C5;
W2 sections 2.5, 3.4.2, 3.16 and 3.17):

1. **Operator fields: the persona itself.** A `PersonaSpec` (this schema) registered on the server by id and version.
   In Release 1 an operator writes it in the server YAML (`front_model.personas`), or budapp publishes it in a Bud
   agent's `front` block; before budapp ships that block, an operator can select a persona for Bud sessions through
   `interaction.front.persona` in an `AliasDefinition` or the server `interaction.defaults.agent` block. From
   Release 2 a tenant can also register one through `PUT /v1/interaction/personas/{id}` (sign-off 17), switched on by
   `front_model.tenant_personas.enabled` and checked by the same compiler, guard and template validator; a tenant may
   keep at most 20 personas with the newest 5 versions of each, and may not sign lines or pick voices the server does
   not allow. What a persona sets: name, organisation, role, channel, language, tone, formality, word caps, backend
   capabilities, extra delegation bullets, forbidden topics, small-talk topics, backchannel intensity and clips,
   interruptibility, local UTC offset, escalation (person handoff and follow-up channel, spoken only by W4
   templates), extra sensitive categories, phrase-bank overrides, template rewording, TTS voice and pronunciations,
   and protected terms. Every line is checked when the persona is compiled; a bad line is refused at registration,
   never at call time.
2. **Caller fields.** A client may set only four fields, `interaction.front.caller.preferred_name`, `verbosity`,
   `pace` and `language`. Formality, pronunciations and accessibility needs come only from server-side sources
   (operator configuration, budapp or memory facts). These go into the per-call header, never into the stable prefix,
   so the cached persona prefix stays byte-identical across calls. A caller ID alone is never used as a name.
3. **Caller identity (Release 2).** `interaction.front.caller.identity_token` carries an operator-signed caller
   identity. It is not one of the four client-settable fields: the client only passes along a token the operator's
   backend minted, in W2's one format, `v1.<payload>.<mac>` (W2 section 3.16). A forged, expired or wrong-tenant
   token is ignored with `config_warning` `caller_identity_rejected`, and the call runs without memory.
4. **Memory facts (Release 2, off by default).** W2's `CallerMemory` port reads at most three typed facts of at most
   80 characters at call start (150 ms budget), keyed by a keyed hash of the verified identity, in the Redis instance
   W3 uses for tasks. Server settings are `front_model.memory.*` (`enabled` false, `identity_key_file`,
   `retention_days` 90). Facts are written after the call by the background or the agent platform, never by the
   front, and are refused if they carry digits, money, dates, codes or names (account facts always come from the
   background). They appear in the call header and in the background preamble's `CALLER CONTEXT (data)` block; they
   are never a template slot value or a trusted entity.

A client can never send persona text, a bank line, a template, a protected term, a memory fact, a model URL or a key.

## How the gateway uses each file, by release

Release names are those of integration decisions section 1; target windows are Addendum D3's: Release 0 2026-10-05 to
2026-11-06, Release 1 2026-11-09 to 2027-01-29, Release 2 2027-02-01 to 2027-03-12, Release 3 2027-03-15 to
2027-04-23; Releases 4 and 5 are scheduled at the Release 3 exit.

| Release | Use |
|---|---|
| Release 0, measure and lay groundwork | `front_act_contract.json` and the contract types come from the act list, wire format and wait reasons used here. The dark front stack (configuration parser, supervisor, client, `ScriptedFront`, persona compiler, grammar builder, parser, guard, rule router, admission) implements what `front_reference.py` specifies. **Done on 2026-10-05:** the bake-off ran these files on the GB10 against the pinned `llama-server` (`FRONT_MODEL_BAKEOFF.md`). **Distillation track** (Addendum B2): the teacher pipeline renders its training data with exactly this prompt and grammar, validates it with this guard, builds the held-out, red-team and re-graded sets seeded with `eval_scenarios.json` and the bake-off's failure list, freezes them at the Release 0 exit, and starts fine-tuning LFM2.5-1.2B, LFM2.5-2.6B and Qwen3.5-2B. **The rule router is gated too** (Addendum D1.2): at the Release 0 exit, on a frozen held-out set, delegate recall at least 0.95 with the lower confidence bound published, tuned only on a separate development split |
| Release 1, cascade interaction on `/ws` | `scripted` on every deployment: the rule router (with Addendum D1.1's addressed-question step) decides the act, the persona's `phrase_banks.en.json` clip groups and lines speak, and W4 speaks every confirmation, choice, value request, status and cancellation line from `templates.en.json`. Acknowledgements are clips. The prompt, grammar and guard ship dark: an operator may force a profile for evaluation. A fine-tuned LFM profile that passes the gates on the frozen sets may ship in a Release 1 point release. Personas from the operator YAML, budapp, an alias or `interaction.defaults.agent` |
| Release 2, full duplex audio, the GA cascade and durable tasks | Fine-tuned profiles not shipped earlier (LFM2.5-1.2B preferred, else LFM2.5-2.6B; Qwen3.5-2B for deployments without the licence attestation), each re-gated on the frozen sets; the tenant persona API; `CallerMemory` (off by default); backchannel clips used by W5's emitter |
| Release 3, speech-to-speech providers, open task protocols and text-output DAGs | The provider instructions block of `front_system_prompt.md` (regimes B and C, only for providers listed in `speech.truth_ack`); the guard's monitor entry point (entity, completion and outcome rules) behind W6's `ClaimMonitor`; persona selection in DAG definitions |
| Release 4, realtime voices on phone and rooms, Hindi and DTMF | Hindi files beside these (`phrase_banks.hi.json`, `templates.hi.json`, `guard_lexicon.hi.json`), authored by native speakers (sign-off 26); the `multilingual` profile, whose Liquid candidate is LFM2.5-VL-3B used text-only (Addendum B7; not yet measured on Hindi); the `wait` reason `incomplete` becomes readable by W5's optional endpoint veto |

## Running it

```sh
python3 front_reference.py --self-test                 # 46 cases
python3 front_reference.py --validate-personas          # jsonschema when importable, else the built-in checker
python3 front_reference.py --render personas/clinic_harbourview.json --all
python3 front_reference.py --grammar T02.d3             # the grammar for one decision
python3 front_reference.py --request T02.d3             # llama-server /v1/chat/completions body (see below)
python3 front_reference.py --score outputs.jsonl        # {"decision": "T02.d3", "output": "<model text>"} per line
python3 front_reference.py --score-router               # the scripted profile's baseline
python3 front_reference.py --score-router --router-wh-questions   # the other reading of the question step
python3 front_reference.py --score-gold                 # every gold output through grammar, rules and guard
python3 front_reference.py --write-grammars             # regenerate the two .gbnf files
CUDA_VISIBLE_DEVICES="" python3 front_reference.py --llamacpp-lib /home/bud/ditto/llamacpp-build/b11200-cuda/llama-b11200/libllama.so
```

`--request` still prints a `/v1/chat/completions` body, the form of the bake-off's "chat endpoint" runs. The form the
gateway ships is different (Addendum B4): the gateway renders the whole prompt itself and sends it to
`POST /completion`, with one assistant-turn opener per model profile (none for LFM2.5, the empty think block for
Qwen3.5, the empty thought channel for Gemma 4), used identically in history and in the generation prompt. On the
chat endpoint `llama-server` inserts `<think></think>` before LFM's prefill, which cost cache reuse on every decision.
The bake-off harness builds the shipped form (`raw_prompt`,
`/home/bud/ditto/waav/research/duplex-agent/bench/bakeoff/bakeoff.py:99-105`); the reference does not.

The last command loads `libllama.so` from the pinned build through `ctypes` and calls its exported grammar functions
(`llama_grammar_init_impl`, `llama_grammar_accept`, `llama_grammar_get_stacks`), so every grammar is parsed and every
gold output is matched by llama.cpp's own code without loading a model. Set `CUDA_VISIBLE_DEVICES=""` on a busy GPU
host.

## Results of the checks

Run on 2026-10-04; the first three re-run on 2026-10-05 after the two fixes the bake-off made to the reference
(a sensitive value named by the agent, and the repeat-request exemption; `FRONT_MODEL_BAKEOFF.md` section 7, items 1
and 2), and again on 2026-10-05 after the final review's router fix (Addendum D1.1; `FRONT_MODEL_BAKEOFF.md`
section 11):

| Check | Result |
|---|---|
| `--self-test` | 46 of 46 passed (after the router fix; 44 of 44 before its two cases were added, 42 of 42 before the bake-off's two regression cases) |
| Gold outputs (`--score-gold`) | 68 of 68 pass: every gold line is legal, producible by its grammar, survives the rule layer and passes the guard (re-run after the router fix) |
| Scripted router baseline (`--score-router`), after the router fix | **acceptable acts 0.90 (61 of 68; Wilson 95 percent interval 0.80 to 0.95), exact 0.74; delegate recall 1.00 (26 of 26; interval 0.87 to 1.00), precision 0.82 (28 of 34)**; status questions sent as new tasks 0. Before the fix: 0.87 (59 of 68), exact 0.71, recall 0.92 (24 of 26), precision 0.81 (26 of 32). The step changed exactly two decisions, T11.d4 and T16.d3, and broke none. Same author-labelled seed set, so still optimistic. Unchanged gaps: it refuses only 1 of 5 refusals, follows 3 of 5 injections (W2's `injection_cue` step, Addendum B5 item 4, is not in this reference yet), and delegates on a request-shaped answer to a pending confirmation (T11.d3) |
| Persona validation | all three personas valid under jsonschema 4.10.3 (draft 2020-12) and the built-in checker, and all three compile (every bank line passes the compile-time guard) |
| llama.cpp build 11200 grammar parser (`--llamacpp-lib`) | 68 of 68 decision grammars parse; 68 of 68 gold outputs accepted; 18 of 18 outputs labelled `grammar_reject` rejected; both packaged `.gbnf` files parse; 5 to 115 rules per grammar |
| Python recogniser against llama.cpp | the same accept or reject verdict on 2,201 of 2,201 strings (gold, bad outputs and mutations; 200 accepted by both) |
| Token counts, measured with the LFM2.5-1.2B tokenizer (`llama-tokenize` from build 11200 on the local Q4_K_M file, CPU) | stable prefix 1,637 (retail), 1,685 (travel) and 1,722 (clinic) tokens; call header 65 tokens for a typical caller; observations over the 68 decisions median 64, mean 70, p95 131, maximum 210 tokens |

Model outputs were scored in the bake-off, not here (`FRONT_MODEL_BAKEOFF.md`, sections 3 and 10).

## Findings for the designs

Written before the bake-off; the status after the bake-off and Addenda B and C is given for each.

1. **W2's example grammar would not load in the pinned llama-server.** Build 11200's parser accepts only letters,
   digits and `-` in rule names, and a continuation line may not start with `|` (both confirmed through the library:
   "expecting newline or end" and "expecting name"). W2 section 3.2.5 uses `prog_t1` and leading-`|` lines. The
   packaged grammars use hyphenated names and one line per rule; the reference's parser enforces the same rules.
   *Status:* the grammar builder must emit this form; the bake-off ran these grammars without a parse failure.
2. **The stable prefix is larger than W2 estimated.** Measured 1,637 to 1,722 tokens against W2's estimate of about
   950 (a first draft measured 2,069 to 2,145 before it was tightened). *Status, measured in the bake-off:* priming the
   prefix and call header took 0.23 to 0.34 s on the shared GPU and 6.4 to 6.7 s on 6 ARM CPU threads for LFM, so the
   first decisions of a call are rule-routed unless cross-call prefix reuse works, which stays a Release 0
   measurement (Addendum B5 item 7). The 9 examples (496 tokens) are the main lever.
3. **`request_not_wait` would undo two correct `wait` acts** (W2 section 3.7.1): an answer to a pending question
   ("Yes please, that's perfect") and an unfinished turn that contains a request cue ("okay so what I wanted to ask
   was"). The reference exempts turns with a pending question and turns that end on a dangling word. *Status:* adopted
   in W2 section 3.7.1, together with the repeat-request exemption the bake-off added (Addendum B5 item 5).
4. **A restatement could read a sensitive value back.** The compose grammar copies the caller's words, so "My date of
   birth is May fourth, nineteen eighty" could become "Noted, May fourth, nineteen eighty". The reference suppresses
   copy spans when a sensitive category (W4's list plus the persona's `sensitive.extra_categories`) is named.
   *Status:* adopted in W2 section 3.2.5 and extended after the bake-off to a category named by the agent's own lines
   or the pending question, not only by the caller (Addendum B5 item 2).
5. **W4 states the answer-phrase check two ways.** Section 3.13.1 limits it to question templates; section 3.13.4
   rule 2 says every template and bank line, which W4's own acknowledgement clips ("Sure, let me check.") would fail.
   The reference applies the question-template reading. *Status:* open for W4.
6. **A status question about a task that is not running has no fitting front line.** The act is `social` and W4
   speaks its status template, so the rule layer always discards whatever social line the model picked. It works, but
   a model call is spent on words that are never used. *Status:* open; it costs nothing under `scripted`.
7. **A just-asked question is rendered twice** (as the `SAID` line and as `EVENT: question pending`), which produces
   the 210-token observation. Rendering `EVENT: question pending (just asked)` when the question was the last `SAID`
   line would halve it. *Status:* open; shorter observations are one of the CPU levers W2 lists for Release 0.
8. **Rule-router gap.** The scripted router has no step for a pending question, so it delegates a request-shaped
   answer. W4's answer parser runs first, so this matters only when the parser defers to its stage 2. *Status:* open;
   since the router is the Release 1 front, W4's parser running first is what covers it.
9. **The router met some complete questions with silence** (final review). W2's last router step sent any turn with
   no cue that was not request-shaped to `wait` (`incomplete`), so "Will I get a reminder text?" (T11.d4) and "Is it
   going to be here by Friday?" (T16.d3), both gold `delegate`, got no answer. *Status:* fixed in the reference
   (Addendum D1.1): a step between W2's step 10 (`request_shaped`) and step 11 (the final `wait`) routes an addressed
   question at E to `delegate` (`front_reference.py:1220-1235` builds the flags with the helpers at `:1861-1910`, and `:1936-1940` is the step). It needs
   no side-talk cue, no pending question and no answer window, the polar-question check's flag, and a question's
   shape: ends in "?", or starts with a wh-word or a yes/no starter. Status, social and repeat turns and fragments
   ending on a dangling word keep their own steps. "Okay, yes, do that." is flagged by the polar check but never
   becomes a task. Regression cases are in `--self-test`, and `eval_scenarios.json` marks both decisions with a
   `note`; labels are unchanged. Chapter 2 ("The rule router") lists the step; W2 section 3.7.2 in the research
   design still ends at step 11. *Still open:* (a) as worded, a wh-question the polar check misses ("Where's my
   refund?", whose contraction misses the bare wh-word request cues) still gets `wait`; chapter 2 raises this for
   decision, and `--router-wh-questions` scores the other reading, which gives the same numbers on the seed set;
   (b) the model path's rule layer (`request_not_wait`) does not override a model's `wait` on such a question (W2).

## Choices this asset makes where the designs were silent

- The elapsed-time bucket "a minute or two" ends at 180 s.
- Copy spans may not start on discourse words, conjunctions, question words or auxiliaries (`start_word_stop_list`);
  they may pass through them. "can you / could you ... ?" is a request, but "check if" and "know if" make a turn a
  yes/no question, so polar openers ("Sure", "Of course") are left out.
- `PersonaSpec` gains `answer_words` (the preamble's word budget), `forbidden_topics`, `escalation` and `sensitive`,
  which the designs reference but W2's struct does not list.
- Two status templates are added for states W2 keeps out of the front's `progress` act: waiting on the caller and
  result ready.
- There is no language word list (owner, 24 October 2026): the gateway reads the caller's language with the front model
  (a short reading beside each decision) or the recogniser's own language field; the reference has no language check.
- The router's question step (Addendum D1.1) fires only at a confirmed end of turn, never at a caller pause. "No
  answer window" is read as W4's `answer_expected()`: a pending question, or an agent line ending in "?" whose playback
  ended at most 4 s (`answer_window_ms`) before. A "status turn" is a status cue on a call that has a task to report
  on, so "Is the store still open?" on a call with no task is still a question. Without a "?", a yes/no starter must
  be followed by at least two more words, so "Do that." and "So is it" stay fragments.
- Sampling is the Liquid card's `temperature 0.1, top_k 50, repetition_penalty 1.05` (fact-checked), with `seed 42`;
  `max_tokens` is 12 + ceil(1.4 x `max_words`), 33 at 15 words. The bake-off ran with exactly these values.
