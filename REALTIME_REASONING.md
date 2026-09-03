# WaaV Realtime Reasoning — making slow-thinking LLMs feel instant in voice

**Goal.** Let WaaV customers build voice agents that stay *natural and realtime* even when the
brain behind them — a reasoning model, a long RAG lookup, or an agentic tool loop — takes seconds
(or tens of seconds) to think. Ship the validated techniques as **default features** with the
**simplest, most intuitive developer experience**.

Grounded in: the three sources the goal named, an intensive adversarially-fact-checked web research
sweep (43 research/verify agents), a fresh on-hardware micro-benchmark, a full read of WaaV's
conversation/LLM machinery, **and a 4-critic adversarial review of this design itself** (its findings
are folded in below and logged in §11).

> Artifacts in `research/realtime-reasoning/`: `bench_ttft.py`+`bench_results.txt` (benchmark),
> `research_synthesis.json` (54 techniques + 34 fact-checks), the two cloned repos.

---

## 0. TL;DR

A reasoning LLM is **30–100× slower to first useful audio** than a fast model on the *same* GPU
(measured: 9–18 s vs 0.17 s). 9–18 s of silence is a dropped call. The fix is **not** "use a
reasoning model and hope," and **not** "say 'um' to fill the gap." It is a small, ordered stack:

**Ship-first MVP (the 80% case, mostly already built):**

| # | Feature | What it does | Status |
|---|---|---|---|
| **D1** | **Fast LLM + `reasoning_effort` dial** | Keep a *fast, non-reasoning* model on the spoken path; one typed knob maps to each vendor's thinking control, clamped to the model's floor. | reasoning-empty guard **done** (`mod.rs:540`); dial = net-new |
| **D2** | **Sentence aggregation** (C-G1+2) | First audio lands on TTFT+TTS-TTFB, not full generation. | **already landed** (`mod.rs:392`) — keep as default |
| **D3** | **`latency_filler`** (unified mask) | One concept: an *action* preamble the instant a slow op dispatches, **and** a pre-rendered wait-time clip if still silent — deduped to one utterance/turn, codec- & language-correct. | net-new (reuses A-G6 queue) |

**Prerequisites before any reasoner ships (table-stakes, not features):**

| **P1** | **Degradation ladder** | fast-LLM down / reasoner over budget / pre-render fail → defined safe fallback. |
| **P2** | **Cost budget** | per-turn max LLM calls + reasoning-token ceiling + eager-refire cap. |

**Opt-in, in order of value (defer until MVP+prereqs land):**

| **S2** | **Per-turn escalation** | Heuristic (length/keyword/low-confidence) picks fast-inline vs escalate-to-reasoner. Start heuristic, not a bespoke classifier. |
| **D5** | **Speculative (eager) start** | Reuse `eager_eot`; **fast-model-only**, behind the cost budget. |
| **S1** | **Two-tier fast-draft + slow-final** | Fast model speaks a *non-committal* opener; the reasoner owns committed facts and splices behind it. Highest value *and* highest risk — last. |
| **S3** | **Async tools** | ~80% already built (`functions.rs`); just expose the existing `is_final`/cancel surface + add `run_llm`. |

**The one config a developer needs:** name a fast `model`; if you also want a smart-but-slow brain,
add `reasoning_model: "o3"` — everything else has good defaults. That's it. (§7.)

**Three invariants the critique proved are load-bearing:**
1. **Fire the slow op in parallel with the filler — never defer it** (Ultravox trap: 88 % filler
   rate → 8.40 s completion). Not a config knob; hard-wired behavior.
2. **The fast tier must never assert committed facts** (LiveAnswer: −15.7 pp on math) — non-committal
   openers only; the reasoner owns the answer.
3. **Barge-in during a filler must still cancel the slow LLM** — today's `handle_barge_in`
   early-returns inside a protected window and cancels *nothing*; that must be fixed *before*
   masking ships, or masking defeats barge-in on the exact turns it targets (§4.4, critique C3).

---

## 1. The problem, quantified

### 1.1 Fresh micro-benchmark (GB10, ollama 0.24, 2026-06-13)

`answer_start` = wall-clock to the first **user-visible answer token** (for a reasoning model,
*after* its hidden thinking). This is the dead-air gap a filler/two-tier must mask.

| Model | "are you open today?" | multi-step refund-math question |
|---|---|---|
| **llama3.2:1b** (fast) | first token ~**0.17 s** | first token **0.17 s** (answers directly — a 1.5B-class model is *unreliable* on the math) |
| **deepseek-r1:1.5b** (reasoning) | **8.9 s** of thinking | **18.0 s** before the first spoken word |

Two conclusions fall straight out: **never route every turn to a reasoning model** (even *"hi"* cost
~9 s), and **while the slow model thinks, something else must hold the line.**

### 1.2 Cross-checks (fact-checked, June 2026)

- arXiv **2603.05413** (Salesforce, the named paper): cascade Deepgram+vLLM+ElevenLabs, **755 ms**
  TTFA, LLM TTFT **P50 337 ms → 4,327 ms** — and *no* thinking-time handling (WaaV's net-new).
- Reasoning ON explodes TTFT to **8–200 s** (Claude Sonnet 4.6 reasoning ~135 s, GPT-5.4 high ~166 s;
  fast tier Grok 4.1 Fast 0.59 s, Haiku 4.5 0.70 s). Categorically unusable for a <500 ms gap.
- WaaV's own data: AUDIT_REPORT `llm_ttft=5905 ms` of a 7478 ms turn (**79 %**) on a reasoning model;
  **empty spoken content on 4/5 runs** (reasoning ate `max_tokens`). Lose-lose.
- **Budget (the measuring stick):** Stivers 2009 (PNAS, 10 langs) modal turn gap **~200 ms**; Cresta
  pauses ~300 ms "feel unnatural," >1.5 s "rapidly degrades." Target perceived gap ~200–500 ms;
  **>~2 s dead air needs masking; never approach 4 s.** Full-Duplex-Bench-v3: the **cascade is the
  slowest** architecture unmasked (first-word 8.78–10.12 s) → needs masking *most*.

---

## 2. What the three named sources teach

- **`openai/openai-realtime-agents` (chat-supervisor):** a fast realtime model converses; a single
  tool defers hard turns to a slow reasoner; **a filler is mandatory before every escalation**; an
  allow-list draws the boundary. *Take the convention, not the config* — fact-check: `gpt-4o-realtime-mini`
  is a non-existent README label, the real default is a deprecation-flagged preview snapshot, the
  supervisor endpoint is the demo's own proxy, and the "~2 s gap" is an unmeasured anecdote. Prefer a
  **deterministic escalation rule** over fast-model-tool-call routing.
- **`neural-maze/realtime-phone-agents-course`:** the masking trick the OpenAI repo lacks — **speak an
  acknowledgment then play a typing sound** while a tool runs; everything streams; one-line answers.
  (No reasoning-model handling — its gap.)
- **arXiv 2603.05413:** validates WaaV's cascade and quantifies the LLM stage as the dominant,
  high-variance cost — but explicitly has no thinking-time handling, which is the whole of this report.

(Full technique catalog + fact-checks: §3 and `research_synthesis.json`.)

---

## 3. Verified technique catalog (condensed)

| Technique | Impact/effort | Verified anchor | Key risk (fact-checked) |
|---|---|---|---|
| Fast LLM default + `reasoning_effort` + cap | very-high | fast 0.59–1.35 s vs reasoning 8–200 s TTFT | adaptive-only models (Opus 4.7/4.8, Gemini Fable) can't hit 0 — surface the floor |
| Sentence aggregation (C-G1+2) | very-high | streaming overlap saves ~276 ms (WaaV) | commit history only after the *full* utterance (Pipecat #4111) |
| `reasoning_effort` dial | very-high | OpenAI `none…xhigh`; Gemini `thinkingBudget 0–24576`; Anthropic `thinking{…}` | Gemini conflicting params = 400; map exactly one per vendor |
| Action preamble (parallel-fire) | high | GPT-Realtime-2 TTFA 1.12 s min / 2.33 s high | **defer-the-op trap** (Ultravox 88 %→8.40 s) |
| Pre-rendered gap-filler | high | Agora `response_wait_ms` 1500 default | uninterruptible clip can't be cut → <1 s; don't fire <1.5 s real gap (Maslych 2025) |
| Speculative/eager start | high | Deepgram Flux 150–250 ms early, **+50–70 % calls** | wrong for slow reasoning; idempotent side-effects (LiveKit #3414/#4219) |
| Two-tier fast-draft + slow-final | high value | fast SLM 329 ms vs slow ~900 ms | **draft/final contradiction** (LiveAnswer −15.7 pp) |
| Heuristic/router escalation | high | classifier overhead single-digit ms | misroute; needs fallback escalation |
| Async tools | medium | perceived ~0 during tool duration | early-commit trap (#4111) |
| Talker–Reasoner / ConvFill | R&D | ConvFill TTFT ~0.16 s; +36–42 pp | **5–7 % contradiction rate** → NLI guard; heaviest lift |
| Backchannels ("mm-hmm") | low | produced ~59–100 ms early | overlap trips half-duplex barge-in → gap-after-EoT only |

**Avoid (each cost someone real latency/accuracy):** keeping a reasoner on the voice path (empty
4/5); bare disfluencies "um/uh" (OpenAI bans; Jeong 2019 less-intelligent); deferring the op behind a
filler (Ultravox); fast-draft asserting facts (LiveAnswer); fillers at <1.5 s or every turn (Maslych);
overlapping backchannels (half-duplex VAD); the chat-supervisor literal config; eager on a reasoner
(+50–70 % calls × think-time); unverified speedups (VoiceAgentRAG, Step-Audio "zero-latency").

---

## 4. The default features (correct, flat, DX-first)

**Design law** (from WaaV's own idiom — `ConversationWebSocketConfig` is 16 *flat* `#[serde(default)]`
fields, the only nested exception is `turn_detection`): every feature is an **additive flat field**
with a great default, mapped 1:1 through `to_conversation_config()`. The common case needs **zero**
new config. Typed enums (like the existing `provider_kind: AdapterKind`) catch typos at deserialize.

### 4.1 D1 — Fast LLM + typed `reasoning_effort` dial

```jsonc
"model": "gpt-4o-mini",          // a FAST, non-reasoning model
"reasoning_effort": "minimal"    // typed enum {off|minimal|low|medium|high}, vendor-mapped
// (do NOT set max_tokens in examples — WaaV's shipped VOICE_DEFAULT_MAX_TOKENS=256 (mod.rs:139)
//  is already voice+multilingual-tuned; D2 makes first-audio independent of it anyway)
```

- **Typed enum**, `off` (honest) not `disabled`. WaaV owns the per-vendor mapping (OpenAI
  `reasoning.effort`; Gemini `thinking_level`/`thinkingBudget=0`; Anthropic `thinking.type`) and
  sends **exactly one** param per vendor (never Gemini's 400-causing pair).
- **Clamp to the model's floor and echo it back** in the session-ack:
  `{reasoning_effort_applied:"low", reasoning_effort_floor:"low", note:"model 'opus-4.8' cannot disable reasoning"}`.
  So "this model's floor is X" is *observable*, never a silent surprise.
- **Seam:** new field on `LlmClientConfig` (`core/llm/mod.rs:432`) + per-vendor build in
  `core/llm/adapter.rs` (net-new — no reasoning param exists today). Reasoning-empty guard already
  ships (`mod.rs:540`).
- **Path note:** this same dial maps onto the **S2S path** via `session.update` (§6) — the one default
  that spans both of WaaV's voice paths.

### 4.2 D2 — Sentence aggregation *(already C-G1+2; keep as default)*

`streaming:true` already pumps tokens through `SentenceAggregator` (`mod.rs:392`). Keep it; recommend
a streaming TTS when streaming (batch TTS otherwise pays one synthesis per flush). No new knob.

### 4.3 D3 — `latency_filler` (ONE unified masking concept)

The critique collapsed the former two features (action-preamble + gap-filler) into one — they shared a
timer, a queue, a cancel condition, and double-fired on 100 % of slow turns. One knob:

```jsonc
"latency_filler": "auto"
//  "off"        → never speak filler
//  "auto"       → (default) the instant a slow op DISPATCHES, speak ONE short ACTION phrase
//                 ("Let me check that order"); if still silent at the wait threshold, instead a
//                 generic pre-rendered clip. Deduped: at most ONE masking utterance per turn.
//  "aggressive" → lower thresholds for known-slow (RAG/agentic) routes.
// Optional flat overrides (power users): latency_filler_after_ms, latency_filler_phrases[].
```

**Behavior + correctness (the masking state machine — all critique fixes folded in):**

- **One latch per turn.** A single timer; once any masking utterance is enqueued, the latch is set and
  no second filler fires (fixes the 350 ms-preamble-then-800 ms-gap-filler double-fire, critique H1).
- **Arm only on a *confirmed* end-of-turn**, never on the speculative/eager path — else a filler
  fires for a turn that gets superseded, talking over a still-speaking user (critique H3/M4).
- **Fire the op in PARALLEL** — non-negotiable, not a knob (the `fire_op_in_parallel:true` "invariant"
  is removed from config; deferring is the Ultravox trap).
- **Interruptible, with a tiny suppression window.** The clip is *interruptible* (dropped by the next
  `clear_interruptible`), but barge-in detection is suppressed for its ≤~400 ms duration via the
  existing `is_interruption_blocked()` window — shorter than human-reaction+STT-partial latency, so a
  real barge-in is almost never swallowed. (Resolves the D3-says-both contradiction, critique C1/C2.)
  This requires a **new VoiceManager seam** `enqueue_prerendered_clip(audio, interruptible)` that
  pushes one `PlaybackUnit` directly onto the A-G6 pump queue — today `interruptible` is a
  *session-global* flag read at egress, not per-clip, and A-G6 is off by default; masking must
  **force the A-G6 pump on** and tag per-clip.
- **Don't double-ack.** When a filler fired, **prepend a note to the LLM turn** ("you already said
  'one moment' — answer directly, no preamble") so the model's own opener doesn't repeat it (critique
  H2). Bias default phrases to task-action wording, never "um/uh".
- **Pre-render at the call's codec + rate + language.** Synthesize the phrase pool at session start at
  the negotiated `client_playback_rate`/codec (the egress `resampler.rs` already does provider→client
  rate) — a 24 kHz PCM clip on an 8 kHz μ-law phone call is garbage (critique M1). Phrases are a
  per-language pool (WaaV runs EN→Hindi live); default English, derived from the session voice/language
  (critique M2). If pre-render fails, fall back to live-TTS filler — never disable masking silently.
- **Recovery, not silence.** If the reasoning-empty guard trips *after* a filler played, speak a
  recovery line ("Sorry — could you say that again?") so the filler's promise isn't broken by dead air
  (critique M3).
- **Don't self-trigger VAD.** Masking audio plays inside the bot-speaking window (`is_audibly_speaking`)
  so the filler tail isn't heard as a new user turn (critique M5).
- **Sensible defaults:** `auto`; action phrasing; `after_ms` ~800 (600–1000 fast; up to 1500–3000 for
  known data-lookup routes — under the ~2 s line); ≥4 phrases; clip <1 s; ≥3 s between any repeats.

### 4.4 D-prereq — fix `handle_barge_in` before masking ships *(critique C3, CRITICAL)*

`handle_barge_in` opens with `if is_interruption_blocked() { return; }`. So a user barging in *during*
a protected filler cancels **nothing** — not the 10 s LLM, not the eager turn, not TTS — for the
clip's duration, exactly when users most want to interrupt. **Required change:** move
`cancel_current_turn()` + eager-cancel **above** the protected-window guard; the guard then suppresses
only the *TTS clear of protected audio* (let the short clip finish) while the slow LLM is cancelled
immediately and a deferred clear is armed for when the clip ends. This is a prerequisite for D3.

### 4.5 D5 — Speculative (eager) start *(opt-in, deferred)*

Reuse `eager_eot`/`trigger_eager_turn` (`mod.rs:656`, `complete_staged` + A-G4 supersede). **Enforce
fast-model-only at config validation** (reject eager+reasoner — each speculative fire pays full
think-time, cancel-on-resume wastes it). Behind the P2 cost budget. Masking does **not** arm on the
eager path (§4.3); if an eager-confirmed turn is itself slow, arm masking around the confirm-await in
`run_finalized_turn` (critique M4).

---

## 5. Opt-in structural patterns (for a reasoning brain behind a realtime voice)

Off by default; opt in per route. **One field turns the whole thing on:**

```jsonc
"model": "gpt-4o-mini",     // FAST tier — speaks the non-committal opener, handles easy turns
"reasoning_model": "o3"     // presence ⇒ WaaV auto-wires:
//   • escalation (fast=model, strong=reasoning_model, default heuristic/threshold)   (S2)
//   • a non-committal fast opener (add_to_history=false, ack-only style — the only safe value)  (S1)
//   • latency_filler stays "auto"; eager binds to the FAST tier only
// Optional flat override: reasoning_route_threshold. No weak_model/strong_model/fast_llm triplet.
```

### S2 — Escalation (heuristic first, classifier later)
Default-route to the fast model; escalate on a cheap heuristic (length / keyword / explicit
`tool_choice` / low fast-tier confidence or refusal). A *dedicated trained classifier* is premature
for v1 (a new model to host/version/bill) — add it only if the heuristic misroutes measurably. Always
keep a fallback escalation path. Run on the partial transcript, off the critical path.

### S1 — Two-tier fast-draft + slow-final *(highest value, highest risk → last)*
The fast model speaks a **non-committal opener** (~170–330 ms); the **reasoner owns every committed
fact** and splices behind it. Required, from the critique:
- **Ordering barrier, not luck:** the final's first `speak` awaits an "opener enqueued" signal (a
  per-turn oneshot), with a **fallback**: if the opener LLM errors or misses a small deadline, the
  final proceeds without waiting (a dead fast tier must not stall the answer) — fixes the parallel-DAG
  race where the committed answer could play before the opener (critique H4).
- **One shared cancel token across both tiers** so barge-in cancels *both* in-flight LLM calls; the
  opener (`add_to_history=false`) never commits; the reasoner commits only after it actually spoke
  (critique H5). On barge-in of the opener with the reasoner in flight, *keep* the reasoner result as a
  candidate for the next turn rather than hard-discard a correct, expensive answer (policy knob, M6).
- **Non-committal is non-negotiable** (LiveAnswer −15.7 pp; ConvFill 5–7 % contradiction). The fast
  tier scaffolds; it never answers.
- **DAG reconciliation:** a reasoner needs >30 s sometimes, but the DAG node timeout is `30 s`
  (`DAGTimeoutsConfig`); the S1 reasoner node must override to the `120 s` LLM budget or it's dead on
  arrival (critique M4).

### S3 — Async tools *(≈80 % already built — expose, don't rebuild)*
`functions.rs` already has `cancel_on_interruption`, the built-in `cancel_async_tool_call` tool, and
the `is_final` progress/final sink with follow-up inference (Pipecat parity). Net-new is only: expose
these in config, add `run_llm` (chain tools without inference), gate follow-up injection on **turn-id
equality** (a stale RAG result must not talk over a new topic, M6), and commit history only after the
full utterance (#4111).

---

## 6. Path applicability — cascade vs S2S *(critique M3)*

WaaV has **two** voice paths. Everything above is **cascade** (`conversation/mod.rs`). The **S2S /
realtime path** (`handlers/realtime/`, `RealtimeFactory`, OpenAI/Hume clients, planned Gemini Live /
Nova Sonic) is separate and the masking lives *in the model*:

- **D1 `reasoning_effort` maps onto S2S** via `session.update` (e.g. gpt-realtime-2's `reasoning.effort`,
  "start at `low` for production voice") — the one default that spans both paths.
- **D3/D5/S1/S2 are cascade-only**; on S2S, masking is model-native (effort dial + an instruction-level
  preamble in `instructions`). The report says so explicitly so an S2S customer isn't silently
  unserved.

---

## 7. The developer experience (the user's explicit requirement)

The whole mental model is two sentences: **"Name a fast model. If you want a smart-but-slow brain, add
`reasoning_model` too — everything else has good defaults."** Flat, plan-tagged, copy-pasteable next to
the existing 16 fields:

```jsonc
{
  "conversation": {
    // ── 99% case: zero new config beyond a fast model ──
    "model": "gpt-4o-mini",          // fast → D1/D2/D3 active with good defaults

    // ── optional flat tuning (matches eager_eot / barge_in_min_words / strip_markdown) ──
    "reasoning_effort": "minimal",   // typed enum; clamps to floor, echoes applied+floor back   (D1)
    "latency_filler": "auto",        // ONE masking concept, deduped, codec+language correct      (D3)
    "eager_eot": true,               // existing field; fast-tier-only, budget-capped             (D5)

    // ── opt-in two-tier: ONE field lights up escalation + fast opener + filler ──
    "reasoning_model": "o3"          // presence ⇒ S1+S2 auto-wired off model + reasoning_model   (S1/S2)
    // (async tools stay on the tool definition, where WaaV already keeps them — S3)
  }
}
```

The minimum "good realtime reasoning agent" is **`model` + `reasoning_model` — two fields.** Vs the
first draft's ~22 leaf fields across 5 nested blocks. DX principles:
1. **Zero-config common case** (pick a fast model → it works).
2. **One knob per concept** (`reasoning_effort`, `latency_filler`, `reasoning_model`); the
   never-`false` invariant (parallel-fire) is *not* a knob.
3. **Progressive disclosure**, all flat, all `#[serde(default)]`, nothing breaks existing configs.
4. **Active, in-band guidance** (not just Grafana): on session-ack, if a *reasoning* model sits on the
   spoken `model` with no two-tier, return a structured `config_warning`
   (`code:"reasoning_model_on_voice_path"`, measured TTFT, the one-line fix) — reusing the existing
   `mute_strategy`/`turn_detection` warn+`waav_degraded_total` idiom; and a runtime nudge if
   `llm_ttft` p50 > 2 s after turn 1. The §1/§3 anti-patterns become errors that suggest their own fix.
5. **Observability is DX:** default SLO alerts on the existing `waav_turn_*` + `/debug/profile`
   (first-audio P95 < 300 ms, full-turn P95 < 800 ms, barge-in-stop < 60 ms, `llm_ttft` p50 < 250 ms),
   plus spend (P2) so a developer *sees* both latency and cost.

---

## 8. Production prerequisites (table-stakes — "default" must fail safe)

- **P1 Degradation ladder:** fast-LLM down → forced-escalate or canned apology; reasoner over its
  budget → commit the fast draft / "let me get back to you"; pre-render fail → live-TTS filler (never
  silent-disable, never crash the session); S1 reasoner node → override the 30 s DAG node timeout.
- **P2 Cost budget:** a `budget` surface (per-turn max LLM calls, reasoning-token ceiling, eager-refire
  cap) with safe defaults, surfaced on `/debug/profile`. Eager (+50–70 %), two-tier (2×), and escalation
  all multiply spend on a billing gateway — bound it.

---

## 9. Implementation sequencing

- **Wave A — MVP (biggest win, lowest risk, mostly built):** D1 dial + fast default; keep D2; the §4.4
  barge-in-cancel fix; D3 unified `latency_filler` (codec+language correct, dedup latch, parallel-fire,
  recovery line). Live gate: `llm_ttft` p50 < 250 ms; no double-fire; no double-ack; barge-in-during-
  filler cancels the LLM. Ship SLO alerts.
- **Wave B — prerequisites:** P1 degradation ladder + P2 cost budget. (Required before any reasoner.)
- **Wave C — opt-in reasoner:** S2 heuristic escalation; D5 eager (fast-only, budget-capped); then S1
  two-tier (ordering barrier + shared cancel token + DAG-timeout override) last; expose S3.
- **S2S parity:** map D1 `reasoning_effort` onto `session.update`.
- **R&D (one line):** ConvFill-style local conversational-infill on `waav-infer`, gated on an NLI
  contradiction guard — out of scope here.

Methodology: each item through WaaV's loop — RED→GREEN→full lib floor→brutal-review workflow→fix→
**live multi-provider e2e gate**→commit — with the §10 SLOs as acceptance criteria.

---

## 10. Acceptance criteria (measurable, voice-realistic)

- **G-fast:** a fast-model turn has `llm_ttft` p50 < 250 ms and never emits empty content.
- **G-mask:** when first audio exceeds the wait threshold, the caller hears **one** masking utterance
  (never two), codec- & language-correct, within ~800 ms — **no dead air ≥ 2 s, never ≥ 4 s**; and if
  content turns out empty, a recovery line plays (never silence after a filler).
- **G-parallel:** the slow op provably runs in parallel with the filler (filler adds 0 to task time) —
  asserted by a timing test (anti-Ultravox).
- **G-barge-in:** a barge-in **during** a filler cancels the in-flight LLM/eager within < 60 ms (the
  §4.4 fix), and no filler/opener/draft talks over a barging user beyond its <1 s clip.
- **G-two-tier:** with S1, first audio < ~400 ms while an 8–18 s reasoner produces the committed answer
  behind it, in correct order (opener before final); barge-in cancels both; the draft never enters
  history nor asserts a fact.
- **G-budget/degrade:** no path silently 3–4×'s spend (P2 caps hold); every failure mode (LLM down,
  reasoner timeout, pre-render fail) degrades to a defined safe state (P1).

---

## 11. Adversarial critique log (what the 4-critic review changed)

| Finding | Severity | Change folded in |
|---|---|---|
| `handle_barge_in` early-returns in a protected window → barge-in during a filler cancels nothing | CRITICAL | §4.4 prerequisite: cancel LLM/eager *above* the guard |
| `interruptible` is session-global (egress-time), not per-clip; A-G6 off by default | CRITICAL | §4.3 new `enqueue_prerendered_clip(audio, interruptible)`; force A-G6 on with masking |
| preamble + gap-filler double-fire on 100 % of slow turns | HIGH | §4.3 unified `latency_filler` with a one-utterance/turn latch |
| filler fires for superseded/eager turns; eager-confirm bypasses masking | HIGH | §4.3 arm only on confirmed EoT; §4.5 arm around eager-confirm await |
| preamble vs the LLM's own opener → double-ack | HIGH | §4.3 prepend a "you already acknowledged" note to the turn |
| S1 parallel nodes race (final before opener); two un-cancelled LLM calls | HIGH | §5 ordering barrier + fallback; one shared cancel token |
| Nested config blocks break WaaV's flat idiom; `max_tokens:120` < shipped 256; `reasoning_effort` should be typed + floor-clamped; two-tier should be one field | HIGH (DX) | §4/§7 fully flat; drop `max_tokens` from examples; typed enum + floor echo; `reasoning_model` one-liner; active `config_warning` |
| Fillers ignore telephony codec/rate and language; S2S path unaddressed; no degradation ladder or cost budget; DAG 30 s kills reasoners | HIGH | §4.3 codec+language pre-render; §6 path applicability; §8 P1/P2; §5 DAG-timeout override |
| S1 over-billed as "headline"; S2 classifier premature; S3 ~80 % already built; D3/D4 duplicative | MEDIUM | §0/§5/§9 MVP=D1+D2+D3, demote S1, heuristic S2, expose S3, unify D3+D4 |
| min spacing between fires; filler-then-empty silence; filler self-trips VAD; D3 should pre-render | MEDIUM/LOW | §4.3 ≥3 s spacing, recovery line, bot-speaking gate, pre-render |
| Round-0-preamble line range imprecise; `reasoning_effort` correctly net-new | fact-check | references corrected (`mod.rs:500-548`; dial = net-new) |

---

*Sources: the three named repos/paper (cloned/fetched), the fact-checked research corpus
(`research/realtime-reasoning/research_synthesis.json` — 54 techniques, 34 verifications, primary URLs
inline), the on-hardware benchmark (`bench_ttft.py`/`bench_results.txt`, GB10, 2026-06-13), and a
4-critic adversarial review of this design (§11), all grounded in the WaaV source.*
