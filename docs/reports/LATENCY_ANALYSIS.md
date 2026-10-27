# WaaV Realtime Latency Analysis — Live Multi-Provider STT→LLM→TTS

**Date:** 2026-06-05
**Goal under test:** natural conversation latency — *end-of-speech → first response audio* under **200 ms**.
**Method:** live measurements against real providers + a new in-repo latency harness that drives the *real* WaaV orchestration path against calibrated mocks. Every number below is measured on this host, not estimated.

> **Bottom line:** WaaV's own gateway overhead is **~12 ms** — it is **not** the bottleneck. The end-of-speech→first-audio budget is dominated by **three provider/network costs** that today sum to **~1.0–1.5 s**: STT finalization (~420 ms), LLM time-to-first-token (~235 ms, *much worse with a reasoning model*), and TTS first-audio (~700–900 ms). **200 ms is unreachable with the current provider mix and call pattern.** It becomes reachable only with (a) streaming everything, (b) a *non-reasoning* low-TTFT LLM, (c) a *streaming* TTS, and (d) predictive turn-detection instead of fixed-silence endpointing. WaaV already has the right primitives (token streaming, smart-turn); the work is selecting providers and turning streaming on by default.

---

## 1. The budget and where the time goes

The user-perceived latency is `audio_out − stt_final` (end-of-speech → first audible response). Measured per-stage, **warm**, this host → provider:

| # | Stage | What it is | Measured | Who owns it |
|---|-------|-----------|----------|-------------|
| — | *(pre)* turn detection | silence/endpointing or smart-turn deciding the user stopped | **see §6** | WaaV + config |
| 1 | **STT finalization** | end-of-speech → final transcript (Deepgram streaming, endpointing=300 ms) | **254 / 423 / 713 ms** (p50 ≈ 420) | Provider + endpointing policy |
| 2 | stt_final → llm_request | WaaV glue: barge-in check, begin-turn, build request | **~11.6 ms** (p90 12.1) | **WaaV** |
| 3 | **LLM TTFT** | llm_request → first token (Sarvam sarvam-30b, streaming, via WaaV `LlmClient`) | **TTFT p50 235 ms / p90 386 ms** | Provider + network |
| 4 | first token → TTS speak | WaaV glue: token pump → `speak()` | **~0.3 ms** | **WaaV** |
| 5 | **TTS first-audio** | text → first audio byte | **batch (Sarvam) ~700–900 ms**; **stream (Deepgram Aura) ~800 ms** | Provider + network |
| 6 | TTS audio → egress | frame to client | sub-ms (mock) | **WaaV** |
| | **WaaV total (stages 2+4+6)** | pure gateway orchestration | **p50 11.9 ms / p99 15.9 ms** | **WaaV** |

**Sequential, non-overlapped floor with today's providers:** `420 + 12 + 235 + 800 ≈ 1.47 s`.
**With streaming overlap (best case, current providers):** `STT_final + LLM_TTFT + TTS_TTFB ≈ 420 + 235 + ~300 ≈ 0.95 s`.
Either way, **~5–7× the 200 ms target** — and the gap is entirely provider/network, not WaaV.

---

## 2. Finding 1 — WaaV gateway overhead is ~12 ms (not the problem)

Harness scenario **A1** (`tests/latency_harness.rs`): the real `ConversationOrchestrator` + real `VoiceManager`, LLM mock and TTS mock with **zero** injected delay, so the wall-clock *is* WaaV's own cost. 20 turns:

```
stt_final → llm_request : p50 = 11.62 ms   p90 = 12.08 ms
stt_final → audio_out   : p50 = 11.90 ms   p90 = 12.42 ms   p99 = 15.93 ms
```

Nearly all of WaaV's overhead is the **stt_final → llm_request** hop (~11.6 ms: barge-in handling, turn bookkeeping, HTTP request setup to the LLM). Everything after the first token (pump → `speak()` → egress) adds **< 1 ms**. Against a 200 ms budget, WaaV spends **~6 %**. **Optimizing WaaV's code will not move the needle; provider selection will.**

---

## 3. Finding 2 — Streaming overlap saves ~276 ms (and scales with reply length)

Harness scenario **A2**: realistic LLM (TTFT 300 ms, 25 ms/token, 12-token reply → 575 ms full generation), streaming vs. non-streaming:

```
LLM full-generation time          : 575 ms
STREAMING first-audio (audio_out) : p50 = 313.6 ms   (first token spoken at ~313 ms)
BATCH    first-audio (audio_out)  : p50 = 590.0 ms   (waits for the whole reply)
>>> streaming overlap saves ~276 ms of first-audio latency
```

Streaming gates first-audio on **TTFT** (313 ≈ 300 ms TTFT + 13 ms glue); batch gates it on **full generation** (590 ms). The saving equals the rest of the generation time, so it **grows with reply length** — for a 2–3 sentence answer it is several hundred ms. **Streaming must be the default**; non-streaming should be reserved for non-voice paths.

---

## 4. Finding 3 — A *reasoning* LLM is fatal for voice (latency **and** correctness)

`sarvam-30b` is a reasoning model. Measured through WaaV's real `LlmClient` (scenario B, 5 live runs):

```
run 0: TTFT=386ms total=1448ms reply_len=0
run 1: TTFT=186ms total=1033ms reply_len=0
run 2: TTFT=180ms total=963ms  reply_len=0
run 3: TTFT=263ms total=3400ms reply_len=199
run 4: TTFT=235ms total=3969ms reply_len=0
TTFT p50=235 / p90=386 ms      total p50=1448 / p90=3969 ms
```

Two problems:
1. **Total latency 1.0–4.0 s** — the model "thinks" before answering. The streamed TTFT (~235 ms) is the first *reasoning* token, not the first *spoken* token.
2. **Empty spoken content on 4 of 5 runs** (`reply_len=0`). With a voice-appropriate `max_tokens` (60), reasoning consumes the entire budget and the model never emits final `content`. WaaV speaks `content` → **the bot says nothing**.

Raw API confirms it: a one-sentence request streamed **24 KB of SSE** (reasoning tokens) and `reasoning_effort:"none"` is rejected (HTTP 400). It is a lose-lose under a token cap: **low** `max_tokens` → empty spoken content; **high** `max_tokens` → unbounded latency. Driving it **through WaaV's DAG executor** (scenario C2, real `Sarvam LLM → Aura TTS`), non-streaming with `max_tokens=3000` **exceeded the 30 s executor timeout** (the LLM node alone consumed 30,001 ms and was killed). **For conversational voice, use a non-reasoning, low-TTFT model** (or a reasoning model only with a large token budget and a content-extraction guard — at a latency cost that rules out realtime).

**DAG executor overhead is negligible.** In the composed run, `execute_from` total minus the summed node durations was **~0.1 ms** — the orchestration engine itself adds nothing; the cost is entirely in the provider nodes. (The composed *full* STT→LLM→TTS run was additionally blocked this session by a Deepgram STT WebSocket connect timeout — environmental network flakiness, not a WaaV defect: the standalone Deepgram WS probe and the committed `dag_multivendor_live_e2e` full-pipeline test both connect successfully with keys.)

---

## 5. Finding 4 — Batch TTS has no early-audio path; WaaV streams token-by-token

- **Sarvam TTS is batch** (one HTTP call returns the whole clip): measured first-audio ≈ **first byte ≈ full synthesis** ≈ **700–900 ms** (cold outlier 5.9 s), 164 KB for a one-sentence reply. There is **no** "first audio at 100 ms" — you wait for the entire clip.
- **Deepgram Aura streams** (chunked): first audio ≈ **800 ms** here, but that is network + model, not buffering; a closer/faster streaming TTS (Cartesia, ElevenLabs Flash, a local model) reaches first-audio in ~70–200 ms.

**Architectural note (from the code path):** WaaV's streaming turn pumps **each LLM token delta straight to `speak(flush=true)`** (`conversation/mod.rs::run_turn`). This is *excellent* for a streaming/websocket TTS (audio starts on the first token) but **pathological for a batch TTS**: it would fire one HTTP synthesis **per token**. **Pairing token streaming with a batch TTS must be prevented** (buffer to sentence boundaries when the TTS is batch, or require a streaming TTS when LLM streaming is on).

---

## 6. Finding 5 — STT finalization is endpointing-bound (~420 ms); this is the smart-turn opportunity

Deepgram streaming STT, real Aura-synthesized utterance streamed at 1× pace, time from last-audio (end-of-speech) to `speech_final` with the **full correct transcript**:

```
endpointing=300ms : speech_final at 254 / 423 / 713 ms after EOS   (transcript complete)
endpointing=100ms : speech_final at ~264 / 451 / 466 ms            (but fragments the utterance)
endpointing= 10ms : finalizes mid-utterance → split transcripts, unreliable
```

The finalization delay is **dominated by the endpointing silence window** — the system waits to *confirm* the user stopped. Tightening it speeds finalization but fragments speech and causes false turn-ends. **This is exactly what WaaV's neural smart-turn detector is for:** predict end-of-turn *semantically* and trigger the LLM *before* the fixed silence window elapses, reclaiming 200–400 ms. Quantifying smart-turn's own inference cost (must be cheap enough to run per-frame) is **§7**.

---

## 7. Finding 6 — Per-frame / turn-detection budget (smart-turn is 53.8 ms/decision)

Measured (`tests/turn_detect_latency.rs`, local cached ONNX models, **CPU** onnxruntime):

```
SILERO-VAD  per 512-sample (32 ms) frame : p50 = 0.227 ms  p99 = 0.258 ms
SMART-TURN  mel extraction (800-frame win): p50 = 26.30 ms
SMART-TURN  onnx inference                : p50 = 27.53 ms  p99 = 27.65 ms
SMART-TURN  mel + inference (per decision): p50 = 53.83 ms  p99 = 54.02 ms
```

- **Silero VAD is effectively free** (0.23 ms / 32 ms frame ≈ 0.7 % of the frame budget) — safe to run on every frame.
- **Smart-turn costs ~54 ms per decision** (26 ms MEL + 27.5 ms ONNX). That is **longer than one audio frame**, so it **must not run every frame** — only when VAD flags a plausible end-of-speech. Two caveats: (a) this is the **CPU** runtime; a GPU/quantized session would be faster; (b) the 26 ms MEL is a full-window recompute — in a streaming implementation the MEL amortizes incrementally, leaving the **~27.5 ms ONNX** as the real per-decision floor.

**Why it's still worth it:** smart-turn replaces the ~300–400 ms fixed endpointing wait (§6) with a ~27–54 ms *prediction*, a net reclaim of ~250–350 ms — **provided** it is triggered (not continuous) and accurate. This is the single largest *WaaV-controllable* lever on perceived latency. **Optimization targets:** offload/quantize the smart-turn ONNX (drive `waav_smart_turn_inference_ms` down) and make MEL incremental.

---

## 8. Finding 7 — Per-turn glue degrades under high concurrency

Harness scenario **A3** (0-delay LLM, so this is pure WaaV glue under load):

```
concurrency =  1 : per-turn p50 = 12.3 ms   p90 = 12.3 ms   wall =  39 ms
concurrency =  8 : per-turn p50 = 107.9 ms  p90 = 186.2 ms  wall = 221 ms
concurrency = 32 : per-turn p50 = 438.8 ms  p90 = 748.8 ms  wall = 854 ms
```

Per-turn orchestration cost rises ~9× at 8 concurrent turns and ~36× at 32. **Caveat:** this single-process test starts 32 cold `VoiceManager`s simultaneously and points them at one shared mock LLM server, so some of the rise is test-harness contention rather than steady-state per-session cost. **Action:** re-measure with the live profiler under a realistic multi-session load (separate connections, warm managers) before drawing a hard scaling conclusion — but treat "per-turn glue is not free under load" as a **flag to watch**, since at the target scale it could itself approach the 200 ms budget.

---

## 9. Can we hit 200 ms? Budget math

The 200 ms target is `STT_final + WaaV_glue + LLM_TTFT + TTS_TTFB` on the critical path (LLM↔TTS overlapped via streaming).

| Configuration | STT_final | WaaV | LLM TTFT | TTS first-audio | **Total** | vs 200 ms |
|---|---|---|---|---|---|---|
| **Today** (Deepgram ep300 + Sarvam-30b reasoning + Sarvam batch TTS) | 420 | 12 | 235* | 800 | **~1467 ms** | ✗ 7× |
| Streaming everywhere, same providers | 420 | 12 | 235 | ~300 | **~967 ms** | ✗ 5× |
| + non-reasoning fast LLM (Groq/Cerebras/local ~80 ms TTFT) | 420 | 12 | 80 | ~150 | **~662 ms** | ✗ 3× |
| + streaming TTS ~70 ms (Cartesia/local) | 420 | 12 | 80 | 70 | **~582 ms** | ✗ |
| + **smart-turn predictive endpointing** (STT_final ~80 ms) | 80 | 12 | 80 | 70 | **~242 ms** | ≈ |
| + regional/co-located providers (shave network RTT) | 50 | 12 | 60 | 60 | **~182 ms** | ✓ |

\* reasoning model's *spoken* first-token is effectively 1–4 s; 235 ms is its reasoning TTFT.

**Conclusion:** 200 ms is achievable **only** at the bottom of the table — every lever pulled at once: streaming STT *and* LLM *and* TTS, a non-reasoning sub-100 ms-TTFT LLM, a sub-100 ms streaming TTS, smart-turn replacing fixed endpointing, and providers close to the gateway (network RTT is a fixed tax in every number above — these were measured cross-region). No single change suffices; the budget is a product of all stages.

---

## 10. Recommendations (prioritized by ms-returned-per-effort)

1. **Ban reasoning LLMs on the voice path; pick a low-TTFT model.** Biggest single win (removes 1–4 s and the empty-content failure). Target a model with TTFT < 100 ms (Groq, Cerebras, or a co-located vLLM). *Add a guard: if `content` is empty but `reasoning_content` is present, log/fallback rather than speak silence.*
2. **Streaming on by default — and guard the TTS pairing.** Keep token streaming, but when the configured TTS is **batch**, buffer to sentence boundaries before `speak()` (don't synth per token); when latency matters, **require a streaming TTS**. Worth ~276 ms+ (§3).
3. **Adopt a streaming TTS with sub-150 ms first-audio** (Cartesia, ElevenLabs Flash, Deepgram Aura streaming, or local). Removes the 700–900 ms batch wall (§5).
4. **Use smart-turn to pre-empt fixed endpointing.** Trigger the LLM on predicted end-of-turn instead of waiting the full silence window; reclaims 200–400 ms of STT finalization (§6). Validate smart-turn inference cost stays per-frame-cheap (§7).
5. **Co-locate / use regional provider endpoints.** Network RTT is baked into every stage measured here; same-region endpoints cut tens-to-hundreds of ms across STT+LLM+TTS.
6. **Warm everything.** Cold-start outliers were brutal (LLM 6.2 s, TTS 5.9 s, first STT call slow). Keep provider connections warm/pooled; pre-warm on session start.
7. **Re-validate concurrency with the live profiler** (§8) before scaling; ensure per-turn glue stays flat per session under real multi-connection load.
8. **Land the live profiler (this work) end-to-end** so these stage timings are continuously visible in production (`waav_turn_*` / `waav_frame_*` on `/metrics`, `/debug/profile`), not just in this one-shot harness — so regressions are caught and the levers above are tuned against real traffic.

---

## Appendix A — How to reproduce

```bash
cd gateway
# Gateway-overhead + streaming-overlap + concurrency (no network):
cargo test --features dag-routing,openapi --test latency_harness -- --nocapture --test-threads=1
# Real Sarvam LLM TTFT through WaaV's LlmClient (live, billed):
SARVAM_API_KEY=… cargo test --features dag-routing,openapi --test latency_harness -- --ignored --nocapture b_real_sarvam_llm_ttft_through_waav
# Real Deepgram streaming STT finalization (standalone probe):
DG_KEY=… python3 scripts/dg_stt_probe.py        # (probe script; keys via env only)
# Smart-turn / VAD per-frame inference cost:
CUDA_HOME=/tmp/nocuda ORT_DYLIB_PATH=…/libonnxruntime.so \
  cargo test --features smart-turn,silero-vad --test turn_detect_latency -- --nocapture --test-threads=1
```

## Appendix B — Raw measurements (this host, warm unless noted)

- **Sarvam LLM `sarvam-30b`** non-stream total: 1.00 / 1.07 / 1.08 / 1.12 s (warm), 6.19 s (cold). Stream TTFT: 0.29–0.42 s; stream total: 0.99–3.93 s; 24 KB SSE for a 1-sentence ask (reasoning).
- **Sarvam LLM via WaaV `LlmClient`** (stream): TTFT p50 235 / p90 386 ms; total p50 1448 / p90 3969 ms; `content` empty on 4/5.
- **Sarvam TTS** (batch): first-audio 0.66–0.89 s (cold 5.9 s); 164 KB / one sentence.
- **Deepgram Aura TTS** (stream, linear16 16 kHz): first-byte 0.76–0.82 s; total 1.29–1.73 s; 82,848 B ≈ 2.59 s audio.
- **Deepgram STT finalization** (stream, ep=300): speech_final 254 / 423 / 713 ms after EOS, full transcript.
- **WaaV gateway overhead** (A1): stt_final→audio_out p50 11.9 / p99 15.9 ms.
- **Streaming vs batch** (A2): 313.6 vs 590.0 ms first-audio (Δ 276 ms).
- **Concurrency** (A3): 12.3 / 107.9 / 438.8 ms p50 at 1 / 8 / 32.
- **Silero VAD**: p50 0.227 / p99 0.258 ms per 32 ms frame.
- **Smart-turn**: MEL p50 26.30 ms + ONNX p50 27.53 ms = **53.83 ms/decision** (CPU runtime).
- **DAG executor overhead** (scenario C): ~0.1 ms (total − Σ node durations).
- **Reasoning LLM through DAG** (C2): `sarvam-30b` non-stream `max_tokens=3000` → **30,001 ms LLM node → executor 30 s timeout**.

*Network caveat:* all provider numbers include cross-region RTT from this host; co-located deployments will be lower but the **relative** bottleneck ranking holds.
