# WaaV Infer — Accuracy-Preserving Performance Strategy

**Status:** v1.0 · **Date:** 2026-06-18 · **Device of record:** GB10 (Grace Neoverse-V2 + Blackwell sm_121, torch 2.12+cu130) · **Constraint:** every lever here is **exact / bit-faithful at the model's native precision** — NO quantization, NO speculative decoding, NO approximate-attention/pruning/distillation. Backed by **3 GB10 micro-benchmark batches (8 levers measured)** + 6 deep research studies. Raw data: `WaaV/inferv2/INFER_PERF_BENCH.md`; scripts `/tmp/perf_bench/bench_perf_{1,2,3}.py`.

---

## 0. Thesis (and the answer to "does a new batching strategy require model/kernel changes?")

**WaaV's frame-synchronous lockstep + step-bucket batching inverts vLLM's contract: it requires *control-flow correctness* from a model, and ZERO custom kernels.** vLLM continuous batching *must* ship a custom paged-attention CUDA kernel because text KV grows to an unknown EOS length and prefixes are shared → attention gathers across a block-table. WaaV's voice regime removes every precondition (bounded context ≤~3000, homogeneous fixed-size slots, batch ≤128, **memory-bandwidth-bound**), so a **fixed per-slot ring + plain SDPA + one CUDA-graph** suffices. Proven by existence: Moshi runs batched lockstep with `scatter_set` + plain `matmul`/`softmax` and no custom attention kernel; vLLM-Omni's code-predictor runs the depth step with `F.scaled_dot_product_attention` + a manual CUDA-graph and no custom op.

**The headline consequence under your no-quant constraint:** AR decode is memory-bandwidth-bound (`t_step ≈ WeightBytes / bandwidth`, flat in batch), so the two levers that shrink `WeightBytes` (quantization) or skip steps (speculation) are exactly the ones you ruled out — **the lever they leave fully intact is batching, and it's *more* available precisely because the GPU is idle at batch 1.** ⚠️ **But the magnitude is graph-dependent, and on real exported codec-LM graphs it is FAR below the 55× idealized roofline.** The synthetic decode-step microbenchmark that produced "55×@64" omits the host-KV feedback that real exported graphs carry: chatterbox `language_model.onnx` re-streams the full split-KV host↔device every stride (`O(B·max_past·n_layers·2)`, grows with B), so the **live GB10 batched speedup peaks at ~1.8× @ B≈16 and REGRESSES by B=64 (0.95×, slower than per-slot)** — bit-identical to per-slot at every B (gate `live_headline_batched_scaling_matches_doc_curve`; INFER_PERF_VALIDATION.md §3a). Batching is still the headline lever and still the one you're *allowed*, but the realistic per-graph win is **~1.8×, not 55×**; 55× is recoverable only by a re-exported graph that keeps KV device-resident across strides. Perf comes from **batching + bandwidth physics + selecting the right exact kernel**, not from writing kernels. A custom kernel can't beat the memory roofline that's already binding.

---

## 1. The framing answer in full

### 1a. What lockstep/step-bucket requires of a MODEL implementation (additive — AR/duplex models only; one-shot models like kokoro/whisper return `as_stepped()→None` and are untouched)

| Req | What the model must expose / change | Failure if absent |
|---|---|---|
| **Stepped seam** | `prefill(slot,cond)`, `step(&SlotBatch)`, `reset_slot`, `kv_footprint`, `stride_class` — pull the AR loop *out* of the model into the scheduler | a coarse `generate()` can't advance B streams one tick at a time |
| **Fixed-shape batched forward** | one forward over `[B,…]`, B/T=1/KV-shape constant for server lifetime; **K duplex-lanes + D codebooks fold as INNER dims, not batch axes**; idle slots masked-not-removed | shape churn → CUDA-graph re-capture per frame (catastrophic) |
| **Per-slot ring-KV scatter** | swap `cat`-append for `scatter_set` at `(offset+delay)%ctx`; attention reads via a reconstructed logical-position mask | wraparound corrupts position (the Kyutai test vectors) |
| **Exec-mask (masked≠absent)** | substitute init/BOS into masked rows *before* embedding; gate EVERY per-slot mutation through `where(exec_mask,new,old)` | a masked row reads sentinel `-2` → CUDA illegal-memory that **kills the whole batch**; ungated mutation = silent corruption only under multi-tenant load |
| **No in-loop host syncs** | zero `.item()/.cpu()` in the step loop; pre-allocate all buffers (no per-step malloc) | each is a device-wide stall; the 9 ms/step budget rests on this |
| **Graph-safe sampling** | argmax/gumbel in-graph OR multinomial outside the captured region; always-on NaN→reject-frame; fp32 sampler/CFM math | argmax-of-NaN emits a garbage codec token = audible pop, zero error signal |
| **(Full-duplex) `DuplexStepModel`** | K-stream `MultiStreamSlot{(role,delay_sign,ring)}`, per-codebook `StepOutput`, a per-step `TurnHead` | — |

**Per-arch reshape work** = decompose `generate()` into prefill+step, swap KV-append for ring-scatter, MaskedCell discipline, route sampling through the graph-safe path. The config-arch registry ("model = data") already exists; this seam is additive.

### 1b. Custom-kernel-necessity verdict: **ZERO required.**

| Candidate fused kernel | Verdict |
|---|---|
| scatter-into-ring-KV + attention | **plain-ops** (scatter is a tiny `[B,H,1,D]` write; bandwidth-bound; Moshi proves separate scatter+SDPA scales) |
| masked-batched attention | **plain SDPA `attn_mask` path** (the exec-mask is an additive `[-inf,0]` bias = exactly SDPA's `attn_mask`) |
| depth-transformer step (Depformer/MTP) | **torch.compile fuses the linears** (host K-loop, no-KV re-prefill; this is MTP = exact model architecture, **not** spec-decode) |
| **RoPE+QKV fusion** | **REJECTED ON ACCURACY** — fused norms/RoPE lose fp32 precision that compounds across AR steps (vLLM-Omni #2274 / vLLM #42325: diverged 0.03–0.06/layer) |
| delay-pattern reverse | **host index bookkeeping, no kernel** |

The only ever-"justified" kernel is **quantized GPU GEMM (int8/fp8)** — a *throughput-not-latency* lever, **off the frame path**, served by **vendored TensorRT-EP / torchao, never hand-written** — and **excluded by your no-quant constraint anyway.** **Bottom line: 100% of the realtime (batch ≤128, ctx ≤3000) perf is reachable with plain SDPA (right backend) + per-slot ring + CUDA-graph + batching, zero hand-written kernels.** This zero-custom-kernel stance is also what buys WaaV its multi-substrate portability (both vLLM-Omni and SGLang-Omni are CUDA-only *because* they reach for fused CUDA kernels).

---

## 2. Measured GB10 evidence (all accuracy-preserving; p50, bf16)

| # | Exact lever | Measured win | Notes |
|---|---|---|---|
| **1** | **Pin SDPA backend → cuDNN/flash (not math, not FlashInfer)** | **40–135×** | decode math 13.9ms vs cuDNN 0.294ms @128,1024; prefill 60.9ms vs 0.53ms @8,1500. cuDNN ≈ flash, cuDNN marginally best. Built-in, no custom kernel. |
| **2** | **KV-on-device / IoBinding (the #1 engine fix)** | **+13% → 2×** | the stateless ORT seam round-trips the whole KV host↔device per step: +1.38ms@B1/3MB → **+18.3ms@B64/403MB (doubles the step)**. Grows with batch×ctx. |
| **3** | **Lockstep batching amortization** | **idealized 55×@64; REAL ~1.8× peak @ B≈16** | synthetic GEMV decode flat 8.6→9.95ms B1→64 (efficiency 100%≤B32→87%@64→66%@128 ridge) — BUT that omits host-KV feedback. On the REAL chatterbox codec-LM (live GB10, bit-identical to per-slot): 1.12×(B2)/0.99×(B4)/1.39×(B8)/**1.81×(B16 peak)**/1.46×(B32)/**0.95×(B64, regresses)** — host KV re-stream caps it. **Still the headline lever the no-quant constraint leaves intact, but size slots at the real per-graph knee (B≈16), not B=64; 55×@64 needs a device-resident-KV graph.** |
| **4** | **GQA-native KV layout (no MHA-expansion)** | **5.5–6.9× + 7× concurrency** | MHA(14kv) vs GQA(2kv): 6.9× slower attention @64,3000; 1085 vs 155 streams/40GB. Free — use native kv-heads. |
| **5** | **Prefix-KV reuse (R1 radix, ~86% hit)** | **~7× TTFA** | skips 9.2ms (ctx64) → 117ms (B16,ctx256) of prefill, bit-identical (the *same* KV). For cloned-voice/agent workloads. |
| **6** | **CUDA-graph / torch.compile(epilogue_fusion=False)** | **1.18–1.24×@B1, HURTS @B32 (0.73×)** | launch-overhead removal — **edge/low-batch tier ONLY**; eager/compile at high-batch DC. |
| **7** | **Zero-D2H-sync discipline** | **+14% tax if violated (inner loops)** | sequential decode = +1% (one arg-sync cheap); CFM/codec inner loops with N syncs/step = +14% — the real "2400 syncs" case. |
| **8** | **CPU bf16 (fp32-accumulate)** | the *only* exact CPU compute speedup | AMX-BF16 (x86) / **BFMMLA on Grace via MLAS-SBGemm**; never int8. |

The roofline math (no-quant): decode arithmetic-intensity ≈ B; `T_comms` (the weight stream) is **flat in B**; streams are ~free below `B_crit = compute_peak/bandwidth`. Quantization would lower the flat `T_comms` floor; **batching fills the area under that floor for free, and it's the one you're allowed.** Size lockstep slots at the efficiency knee (B≈64 on GB10), not the KV-capacity wall.

---

## 3. Ranked exact-lever catalog

**Engine-level (the big structural wins):**
1. **IoBinding + on-device persistent KV on the `StaticGraph` seam** — the #1 engine change (measured 13%→2×, grows with scale; captures ~8 Path-A AR models bit-identically). Bind in/out `OrtValue` on CUDA, reuse a persistent KV buffer written at `cache_position`.
2. **Lockstep batching** — amortize the weight stream across B slots. The headline throughput lever — but the realistic per-graph magnitude is **~1.8× peak @ B≈16 on the real chatterbox codec-LM (regressing by B=64)**, NOT the idealized 55×@64; the host-KV re-stream of exported graphs caps it (size slots at the measured per-graph knee). 55×@64 is recoverable only by a device-resident-KV re-export. *(live gate `live_headline_batched_scaling_matches_doc_curve`; INFER_PERF_VALIDATION.md §3a.)*
3. **Two-tier KV: radix prefix-cache + ring suffix** — skip ~86% of conditioning prefill exactly (~7× TTFA on reuse); a fixed ring *can't* share prefixes across slots, hence two-tier.
4. **Honor native GQA** — `g/h×` less KV bandwidth (5.5–6.9× measured), `h/g×` more concurrency, free.

**Per-step kernel-level (ranked by GB10 voice ROI; each with its exactness condition):**
1. **CUDA-graph replay** of the fixed-shape step — bit-exact by construction (replay = identical kernels on static buffers); 1.18×@B1, **edge/low-batch only**.
2. **Host-sync elimination** — `dst.copy_(src)` not `fill_(item())`, `torch.where` not tensor-branches; bit-exact; also the prerequisite that *unlocks* CUDA-graph capture.
3. **Persistent / pre-allocated buffers** — bit-exact (memory provenance is irrelevant); prerequisite for graph capture (static addresses).
4. **`torch.compile(epilogue_fusion=False, dynamic=False)`** — fuses linears+SDPA; bit-faithful **iff** the fp32 RMSNorm/RoPE epilogues stay un-fused; 1.24×@B1.
5. **GEMM-dim padding to ×8/×16** (lm_head/MLP/vocab) — zero-pad + mask padded logits to −inf; bit-exact; 215 vs "unstable" TFLOPs measured elsewhere on misaligned shapes.
6. **Fused RMSNorm+residual** — exact **iff** fp32 *accumulate* + **native-dtype** weight-multiply (the #42325 trap: fp32 weight-mul diverges).
7. **SDPA backend pin → cuDNN/flash** — exact (online-softmax ≡ dense softmax); the 40–135× lever (#1 measured).
8. **Fused RoPE (cached cos/sin)** — exact **iff** cos/sin derived in fp32.
9. **Layout/copy cleanup** (drop needless `.contiguous()`/`cat`/transpose-copy) — bit-exact (stride change, not value change).
10. **Fused SiLU/GeGLU-mul** — bit-exact (elementwise, no reduction seam).
11. **Fused QKV pack** — exact **iff** concatenated weights (not vLLM's fused-numerics path); measured A/B + identity test.
12. **Cache CFM/scheduler coeffs + timesteps** (fixed schedule) — bit-exact; removes per-step tiny work.
13. **Pinned-memory + `non_blocking=True`** at the audio I/O boundary (outside the captured step) — bit-exact; overlaps copy with compute.

---

## 4. Per-hardware exact matrix ("every hardware")

### Exact attention kernel (the 40–135× lever) — pin it, never auto-select
| Hardware | AR-decode | prefill/encoder | rule |
|---|---|---|---|
| **GB10 / sm_121** | **cuDNN-SDPA** / FA2 | **cuDNN-SDPA** / FA2 | **NEVER FlashInfer** (3 compounding sm_12x failures + GQA=16 crash); FA3/FA4 don't exist for sm_12x → FA2-class |
| Hopper H200 sm_90 | FA3 split-KV / cuDNN | FA3 / cuDNN | FA3 = 1.5–2× over FA2 |
| MI300X CDNA3 | AITER asm paged-decode / CK-FA | CK-FA varlen / AITER | ROCm-AITER 2.7–4.4× over legacy |
| RTX Ada sm_89 | built-in FA2 / cuDNN | FA2 / cuDNN | FA3 excluded |
| RTX Blackwell sm_120 | built-in FA2 / cuDNN | FA2 / cuDNN | external flash-attn wheel broken on sm_120 |
| CPU x86 + ARM | fused `cpu_flash_attention` (PyTorch / ORT `MlasFlashAttention` / ggml) / math | same | no CUDA-flash, but a real fused online-softmax kernel exists everywhere |

(GQA caveat: split-KV/flash-decoding mostly does NOT fire at voice ctx 64–3000; use `num_splits=0` and let the non-split fused kernel run — the always-paying lever is GQA + a fused kernel that fills the SMs.)

### Graph / backend per (hardware × path)
| Hardware | Path-A (ORT) | Path-B (torch) |
|---|---|---|
| GB10 GPU, **fixed-shape step** | `ORT_ENABLE_ALL` + **IO-binding + persistent KV** (+ optional TensorRT-EP static `min=opt=max` + engine cache) | `compile(reduce-overhead, dynamic=False, fullgraph=True)` OR manual CUDA-graph (sampler outside); `epilogue_fusion=False` + fp32 custom RMSNorm/RoPE |
| GB10 GPU, **variable encoder** | `ORT_ENABLE_ALL` + TensorRT-EP (bucketed optimization-profiles) + engine+timing cache | `compile(max-autotune, dynamic=True)` per length-bucket; **don't** graph large batch (0.72×) |
| **Grace CPU** (compute-bound stage offload) | MLAS SGEMM (exact-fp32) / **MLAS-SBGemm bf16 (exact, fp32-accum)**; ACL fast-math OFF | Inductor-CPP `max-autotune` (NEON/bf16 µkernels); `set_num_threads(72)` + `OMP_PROC_BIND=spread`; NVPL BLAS |
| x86 server CPU | MLAS + **AMX-BF16** (exact-bf16) / oneDNN-EP `avx512_core_amx_bf16` | Inductor-CPP AMX template; oneDNN autocast(bf16)+channels_last |
| NPU/edge (portability) | TensorRT/QNN/CoreML/OpenVINO/DirectML/XNNPACK via the same `StaticGraph`+IO-binding seam (fp16/fp32) | n/a (torch sidecar = CUDA/ROCm/CPU only) |

**Universal free levers:** thread-pin 1/physical-core (Grace = no SMT), NUMA-bind (`numactl`, ≈+20%), ORT spin-tune, lock GPU clocks. **The exactness tripwire across all of it:** a fused matmul epilogue (or weak-typed TRT) silently demoting fp32 RMSNorm/RoPE → fenced by `epilogue_fusion=False` + fp32 custom norm/rotary (Path-B) and **strongly-typed TRT-11** networks (Path-A); leave `kTF32` unset for strict fp32.

---

## 5. Per-onboarded-model exact-perf opportunities (the audit)

The single highest-ROI finding: **the `StaticGraph` ORT seam (`backend-ort/lib.rs:114-200`) has no IoBinding**, so every Path-A AR decoder round-trips its KV host↔device every step (O(N²) total for a growing cache). **One engine change — a stateful/IoBinding `run_bound` path on `StaticGraph`** — fixes them all bit-identically:

> **STATUS (M2-PERF-T1, landed + WIRED + MEASURED): the `run_bound` seam eliminates the per-step constant H2D and is live on the production estimator loop.** `waav-infer-backend-api` declares a backend-agnostic `IoBinding` (pure data, no ort type crosses the seam, the crate stays `#![forbid(unsafe_code)]`) that **splits a stepped run's inputs into loop-invariant `constants` (uploaded once) + per-step `inputs` (varying)** plus a `device_outputs` residency set and a constants `epoch`. `StaticGraph::run_bound` defaults to `run` (host-materialized merge — correct on CPU/NPU). `waav-infer-backend-ort` overrides it with a **persistent `ort::IoBinding` held inside `OrtModel`, keyed on the epoch**: the constants are `bind_input`-copied to the device EXACTLY ONCE per utterance (ort: "queued to be copied ... used in all future invocations until overridden") and reused across every step; only the varying inputs are re-bound per call; outputs bind host-accessible (`CUDA_PINNED`/`HIP_PINNED`). The input-build/extract helpers are shared with `run`, so `run_bound` is **bit-identical**. The win is now **measured, not prose**: a deterministic `h2d_input_bytes` counter shows the 8-step estimator loop copies **86.2% fewer input bytes** (7.22× reduction: 8320→1152 B; the eliminated bytes == `(n_steps−1)·const`), and the GB10 CUDA EP wall-clock is **1.18–1.29× faster** over a 200-step loop with a Supertonic-scale constant. The seam is **wired into the production hot loop**: `supertonic::flow_solve` drives `vector_estimator` through `run_bound` with the 5 CFM constants (`text_emb`/`style_ttl`/`latent_mask`/`text_mask`/`total_step`) declared once per `synth_epoch` and only `noisy_latent`+`current_step` per step (a `flow_solve_drives_run_bound_with_constants` gate proves `run` is never used and the trajectory is bit-identical to the old `run` loop). Gates: backend `run_bound_eliminates_constant_h2d_on_estimator_loop` (byte gate) + `run_bound_rebinds_constants_on_epoch_change` (epoch-keyed, no stale-constant bleed) + `run_bound_estimator_loop_faster_on_cuda` (live GB10 wall-clock) + the CPU bit-identity / ≥4-concurrent-no-crosstalk gates; the standing AR-compounding identity gate stays green. Follow-on still open: fully device-resident persistent-KV retention across steps (the `cache_position`-written buffer / O(N²)→O(N) for growing-cache AR decoders) — `device_outputs` is the declarative hook in place for it. The per-model wins below are now unblocked on this seam:


- **Path-A AR (IoBinding + persistent KV):** qwen3_asr (worst — growing KV per step), funasr_nano, whisper/moonshine/canary/cohere (enc-dec: encoder output + encoder-KV re-uploaded every token), voxtral, nemotron (streaming caches), supertonic (the 4 constant CFM tensors re-uploaded per step).
- **Path-B torch (HF `StaticCache` + `torch.compile(dynamic=False)` — bit-identical):** dia, dia2, csm, dots_tts, vibevoice, neutts_air, arkasr, granite, qwen3_tts (all use the default *dynamic* cache → no graph capture today). **dia2 already ships `use_cuda_graph`/`use_torch_compile` flags — just turn them on.**
- **Path-B ORT estimators (IoBinding):** cosyvoice3 (`.cpu().numpy()` per CFM step → keep `x` on GPU via IOBinding/dlpack), higgs_tts (RTF 18.8 — KV host round-trip per frame).
- **omnivoice:** batch the CFG cond+uncond pair into one `[2,…]` forward (exact — CFG is linear in the two logit sets) → halves launches.
- **CLEAN (no opportunity):** kokoro, melo, sensevoice, nemo_ctc (single-shot, non-autoregressive).

---

## 6. The accuracy gate (every lever passes this, or it's rejected)

1. **Per-op tolerance.** `max|fast − unfused-eager-ref|` at native dtype ≤ the op's own fp32-vs-native rounding noise. Elementwise fusions ≈ machine-eps; fused-reduction kernels (RMSNorm/RoPE) **must keep the fp32 reduction** or they fail by 0.03–0.06/layer.
2. **The AR-compounding identity test (the #2274 gate, non-negotiable).** Run the full N-step AR loop and compare the **emitted integer codes** vs the eager reference — they must be **IDENTICAL**, not just close. This catches the precision loss that's invisible per-op but audible after compounding. (Mirror via mel/waveform `allclose` for CFM/diffusion, WER-disagreement for STT — the existing native-parity bar.)
3. **Concurrency gate.** Re-run at `max_num_seqs≥4`, 4+ parallel prompts; each output bit-identical to its serial run (catches the shared-buffer crosstalk that offline RTF misses).
4. **Measurement hygiene.** Baseline with profiler OFF; CUDA events; ≥3 warmup; report p50 **and** p99 (coordinated-omission-corrected); one variable per A/B.

---

## 7. Implementation alignment (milestone mapping)

| Lever | Where | Milestone |
|---|---|---|
| Lockstep batching + per-slot ring KV + CUDA-graph(batch-tiered) | the core M2 seam | M2 |
| **IoBinding + on-device persistent KV on `StaticGraph`** | `backend-ort` (the #1 engine change) | M2 (new seam method `run_bound`) |
| SDPA-backend pin (cuDNN/flash; never FlashInfer on sm_12x) | the attention kernel-selection table | M2/M4 (HAL §2 + StagePlacer) |
| Honor native GQA KV layout | the ring-KV layout | M2.3 |
| Two-tier KV (radix prefix + ring suffix) | R1 | M3 |
| Zero-D2H-sync + persistent buffers + fp32-fusion-safe compile | the sidecar/runner discipline | M2/M3 (gates `zero_d2h_sync_during_decode`, `fp32_reduction_survives_fusion`) |
| Path-B HF StaticCache + torch.compile; flip dia2 flags; IoBinding cosyvoice3/higgs | per-model runners | M2 (per-model) |
| CPU bf16 (MLAS-SBGemm / AMX) + thread-pin/NUMA | the CPU/edge tier | M4.x (heterogeneous placement) |
| TensorRT-EP static/bucketed engines + cache for the encoder | Path-A variable stages | M4.x |

**The single biggest engine change** is adding the IoBinding/persistent-KV path to the `StaticGraph` seam — it's measured at 13%→2×, grows with the batching that is itself the headline lever, captures ~8 models with zero output change, and needs an accuracy gate that already exists (the native-parity bar). **Everything in this document is exact** — verified by the AR-compounding identity test — and **needs zero hand-written kernels.**
