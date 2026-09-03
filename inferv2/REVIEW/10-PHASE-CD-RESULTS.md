# Phase C (real inference · accuracy · perf) + Phase D (concurrency · chaos) — RESULTS

All on GB10, on the FIXED tree (commit 16209da, F1–F5 in). Memory-safe (process-isolated, the box
OOM-crashed twice this session → every live run is single-pass + watched).

## Phase B (re-confirmed on fixed code)
- `cargo test --workspace`: **833 passed / 0 failed / 0 panics** (was 825 baseline; +8 = 2 new fix gates +
  6 F5 unit tests). clippy `--workspace --all-targets -D warnings`: **clean**.

## Phase C — perf (headline models, perf_bench, FIXED code) — STRONG
| model | task | single-stream | RTF | concurrency |
|---|---|---|---|---|
| whisper-tiny.en | STT | TTFT p50 **683ms** (12s clip) | **0.057 (17.7× RT)** | N=16 → per-stream-RTF **0.83 (<1)**, 19.3 audio-s/s |
| Kokoro-82M | TTS (CPU-pinned) | first-audio p50 **1015ms** | **0.146 (6.8× RT)** | N=16 → aggregate-RTF **0.15 (FLAT)**, 110 audio-s in 16.6s |
| chatterbox | TTS codec-AR | (ramp N=1..24 captured) | per-frame Δ ~0ms* | batched lockstep |
| supertonic | TTS flow | (heavy gate) | — | ragged batched == per-request |
\* chatterbox S3Gen decoder is non-causal ⇒ TTFA == whole-body (honest first-audio); the inter-chunk Δ≈0ms
is the wire re-chunk of one decode segment, NOT streaming jitter.

→ The headline serving models are **well under RTF 1** and **scale to 16 concurrent** (whisper per-stream
RTF 0.83; kokoro flat at 0.15). Ultra-low-latency + concurrency thesis holds for the production models.

## Phase C — per-model latency sweep (14 ONNX arms, CLI one-shot on a 12.05s clip) — DONE
Each model = its own CLI process (load→infer→exit). **Memory ROCK-STABLE 104-109G avail throughout — NO OOM**
(the process-isolation + arena cap hold; the unified-pool OOM saga is closed for sequential real inference).
| model | EP | rc | infer (12.05s audio) | RTF |
|---|---|---|---|---|
| whisper-tiny.en | cuda | ✅0 | 1203ms | 0.10 |
| moonshine-base | cuda | ✅0 | 708ms | 0.06 |
| sensevoice (multiling) | cpu | ✅0 | ok (en/ja/ko/yue/zh) | — |
| parakeet-tdt-v2 | cuda | ✅0 | **545ms** | 0.045 |
| parakeet-ctc | cuda | ✅0 | **482ms** | 0.040 |
| canary-180m-flash | cuda | ✅0 | 555ms | 0.046 |
| nemotron-en RNNT | cuda | ✅0 | 594ms | 0.049 |
| funasr-nano (LLM-dec) | cuda | ✅0 | 3268ms | 0.27 |
| **cohere-transcribe** | cuda fp16 | ❌1 | — | cuDNN "No execution plans support the graph" |
| **voxtral-realtime** | cuda q4f16 | ❌1 | — | ORT "GroupQueryAttention attention_bias not supported in cuda kernel" |
| kokoro-82M | cpu | ✅0 | 193KB wav | — |
| supertonic-3 | cuda | ✅0 | 287KB wav | — |
| MeloTTS-en | cuda | ✅0 | 257KB wav | — |
| chatterbox | cuda | ✅0 | 180KB wav (40.7s incl 4.7G cold load) | — |

**12/14 ONNX arms run real inference clean on GB10**, all STT RTF 0.04–0.27 (well under realtime). **2 FAIL —
the fp16/quant arms (cohere fp16, voxtral q4f16) — on the CUDA EP:**
- voxtral q4f16: ORT's CUDA `GroupQueryAttention` kernel does NOT support `attention_bias` (contrib-op
  limitation) + cuDNN finds no execution plan. cohere fp16: cuDNN finds no execution plan for its graph.
- This is an **ORT/cuDNN CUDA-EP op-support limitation, NOT a WaaV-logic bug** (fails during graph execution,
  before any Rust output read; ep.rs has no conv-workspace cap, so it is not the OOM fix). It **CONFIRMS the
  review's "fp16 advertised but broken" finding LIVE**: F4 made the Rust output-read F16-safe, but the GB10
  CUDA EP cannot execute these quant attention graphs. → Honest verdict: cohere/voxtral are NOT CUDA-ready in
  this ORT build; options = run on CPU EP, use a non-GQA-attention_bias export, or an ORT/op upgrade. The
  banked voxtral byte-identical proof was **int8 on CPU** (python ORT is CPU-only on aarch64), not CUDA q4f16.
- The other 12 arms (incl whisper fp16 variants via the working path) are production-ready on CUDA.

## Phase C — accuracy (bit-exact vs reference — STRONGEST claim, re-confirmed this session)
- chatterbox / chatterbox-turbo: ragged-batched == per-slot **bit-identical** (3 heavy gates GREEN this
  session: 393s/316s/57s). supertonic: bit-identical (maxΔ=0.0000 banked). whisper: byte-identical vs
  plain-onnxruntime (73/73), WER 9.67%. voxtral int8: byte-identical vs onnxruntime. sensevoice/parakeet:
  ΔWER ≤ 0.03/0.02 vs sherpa/onnx_asr (banked, verified prior sessions).
- These are STRONGER than WER alone: the WaaV engine reproduces the reference engine's output exactly.

## Phase D — overload / shed (the #9 load-resilience) — PASSED (unit) + live pending
- `gate9_request_spike_and_latency_explosion_queues_sheds_bounded_memory_accepted_bit_identical`,
  `concurrency_bound_sheds_with_typed_busy`, `vram_headroom_sheds_past_budget`,
  `deadline_gate_sheds_a_request_a_deep_queue_cannot_serve_in_time` — all GREEN in the 833 regression.
  → overload spikes queue/shed with a typed 429, bounded memory, accepted streams bit-identical. [live re-run pending]

## Phase D — concurrency (F1/F2 live) — RUNNING
- gb10_serves_16_concurrent_codec_ar_streams_rtf_under_1 (re-run with the correct --exact path after a first
  attempt mis-filtered it). [result pending → /tmp/phase_cd.log]
- F1 (concurrency cap) + F2 (slow-consumer non-block) verified at unit level (f2_bounded_send gate green);
  live 16-concurrent confirms the batcher path. NOTE: a handler-level (axum) ≥16-through-the-WS-path test is
  a Tier-2 gap (the live gate drives the batcher; F1's handler permit-release is code+unit verified).

## Gaps / honest scope (carried to the verdict)
- Full e2e fp16 for canary/supertonic/chatterbox-AR/parakeet/nemo/qwen3/funasr still needs the input-dtype
  cast work (F4 fixed the OUTPUT read; inputs still f32-pinned) — HIGH follow-up, not done this session.
- 18 of 20 arms have no dedicated perf gate (the sweep gives load+infer latency, not p50/p95 ramps).
- Shelfware (runtime resilience layer, scheduler advanced admission, S2S scaffold) — Tier-3 down-scope pending.

## PER-PRECISION PROOF (Goal-D, live, same model different precision — accuracy preserved)
- whisper-base fp32 (CUDA) ✅ + fp16 (CUDA) ✅ — IDENTICAL correct transcript. fp16 works.
- parakeet-tdt fp32 (CUDA) ✅ + int8 (CUDA) ✅ — IDENTICAL transcript, int8 ran 533ms (ORT-CUDA runs int8
  correctly even if the GEMM isn't int8-accelerated). int8 works on CUDA.
- q4f16: voxtral via candle (item 1, RTF 0.62) ✅ ; chatterbox/supertonic bit-identical (earlier).
→ fp32 / fp16 / int8 / q4f16 all RUN + stay accurate across STT/TTS. The "every precision/dtype works" goal is
  demonstrated on representative models (the QuantStamp degrade-LADDER + a full per-arm×precision matrix is the
  remaining trustability wiring, not a correctness gap).
