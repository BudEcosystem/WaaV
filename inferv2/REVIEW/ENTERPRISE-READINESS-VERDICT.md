# WaaV Infer — Enterprise-Readiness Verdict
Date 2026-06-21 · branch `waav-infer-v2-build` · fixes at HEAD 16209da · GB10 (Grace-Blackwell, 121GB unified).
Method: 6-investigator line-by-line review (all findings lead-verified) → 5 CRITICAL fixes → full regression
→ live per-model perf/accuracy + concurrency/overload on the fixed code. Evidence: WaaV/inferv2/REVIEW/.

## BOTTOM LINE (honest)
The **production serving core + 12 of 14 ONNX model arms are READY** for super-scalable, ultra-low-latency,
chaotic enterprise voice on GB10 — **but only after this session's 5 critical fixes**, and **NOT** with a
blanket "production-ready" stamp: a defined backlog (fp16/quant-on-CUDA for 2 arms, fp16 input-dtype work,
advertised-but-unintegrated "shelfware", load-resilience metrics, a handful of HIGH scheduler hazards, and 3
test-coverage gaps) stands between "the core works and scales" and "fully certified for all advertised
capability." The earlier session's flat "production-ready" was **overstated** — the brutal review found 9
verified CRITICALs the happy-path tests missed.

## CERTIFIED READY (measured this session, on the fixed code)
- **Ultra-low-latency + concurrency (the headline thesis HOLDS).** Whisper STT RTF **0.057 (17.7× realtime)**,
  scales to **16 concurrent at per-stream RTF 0.83 (<1)**. Kokoro TTS RTF **0.15**, **flat to N=16** (110
  audio-s in 16.6s). 12/14 arms real-inference RTF **0.04–0.27**. parakeet-ctc 482ms / tdt 545ms for 12s audio.
- **Accuracy = BIT-EXACT vs the reference engine** (stronger than WER): chatterbox/turbo ragged-batched ==
  per-slot bit-identical (3 heavy gates GREEN this session); supertonic maxΔ=0.0000; whisper byte-identical;
  voxtral int8 byte-identical. Re-confirmed on the fixed tree.
- **Stability / memory — the unified-pool OOM saga is CLOSED.** Full 14-arm live sweep + 16-concurrent serve:
  free memory **rock-stable 91–109G, zero OOM**. Regression `cargo test --workspace` **833 passed / 0 failed /
  0 panics**; `clippy --workspace --all-targets -D warnings` clean.
- **Load-resilience (overload).** #9 gates GREEN: a request spike + latency explosion **queue/shed with a typed
  429**, bounded memory, accepted streams bit-identical (gate9 + concurrency/VRAM/deadline shed tests).
- **Crash-safety.** Cross-cutting sweep: **0 client-reachable panics** on the server front door, exactly ONE
  (correct) `unsafe` tree-wide, no blocking-in-async, no live `todo!`/stub. A worker survives hostile input.
- **Concurrency correctness fixes verified.** F1 (codec-AR concurrency 4→24), F2 (slow consumer no longer
  head-of-line-blocks all tenants), F3 (drain actually drains) — committed, unit+integration gated.
- **F1/F2 live 16-concurrent gate (on the FIXED code): PASSED** — `16 concurrent codec-AR streams,
  audio=75.4s, wall=53.1s, RTF=0.7044, ep=cuda, no hang, no OOM` (mem returned to 110G). Confirms the
  batcher serves 16 concurrent under realtime on the fixed tree.

## FIXED THIS SESSION (5 CRITICALs, commit 16209da, bit-faithful, 833/0)
F1 concurrency-cap-4→24 · F2 slow-consumer-head-of-line-block · F3 drain-doesn't-drain · F4 fp16 output-read
(35 sites) · F5 per-arm corruption (funasr KV / qwen3 stride+token / voxtral prompt / encdec+diarize bounds).

## NOT YET CERTIFIED — the backlog between "core works" and "fully enterprise-certified"
Ranked by enterprise impact:
1. **[HIGH] fp16/quant on CUDA broken for 2 arms.** voxtral q4f16 + cohere fp16 fail on the GB10 CUDA EP
   (ORT `GroupQueryAttention attention_bias not supported in cuda kernel` + cuDNN "no execution plans"). An
   ORT/cuDNN EP limitation, not WaaV logic — but it means those 2 arms are CPU-only here. The other 12 arms
   (incl whisper) are CUDA-ready. → Fix path: non-GQA-attention_bias export, an ORT/op upgrade, or CPU EP.
2. **[HIGH] Full e2e fp16 needs input-dtype casts.** F4 fixed the F16 OUTPUT read; canary/supertonic/
   chatterbox-AR/parakeet/nemo/qwen3/funasr still build step INPUTS as f32 → fp16 not end-to-end (the
   `StaticGraph::input_types()`-driven cast, cohere-pattern). Necessary follow-up, not done.
3. **[HIGH→arch] "Shelfware" — advertised but UNINTEGRATED.** The runtime resilience layer (CUDA-graph
   fallback, crash-isolation, poison-firewall, paged-KV, prefix-cache), the scheduler's advanced admission
   (LayeredAdmission/CohortPlanner/TierExecutor/RingKv/KvFirewall), the S2S `CodecArDuplexModel` (a synthetic
   hash-conditioned scaffold, not a real model), and whole crates (router, provider, gateway-provider-api,
   most of features + backend-api, dag-except-CLI) are `pub`/tested but have NO live callers. → Decide per
   item: WIRE it, or DOWN-SCOPE the docs/claims (INFER_ENGINE, INFER_SPEC, the #6 memory) to match reality.
4. **[HIGH] Scheduler live hazards** (DutyLedger IS live): admission.rs clone-full-map-per-call + admit/commit
   TOCTOU over-admit; migration stale-epoch guard is prose-only (no dest-side reject). Not fixed.
5. **[HIGH] Load-resilience layer emits ZERO metrics** (429/shed/in-flight/queue-depth/reserved-VRAM) →
   operationally blind under load. reconnect-storm cap `admit_reconnect` is dead code.
6. **[MED] Coverage/verification gaps.** No handler-level (axum) ≥16-through-the-WS-path concurrency test
   (F1's permit-release is code+unit verified, the live gate drives the batcher). No chaos/fault-injection,
   fairness/starvation, or oversized-input tests. 18/20 arms lack deep perf gates (sweep = load+infer latency).
7. **[MED] Batched-path TTFA = full synthesis** (F6, deferred): the incremental-TTFA fix reached single-stream
   but not the multiplexed loop; concurrent codec-AR gets first audio at completion. (Mitigated for chatterbox:
   its S3Gen decoder is non-causal ⇒ TTFA==whole-body regardless.)

## RECOMMENDATION
Deploy the **serving core + the 12 CUDA-ready arms** for enterprise voice NOW — they are low-latency,
concurrency-scaling, accuracy-bit-exact, memory-bounded, overload-resilient, and crash-safe after the 5 fixes.
Treat items 1–7 as the **certification backlog**; #1/#2 (fp16) and #5 (metrics) are the highest-leverage next,
then honestly reconcile #3 (shelfware) in the docs so advertised ≠ shipped never recurs.
