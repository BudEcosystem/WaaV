# Making WaaV Realtime — The Optimal Path

**Date:** 2026-06-05
**Companion to:** `WaaV/LATENCY_ANALYSIS.md` (the measured baseline this roadmap acts on).
**Question:** what are the *optimal* solutions to make WaaV a genuinely realtime conversational system — ideally end-of-speech → first-audio in the **300–500 ms** range that feels natural, and as close to **200 ms** as physics and providers allow.

This document combines (a) WaaV's **measured** bottlenecks and (b) **extensive 2025–2026 web research** into the state of the art, then turns both into a prioritized, code-level roadmap. Sources are listed at the end; key numbers are cited inline.

---

## 0. The two facts that frame everything

1. **WaaV's own gateway is already realtime.** Measured orchestration overhead is **~12 ms** (p99 15.9 ms); DAG-executor overhead **~0.1 ms**. *Nothing in this roadmap is about making WaaV's Rust faster* — it's about the **components WaaV orchestrates** and the **pattern it orchestrates them in**.
2. **The target is reachable, but only as a *product* of every stage.** Natural turn-taking is **300–500 ms**; >500 ms "feels unnatural" (Pipecat/Chanl). A *well-built cascade* already hits **0.5–1.1 s** in production (the "Riya" example: Groq STT 150–320 ms + Groq Scout LLM 85–500 ms + streaming TTS 200–500 ms + ~100 ms network) and **sub-300 ms first-audio is achievable without a native speech model** by "chunking LLM output at sentence boundaries and streaming TTS against those chunks" (Inworld). Speech-to-speech models reach **~200–500 ms**. So 200–500 ms is an engineering problem, not a research one.

> **Thesis:** WaaV is a DAG voice gateway. Its unique advantage is that it can offer **both** an optimized cascade **and** a speech-to-speech path, and route per use-case. The optimal strategy is to (1) fix the cascade defaults, (2) attack turn-detection — the single biggest measured latency — (3) remove the network/cold-start tax, and (4) add speech-to-speech as a first-class DAG node. Each layer is independently shippable.

---

## 1. Where WaaV's time actually goes (measured) → the SOTA answer for each

| WaaV measured bottleneck | What it costs today | SOTA answer (researched) | Realtime target |
|---|---|---|---|
| **Reasoning LLM** (sarvam-30b) | 1–4 s; **30 s DAG timeout**; empty content | Non-reasoning, low-TTFT model: **Groq Scout ~80 ms TTFT**, Cerebras 80–150 ms | **<150 ms TTFT** |
| **Batch/HTTP TTS** (Sarvam) | 700–900 ms, no early audio | **Streaming WebSocket TTS**: ElevenLabs Flash v2.5 model 75 ms / e2e ~135–186 ms; Cartesia Sonic model 40–90 ms; Inworld P90 <130–250 ms | **<200 ms first-audio** |
| **Endpointing-bound STT** | ~420 ms (silence-window wait) | **Semantic / all-in-one EoT**: Deepgram Flux (EoT 200–600 ms faster, eager 150–250 ms early); AssemblyAI Universal-Streaming (native EoT ~300 ms); **Smart Turn v3 (12 ms CPU)** | **<150 ms to LLM-start** |
| **Smart-turn inference** | **54 ms** (26 ms MEL + 27.5 ms ONNX, v1/v2-era) | **Smart Turn v3: 12 ms CPU** (8 MB, ONNX, 23 langs), 20–60× faster than v2 | **~12 ms** |
| **Per-token `speak(flush=true)`** | 1 HTTP synth per token (batch TTS) | **Sentence-boundary aggregation** + stream against WS TTS (ranked the #3 optimization industry-wide) | n/a (correctness) |
| **Network / cross-region** | baked into every stage; cold-start 6 s | **Colocation / regional endpoints / warm pools** (async.com measured Cartesia 640 ms e2e vs 40 ms model — network dominates) | shave 100–300 ms |
| **Transport** | WebSocket path (TCP, head-of-line blocking) | **WebRTC** (UDP, jitter buffer) for media — WaaV already has this via LiveKit | barge-in stop <60 ms |

---

## 2. The architectural decision: optimized cascade vs. speech-to-speech

There are two roads to realtime. WaaV should support **both** and route per use-case — this is its differentiator.

### Road A — Optimized cascade (STT → LLM → TTS)
**Achievable:** ~300–800 ms turn latency; **<300 ms first-audio** with sentence-streaming.
**Keep it when you need:** tool-calling / RAG / function-calling, transcripts & auditability, per-stage model choice and cost control, broad language coverage, and the ability to swap any component. This is WaaV's core DAG model.

### Road B — Speech-to-speech (native audio-in / audio-out, single model)
**Achievable:** ~200–500 ms; native turn-taking, interruptions, backchannels, prosody/emotion.
| Model | Latency | Notes |
|---|---|---|
| **Kyutai Moshi** | 160 ms theoretical, **~200 ms practical (L4 GPU)** | **Open-weights, self-hostable** — full-duplex, no per-minute vendor cost |
| **OpenAI gpt-realtime** | time-to-first-voice 450–900 ms; first-token 180–300 ms | Strong reasoning/tool-use; vendor-locked |
| **Gemini 2.5/3.1 Flash Native Audio** | sub-second, single model | Preserves pitch/pace/emphasis |
| **Amazon Nova (2) Sonic** | streaming-first, native barge-in | On Bedrock; LiveKit integration exists |
| **Hume EVI 3/4** | <300 ms, prosody-based EoT | Emotion-forward |

**Trade-offs of S2S:** vendor lock-in, higher per-minute cost at scale, weaker tool-use/auditability, fewer model choices. Self-hosted **Moshi** removes the cost/lock-in axis at the price of running a GPU.

> **WaaV's move:** make S2S a **first-class DAG node type** (audio-in → audio-out) that bypasses the STT→LLM→TTS sub-graph. The DAG already routes data between nodes; an `S2sProvider` node slots in exactly like `LlmEndpoint`. Then a single WaaV deployment serves low-latency S2S agents *and* controllable cascade agents from the same engine.

---

## 3. Stage-by-stage optimal choices (the cascade, done right)

### 3.1 Turn detection — **the biggest measured lever (~420 ms)**
Fixed silence endpointing "adds nearly a full second to every response" (LiveKit). The fix is **semantic, eager turn detection**:
- **Adopt all-in-one ASR+EoT** (Deepgram **Flux** or AssemblyAI **Universal-Streaming**): one model jointly transcribes and detects end-of-turn, **200–600 ms faster** than stitched VAD+endpointing and **~30 % fewer false interruptions** (Flux). Distinguishes "because…" (continue) from "Thanks so much." (done) — silence can't.
- **Use EagerEndOfTurn (speculative start):** Flux fires an eager event **150–250 ms before** confirmation → **start the LLM speculatively**, cancel on `TurnResumed`. Cost: 50–70 % more LLM calls for 150–250 ms saved. **WaaV already has the cancellation/barge-in machinery** (`handle_barge_in`, turn `CancellationToken`) — this is a natural fit.
- **Upgrade WaaV's own smart-turn to v3:** **12 ms CPU** (vs the **54 ms** measured), 8 MB ONNX, 23 languages, also **absorbs the 26 ms MEL step** (v3's Whisper-Tiny encoder ingests audio directly). Drop-in: WaaV already runs smart-turn via `ort`; swap the model + preprocessing. Use it as the local trigger that lets STT finalize *immediately* instead of waiting the silence window.
- **Tune the threshold** per use-case (Flux `eot_threshold` 0.5–0.9; lower = faster, more false-positives).

**Expected reclaim: 200–400 ms off the front of every turn.**

### 3.2 STT
Stream, don't batch. **Deepgram Nova-3** (~150 ms TTFT US) or **AssemblyAI Universal-Streaming** (~300 ms, native EoT). Prefer the provider whose **endpointing is fused** (Flux / Universal-Streaming) so 3.1 and 3.2 are one model, not two. WaaV's streaming STT path + reconnect resilience already exists.

### 3.3 LLM
- **Ban reasoning models on the voice path** (measured: 1–4 s, 30 s DAG timeout, empty content). Add a guard: if `content` is empty but `reasoning_content` is present, fall back / log — never speak silence.
- **Default to a sub-100 ms-TTFT model:** **Groq Scout ~80 ms**, Cerebras 80–150 ms — these make the LLM a non-bottleneck. (For comparison: GPT-4o-mini 300–400 ms, Claude Haiku 200–300 ms TTFT.)
- **Prompt caching / KV-cache:** 13–31 % TTFT improvement and 41–80 % cost reduction by caching the shared system-prompt prefix.
- **Cap `max_tokens`** for voice (short replies; "the first sentence is often the only sentence").
- **Fallback chain** across rate-limit pools (Scout → 70B → 8B) for reliability under load.

### 3.4 TTS — **use streaming WebSocket, fix the call pattern**
- **Default to a streaming WS provider.** WaaV already wires **Cartesia** (true WebSocket). ElevenLabs Flash v2.5 (e2e ~135 ms) and Inworld (P90 <130–250 ms) are also excellent. WaaV's **ElevenLabs and Deepgram TTS paths are HTTP/REST** today — either wire their WS APIs or treat them as batch-only.
- **Fix the flush/aggregation policy (correctness + latency).** WaaV's conversation loop calls `speak(token, flush=true)` **per token**. For an HTTP provider that is **one synthesis request per token** (pathological); for a WS provider, `flush=true` every token forces per-token synthesis and hurts prosody. **Aggregate to sentence/clause boundaries**, send deltas with `flush=false`, flush at boundaries — the industry's #3-ranked optimization ("batch TTS by complete sentences… eliminates audible chunking"). This is the concrete fix for the token-by-token finding in the analysis.

### 3.5 Overlap / pipelining — **the #1 optimization**
Start TTS before the LLM finishes; process partial transcripts before the user finishes. WaaV's streaming DAG executor (`execute_streaming_from`) and conversation pump already do token streaming — the missing piece is **sentence aggregation between LLM and TTS** (3.4) so the overlap produces clean, continuous audio. Measured in the analysis: streaming overlap already saves **~276 ms**; with sentence-streaming against a WS TTS it gates first-audio on **TTFT + TTS-TTFB (~150 ms + ~150 ms)**, not full generation.

---

## 4. Network, transport & deployment — the hidden tax

- **Colocation / regional endpoints.** Every provider number in the analysis includes cross-region RTT; async.com measured **Cartesia at 640 ms e2e vs ~40 ms model latency** — *network dominated by 16×*. Deploy WaaV near the providers (or use their regional PoPs); for telephony, colocate with the telephony PoP (the Telnyx pattern).
- **Warm pools / persistent connections.** Measured cold-starts were brutal (LLM 6.2 s, TTS 5.9 s, first STT slow). Pre-warm STT/LLM/TTS connections on session start; keep pools hot; never pay cold-start mid-call.
- **WebRTC for media.** Prefer WaaV's **LiveKit (WebRTC/UDP)** path over the WebSocket (TCP) path for browser/mobile media — TCP head-of-line blocking adds unpredictable latency under loss. Keep WS for telephony/orchestration.
- **Barge-in must stop TTS <60 ms.** Anything slower "feels like the agent ignored the interruption." WaaV has `clear()` + interruption; **measure and enforce the stop latency** (the live profiler can track it).

---

## 5. Measure it continuously — the instrument is built

The live profiler from the prior task (observer hooks → `TurnProfiler` → `LatencyProfiler` → Prometheus `waav_turn_*`/`waav_frame_*` + `/debug/profile`) is the control loop for all of the above. **Wire it end-to-end (Phases 3–5)** so every turn's stage budget, the streaming-vs-batch ratio, the smart-turn cost, and queue depths are visible in production — then tune §3–§4 against real traffic and catch regressions. Targets to alert on (industry P95s): **TTFB-to-first-audio P95 < 300 ms, full turn P95 < 800 ms, barge-in stop < 60 ms.**

---

## 6. Prioritized roadmap (each phase independently shippable)

| Phase | Change (WaaV-specific) | Effort | Expected end-of-speech→first-audio |
|---|---|---|---|
| **Baseline** | today (reasoning LLM, batch TTS, fixed endpointing) | — | **~1.4 s** (or 30 s / silence with reasoning LLM) |
| **P0 — Cascade defaults** | Non-reasoning sub-100 ms-TTFT LLM (Groq/Cerebras) + reasoning-empty guard · streaming WS TTS default (Cartesia) · **sentence-aggregation flush fix** · cap max_tokens · streaming DAG path default | days | **~500–700 ms** |
| **P1 — Turn detection** | Smart Turn **v3** (54→12 ms) · semantic/all-in-one EoT (Flux/Universal-Streaming) · **EagerEndOfTurn → speculative LLM start** (reuse barge-in cancel) · tune `eot_threshold` | 1–2 wks | **~300–450 ms** |
| **P2 — Observability** | wire live profiler Phases 3–5; alert on P95 budgets; verify barge-in stop <60 ms | 1 wk | (no latency change; locks in the gains) |
| **P3 — Network/warm** | regional/colocated provider endpoints · warm connection pools · WebRTC-first media · kill cold-starts | 1–2 wks | **~200–350 ms** |
| **P4 — Speech-to-speech node** | `S2sProvider` DAG node (gpt-realtime / Gemini Live / Nova Sonic / **self-hosted Moshi**) as an alternate path for latency/naturalness-first agents | 2–4 wks | **~200–500 ms**, native turn-taking |

**Net:** P0+P1 alone take WaaV from ~1.4 s (or unusable) to **~300–450 ms — natural conversational latency**. P3 pushes toward **~250 ms**. P4 gives a **200–300 ms** S2S option where control can be traded for raw latency. The 200 ms aspiration is reachable for first-audio with P0–P3 on a colocated stack, and S2S (P4) is the structural way under it.

---

## 7. The single most important changes (if you do only three things)

1. **Kill the two anti-patterns from the analysis:** reasoning LLM on the voice path, and per-token `speak()` to a batch/HTTP TTS. Replace with a fast non-reasoning LLM + sentence-aggregated streaming WS TTS. *(Removes seconds; pure config/wiring.)*
2. **Attack turn detection:** Smart Turn **v3** (12 ms) + eager/semantic end-of-turn with speculative LLM start. *(Reclaims 200–400 ms — the biggest measured lever, and WaaV already has the cancellation machinery for speculation.)*
3. **Add a speech-to-speech DAG node.** *(The structural path to sub-300 ms and the most natural turn-taking; positions WaaV to serve both worlds from one engine.)*

---

## Sources

- Cartesia Sonic latency — cartesia.ai/sonic; inworld.ai/resources/best-speech-to-speech-apis
- ElevenLabs Flash v2.5 latency — elevenlabs.io/docs/eleven-api/concepts/latency; async.com/blog/tts-latency-vs-quality-benchmark
- Streaming-TTS measured TTFB benchmark — async.com/blog/tts-latency-vs-quality-benchmark
- Groq / Cerebras TTFT — console.groq.com/docs/production-readiness/optimizing-latency; speko.ai/benchmark/groq-vs-cerebras; dev.to "From 7 Seconds to 500ms"
- Turn detection taxonomy & silence-timeout cost — livekit.com/blog/turn-detection-voice-agents-vad-endpointing-model-based-detection
- Deepgram Flux (joint ASR+EoT, EagerEndOfTurn) — deepgram.com/learn/introducing-flux-conversational-speech-recognition; developers.deepgram.com/docs/flux/configuration
- AssemblyAI Universal-Streaming — assemblyai.com/blog/introducing-universal-streaming
- Smart Turn v2/v3 (12 ms CPU) — daily.co/blog/announcing-smart-turn-v3-with-cpu-inference-in-just-12ms; huggingface.co/pipecat-ai/smart-turn-v2
- Speech-to-speech models — openai.com/index/introducing-gpt-realtime; ai.google.dev/gemini-api/docs/live-api; aws.amazon.com (Nova Sonic); kyutai-labs/moshi (Moshi 160/200 ms); artificialanalysis.ai/speech-to-speech
- Pipecat/voice-agent latency budgets — channel.tel/blog/voice-ai-pipeline-stt-tts-latency-budget; introl.com/blog/voice-ai-infrastructure-real-time-speech-agents
- WebRTC vs WebSocket & barge-in — getstream.io/blog/webrtc-ai-voice-video; futureagi.com/blog/voice-ai-barge-in-turn-taking-2026
- Optimization techniques (prompt caching, speculative, colocation, warm pools) — ruh.ai/blogs/voice-ai-latency-optimization; dev.to "From 7 Seconds to 500ms"; callsphere.ai/blog/voice-agent-latency-optimization
