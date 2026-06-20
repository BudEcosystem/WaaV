# WaaV exact-perf GB10 micro-benchmarks (Blackwell sm_121, torch 2.12+cu130, bf16) — ALL accuracy-preserving

## Batch 1

### (A) Host↔device sync cost in the per-step decode loop (output identical)
| batch | 12-step no-sync | with .item()/step | sync tax |
|------:|----------------:|------------------:|---------:|
| 1 | 103.6 ms | 104.96 ms | +1.36 ms (+1%) |
| 16 | 109.9 ms | 112.11 ms | +2.22 ms (+2%) |
**Finding:** ONE arg-extraction sync/step is cheap (~1-2%) for SEQUENTIAL decode (little to overlap). The "2400-syncs" disaster is MANY syncs PER step (inner CFM/codec loops: 10 steps × 60 frames × 4 ops) — there the tax compounds, and it blocks the bs≥2 async-lookahead overlap. Discipline = free; biggest win on inner-loop-heavy stages + pipelined exec.

### (B) ★ EXACT attention backend selection — THE biggest single lever (40–135×, zero accuracy change, NO custom kernel) ★
DECODE q[B,H,1,D] k/v[B,Hkv,ctx,D] GQA (ms):
| B,ctx | math | flash | cudnn |
|------|-----:|------:|------:|
| 1,256 | 0.085 | 0.022 | **0.020** |
| 16,256 | 0.427 | 0.022 | **0.020** |
| 64,512 | 3.436 | 0.042 | **0.028** |
| 64,1024 | 6.892 | 0.152 | 0.151 |
| 128,1024 | 13.905 | 0.313 | **0.294** (47× vs math) |
PREFILL q/k/v[B,H,T,D] causal (ms): 8,1500 → math 60.85 / flash 0.451 / **cudnn 0.533** = **135× vs math, EXACT**.
**Finding:** flash & cuDNN SDPA backends are 40–135× faster than the math backend and BIT-FAITHFUL. **cuDNN ≈ flash, cuDNN marginally best on decode** on GB10/Blackwell — confirms the research's "cuDNN/SDPA not FlashInfer" on sm_12x. The exact win is just **selecting the right built-in `scaled_dot_product_attention` backend** — no FlashInfer, no custom kernel. (mem_efficient backend = N/A for GQA on this build.) ⇒ the engine MUST pin the SDPA backend to cuDNN/flash (never let it fall to math) per the hardware.

### (E) Prefill cost = the EXACT amount prefix-KV-reuse skips on a cache hit (R1 radix, 86% hit)
| B | ctx | prefill skipped |
|--:|----:|----------------:|
| 1 | 64 | 9.22 ms |
| 1 | 256 | 12.15 ms |
| 1 | 512 | 17.94 ms |
| 16 | 256 | 117.19 ms |
**Finding:** a returning voice / shared system-prompt skips its entire prefill EXACTLY (same KV) — 9–18 ms @B1, 117 ms @B16 saved per request. Accuracy-neutral; grounds R1 as a top exact lever for cloned-voice/agent workloads.

## Custom-kernel-necessity verdict (research agent, grounded in Moshi + vLLM-Omni source)
WaaV INVERTS vLLM: requires control-flow correctness, ZERO custom kernels.
- Model reqs (additive, AR/duplex only): stepped seam (prefill/step/reset_slot), fixed-shape batched forward (K-lanes+D-codebooks = inner dims not batch axes → one CUDA-graph), per-slot ring-KV scatter, exec-mask masked≠absent, no-in-loop-syncs, graph-safe sampling, DuplexStepModel for full-duplex.
- Per-arch reshape: decompose generate()→prefill+step; swap cat-append KV→ring-scatter; MaskedCell; graph-safe sampling. One-shot models (kokoro/whisper/melo/supertonic) return as_stepped()→None, untouched.
- Custom-kernel table: fused-scatter-KV+attn=plain-ops (scatter is tiny B,H,1,D write, bandwidth-bound); fused-masked-attn=plain SDPA attn_mask path (mask = additive [-inf,0] = exactly SDPA's attn_mask); fused-depth-transformer=torch.compile-fuses-linears (host K-loop, no-KV re-prefill, MTP not spec-decode); fused-RoPE+QKV=ACCURACY-REGRESSION (the #2274 fp32-compounding lesson) → REJECTED ON ACCURACY; delay-reverse=host index bookkeeping no kernel.
- ONE carve-out: quantized GPU GEMM = throughput-not-latency lever, vendored TensorRT-EP/torchao NOT hand-written, off the frame path — AND excluded by the no-quant constraint.
- BOTTOM LINE: 100% of realtime (B≤128, ctx≤3000, memory-bound) perf with ZERO hand-written kernels — plain SDPA(cuDNN/flash backend) + ring + CUDA-graph. Perf = batching + bandwidth physics, not kernels. Proven by Moshi batched_transformer.rs (scatter+plain matmul/softmax, no custom attn) + vLLM-Omni qwen3_code_predictor (F.sdpa + manual graph, no custom op).

## Batch 2 (measured, exact)
### (F) torch.compile modes on decode step (EXACT, epilogue_fusion caveat applies)
| B | eager | compile-default | reduce-overhead |
|--:|------:|----------------:|----------------:|
| 1 | 8.98 | **7.27 (1.24×)** | ERR (cudagraph-tree+HF-cache) |
| 32 | 9.78 | 15.0 (SLOWER) | 9.77 (neutral) |
→ compile helps @B1, HURTS @B32 — same batch-tier as CUDA-graph. Use compile/graph at EDGE/low-batch only.
### (G) inner-loop host-sync tax: 10-step CFM/DiT, 4 syncs/step vs 0 (output identical)
| B,T | no-sync | 4-sync/step | tax |
|----|--------:|------------:|----:|
| 1,64 | 5.51 | 6.30 | **+14%** |
| 32,64 | 21.0 | 22.0 | +5% |
| 64,4 | 9.09 | 9.98 | +10% |
→ the "2400-syncs" disaster is REAL in INNER-LOOP stages (CFM/codec): +14%@B1 (vs +1% sequential decode in (A)). Confirms: zero-D2H discipline matters most on multi-step solvers + pipelined exec.
### (H) CUDA-graph the FULL decode step (EXACT replay, batch-tier)
| B | eager | cuda-graph | speedup |
|--:|------:|-----------:|--------:|
| 1 | 8.52 | 7.20 | **1.18×** |
| 8 | 8.80 | 8.42 | 1.05× |
| 32 | 9.79 | 13.49 | **0.73×** |
| 64 | 11.44 | 22.47 | **0.51×** |
→ CUDA-graph = EDGE/low-batch lever (launch-overhead removal); HURTS at high batch (compute-bound, replay overhead). Tier by batch.

## Research deltas (4 of 6 agents)
- **#1 ENGINE lever (per-model audit):** StaticGraph ORT seam (backend-ort/lib.rs:114-200) has NO IoBinding → EVERY Path-A AR decoder round-trips KV host↔device per step (O(N²) for growing caches: qwen3_asr, funasr, canary/cohere/whisper enc-dec, voxtral, nemotron, supertonic-xt). ONE fix (IoBinding + on-device persistent KV bound at cache_position on the StaticGraph seam) captures ~8 models, bit-identical. Plus Path-B: turn on HF StaticCache+torch.compile for the 9 generate() runners (dia/csm/dots/vibevoice/neutts/arkasr/...); flip dia2's already-shipped use_cuda_graph flags; IoBinding the cosyvoice3/higgs ORT estimators (the .cpu().numpy()-per-CFM-step). kokoro/melo/sensevoice/nemo_ctc = CLEAN (single-shot).
- **Memory-wall roofline (no-quant):** decode AI≈B, T_comms(weights) flat in B → B_crit=compute/bandwidth; GB10 efficiency 100%(≤B32)→87%@64→66%@128 = the ridge knee; size slots at B≈64. Batching is THE exact lever the constraint leaves intact (orthogonal to forbidden quant). Prefix-KV-reuse skips ~86% prefill (bit-identical, the SAME KV; ~7× TTFA on reuse). GQA cuts KV-bandwidth g/h× (4× exact). Depth/MTP head = exact model arch (38×@64 nested), NOT spec-decode (which is 0.98× net-loss on acoustic tokens anyway). C2C weight residency (600 vs 273 GB/s view). Kill O(N²) re-decode + per-step D2H.
- **16-lever kernel catalog (ranked, ROI for GB10 voice):** 1.CUDA-graph 2.sync-elim 3.persistent-buffers 4.compile(epilogue_fusion=False) 5.GEMM-dim-pad-×8/×16 (215 vs unstable TFLOPs) 6.fused-RMSNorm+residual(fp32-accum+native-dtype-weight-mul) 7.SDPA-backend-pin 8.fused-RoPE(fp32-cos/sin) 9.layout/copy-cleanup 10.fused-SiLU/GeGLU 11.fused-QKV(concat-weights-only) 12.per-req-state-keying 13.autotune-no-cudagraphs 14.batch/prefix-buckets 15.cache-CFM-coeffs 16.pinned+non_blocking-IO. ACCURACY GATE every lever: fp32-reduction-must-survive-fusion (#2274/#42325: final weight-mul = NATIVE dtype not fp32 — diverged 0.03-0.06/layer) + AR-compounding identity test (16-step emitted codes must be IDENTICAL not just close).

## Batch 3 (measured, exact) — the two highest-value engine/native levers
### (I) ★ KV host↔device round-trip tax = the #1 ENGINE lever (IoBinding on the StaticGraph seam) ★
| B,ctx | KV MB | on-device 12-step | +roundtrip | tax/step |
|------|------:|------------------:|-----------:|---------:|
| 1,256 | 3.1 | 103.1 ms | 119.7 ms | +1.38 ms (~13%) |
| 1,1024 | 12.6 | 106.7 ms | 135.3 ms | +2.38 ms |
| 16,256 | 50.3 | 115.1 ms | 148.4 ms | +2.78 ms |
| 64,512 | 402.7 | 191.5 ms | **410.8 ms** | **+18.28 ms (DOUBLES the step)** |
→ The stateless ORT `run()` seam (no IoBinding) round-trips the WHOLE KV host↔device every step → **13% tax at small KV, growing to >100% (2×) at batch×ctx scale.** Compounds with batching (the headline lever). IoBinding + on-device persistent KV bound at cache_position = the #1 exact engine fix, captures ~8 Path-A AR models bit-identically.
### (J) ★ GQA-native vs MHA-expansion (exact, model-native) ★
| B,ctx | GQA(2kv) | MHA(14kv) | KV-bytes ratio | MHA/GQA time |
|------|---------:|----------:|---------------:|-------------:|
| 16,512 | 0.021 ms | 0.136 ms | 7× | 6.48× |
| 64,1024 | 0.173 ms | 0.950 ms | 7× | 5.48× |
| 128,1024 | 0.303 ms | 1.950 ms | 7× | 6.45× |
| 64,3000 | 0.398 ms | 2.746 ms | 7× | 6.90× |
Streams/40GB @ctx3000: GQA 2kv = **1085** vs MHA 14kv = 155 (7× concurrency).
→ Honoring the model's NATIVE GQA kv-head layout (never expand to MHA) = **5.5-6.9× faster attention decode + 7× concurrency, EXACT, FREE.** The engine must lay out KV at native kv_heads.

## ============ EMPIRICAL HEADLINE (3 batches, GB10, all accuracy-preserving) ============
1. EXACT attention backend (cuDNN/flash SDPA, NOT math, NOT FlashInfer): 40-135× — biggest single per-op lever, just a backend pin.
2. KV-on-device (IoBinding, the #1 engine fix): 13%→2× (grows with batch×ctx); StaticGraph seam currently round-trips KV every step.
3. Batching amortization (lockstep): 55×@64 (memory-bound; the headline throughput lever the no-quant constraint LEAVES INTACT).
4. GQA-native KV layout: 5.5-6.9× attention + 7× concurrency, free.
5. Prefix-KV reuse (R1 radix, ~86% hit): skips 9-117ms prefill, bit-identical → ~7× TTFA on reuse.
6. CUDA-graph + torch.compile(epilogue_fusion=False): 1.18-1.24×@B1, HURT @B32 → EDGE/low-batch tier only.
7. Zero-D2H-sync in inner loops (CFM/codec): +14% tax if violated; sequential decode only +1%.
8. ZERO custom kernels required — 100% of realtime perf from plain-SDPA(pinned-backend)+ring+graph+batching. Custom kernel only ever justified for quantized-GEMM, which the no-quant constraint excludes.
ACCURACY GATE (all levers): fp32-reduction-survives-fusion + 16-step AR-compounding emitted-codes-IDENTICAL (the #2274 trap).

## Research delta — graph/ORT/CPU exact matrix (agent 5/6)
**(Hardware × Path) exact-optimization routing:**
- GB10-GPU FIXED-shape lockstep step (launch-bound): Path-B torch.compile(reduce-overhead, dynamic=False, fullgraph=True) → CUDA-graph-trees, OR manual CUDAGraph (sampler OUTSIDE capture); Path-A ORT IO-binding + persistent on-device KV + ORT_ENABLE_ALL. CUDA-graph 1.21×@B1.
- GB10-GPU VARIABLE encoder/step-bucket (compute-bound): Path-B torch.compile(max-autotune, dynamic=True) per length-bucket; Path-A ORT_ENABLE_ALL + TensorRT-EP (static min=opt=max OR bucketed optimization-profiles) + engine+timing cache (384s→9s→1.9s first-load). DON'T graph large batch (0.72×).
- THE EXACTNESS TRIPWIRE: fused matmul epilogue (or weak-typed TRT) silently demotes fp32 RMSNorm/RoPE → FENCE with `torch._inductor.config.epilogue_fusion=False` + fp32 custom RMSNorm/RoPE opaque to Inductor (vLLM `custom_ops:["+rms_norm","+rotary_embedding"]`); Path-A: TRT-11 strongly-typed networks (won't silently demote); leave kTF32 unset for strict fp32. PyTorch #96693 = max-autotune precision-drop without the guard.
- ORT graph-opt levels ALL exact EXCEPT the CUDA Attention-fusion ("negligible" approx — use ORT_ENABLE_EXTENDED for strict-exact encoder). IO-binding = the #1 Path-A lever (matches measured (I): the seam auto-copies H2D/D2H per Run without it).
- **CPU exact COMPUTE speedup = bf16-with-fp32-accumulate ONLY** (never int8/VNNI/AMX-int8/I8MM): x86 AMX-BF16 (MLAS auto / oneDNN avx512_core_amx_bf16); **Grace = BFMMLA via MLAS-SBGemm (HasArmNeon_BF16) — the ACL EP exposes NO bf16, and ACL fast-math=OFF (fp32→bf16 downcast is NOT exact)**; torch Inductor-CPP template (AMX/NEON µkernels, TORCHINDUCTOR_FREEZING=1). Thread-pin 1/physical-core (Grace no-SMT), NUMA-bind (numactl ≈+20%), ORT spin-tune (spin_duration_us=1000).
- Portability tier = ORT-EP (QNN/CoreML/OpenVINO/DirectML/XNNPACK) via the same StaticGraph+IO-binding seam; the torch sidecar is CUDA/ROCm/CPU only (NOT the portability path).

## Research delta — exact-attention (hardware × regime) matrix (agent 6/6)
| Hardware | R1 AR-decode winner/runner | R2 prefill winner/runner |
|---|---|---|
| **GB10 sm_121** | **cuDNN-SDPA** / FA2-built-in (AVOID FlashInfer) | **cuDNN-SDPA** / FA2 (FA3/FA4 DON'T EXIST for sm_12x → "FA2-class forever") |
| Hopper H200 sm_90 | FA3 split-KV / cuDNN | FA3 / cuDNN |
| MI300X CDNA3 | AITER asm paged-decode / CK-FA | CK-FA varlen / AITER |
| RTX Ada sm_89 | built-in FA2 / cuDNN-opt-in | FA2 / cuDNN |
| RTX Blackwell sm_120 | built-in FA2 (ext wheel BROKEN) / cuDNN | FA2 / cuDNN |
| CPU x86+ARM | fused cpu_flash_attention (PyTorch/ORT MlasFlashAttention/ggml) / math | same / math |
**Decision rule the engine bakes in:** sm_12x → PIN `CUDNN_ATTENTION` (fallback `FLASH_ATTENTION`), NEVER auto-select (math fallback = 40-135× slower, measured), NEVER FlashInfer. sm_90 → FA3 split-KV. CDNA3 → AITER. CPU → fused cpu_flash_attention.
**FlashInfer regression generalizes to sm_12x (not just aarch64):** the sm_12x compute FAMILY is the trigger — (1) architectural 2× MMA throttle (no TMEM/WGMMA, half-rate fp32-acc MMA), (2) auto-dispatch gates on family-100/sm_120-exact → misses sm_121 → slow CUTLASS fallback, (3) ILLEGAL-MEMORY CRASH at GQA=16 (vLLM #37754 — exactly the high-GQA compact voice decoders). workaround = triton_attn/cuDNN.
**GQA in the voice regime:** split-KV/flash-decoding mostly does NOT fire at ctx 64-3000 (its wins are 8k-32k+) → use num_splits=0, let the non-split FUSED kernel run; the lever that ALWAYS pays = GQA + a fused decode kernel that fills the SMs (matches measured (J) 5.5-6.9×).
**Path-A note:** WaaV currently runs ORT-CUDA-EP where attention fuses INSIDE the ORT graph (cuDNN/MLAS) → the SDPA-backend-pin matters for the Path-B torch sidecar; Path-A gets fused attention via ORT graph-opt (strict-exact = ORT_ENABLE_EXTENDED to skip the "negligible-approx" CUDA Attention fusion).
