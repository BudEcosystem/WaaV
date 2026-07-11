# dia2 / Path-B Fleet — PERF-ACCELERATION Plan

**Date:** 2026-06-26 · **Host:** GB10 (Grace-Blackwell **sm_121**, aarch64, 121 GB unified pool) · **libtorch:** tch / PyTorch 2.12+cu130
**Scope:** dia2 (2B codec-AR, CFG, 31-stage depformer) + the tch Path-B fleet (csm/qwen3-tts/voxtral/cohere/dia/neutts/higgs).
**Hard constraint (non-negotiable):** the DEFAULT serve path (`PerfMode::Accuracy`) MUST stay **byte-identical (max|Δ|=0 on decoded codes)**. Any numerics-trading lever lives ONLY in opt-in `PerfMode::Throughput`. The hardware-abstraction + auto-selection seam (`AccelMapper::select_perf`, `DeviceCaps`, `AccelBackend`) must be preserved — no hardcoded CUDA-only.

This plan corrects the task brief's central hypothesis. The brief assumed dia2 is slow because of **manual f32-softmax attention** and proposed **FlashAttention / TRT-FP16** as the fix. **The live GB10 profile refutes both.** dia2 is **launch-bound**, not compute- or attention-bound; the recoverable time is dominated by kernel-launch / inter-kernel-gap removal, which is **byte-identical** (CUDA-graph replay re-runs the identical kernels). Flash/TRT target attention (~1.5% of GPU-busy) and would have near-zero ROL while breaking the bf16-tail byte-identity the manual path exists to protect.

---

## 1. The dia2 RTF problem — root cause + bottleneck breakdown

### 1.1 The number

Serve-path dia2 (the default-on Path-B grouped-ring batcher) measures **RTF ~3.2–3.46, flat across B=1→16, 0 shed** (`PATHB-FLEET-PERF.md` row 4: B1 3.199, B4 3.442, B8 3.462, B16 3.440). It is **depth-bound**: batching adds streams at constant per-stream cost but never crosses RTF<1, because each single stream is already slower than realtime. The solo `generate_codes` path (same structure, no ring overhead) measures RTF(AR)=2.08 eager.

### 1.2 Root cause — LAUNCH-BOUND eager dispatch (one number)

Live GB10 profile (2026-06-26, `dia2_rtf_profile.rs`, 103 frames / 6.88 s audio, best-of-3):

- **GPU occupancy 39.2% ⇒ 60.8% IDLE GAP.** 4.13M kernel launches, avg 9.6 µs, ~40.8k launches/s, ~2400–4000 launches **per 12.5 ms audio frame**.
- The DEFAULT serve path is **pure EAGER**: CUDA-graph is opt-in / default-OFF (`WAAV_DIA2_CUDA_GRAPH`, `dia2.rs:1069,1132`). The model fires ~150 transformer-layer-steps + 33 samplers per frame as individual eager launches; the GPU starves between them.
- nsys by kernel class of GPU-busy: **cutlass tensorop GEMM 61.7%** (267k launches — the tiny B=2/seq=1 projections), gemvx 4.4%, **elementwise/copy/cat 25.3%** (2.5M launches — f32↔bf16 casts + ring-copy + cat + reshape), layernorm 1.3%, **attention (memeff-fmha + softmax_warp) ~1.5%**.

At B=2 / seq=1 every kernel is trivially small, so wall time is **CPU-side dispatch + inter-kernel gaps, not GPU math**. Web-corroborated: CUDA graphs eat/recover 20–30% of step time at small-batch decode (pytorch.org/blog; CUDA-graphs deep-dives).

### 1.3 Component breakdown (EAGER default; AR-loop wall 14299.8 ms / 103 frames)

| Component | Share of AR wall | ms/frame | File:line | Note |
|---|---|---|---|---|
| **Depformer** (31 stages × 4 layers × CFG B=2) | **~50%** (50.4% eager / 44% graph-on) | 70.0 (2.26/stage) | `dia2.rs:1469` (31-stage loop), `:553-602` step | THE single largest cost. 31 sequential depth stages/frame, each chained on the prior sampled token by a host→device upload (`dia2.rs:1470` `Tensor::from_slice(&vec![prev;branches]).to(dev)`) so stages CANNOT pipeline. Dominated by per-stage launch overhead. CUDA-graph cut it 70.0→49.6 ms/frame (**−29%**, the biggest graph win). |
| **Backbone** (28 layers × CFG B=2) | **~46%** (45.8% / 50.7%) | 63.6 (2.27/layer) | `dia2.rs:393` `forward_graph` / `:419` `forward_ring_grouped`, `:286` build_backbone_layer | 2B transformer, B=2 (CFG cond+uncond batched in ONE forward), seq=1 decode. Also launch-bound; CUDA-graph 63.6→57.0 ms/frame (**−10%**). |
| **CFG 2× doubling** | NOT separable (the B=2 axis folded into backbone+depformer) | — | `dia2.rs:1436/1454/1473` `classifier_guidance`, `CFG_SCALE=2.0` `:133` | cond+uncond as a 2-row batch through the same kernels; at B=2/seq=1 kernels stay launch-bound, so its marginal WALL cost is **well under 2×**. Removing it changes output (NOT a free lever). |
| **CFG-lerp + sampling** (33 calls/frame) | ~3.4% | 4.73 (0.143/call × 33) | `dia2.rs:744-760` (host read-back) | 1 text + 1 cb0 + 31 depformer `sample_token`. Each softmax→top-k→multinomial→`int64_value()` D2H. 33 D2H syncs/frame, but modest. |
| **GEMMs (proj+MLP)** — substance INSIDE backbone+depformer | ~66% of GPU-BUSY | — | (projections in the two rows above) | s1688 TF32 tensorop GEMM is the top kernel (46% of GPU time) spread over 245k launches @ ~64–75 µs — tiny B=2/seq=1 matmuls, **launch+small-GEMM-inefficiency bound, NOT compute-bound.** Already TF32-accelerated. |
| **Elementwise / casts / ring-copy / cat / reshape** | ~25% of GPU-BUSY (2.5M launches) | — | `dia2.rs:297-302,436-470` (f32-sandwich casts), KV ring index_copy/cat | The f32↔bf16 byte-identity "sandwich" + RmsNorm pieces + ring ops. **Pure overhead tax, fully byte-identical-recoverable** by graph + fusion. |
| **Attention** (manual f32-softmax SDPA) | **~1.5% of GPU-BUSY** | — | `self_attention.rs:310-316` `sdpa_*` | **REFUTES the brief.** At B=2, ctx≤1500, attention is negligible. Flash would recover ~nothing ⇒ essentially ZERO flashable headroom. |
| **Codec (Mimi decode)** | ~0.1% | 12.4 ms one-shot | `dia2.rs:1522` | Not autoregressive, runs once at the end. A non-issue for RTF. |

**Note on the brief's framing:** dia2's backbone does NOT use `sdpa_manual`. It uses the FUSED `scaled_dot_product_attention` steered to the libtorch **MATH** backend by an explicit `finfo.min` mask (`Kernel::FusedMaskedGqa`, `self_attention.rs:319-344`; TF32-on `dia2.rs:299-313`). The hand-written `sdpa_manual`/`sdpa_gqa_manual` f32-family is used by the ASR towers (voxtral/cohere/ark), not dia2. Either way attention is ~1.5% — the framing detail does not change the conclusion.

### 1.4 Ranked bottlenecks

1. **#1 LAUNCH-BOUND eager dispatch (root cause)** — 60.8% GPU-idle, 4.13M tiny launches, default pure-eager.
2. **#2 Depformer 31-stage serial depth chain** (~50%, 70 ms/frame) — biggest CUDA-graph win (−29%); serialized by the per-stage host→dev prev-token upload.
3. **#3 Backbone 28-layer step** (~46%, 63.6 ms/frame) — launch-bound, TF32 GEMMs tiny.
4. **#4 CFG 2× as a batch axis** — folds into #2/#3; removing it changes output (not free).
5. **NON-bottlenecks (measured):** attention ~1.5%, codec 0.1%, sampling ~3.4%, prev-token upload 0.2% of wall (but it is the SERIALIZER preventing depformer pipelining).

---

## 2. BYTE-IDENTICAL levers (default `PerfMode::Accuracy`, max|Δ|=0) — ranked by impact

These are the correct levers for the dia2 RTF problem because the recoverable time is launch/gap removal, which a CUDA-graph replay reproduces bit-for-bit. **Measured proof:** flipping the already-shipped opt-in CUDA-graph (byte-identity gated by `cuda_torch_dia2_graph_ab.rs`, 1188 AB-identical sample calls; 608/608 CUDA-bf16 + 544/544 CPU unchanged) cut AR-loop wall 14299→11577 ms (**−19%**), RTF(AR) 2.08→1.68, depformer 70.0→49.6 (−29%), backbone 63.6→57.0 (−10%) — **same 103 frames, byte-identical codes.**

### B1 — CUDA-graph the DEFAULT on CUDA + wire it into auto-selection (HIGHEST ROI, do FIRST)

- **What:** the graph capture for BOTH the 28-layer backbone (`Backbone::forward_graph`, `dia2.rs:393`) and ALL 31 depformer stages (`Depformer::step_graph`/`StageGraph`, `dia2.rs:503-619`) already exists, byte-identical, but is DEFAULT-OFF behind `WAAV_DIA2_CUDA_GRAPH` (`dia2.rs:1069,1132`) and is read ONLY inside dia2.rs — `engine.rs`/`select_perf` never set it.
- **Gain (measured, byte-identical):** ×1.18–1.22 solo (RTF 2.08→1.68 / 1.91→1.57). The 19-frame gate sees only ×1.04 because warmup amortizes less — so auto-enable should be **on-but-measured**, never assumed on short utterances.
- **Two gaps to close (real work, both byte-identical):**
  - **GAP-A (the production blocker):** the graph win today is on the SOLO `generate_codes` path. The DEFAULT serve path is the Fork-A1 ragged ring (`step_ring`→`forward_ring_grouped`, `dia2.rs:410-424`) documented **eager-only**, so the RTF ~3.4 serve number gets ZERO graph win. **Batching and graphing are currently mutually exclusive.** Graphing the ring is the bigger latent win but real new work: the grouped ragged ring has dynamic per-slot/per-group membership, so capture needs **per-group-width bucketed capture** (one `gpu_graph_id` per cohort width — the same bucketing the KV-ACCEL plan §7.3 specifies for ORT). `StageGraph` binds fixed `KvCache` addresses (`dia2.rs:1359,1376` invalidate-on-regen), so a bucketed ring graph must pin per-slot ring addresses.
  - **GAP-B (auto-selection):** `select_perf` (`lib.rs:3169`) returns `EAGER_FLOOR` for any `graphable==true` model in BOTH modes. dia2 is graphable so it never auto-enables the graph. See §4 for the `ByteIdenticalGraph` `AccelBackend` that fixes this without weakening the TRT exclusion.
- **Risks:** capture binds cache tensor ADDRESSES — `reset_graph` must fire every generation (it does, `dia2.rs:1359/1376`); gate any wiring change with the 1188-call AB. Per-generation re-capture cost hurts SHORT utterances. A profiler teardown SIGSEGV was observed in graph mode — **fix the graph-mode teardown before default-on.**

### B2 — Cast / copy / cat / reshape fusion (the ~25%-of-GPU-busy tax)

- **What:** the f32↔bf16 "f32-sandwich" casts (`dia2.rs:297-302,436-470`) + RmsNorm pieces + KV ring index_copy/cat + reshapes are 2.5M launches / ~25% of GPU-busy — pure overhead, **byte-identical-recoverable** by graph + fusion.
- **How (byte-identical only):** (a) CUDA-graph (B1) already fuses these into the replay — most of the 25% is recovered FREE by B1. (b) On top, hand-fuse the cast epilogue or `torch.compile(epilogue_fusion=False)` **keeping fp32 RMSNorm/RoPE** per `INFER_PERF §6`. **Constraint:** `INFER_PERF §1b` REJECTED RoPE/QKV fusion on accuracy (the omnivoice GQA-fold and misotts enable_gqa scars: ~6e-5 / 0.016 reassociation flips a codebook over 28 layers). So fusion is admissible ONLY where it provably preserves the f32 reduction order — i.e. cast-only / copy-elision fusion, NOT GEMM/norm reassociation. Each fused op needs its own RED Δ==0 codes gate.
- **Gain:** modest beyond B1 (B1 already captures most of it in replay); a clean hand-fused cast might add ~5% but only after a bit-gate proves it. **Lower priority than B1.**

### B3 — Depformer pipelining / de-serialize the prev-token upload (STRUCTURAL, byte-identical)

- **What:** the 31 stages can't overlap because each chains on the prior sampled token via a host→device upload (`dia2.rs:1470`). Keeping that upload OFF the per-stage critical path (e.g. device-side gather of the sampled id, no D2H round-trip per stage) lets the stages pipeline. This is byte-identical (same arithmetic, different scheduling).
- **Gain:** this is the lever most likely to push RTF<1 at B=1 (the depformer is ~50% of wall and the most serialized). But it is genuine structural work and the hardest of the byte-identical levers.
- **Honest target:** B1 (graph default) + B2 (cast fusion) pushes RTF from ~3.4 (serve) / 2.08 (solo) toward **~1.3–1.6**; reaching **RTF<1 at B=1 likely also needs B3** (depformer pipelining), still byte-identical.

### B4 — Byte-identical FlashAttention — NOT achievable on dia2 (rejected with reason)

- A truly byte-identical flash via "f32 accumulate / math backend" is a **contradiction**: forcing f32 accumulation gives you the math kernel back — which IS dia2's current path. cuDNN/efficient flash kernels use online-softmax with a **different (block-streaming) reduction order** by construction; in dia2's bf16 tail this rounds ~1 ULP differently, flips a sampled token, and the CFG top-k + multinomial compounds it (`dia2.rs:27-28` scar). So byte-identical flash on dia2 is impossible, AND attention is only 1.5% so even a lossy flash recovers nothing. **Reject.** (Flash lives in §3 as a Throughput-only lever for the PREFILL of the longer-context ASR towers, where it is seq²-compute-bound.)

### B5 — Custom byte-identical CUDA kernels (fused-QKV / fused-RMSNorm+matmul) — NOT recommended for default

- A custom CUTLASS GEMM almost certainly **cannot match dia2's TF32-cuBLAS split-k reduction order bit-for-bit** (the omnivoice/misotts reassociation scars prove sub-ULP reorders flip codes over 28+ layers). Fusion reassociates the reduction ⇒ violates the default byte-identity bar. AND at B=1/2 seq=1 the step is launch-bound, so even a perfect GEMM kernel yields little — the measured fix is launch-ELIMINATION (CUDA-graph, B1), already built. **Verdict: do NOT add custom CUTLASS kernels for the default path.** The only byte-identical-by-construction win in this family is CUDA-graph launch-capture (B1). Custom kernels, if ever pursued, belong in Throughput (§3) where the existing TRT route already delivers the gain at zero custom-kernel cost. (Build integration for any kernel is specified in §5.)

**Byte-identical ranking:** **B1 (graph default + auto-select) ≫ B3 (depformer pipelining) > B2 (cast fusion) ≫ B4/B5 (rejected/not-recommended).**

---

## 3. Opt-in `PerfMode::Throughput` levers (lossy-fast) — gain + accuracy delta

All of these are **NO — lossy by design** and may ONLY be reached under `PerfMode::Throughput`. They are the wrong levers for dia2's launch-bound regime but are documented honestly for throughput-mode deployments that accept a **forked (different-but-valid) utterance**.

### T1 — Torch-TensorRT FP16 (the existing opt-in `PerfMode::Throughput`)

- **State:** the TRT runtime seam is REAL and proven on GB10 (`trt.rs`, `#[cfg(accel_tensorrt)]`, `TrtStepBackbone::load`/`step`/`step_xattn`; `TrtPrecision{Fp16(default),Int8,Fp8,Nvfp4}` `:217`). Wired for neutts/dia/higgs (all `with_graphable(false)`). **dia2 is deliberately EXCLUDED**: `grep trt dia2.rs == 0 matches`; `select_perf` (`lib.rs:3169`) hard-returns `EAGER_FLOOR` for `graphable==true` even in Throughput; the test `select_perf_never_routes_graphable_models_to_tensorrt` (`lib.rs:6325`) enumerates "dia2" and asserts "eager" in BOTH modes. There is no `trt_compile_dia2.py`.
- **GB10 compat:** YES for FP16 only. Proven live (B49: torch-tensorrt 2.12.0+cu130 + tensorrt-cu13 10.16.1.11 aarch64, targets "Device(NVIDIA GB10, SM 12.1), linux_aarch64"). Hard floor TRT 10.16.1 / torch-tensorrt 2.12 (TRT ≤10.15 has NO sm_121 path; `trt_supported_sm` bands 70–129, `lib.rs:2617`). **INT8/FP8/NVFP4 have documented SM_12x gaps** (CUTLASS no FP8 grouped-GEMM CollectiveBuilder for SM_120/121 [vllm #43507]; trtllm-gen FMHA cubins missing [TensorRT-LLM #11799]; NVFP4 blocked [CUTLASS #2947, vllm #43906]) — treat as experimental, **never default-on**.
- **Accuracy delta:** per-step backbone corr 0.999964 / rel max|Δ| 0.55% — but dia2 is an AR greedy loop with a 9/32-codebook delay + CFG top-k; a ~5e-4 hidden perturbation flips one borderline argmax, the KV diverges, and codes **fork into a different valid utterance** (B49 §4: neutts forked at code ~15). dia2 is MORE fragile (CFG 2-branch agreement + 31-stage feedback). **Breaks every byte-identity gate.**
- **Gain (honest):** backbone-only TRT caps at ~18% of AR time × ~1.8× ⇒ ~8–9% ⇒ RTF 3.4→~3.1 — **barely better than the FREE byte-identical CUDA-graph already in-repo (×1.04 backbone-only)**, so backbone-only TRT is strictly dominated. The only version worth doing is **backbone + depformer TRT FP16** (~1.6–1.8× e2e ⇒ RTF 3.4→**~2.0–2.1**) — still NOT <1 (depth-bound regime unchanged; TRT shrinks the constant). Requires a NEW `trt_compile_dia2.py` (decoder-only `step()`, no `step_xattn` — dia2 has no cross-attn) + a 31-stage depformer engine + KV-export plumbing.
- **Wiring (if pursued):** add a dia2-local `maybe_load_trt` mirroring `dia.rs:733-791` (gate the `with_graphable(false)` spec INSIDE the Throughput branch ONLY; do NOT flip dia2's global graphable flag — that breaks the Accuracy gate). Export two engines: backbone as a pure-KV `step(embed[B=2,1,H], cos, sin, past_k, past_v)` with dynamic `Dim('kv_len',1..MAX)` (cover max_prefill+MAX frames or it hard-fails — B49 hit "does not satisfy any optimization profile" until max raised to 1024) and B fixed at 2 (CFG); depformer as 31 static-shape stage engines (q==1, fixed RoPE pos, fixed weight-group). Keep CFG lerp/top-k/delay/codec in Rust. Add `trt_active()` + an honest "forked-not-byte-identical" served-metadata label.

### T2 — FlashAttention (which FA version on sm_121) — REJECT on dia2, marginal on fleet prefill

- **GB10 compat (web-grounded):** the Dao-AILab FlashAttention library does NOT support GB10/sm_121. **FA2:** Ampere/Ada/Hopper only — CUDA errors on sm_120/121 Blackwell (#1987/#1853/#1810). **FA3:** Hopper sm_90 ONLY (WGMMA-specialized, excludes Blackwell; arxiv 2407.08608). **FA4:** datacenter Blackwell sm_100/sm_103 ONLY (B200/B300) — consumer/Spark Blackwell sm_120/121 LACK the TMEM subsystem + tcgen05 FA4 depends on. So the hard rule holds, web-grounded: **NEVER FlashInfer/FA3/FA4 on sm_12x — "FA2-class forever."** What DOES work on GB10: PyTorch's native fused SDPA (cuDNN/mem-efficient, FA2-class via `F.scaled_dot_product_attention`); the FFI to steer it EXISTS (`neutts.rs:114-134` binds `setSDPUseFlash`/`setSDPUseMemEfficient`).
- **dia2:** WRONG lever. dia2 decode is q=1 (GEMV-shaped, memory-bound), attention 1.5%; the 40–135× SDPA-pin figure (`INFER_PERF`) is per-attention-op at B=128/seq=1024 PREFILL (compute-bound) and does NOT transfer to B=2 q=1 decode. And dia2's `finfo.min` mask is REQUIRED for correctness, forcing Math; a flash swap forks codes. **Triple-blocked (mask required, byte-id break, sm_121 FA-unsupported).**
- **Fleet prefill:** a fused efficient/flash PREFILL is the genuine seq²-compute win for the voxtral/cohere/ark encoders + prefill (today hand-math `sdpa_manual`). But it changes the reduction order ⇒ needs its OWN golden (like neutts ENABLED flash to match its golden). **Throughput-only.**
- **Process-wide hazard:** `setSDPUseFlash` flips the GLOBAL libtorch context — enabling it for one model changes the dispatcher for EVERY model sharing the process. Must re-run the FULL byte-identity gate fleet under the flag (mirrors the ORT process-wide-knob caveat, KV-ACCEL §7).

### T3 — Fused CUTLASS / FP8 / FP4 custom kernels

- **GB10 compat (PARTIAL, web-grounded):** CUTLASS 4.2+ DENSE FP8/FP4/FP16 GEMM works on sm_120/sm_121 (target `sm120f`/`compute_120f`). **BUT the GROUPED / block-scaled FP4/FP8 GEMM — the exact lever to batch the 31 depformer stages or CFG branches in low precision — is broken/unshipped on sm_120/121** (grouped FP4 garbage output [cutlass #2723/#3096]; CuTe-DSL BlockScaledMmaOp restricts FP4 to sm_100a [#2800/#2867]; no FP8 grouped CollectiveBuilder [vllm #43906/#43507]). sm_12x lacks TMEM + tcgen05/WGMMA, so any sm_100a kernel won't compile — must target sm_120a with the older mma.sync path.
- **Accuracy:** FP8/FP4 = lossy (Throughput-only). A fused-QKV/RMSNorm kernel that keeps dia2's exact arithmetic is "yes-IF-f32-accumulate-and-same-reduction-order" — effectively impossible against a different (CUTLASS) GEMM schedule (fusion reassociates the reduction; same omnivoice/misotts scar class). **Not byte-identical.**
- **Gain:** LOW ROI for dia2 — at B=1/2 q=1 the step is launch-bound, a faster GEMM saves FLOPs the step never spends. FP8/FP4 is the biggest theoretical win (2–4× on the GEMM) but (i) lossy ⇒ Throughput-only, (ii) the GROUPED form needed is broken on sm_121, (iii) the existing TRT Throughput path already captures most lossy-FP16 gains (~1.85×/step) at ZERO custom-kernel cost. **Verdict: do NOT add custom CUTLASS kernels; the launch-bound win is CUDA-graph (B1), the lossy win is TRT (T1).**

**Throughput ranking (honest):** **T1 backbone+depformer TRT FP16 (RTF 3.4→~2.0, forked codes) > T2 fleet-prefill flash (ASR towers, not dia2) ≫ T3 CUTLASS/FP8 (broken grouped path on sm_121, dominated by T1).** None reach RTF<1 and all are lossy.

---

## 4. Hardware-abstraction + auto-selection design

Everything plugs into the existing `AccelMapper::select_perf(model, dev_caps, perf_mode, staged)` seam (`lib.rs:3169`). The design adds ONE new byte-identical backend and preserves the lossy-exclusion guard.

### 4.1 New `ByteIdenticalGraph` `AccelBackend` (the B1/GAP-B fix)

- Implement `AccelBackend` with **priority between `Eager`(0) and `TorchTensorRt`(80)**. `is_compatible` returns `Compatible` IFF `dev.vendor()==Nvidia` && CUDA-graph shim is available (`cfg(waav_cuda_graph)`, `build.rs:317`) && `model.graphable`; else `Incompatible{reason}` ⇒ falls to `Eager`. `accelerate` flips the model's graph-enable flag (replacing the env-only `WAAV_DIA2_CUDA_GRAPH` switch, which becomes an OVERRIDE not the only switch — mirrors `dia.rs:740` making `WAAV_DIA_TRT` an override of PerfMode).
- **Crucially, `select_perf` may return it for `graphable` models in BOTH `Accuracy` AND `Throughput`** — it is byte-identical so legal even in Accuracy. This does NOT weaken the TRT exclusion: `graphable→never-TRT` stays; we only replace `graphable→Eager` with `graphable→ByteIdenticalGraph→(falls to Eager off-CUDA)`.
- **Degradation:** non-CUDA / non-Blackwell / non-graphable / shim-absent all fall through to `Eager` (the always-compatible floor, `lib.rs:3146/3220`). CudaGraph `is_available` is false on CPU; `vendor()!=Nvidia` returns Incompatible. **No hardcoded `is_cuda`** — gates on `DeviceCaps.vendor()`/`is_gb10()` (`lib.rs:773,821`) from the same registry `select` uses.
- Pass `EngineConfig.perf_mode` + `DeviceCaps` (`query_cuda_device`, `dia.rs:752`) into the dia2 loader so the graph is selected by the mapper, not by an env read buried in dia2.rs.

### 4.2 How a new LOSSY lever plugs in (abstraction already built)

- Implement `AccelBackend`; `is_compatible` gates on `vendor` + `sm_arch` + `is_gb10` + model ops + dtype. A `FlashKernel`/`CutlassFp8` backend returns `Incompatible{reason}` when `sm_arch>=120` (web-cited FA/grouped-GEMM gaps), so it **auto-declines on GB10** and the mapper logs why (`lib.rs:3128`) then falls to Eager. `accelerate` returns the module or a typed `AccelUnavailable` for clean Eager fallback. Register in `with_features` behind a per-vendor cargo feature (like `accel-tensorrt`→`TorchTensorRt`). Pure-data readiness predicates (`trt_supported_sm:2617`, `is_gb10:821`, `vendor:773`) are table-testable, no hardcoded CUDA.
- A dia2 attention-kernel lever attaches via the `KernelPolicy` seam (`kernels/mod.rs:145-156`): a new policy returns a different `AttnKernel`/`allow_tf32` per `(dev, vendor, KernelSig)` with ZERO model edits — but only WINS on dia2 if it also restructures the required mask (it doesn't, so it's a no-op on dia2, correctly).
- **Per-vendor branch:** `KernelPolicy::attn_kernel`'s `caps: Option<&DeviceCaps>` returns mem-efficient/cuDNN-flash (FA2-class) on sm_12x, FA3 on Hopper, AITER on ROCm — the HW-abstraction hook already exists. NEVER FlashInfer on sm_12x.

### 4.3 The invariant the design must NOT break

`select_perf` keeps the lossy-exclusion: `graphable==true ⇒ never TorchTensorRt` even in Throughput (`lib.rs:3178`, test `lib.rs:6325`). The `ByteIdenticalGraph` backend is byte-identical so it is the ONLY accelerator added to the Accuracy path. Do NOT remove dia2's graphable flag — that would expose it to lossy TRT and break 608/608 + 544/544 gates.

---

## 5. Custom-CUDA-kernel build integration + byte-identity gate (for ANY kernel, if ever pursued)

The repo has NO custom `.cu` kernels today — the only `csrc/` is `cuda_graph_shim.cpp` (a C-ABI wrapper over `at::cuda::CUDAGraph`, compiled by `build.rs:262-323` via `cc::Build`, gated `cfg(waav_cuda_graph)`). The "bespoke byte-identical kernels (rsb/vibevoice)" are mangled-symbol FFI toggles into libtorch's own `at::` C++ (`rsb.rs`/`neutts.rs:116-121`), NOT `.cu` files. So a real CUTLASS `.cu` is a genuinely NEW build surface. If pursued:

1. **Build:** extend the existing `build.rs` `cc::Build` path (`:262-323`, already compiles `csrc/*.cpp` against the wheel's libtorch headers + CXX11 ABI + CUDA include) to nvcc-compile `csrc/dia2_fused.cu` behind a new cfg (like `waav_cuda_graph`/`accel_tensorrt`, `build.rs:173-177`). Target `-arch=sm_120a` (NOT sm_100a — no TMEM/tcgen05 on GB10), CUTLASS header-only include.
2. **Expose:** a C-ABI `extern "C"` entry (like `cuda_graph_shim`'s `waav_cuda_graph_*` or the `rsb.rs` mangled-symbol pattern) into a caller-owned output tensor, called as raw FFI on the model's single thread; OR register as a torch custom op (`torch::Library`).
3. **Gate:** route via `KernelPolicy` keyed on `DeviceCaps.vendor()/sm` (e.g. `CutlassSm120Policy`) returning the kernel only for sm_120a/121a CUDA, falling back to `nn::sdpa`/`Linear` on every other device — nothing hardcoded CUDA-only.
4. **PerfMode:** reach it ONLY under `PerfMode::Throughput` (a custom GEMM can't be byte-identical, §B5/T3); Accuracy keeps the eager byte-identical path.
5. **Byte-identity gate:** RED-first test reproducing eager (the force-solo CODES oracle dia2 already has) for any Accuracy claim, + a Throughput-mode tolerance gate. Re-verify bit-faithful on EVERY libtorch/CUTLASS/CUDA point-release bump.

**Recommendation: skip custom kernels.** The launch-bound win is CUDA-graph (already built, byte-identical); the lossy win is TRT (already staged for the fleet).

---

## 6. Dependency-ordered roadmap + the #1 implementation to do FIRST

```
[byte-identical, DEFAULT — PerfMode::Accuracy]
 R0  Fix graph-mode teardown SIGSEGV (prereq for default-on)            [no behavior change]
  └─ R1  ByteIdenticalGraph AccelBackend + wire select_perf (GAP-B)     [×1.2 solo, byte-id]   ◀── #1 DO FIRST
        ├─ R2  Graph-over-ragged-ring, bucketed by cohort width (GAP-A) [the serve-path RTF~3.4 win, byte-id, NEW WORK]
        └─ R3  Cast/copy/cat fusion on top of graph (B2)                [~+5%, byte-id, per-op Δ==0 gate]
              └─ R4  Depformer pipelining / de-serialize prev upload (B3)[the RTF<1@B1 lever, byte-id, STRUCTURAL]

[opt-in, lossy — PerfMode::Throughput, PARALLEL, lower priority]
 R5  trt_compile_dia2.py (backbone + 31-stage depformer FP16)           [RTF 3.4→~2.0, FORKED codes]
      └─ R6  dia2-local maybe_load_trt (Throughput-branch only)         [graphable flag NOT flipped globally]
 R7  Fleet-prefill flash (voxtral/cohere/ark ASR towers, own golden)    [Throughput, NOT dia2]
```

### The #1 highest-ROI implementation to do FIRST — **R1: `ByteIdenticalGraph` + auto-select (after R0)**

- **Why:** it is the single highest-ROI byte-identical fix, **already exists and is byte-identity-gated** (1188-call AB, 608/608, 544/544). It needs only (a) the teardown segfault fixed and (b) wiring into `select_perf` so the env toggle becomes an override not the only switch. Measured **−19% AR wall / ×1.2 solo at zero accuracy cost.** It respects the abstraction (byte-identical ⇒ legal in Accuracy; degrades to Eager off-CUDA/non-graphable; no hardcoded CUDA).
- **What it does NOT do (honest):** it does NOT touch the production serve-path RTF ~3.4 — that is the ragged-ring (R2, GAP-A), eager-only today. Do not promise a batched-serve speedup from R1 alone. And it cannot reach RTF<1 on a 2B CFG-2× depformer model; RTF<1 lives in R4 (depformer pipelining) — still byte-identical — and/or accepting lossy T1.

---

## 7. Honest bottom line — which gains require accepting lossy numerics

- **The dia2 RTF problem is recoverable MOSTLY WITHOUT touching numerics.** It is launch-bound (60.8% GPU-idle), so the byte-identical CUDA-graph (−19% measured, already shipped+gated) is the correct, highest-ROI lever — the OPPOSITE of the brief's flash/TRT-fp16 hypothesis (those target attention=1.5% with near-zero ROI AND break the bf16-tail byte-identity the manual path protects).
- **Byte-identical path to RTF ~1.3–1.6:** graph-default (R1) + ring-graph (R2) + cast fusion (R3). **RTF<1 at B=1** likely needs the structural depformer-pipelining change (R4) — still byte-identical.
- **Lossy is only needed if** you want RTF ~2.0 WITHOUT the structural work (T1 backbone+depformer TRT FP16, accepting a forked-but-valid utterance) — and even that does NOT reach RTF<1 (dia2 stays depth-bound; TRT shrinks the constant, not the regime). FP8/FP4/grouped-CUTLASS gains are NOT bankable on sm_121 today (documented library gaps).
- **Abstraction preserved end-to-end:** the byte-identical graph is selected by `AccelMapper` in `PerfMode::Accuracy` (CUDA-only, falls to Eager elsewhere); TRT stays the `PerfMode::Throughput` lever for NOT-graphable models, dia2's `graphable` flag keeps it out of lossy TRT, and every lever gates on `DeviceCaps.vendor()/sm_arch` — no hardcoded CUDA-only.
