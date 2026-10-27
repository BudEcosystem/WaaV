# WaaV Realtime-Reasoning — Follow-up Plan (deferred items + residual failure cases)

*Produced by a 4-phase adversarial workflow (assess → rank → design → 3-critic critique, 18 agents). Every claim below was verified against the live code; one assessor ran probe tests. This is a PLAN — nothing here is implemented yet.*

---

## 0. TL;DR

Ten deferred items / residual failure cases were value-assessed for a **production cascade voice gateway**. Four are worth building; the rest defer or are skipped (two were proven non-issues against the real code).

| Build order | Item | What it fixes | Effort | Why it's worth it |
|---|---|---|---|---|
| **1** | **A5 — heuristic hardening** | false-escalation onto the 2× reasoning tier (`"not interested"`, `"no refund"`, `"analysis paralysis"` all escalate today) + dropped multi-turn reasoning threads + non-English never escalates | **M** | only **recurring, invisible money+latency leak**; makes the two-tier opt-in *safe to turn on* |
| **2** | **A7 — stall watchdog** | a reasoner that emits one token then freezes 40 s → **mid-call dead air**, the one silence case D3 masking provably does *not* cover | **M** | **highest user-audible correctness lever left** |
| **3** | **A10a — cross-vendor wire fix** | a cross-vendor reasoning tier inherits the wrong wire format → POSTs OpenAI body to `api.anthropic.com` → **400 every escalated turn** | **S** | hard correctness bug; do before F4 pushes more traffic through the tier plumbing |
| **4** | **F4 — minimal honesty** | the S3 async-tool sink is **dead on every production path** (`wire_async_sink` always no-ops) — a reviewer trap | **S** | cheap honesty now; the **full** webhook-tools surface is a scoped opt-in, deferred |

**Deferred** (real but low-value or already-mostly-covered): D3-perf clips, A4 (observability gap, not a bug), A2 (cosmetic), A8 (~40 tokens). **Skipped** (proven non-issues): A6 (`add_pairing_safe` already closes it — probe-verified zero orphans), S1 opener-splice (XL, re-introduces the 2× cost the shipped design deliberately avoided; D3 masking already solves the silence).

The three picks A5/A7/A10a are all **two-tier-gated** (inert for the common single-tier deployment), so they ship together as "harden the two-tier router before we encourage operators to enable it." F4 is enablement, not a fix.

---

## 1. Full triage (all 10)

| Item | Value | Effort | Reach | Verdict | Tier |
|---|---|---|---|---|---|
| A5 heuristic quality | 5 | M* | common-when-two-tier | worth-it | **do-now** |
| A7 stall watchdog | 5 | M | the only live dead-air gap left | worth-it | **do-now** |
| A10a cross-vendor 400 | — | S | edge (cross-vendor + explicit provider_kind) | worth-it | **do-next** |
| F4 S3 reachability (minimal) | 3 | S | zero today (dead code) | enablement | **do-next** |
| D3-perf pre-rendered clips | 4 | M | WS-streaming-TTS only | defer | defer |
| A4 eager/async bypass | 4 | S | edge | observability, not a bug | defer |
| A2 phrase rotation | — | XS | cosmetic | confirmed-but-cosmetic | defer |
| A8 filler guard on reasoner | — | XS | <1% token noise | negligible standalone | defer |
| A6 max_history orphan 400 | 2 | — | **none (proven)** | unreachable today | **skip** |
| S1 opener-splice | 2 | XL | — | worst value/effort | **skip** |

\* A5 was assessed effort-S; the critics re-scoped it to **M** (three real design bugs must be fixed — see §2.1).

---

## 2. The picks (critic-hardened designs)

### 2.1 A5 — `turn_needs_reasoning` routing quality  *(do-now, M)*

**Problem (all empirically reproduced against the live 17-keyword list):**
- **(c) false-escalation — the money leak.** `t.contains("interest" | "refund" | "analy" | "how much")` fires on everyday billing/sales speech: `"not interested"`, `"no refund please"`, `"analysis paralysis"`, `"that's reasonable"`, `"interesting weather"`, `"how much fun!"` → silently routes benign chit-chat onto the **2×-cost, +seconds-latency** reasoning tier on exactly the billing gateway the doc-comment names. Invisible to the operator (P2 caps cost-per-turn, not misroute frequency).
- **(a) dropped thread.** The heuristic only sees the single final transcript, so `"and the second one?"` after `"calculate my mortgage"` drops back to fast (fails *safe* — cheaper, shared history keeps it coherent — but loses the reasoning thread).
- **(b) non-English escalates nothing** (English-only keywords ⇒ `reasoning_model` is effectively `route=never` for non-English).

**Design.** Keep the cheap synchronous µs-scan (the whole point of `route=Auto`); fix the three bugs in `turn_needs_reasoning` (mod.rs:267) + give it one bit of context via `select_tier`:
1. **Word-aware matching.** Tokenize on Unicode non-alphanumerics (`split(|c| !c.is_alphanumeric())`, Unicode-correct for Devanagari/CJK/accented), match single-word keywords as token-equality and multi-word as a contiguous token window. Drop the `"analy"` prefix-hack; list real lemmas. Kills `interest⊂interested`, `refund⊂refunded`, `analy⊂analysis paralysis`.
2. **Negation guard.** If the token before a hit is a negator (`no, not, never, without, n't`-contractions…), don't count it.
3. **Reasoning stickiness.** `turn_needs_reasoning_ctx(transcript, prev_was_reasoning)`: a *short anaphoric* continuation (`and…`, `what about…`, `the second…`) after an escalated turn, with no closing cue (`thanks/bye/got it`), stays on the reasoning tier for **one** turn. A 1-bit `last_turn_was_reasoning` ledger on the orchestrator, written in `select_tier`.
4. **Language-agnostic numeric signal** (replaces the English lexicon for the dominant case): escalate on **digits adjacent to a math operator** (`% + × * / =`) — `"calcule 15% de 2400"`, `"₹50000 पर ब्याज"`. Keep the existing >28-token length signal (already language-agnostic).

**BLOCKING critic fixes (all three critics flagged these — A5 is not shippable without them):**
- **Apostrophe-safe tokenizer.** Splitting on all non-alphanumerics destroys `what's` → breaks the existing `what's the total` test, and `won't`→`["won","t"]` so contraction-negators never match. Preserve intra-word apostrophes (treat `'` and `'` as word-internal).
- **Unconditional ledger write** in the `Auto` arm. Gating the write on `escalate` sticks the bit `true` forever → **unbounded over-escalation** (inverts the fix). Write the fresh decision every turn.
- **Barge-in ledger race.** A barge-in that kills a reasoner must not leave `ledger=true` and stick the *interrupting* turn onto the 2× tier — the ledger reflects the *completed* tier choice, not an aborted one.
- **Numeric false-positive class.** A bare-digit signal would escalate **phone numbers / order IDs / dates** (`"call me at 5551234"`, `"order 12345"`, `"the 15th"`) on a phone-billing line — trading a lexical leak for a larger numeric one. Require a **math operator adjacent to digits**, not bare digit count.
- **Cut** the dead shutdown-path reset (mod.rs:1794 is `shutdown()` — resets a field on a dying per-session object) and the **redundant** `s2_sticky_followup_live_ollama` live test (routing is decided *before* any model call — the deterministic mock test covers it through the real `select_tier→run_turn` path).

**Integration / composition.** Pure refinement of the single routing entry (`select_tier`, called once from `run_turn`); the `(tier, is_reasoning)` bool flows through the identical P1 budget + degrade + shared-history machinery. No new path, no config/wire/DAG/OpenAPI change. `RoutingMode::Always` untouched.

**Tests (RED→GREEN).** Unit: negation stays fast, substring no longer leaks, numeric+operator escalates cross-lingually, phone/order/date stays fast, stickiness on/off, the existing positives (and `what's the total`) still pass. Integration (mock endpoints): `two_tier_sticky_followup_stays_on_reasoning` (turn 2 `"and the second one?"` hits the reasoning mock, requests==2), `two_tier_negation_stays_fast` (`"i said no refund"` hits the fast mock). No live test (cut as redundant).

**Optional future (out of scope):** a flat `reasoning_keywords: Vec<String>` (defaults empty, mirrors `latency_filler_phrases`) for non-English operator tuning — `route=always` already covers the cross-lingual need.

---

### 2.2 A7 — reasoning budget → inter-audio **stall watchdog**  *(do-now, M)*

**Problem.** `reasoning_budget_ms` times only **first audio** and disarms the instant `spoke==true`. A reasoning model that streams one filler token (`"Let…"`) in 200 ms then thinks 40 s leaves the caller hearing a partial phrase then **real silence** until the 60 s whole-request timeout. D3 masking can't help — it aborts on first audio with a one-utterance-per-turn latch (code-verified). This is the gateway's worst residual call-quality failure: **dead air mid-sentence on a live call.**

**Design.** Convert the one-shot TTFA deadline into a periodic **last-audio** watchdog on the *same* `tokio::select!` loop in `run_turn` (mod.rs:1147–1192) — no new task, no parallel actor:
1. Add `last_audio_at: AtomicU64` (monotonic ns) beside `spoke`; the pump's `speak_sentence` (the single TTS egress point) stores `now()` on **every** sentence (heartbeat), not just the first.
2. Replace the one-shot `sleep` with an interval tick (`min(budget, ~250 ms)`). Each tick: `gap = now − (last_audio_at != 0 ? last_audio_at : req_start)`; if `gap ≥ budget_ms && !token.is_cancelled()` → `budget_exceeded = true; reasoner_token.cancel()`. This single predicate **subsumes** the old TTFA case (no audio → gap from request start) *and* the new stall case (audio then freeze → gap from last chunk).
3. The cheap fast/single-tier path stays the bare `await` (same `is_reasoning && budget_ms>0` gate as today).

**Compose with the existing degrade ladder (the load-bearing subtlety):** the `Err(Cancelled) if budget_exceeded` arm already calls `speak_degraded(…, spoke)`, which correctly **early-returns when `spoke==true`** (don't talk over a coherent partial). So a stall *after partial audio* must instead **commit the partial** and end cleanly; a stall with *no* audio is byte-identical to today's TTFA degrade (fast draft / canned apology).

**BLOCKING critic fix (failure-mode + integration critics):** the partial-commit path reads `streamed_text`, **which the tool-loop already `clear()`s** (mod.rs:1219). A reasoner stalling *after a tool-call preamble* would commit an empty string while `spoke=true` suppresses degrade → **a brand-new silent dead-air case**. The commit must guard `!partial.is_empty()` and must not double-commit (the plain barge-in arm already commits on its path — the two arms are mutually exclusive). Add a no-double-commit test. Skip the immediate first interval tick.

**Config surface.** **Zero new knobs** — reuse `reasoning_budget_ms` (default 15 000, `0` disables) as the unified *max-silence-gap* bound; extend its doc from "first-audio budget" to "max silence gap — to first audio AND between audio chunks". An internal `STALL_POLL_MS≈250` const (not exposed) sets granularity. (Optional additive `reasoning_stall_ms` only if a deployment needs to split the two — omit from the minimal change.)

**Tests (RED→GREEN).** New SSE mock emits one delta then `sleep(3s)` then the rest: `p1_reasoner_stalls_after_first_token_degrades` (turn returns in ≪3s, post-stall remainder never spoken — RED today: hangs ~3s). `p1_stall_after_partial_commits_partial_not_restart` (partial spoken, fast draft NOT spoken, history ends with the partial assistant msg). Keep the TTFA test (no audio) for byte-identical regression. Negative: a healthy 100 ms/sentence stream under a 300 ms budget must NOT trip. Live (ollama deepseek-r1/qwq): first audio arrives, turn completes without a multi-second silent hang.

---

### 2.3 A10a — cross-vendor reasoning tier wire-format **400**  *(do-next, S)*

> **Critic correction (unanimous):** the original A10 *design* fixed a lesser bug (a dishonest floor echo). The ranking's actual rank-3 justification describes a **different, more severe** bug — the one to build. We split it: **A10a** (this, the real fix) and **A10b** (floor honesty, rides along / defer).

**Problem (A10a).** `with_tier_overrides` (llm/mod.rs:749) does `config.provider_kind = provider_kind.or(config.provider_kind)` then re-infers via `select_adapter`. When the reasoning tier sets `reasoning_base_url` to a canonical Anthropic/Gemini host but leaves `reasoning_provider_kind=None` **while the fast tier had an explicit `provider_kind` (e.g. OpenAi)**, the `.or()` inherits OpenAi and **shadows host re-inference** → the OpenAI wire format is POSTed to `api.anthropic.com` → **hard 400 on every escalated turn**. The P1 ladder then masks it as "reasoning tier failed → degrade", so it looks like flaky degradation, not a config bug.

**Design.** When the reasoning tier's `base_url` differs from the fast tier's **and** `reasoning_provider_kind` was not explicitly set, do **not** inherit the fast tier's `provider_kind` — let `select_adapter` re-infer the vendor from the reasoning `base_url`/`model`. (Equivalently: only inherit `provider_kind` when `base_url` is also inherited.) One conditional in `with_tier_overrides`.

**A10b (rides along, near-free):** make the adaptive-only **floor** provider-kind-aware — `floor_for_model(model, kind)` asserts the `Low` floor only when `kind == Anthropic` (the floor encodes an Anthropic-native fact). Thread `kind` (from the same `select_adapter(cfg).kind()` resolver `LlmClient::new` already uses) through `resolve` and the 3 call sites; `is_reasoning_model` keeps the conservative `Anthropic` assumption (opus/fable are reasoning models for the advisory regardless of transport). Stops the `reasoning_effort_clamped` ConfigWarning from misfiring on a proxy. **A8** (reasoning tier inherits the ~40-token filler guard) also rides along in this same `with_tier_overrides`/`build_reasoning_tier` edit — reset/regenerate the reasoning tier's `system_prompt`.

**Compose.** Zero new path, zero new knob; `select_adapter` is the single resolver `LlmClient::new`/`with_adapter`/`with_tier_overrides` already share, so classification and wire-rendering can never disagree about the vendor. Real-Anthropic-host behavior is byte-identical to today.

**Tests.** Unit: `floor_for_model("claude-opus-4-8", OpenAi)==Off` vs `==Low` for `Anthropic`. Integration: a two-tier config with fast `provider_kind=OpenAi` + a reasoning tier whose `reasoning_base_url` is an Anthropic host (mock) asserts the reasoning request renders **Anthropic** wire format (a `messages`/`max_tokens` body, not `reasoning_effort`). Live (ollama): `provider_kind=openai` + an `opus-4-8`-aliased local model + `reasoning_effort=off` → the request carries **no** `reasoning_effort` field (no "does not support thinking" 400).

---

### 2.4 F4 — S3 async-tool sink reachability  *(do-next — minimal now, full surface deferred)*

**Problem (diagnosis fully code-verified).** No production path registers tools for the conversation orchestrator (`LlmClient::with_functions` has **zero** non-test callers on the conv path; `ConversationWebSocketConfig` has no `tools` field; `FlowManager` has zero external callers and never sets the sink). So `wire_async_sink` **always no-ops** and the entire S3 surface (`handle_async_final`, `speak_async_followup`, turn-id gating, the metric) is reachable only from unit tests — and it *looks* wired (a reviewer trap).

**Recommendation: do the MINIMAL honesty now; DEFER the full webhook-tools surface.**

- **Now (S):** document that conversation-path async tools are not yet operator-reachable (only the DAG path registers tools), and downgrade `wire_async_sink`'s silent no-op to a one-line `debug!` so it isn't mistaken for live. No new surface, no risk.
- **Deferred (M, scoped opt-in, build only on operator request):** add `tools: Vec<ConversationToolConfig>` to the WS config (HTTP-webhook tools — name/description/parameters/url + `is_async`/`run_llm`), build a `FunctionRegistry`, and attach it to the LlmClient **before** `Arc::new` and **before** `build_reasoning_tier` (so the fast and reasoning tiers share one registry — exactly what `wire_async_sink`'s doc-comment promises). That lights up *both* the existing sync tool-loop and the async sink with **zero new orchestration path**.

**Why deferred, not now (critic consensus):** it builds **new client-facing risk for shipped-but-unrequested code**:
- **SSRF / DNS-rebinding TOCTOU.** Tool URLs are client-supplied and the gateway makes outbound POSTs → it becomes an SSRF pivot. Config-time `validate_llm_url` is **insufficient** against DNS rebinding; the full surface needs resolve-and-pin or a connect-time private-range reject.
- `async` is a **reserved Rust keyword** — the field must be `is_async`.
- The shared-registry **cross-vendor tool-call rendering** on the escalated tier is asserted-working but **untested** (the strict-provider 400-brick class) — and it depends on **A10a landing first** (a cross-vendor reasoning tier with the wrong wire format would 400 its tool POSTs).
- The webhook handler must honor the **sync-tool cancel token** (a naive `reqwest` POST ignores barge-in and holds the turn-busy window open).

So F4's full surface is a *feature*, gated behind A10a + an explicit operator ask — not a cleanup follow-up.

---

## 3. Deferred — real but low-value / mostly-covered

- **D3-perf pre-rendered clips** — the existing process-wide TTS cache (config_handler.rs:969, 30-day TTL, keyed by `provider|voice|model|format|rate|features`) already makes the 2nd+ filler utterance near-instant and shared across sessions for **HTTP** TTS; first-masked-turn-per-config amortizes to ~zero. The only real gap is **WebSocket-streaming TTS** (re-synthesizes live every time). Fold into broader WS-TTS cache coverage when a WS-TTS operator reports it — not filler-specific warming.
- **A4 eager/async bypass** — half the claim is code-wrong (the eager-confirm path already falls through to `run_turn`'s full P1 net; the async-followup silence is a failed *unsolicited* volunteer whose result is already in history). The genuine residue: an escalating turn that an eager prediction matched gets the fast answer with **no log/metric** marking the bypass — an **observability gap**, not a bug. Add a `debug!`+counter ride-along when in the eager/two-tier/Auto file.
- **A2 phrase rotation** — confirmed real but **cosmetic** (the rotation index advances at arm-time, not speak-time, so consecutive *spoken* fillers can repeat). Trivial fix: advance inside the spawned task at actual speak time. Ride-along when touching the masking code.
- **A8 filler guard on the reasoner** — ~40 input tokens (<1% of a reasoning request) + a mildly off-key "be terse" nudge to a deliberate reasoner. **Folded into A10a's `with_tier_overrides` edit** (reset the reasoning tier's `system_prompt`).

## 4. Skipped — proven non-issues

- **A6 max_history orphan 400** — **proven unreachable** by probe tests against the real mutation methods: `add_pairing_safe` keeps every tool pair adjacent, so count-based eviction always removes `assistant{tool_calls}` + its `tool_result` **together** (zero orphans across caps 4–12 × 10 eviction steps). The orphan only appears by wedging a System message mid-pair via raw `add()`, which **no production path does**. The underlying adapter gap (renders any `Tool` as a `tool_result` with no preceding-`tool_use` check) is genuine but latent; revisit only if a future path (e.g. F4 + summarization interleaving) can actually produce an orphan.
- **S1 opener-splice** — the problem it targets is **already solved**: D3 masking guarantees no pre-first-audio silence on a slow reasoner, §4.4 barge-in cancels the LLM, P1 degrades a stuck one. S1's entire net delta is replacing a *canned* holding phrase with an *LLM-generated* one — while paying a **2× LLM-call multiplier on every escalated turn** (fast opener + reasoner concurrently) vs the shipped either/or routing. value=2, effort=XL, risk=high — worst value-per-effort in the set, and it re-introduces a cost multiplier the shipped design deliberately avoided.

---

## 5. Sequencing & acceptance

**Order:** A5 → A7 → A10a (+A10b/A8 ride-along) → F4-minimal. A5/A7/A10a are all two-tier-gated and ship as one "harden the router before operators enable it" set; A5 first because it's the invisible cost leak that makes the opt-in safe to turn on, A10a before F4 because F4 pushes tool POSTs through the same cross-vendor tier plumbing.

**Methodology (same as the last goal):** each item RED→GREEN (failing test encoding the bug) → minimal fix → full `--lib` floor + `clippy --all-targets` → brutal-review workflow → fix → **credential-free live e2e gate (ollama localhost:11434)** → commit. Full regression (lib + all integration suites + `--all-targets`) at the end.

**Acceptance criteria (measurable):**
- A5: `"i said no refund"`/`"i'm not interested"`/`"order 12345"` route to the **fast** tier; `"calculate my mortgage"` then `"and the second one?"` both route to **reasoning** (requests==2); existing positives + `what's the total` still pass; **no** unbounded over-escalation (ledger overwritten per turn).
- A7: a first-token-then-3s-stall turn returns in ≪3s and never speaks the post-stall remainder; a stall-after-partial commits the partial and does **not** restart; a healthy 100 ms/sentence stream never trips; no empty-partial double-commit.
- A10a: a cross-vendor reasoning tier renders the **reasoning host's** wire format (no 400); real-Anthropic behavior byte-identical.
- F4-minimal: `wire_async_sink` logs its no-op; docs state conv-path async tools are DAG-only today. (Full surface: not built unless an operator requests it, and only after A10a + SSRF resolve-and-pin.)

**Net:** ~4 focused changes (3 two-tier hardening fixes + 1 honesty cleanup), each small-to-medium, each composing with the existing turn lifecycle with no rogue parallel path. Two "findings" (A6, S1) correctly retired as non-issues, saving the effort they'd have cost.
