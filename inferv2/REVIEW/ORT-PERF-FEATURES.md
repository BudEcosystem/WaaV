# WaaV Infer — ONNX-Runtime Acceleration Features: Full Catalog + Ranked Opportunities

**2026-06-25. Path A (`waav-infer-backend-ort`, `ort 2.0.0-rc.12`, `api-24` / ORT 1.20+ era, `load-dynamic`).**
Target hardware: GB10 (Grace-Blackwell `sm_121`, unified 121 GB pool) + aarch64 CPU floor.
Companion to `BATCHING-ANALYSIS-SYNTHESIS.md` (the host-KV-restream `G1` bottleneck this report's `#3`/`#5`
levers attack) and `INFER_PERF.md` (the IoBinding seam = the #1 already-shipped change).

This is an EXHAUSTIVE sweep of every ORT acceleration lever **beyond the KV-cache/IoBinding fix**, each scored
for WaaV's two voice cohorts: **STT** (whisper/moonshine/sensevoice/parakeet — encoder + short decode) and
**codec-AR TTS** (chatterbox — the hundreds-of-strides AR loop that is the perf-critical path).

---

## 0. What is ALREADY used in `waav-infer-backend-ort` (the baseline)

Verified by reading `src/ep.rs`, `src/lib.rs`, `src/cpu_tier.rs` and grepping the whole repo.

| Lever | State | Where |
|---|---|---|
| **`ort::IoBinding` bound loop** (constants-once H2D, device-resident I/O) | ✅ USED (the #1 change) | `lib.rs` `run_bound` / `BoundState` |
| **CUDA EP `gpu_mem_limit`** (live-device-sized arena cap; G8 OOM guardrail) | ✅ USED | `ep.rs` `cuda_arena_limit_bytes` → `.with_memory_limit` |
| **CUDA EP `arena_extend_strategy = SameAsRequested`** (GB10 anti-frag) | ✅ USED (GB10-scoped) | `ep.rs` `.with_arena_extend_strategy` |
| **CUDA EP `use_tf32 = 0`** (batch-invariance, NOT a speed knob — it *forgoes* TF32) | ✅ USED (GB10-scoped, accuracy) | `ep.rs` `.with_tf32(false)` |
| **CPU intra-op = physical cores + affinity pinning + intra-op spin** | ✅ USED | `cpu_tier.rs` `apply_to` |
| **CPU inter-op = 1** (latency-favored single-stream floor) | ✅ USED | `cpu_tier.rs` |
| **EP auto-probe + graceful CPU fallback + telemetry** | ✅ USED | `ep.rs` `apply_request` |
| **int8-never-on-CUDA / int8-never-on-CPU-tier precision guards** | ✅ USED | `lib.rs` `guard_precision_ep`, `cpu_tier.rs` |
| **Graph optimization level** | ⚠️ DEFAULT (never set explicitly) — ORT default is `ORT_ENABLE_ALL`/Level3 incl. layout | implicit |
| **Everything else below** | ❌ NOT used | — |

**NOT used (the opportunity surface):** ORT-EP CUDA Graph capture; explicit graph-opt level / **offline
optimized-model serialization** (prepacking amortization); the **transformer optimizer** (`optimize_model`
attention/LN/Gelu/MatMulNBits fusions) as an offline export step; `cudnn_conv_algo_search` /
`cudnn_conv_use_max_workspace` / `prefer_nhwc` / `fuse_conv_bias`; the `sdpa_kernel` attention-backend pin;
MatMulNBits 4-bit weight-only quant; `do_copy_in_default_stream`; `with_memory_pattern`; parallel execution
mode; TensorRT EP (any option); OliveAI.

**The `ort` rc.12 Rust surface is rich** — every CUDA knob below is a one-line builder call already in the
crate (`src/ep/cuda.rs`: `with_cuda_graph`, `with_conv_algorithm_search`, `with_conv_max_workspace`,
`with_prefer_nhwc`, `with_attention_backend`/`sdpa_kernel`, `with_fuse_conv_bias`; `src/session/builder/`:
`with_optimization_level`, `with_optimized_model_path`, `with_memory_pattern`, `with_parallel_execution`).
The full TensorRT EP option set is likewise exposed (`src/ep/tensorrt.rs`, ~40 `with_*`). So **effort for most
of these is config, not new FFI** — the constraint is validation (bit-identity + measured gain), not plumbing.

---

## 1. CUDA-Graph capture (`enable_cuda_graph`)

**Mechanism.** Captures the kernel-launch sequence of a `Run()` into a CUDA graph and *replays* it, eliminating
per-kernel CPU launch overhead. Replay requires **fixed input/output shapes AND fixed device addresses** for a
given graph-annotation id; uses IOBinding to pin those addresses; the **first `Run()` is slow** (capture).
Models with control-flow ops (`If`/`Loop`/`Scan`) are **unsupported**, all nodes must partition to the CUDA EP,
I/O must be tensors, and **multi-threaded use is unsupported**. Multi-graph capture exists via
`run_options.add_config_entry("gpu_graph_id", N)` — different batch buckets ⇒ different ids — but a captured
graph lives for the session lifetime (no deletion). [1][6][7][9]

**WaaV applicability.** This is the natural pair to the AR codec-decode loop: per stride WaaV re-runs the *same*
LM graph, and `run_bound` *already* pins constant I/O addresses via IoBinding — exactly CUDA Graph's
precondition. The launch-overhead it removes is precisely the per-stride CPU cost that hurts the
small-tensor decode step.
- **STT cohort:** encoder is one big shape (some benefit on the decode loop; lower priority).
- **codec-AR TTS:** highest-value, BUT collides with two WaaV realities: (a) the **host-KV re-stream grows the KV
  seq dim by 1 every stride** (`chatterbox.rs lm_forward_batched`) → shapes/addresses change per stride → a
  naive capture is invalid. CUDA Graph only becomes clean **after** the `G1` device-resident **ring-KV** (fixed
  max-seq buffer) lands. (b) **dynamic batch** needs bucketed `gpu_graph_id`s (one capture per cohort width).
- **G7 already flags** "CUDA-graph shape-bound vs dynamic batch (bucket B)".

**Expected gain.** Launch-bound AR steps: the tch backend already proved the analog — `with_cuda_graph` on
voxtral/dia2/csm gave **1.04×–1.20×** and made csm RTF<1 (`waav-infer-cuda-graph-fanout` memory). On the ORT
codec-AR step (smaller per-kernel work, hundreds of strides) a similar **~1.1×–1.3×** is plausible *once
ring-KV makes shapes static*.
**Accuracy:** neutral (replay = identical kernels). **Effort:** MEDIUM — one-line `.with_cuda_graph(true)` +
`gpu_graph_id` bucketing, but **gated on `G1` ring-KV** (fixed shapes) to be valid. **Already used?** ❌ on the
ORT EP (✅ on the tch/torch backend).

---

## 2. Graph optimization levels, prepacking, constant folding, transformer fusions

**Mechanism.** Four levels: `DISABLE_ALL` → `ENABLE_BASIC` (constant folding, redundant-node elimination,
Conv+Add/Mul/BN + Reshape fusions) → `ENABLE_EXTENDED` (GEMM/MatMul fusion, Conv-activation, **LayerNorm fusion,
attention fusion, skip-layernorm fusion** on CPU/CUDA/ROCm) → `ENABLE_ALL`/layout (NCHWc layout xform). **Default
is already ALL/Level3.** Optimized graphs can be **serialized offline** via `optimized_model_filepath` /
`with_optimized_model_path` — skips re-optimization (incl. **weight prepacking**) on every cold start, provided
identical HW + options. [2] Rust: `with_optimization_level(GraphOptimizationLevel::{Level1,Level2,Level3,All})`,
`with_optimized_model_path` (both confirmed present in `ort` rc.12).

**The transformer optimizer** (`onnxruntime.transformers.optimizer.optimize_model`, O1–O4) is an **offline,
model-specific** step that fuses what the runtime can't (latest attention/LN/Gelu fusions; **fp16 conversion**;
works around dynamic-axis shape-inference that *blocks* runtime fusion). O2 = transformer fusions, O3 = +Gelu
approx, O4 = +mixed precision (GPU). It emits contrib ops: `Attention`/`MultiHeadAttention`/`GroupQueryAttention`/
`RotaryEmbedding`/`SkipLayerNormalization`/`FastGelu`/`BiasGelu`/`MatMulNBits`. Microsoft reports **up to ~20×**
vs other frameworks on transformer LLMs with these fusions. [3][10][11]

**WaaV applicability.** The runtime already applies Level3, so the *runtime* lever is near-zero. **The real win
is OFFLINE:**
1. **Optimized-model serialization** (prepacking amortization): WaaV loads 16+ models and re-prepacks weights on
   every cold start. Serializing the optimized graph once removes that startup tax — **pure cold-start /
   load-latency win, accuracy-neutral, zero risk.**
2. **Offline transformer-fusion of the encoder/decoder graphs** (whisper STT, chatterbox LM): the
   onnx-community exports WaaV consumes are often *un-fused* (the well-known "attention not fused" trap with
   dynamic axes [3]). Running `optimize_model` once at onboarding to emit fused `MultiHeadAttention` /
   `GroupQueryAttention` / `SkipLayerNorm` is the **biggest accuracy-preserving compute win on this list** for
   both cohorts — it shrinks the per-stride AR step (the host-KV bottleneck's other half) and the STT encoder.

**Expected gain.** Encoder/AR-step fusion: model-dependent **1.2×–2×** on the fused subgraph; prepacking
serialization: seconds off cold start per model. **Accuracy:** fusions are math-equivalent (bit-identical gate
applies; fp16 conversion is a *separate* opt-in, NOT accuracy-neutral — keep off for the AR LM per `G5`).
**Effort:** LOW–MEDIUM (offline tooling at onboarding + a `weight_path`-style "fused export" convention; fits
the existing `waav.json` zero-code-add model). **Already used?** Runtime level ✅ (default); **offline
serialization + transformer-fusion ❌ (the opportunity).**

---

## 3. Execution-provider options

### 3a. CUDA EP knobs (beyond arena/TF32 already used)
Rust-exposed (`src/ep/cuda.rs`): `with_conv_algorithm_search`, `with_conv_max_workspace`,
`with_conv1d_pad_to_nc1d`, `with_prefer_nhwc`, `with_fuse_conv_bias`, `with_attention_backend` (`sdpa_kernel`),
`do_copy_in_default_stream` (default true), `use_ep_level_unified_stream`. [4][5]

| Knob | Default | WaaV relevance |
|---|---|---|
| `cudnn_conv_algo_search` | EXHAUSTIVE | EXHAUSTIVE benchmarks every conv algo on **first run** (slow warmup, picks fastest). HEURISTIC/DEFAULT cut warmup. Vocoders (supertonic, any GAN/conv vocoder) and conv-front-end STT (moonshine, parakeet) have convs → EXHAUSTIVE may add seconds of warmup; **HEURISTIC trades a little steady-state for much faster first inference** (matters for the cold-load latency budget). |
| `cudnn_conv_use_max_workspace` | 1 (v1.14+) | already the fast default; leave on. |
| `prefer_nhwc` | 0 (v1.20+) | NHWC is the Tensor-Core-friendly layout for convs on modern GPUs; **could speed conv-heavy STT front-ends / vocoders on Blackwell**. UNVALIDATED on `sm_121`; measure. Accuracy-neutral (layout only). |
| `fuse_conv_bias` | 0 | cheap conv+bias fusion for the same conv-heavy graphs. |
| `do_copy_in_default_stream` | 1 (same stream) | =1 is safe/correct; =0 (separate copy stream) can overlap H2D/D2H with compute — **directly targets the `G1` host-KV re-stream wall** (overlap the per-stride KV copy with the next step's compute). Higher-risk (stream correctness); measure. |
| `sdpa_kernel` attention backend | auto | pin FlashAttention / memory-efficient attention kernel for `MultiHeadAttention`/`GQA` nodes (see §8). Pairs with §2 offline fusion (no fused-attention node ⇒ this knob is inert). |

**Expected gain.** Conv knobs: vocoder/STT-front-end **1.0×–1.3×** + faster warmup. `do_copy_in_default_stream=0`:
potentially the cheapest partial relief for the AR host-KV wall (overlap), **low single-digit %** but
zero-export-cost. **Accuracy:** all layout/stream/algo knobs are accuracy-neutral. **Effort:** LOW (one-line
each; validate bit-identity + measure). **Already used?** ❌ (only arena/TF32 are).

### 3b. TensorRT EP (vs the tch Torch-TensorRT WaaV already uses on Path B)
~40 options exposed (`src/ep/tensorrt.rs`): `with_fp16`, `with_int8`, `with_engine_cache(_path/_prefix)`,
`with_timing_cache`, `with_builder_optimization_level(0–5)`, `with_profile_{min,max,opt}_shapes`,
`with_cuda_graph`, `with_sparsity`, `with_build_heuristics`, EP-context (`with_dump_ep_context_model`). [8]
- **Trade-off:** TRT fuses aggressively for **big single-digit-× speedups** but pays a **long engine build**
  (384s→9s *with* cache for SD UNet [8]); dynamic shapes need explicit min/opt/max profiles; non-TRT ops fall
  back to CUDA EP (co-register both). `trt_builder_optimization_level<3` trades engine quality for build time.
- **WaaV stance:** WaaV's **Path B already chose Torch-TensorRT** (the in-process tch accel layer per the
  vLLM-voice substrate decision). Bringing the **ORT** TRT-EP in would be **redundant for the AR/codec models**
  (Path B owns those) but is a legitimate option for the **pure-ONNX Path-A one-shot graphs** (vocoders,
  encoders) where TRT engine-cache + fp32/fp16 fusion could beat the CUDA EP — IF the dynamic-shape profiles are
  bounded and engine-cache amortizes the build. **Lower priority than §2 (offline ORT fusion gets ~half the win
  with none of the build-time/dynamic-shape pain and stays bit-identical).**

**Expected gain.** Path-A one-shot encoders/vocoders **1.3×–2×** *if* engine-cached + bounded shapes.
**Accuracy:** fp32-TRT is fusion-equivalent (validate); fp16/int8-TRT is lossy — out per the accuracy invariant.
**Effort:** HIGH (engine cache lifecycle, shape profiles, fallback co-registration, build-time ops). **Already
used?** ❌ (ORT TRT-EP); Torch-TensorRT ✅ on Path B.

### 3c. OpenVINO / others — not relevant on GB10 (CPU-floor only on x86 hosts; out of scope here).

---

## 4. Quantization

**Mechanism.** int8/uint8 (dynamic [per-inference scale, better accuracy, recommended for transformers] vs
static [calibrated, faster]); QDQ vs QOperator format; **MatMulNBits = 4-bit block-wise weight-only** quant
(RTN default, GPTQ/HQQ supported; AWQ/GPTQ via Gen-AI model-builder tooling). **EP support: CPU does int8-GEMM
(VNNI / Arm dot-product — the *fast* int8 path); ORT CUDA/TensorRT only int8-GEMM with S8S8 Tensor-Core int8
and NOT general int8 ONNX graphs.** Quantization "is not loss-less." [13][14]

**WaaV applicability — heavily constrained by existing invariants:**
- **int8 is forbidden** on both the CUDA EP (can't int8-GEMM general graphs → silent per-node CPU degrade, the
  `guard_precision_ep` trap) **and** the CPU tier (`cpu_bf16_fp32_accumulate_only` — int8 = lossy quant,
  GUIDELINES §6). Both are already guarded. So **int8 PTQ is essentially closed for WaaV's accelerator path.**
- **`q4f16` is the chosen quant** (voxtral validated **byte-identical** Rust-int8 vs onnxruntime-int8; q4f16
  weights load zero-code). **MatMulNBits 4-bit weight-only** is the same family — it shrinks the LM weight
  footprint (helps the GB10 unified-pool OOM pressure) and *can* speed memory-bound decode. BUT the synthesis
  doc's **`G5` finding is decisive: `q4f16` LM made the host-KV bottleneck WORSE** (host-bound; N=16 didn't
  finish in 15 min vs fp32 ~94s) → **recommendation: keep the codec-AR serve LM at fp32 until `G1` ring-KV
  fixes the host round-trip.** 4-bit's win is latent behind the same `G1` blocker.

**Expected gain.** Memory: 4-bit ≈ 4× weight shrink (real OOM-headroom value on GB10). Compute: **negative on
the current host-KV path** (`G5`); positive only post-`G1`. **Accuracy:** lossy in general — only the validated
`q4f16`/4-bit-weight-only paths are admissible, each needing a per-model bit/WER gate. **Effort:** LOW to *try*
(swap `waav.json` weights), but the perf payoff is `G1`-gated. **Already used?** ✅ `q4f16` supported;
**MatMulNBits-as-such not specifically exercised** — but it is the wrong lever to push before `G1`.

---

## 5. Threading / concurrency

**Mechanism.** `intra_op_num_threads` (parallelism *inside* an op; default = physical cores w/ auto-affinity),
`inter_op_num_threads` (across-op parallelism; only used in **parallel** execution mode), `execution_mode`
(`SEQUENTIAL` default vs `PARALLEL` — "helps models with many branches, can hurt models without"), thread
affinity, spin (`with_intra_op_spinning`, `spin_duration_us`). NUMA single-node binding ≈ **+20%**. [12]
Rust: `with_intra_threads`, `with_inter_threads`, `with_parallel_execution`, `with_intra_op_spinning`,
`with_config_entry("session.intra_op_thread_affinities", …)` — **all already exercised by `cpu_tier.rs`.**

**WaaV applicability.** The **CPU tier is already SOTA-tuned**: one-intra-op-thread-per-physical-core, affinity
pinned, intra-op spin on, inter-op=1, NUMA-count carried. The two residual levers:
- **Parallel execution mode** — WaaV deliberately keeps inter-op=1 (latency-favored single-stream floor; the
  AR/STT graphs are mostly linear, not branchy → PARALLEL would *hurt*). **Correctly left off.** The one place
  it *could* help is a multi-branch DAG stage, but that's the engine's stage-DAG concern, not a single session.
- **The aarch64-CPU batching cliff** (synthesis: chatterbox CPU **4.14×@B8** but regresses past B8; whisper STT
  1.19×): this is a **batch-knee** finding, not a thread-count one — the fix is **per-backend batch-knee tuning**
  (cap CPU cohort width at the knee), already called out in the synthesis "Both hardware" section. Thread tuning
  is done; **cohort-width capping per HW is the open item** (engine-level, not an ORT session knob).

**Expected gain.** Threading: ~0 (already optimal). Per-HW batch-knee cap: avoids the CPU regression past the
knee (protects throughput under load). **Accuracy:** neutral. **Effort:** LOW (a config knob in the batcher,
not the ORT backend). **Already used?** Threading ✅ (fully); batch-knee cap ❌ (open, engine-side).

---

## 6. IO-binding + arena / memory

**Mechanism.** IOBinding keeps inputs/outputs device-resident & enables **pinned (page-locked) host memory** for
async `cudaMemcpyAsync` overlap. `enable_cpu_mem_arena` (BFC CPU arena), `enable_mem_pattern`
(`with_memory_pattern` — pre-plans a single contiguous allocation from the **first** run's shapes; **must be
disabled for dynamic shapes** or it mis-plans), `arena_extend_strategy`/`gpu_mem_limit` (CUDA, already used),
mimalloc override (single/double-digit % on some models). [Memory-optimization / device-tensor docs] [4]

**WaaV applicability.**
- **IoBinding is the already-shipped #1 change.** The remaining IoBinding upside is making the *output* side
  device-resident across AR strides (today `run_bound` extracts to `CUDA_PINNED` host memory each step — correct,
  but it's still the D2H the `G1` ring-KV would remove). **`G1` device-resident ring-KV is the headline; it IS
  the "finish the IoBinding story" lever** — keep the KV on device across strides instead of re-streaming through
  the host. This is THE perf item per the synthesis (host-KV caps Path-A at ~1.8× and regresses; device-resident
  scales ~30×).
- **`with_memory_pattern`**: WaaV's AR KV grows per stride (dynamic) ⇒ memory-pattern would mis-plan; **leave OFF
  for the AR LM.** For the *fixed-shape* one-shot encoders/vocoders it could help — selective opt-in.
- **Pinned-memory input staging**: the varying per-stride inputs still upload from pageable host memory; staging
  them through pinned buffers would overlap with compute (synergy with `do_copy_in_default_stream=0`, §3a).

**Expected gain.** `G1` ring-KV: **the ~1.8×→~30× unlock** (the single biggest number in the whole analysis).
Pinned-input + separate-copy-stream: low single-digit % bridge before `G1`. **Accuracy:** neutral (copy
placement only). **Effort:** `G1` is HIGH (ONNX re-export with a fixed ring-KV buffer, or route the AR LM to the
device-resident tch path) — but it is *the* lever. Pinned-staging: MEDIUM. **Already used?** IoBinding ✅;
device-resident ring-KV ❌ (`G1`, the #1 open perf item); memory-pattern ❌ (correctly).

---

## 7. OliveAI (the ORT model-optimization toolkit)

**Mechanism.** Microsoft Olive ("ONNX Live") — hardware-aware, **pass-based** offline optimizer: graph capture,
**quantization** (incl. 4-bit AWQ/GPTQ/RTN), **graph fusion / ORT transformer optimization**, mixed precision,
auto-tuning of pass params against a latency/accuracy objective. CLI: `olive auto-opt`, `olive quantize`,
`olive finetune`. Auto-optimizes Llama/Phi/Qwen/Gemma out-of-the-box; emits ORT-ready ONNX. [15][16]

**WaaV applicability.** Olive is the **tooling that automates §2 + §4** (fusion + quant) as a repeatable onboard
step. It does **not** belong at serve time (HARD RULE: no per-venv/pip serving paths) — but as a **throwaway
offline export tool at model-onboarding** (analogous to the existing eval harnesses), `olive auto-opt` could
produce the fused/serialized ONNX that WaaV then loads zero-code via `waav.json`. **Caveat:** Olive's defaults
lean on int8/fp16 (lossy) — WaaV must constrain it to **fusion + prepacking-serialization only** (accuracy-
neutral passes) and bit-verify every output against the reference engine (the existing 100%-correctness bar).
Most Olive recipes target LLMs, not codec-AR-TTS/STT — partial fit; the generic fusion/optimize passes apply,
the model-specific auto-recipes mostly don't.

**Expected gain.** Indirect — it's the delivery vehicle for §2's gains, not a new gain. **Accuracy:** depends
entirely on which passes are enabled (must whitelist accuracy-neutral). **Effort:** MEDIUM (onboard-time
toolchain; bit-verification gate). **Already used?** ❌.

---

## 8. Newer / contrib ops (FlashAttention, sparse/paged attention, multi-stream)

**Mechanism.** ORT ships **FlashAttention + memory-efficient attention** CUDA kernels behind the
`Attention`/`MultiHeadAttention`/`GroupQueryAttention` contrib ops; the CUDA EP `sdpa_kernel` /
`with_attention_backend` knob pins which backend (flash vs efficient vs math). GQA/MHA/RotaryEmbedding got
dedicated kernels (ORT reports up to ~20× vs other frameworks on LLMs). ORT 1.25 added **opset-24 Attention on
CUDA** (disjoint from the contrib op) + flash-attention head-sink. Multi-stream = `use_ep_level_unified_stream`
/ multiple sessions. [17][18][19]

**WaaV applicability.** These kernels only fire **if the graph actually contains the fused attention node** —
which loops back to **§2 offline transformer-fusion**: fuse the whisper/chatterbox attention into
`MultiHeadAttention`/`GroupQueryAttention` first, *then* `sdpa_kernel` selects FlashAttention for it. On GB10
(`sm_121` Blackwell) the **hard rule from `INFER_PERF` stands: NEVER FlashInfer on `sm_12x`** — but that's
FlashInfer specifically; ORT's own FlashAttention/cuDNN-flash via `sdpa_kernel` is the SDPA-pin lever
`INFER_PERF` measured at **40–135×** on the right kernel. So the WaaV-correct move is: **fuse attention offline
(§2) + pin the cuDNN/efficient SDPA backend** — NOT roll a custom kernel. **Paged/sparse attention** contrib ops
are LLM-serving constructs; WaaV's lockstep batcher is the voice analog and doesn't map onto ORT's paged-attn.

**Expected gain.** Large on the attention subgraph **once fused** (the SDPA-pin is the highest per-op multiplier
in `INFER_PERF`). **Accuracy:** flash/efficient/math attention are numerically equivalent up to reduction order —
**bit-identity gate required** (same discipline as TF32). **Effort:** MEDIUM (depends on §2 fusion landing
first). **Already used?** ❌ on Path A (the tch backend pins SDPA per `INFER_PERF`).

---

## RANKED CATALOG (impact × ease, for WaaV's voice cohorts)

| Rank | Lever | Cohort | Impact | Ease | Accuracy-safe | Used? | Gating dependency |
|---|---|---|---|---|---|---|---|
| **1** | **Device-resident ring-KV** (§6, the `G1` IoBinding finish) | codec-AR (++), STT decode | ★★★★★ (~1.8×→~30×) | ★★ (HIGH effort) | ✅ | ❌ | none — *this is the prerequisite for #2/#5/#8's full value* |
| **2** | **Offline transformer-fusion + optimized-model serialization** (§2) | STT (++), codec-AR LM | ★★★★ (1.2–2× + cold-start) | ★★★★ | ✅ (fusions equiv; fp16 OFF) | ❌ | none (offline onboard step) |
| **3** | **ORT FlashAttention via offline-fused attn + `sdpa_kernel` pin** (§8) | STT, codec-AR | ★★★★ (SDPA-pin = 40–135× on the attn op) | ★★★ | ✅ (bit-gate) | ❌ | needs #2 fusion first |
| **4** | **CUDA-Graph capture on the AR step** (§1, `gpu_graph_id` buckets) | codec-AR (++) | ★★★ (~1.1–1.3×, tch proved 1.04–1.20×) | ★★★ | ✅ | ❌ | needs #1 ring-KV (static shapes) |
| **5** | **Conv/stream CUDA knobs**: `prefer_nhwc`, `fuse_conv_bias`, `cudnn_conv_algo_search=HEURISTIC` (warmup), `do_copy_in_default_stream=0` (overlap) (§3a) | STT front-ends, vocoders; AR-step overlap | ★★ (1.0–1.3× + warmup; copy-overlap bridges `G1`) | ★★★★★ | ✅ | ❌ | none (one-line + measure) |
| 6 | MatMulNBits 4-bit weight-only (§4) | codec-AR LM (memory), GB10 OOM headroom | ★★ (compute NEGATIVE pre-`G1` per `G5`; memory ✓) | ★★★ | ⚠️ (q4f16 validated only) | partial | `G1` (compute), per-model WER gate |
| 7 | OliveAI onboard toolchain (§7) | all | ★★ (delivery vehicle for #2/#3) | ★★★ | ⚠️ (whitelist passes) | ❌ | constrain to neutral passes |
| 8 | Per-HW batch-knee cap (§5) | CPU cohorts | ★★ (protects vs CPU regression past knee) | ★★★★ | ✅ | ❌ | engine-side, not ORT backend |
| 9 | ORT TensorRT-EP for Path-A one-shot graphs (§3b) | vocoders/encoders | ★★ (1.3–2× if cached) | ★ (HIGH: build/shape/cache) | ✅ (fp32 only) | ❌ (Torch-TRT on Path B) | bounded shapes + engine cache |
| — | Parallel execution mode (§5) | — | ✗ (would HURT linear AR/STT graphs) | — | — | correctly OFF | — |
| — | `with_memory_pattern` on the AR LM (§6) | — | ✗ (mis-plans dynamic KV) | — | — | correctly OFF | (OK for fixed-shape one-shots only) |
| — | int8 PTQ on CUDA/CPU-tier (§4) | — | ✗ (forbidden: silent CPU degrade / lossy) | — | ❌ | guarded OUT | — |

---

## TOP 5 DO-NEXT ORT PERF OPPORTUNITIES (concrete WaaV application)

1. **Finish the IoBinding story — device-resident ring-KV for the codec-AR LM (`G1`).**
   *What:* re-export the chatterbox LM with a **fixed max-seq ring KV buffer** (or route the AR LM to the
   device-resident tch path), so the KV never round-trips through the host between strides. *Why:* the synthesis
   pins this as the #1 lever — host-KV caps Path-A at **~1.8× and regresses to 1.0×@B64**, device-resident scales
   **~30×@B64**. *Where:* `chatterbox.rs lm_forward_batched` + the ONNX export; `run_bound` already binds the
   device side. *Risk:* bit-identity gate on the ring-KV rewrite. **This unblocks opportunities 3 & 4.**

2. **Add an offline "fuse + serialize" onboarding pass (transformer optimizer / Olive, accuracy-neutral only).**
   *What:* at model onboarding, run `optimize_model` (O2 fusions, **fp16 OFF**) on the whisper/chatterbox
   encoder+decoder graphs and emit a **fused, prepacking-serialized** ONNX that `waav.json` loads zero-code.
   *Why:* onnx-community exports are frequently un-fused (the dynamic-axis "attention not fused" trap) — fusing
   `MultiHeadAttention`/`GQA`/`SkipLayerNorm` shrinks the STT encoder AND the AR per-stride step; serialization
   removes per-cold-start re-prepacking across all 16 models. *Where:* a new onboard harness step + a
   `fused_weight_path` convention. *Risk:* bit-verify each fused export vs the reference engine (existing bar).

3. **Pin the cuDNN/efficient SDPA backend on the fused attention nodes (`with_attention_backend`/`sdpa_kernel`).**
   *What:* once #2 produces fused attention, select ORT's FlashAttention/efficient-attention backend (NOT
   FlashInfer — banned on `sm_12x`). *Why:* `INFER_PERF` measured the SDPA-pin at **40–135×** on the attention
   op — the single highest per-op multiplier. *Where:* `ep.rs` CUDA EP builder (`.with_attention_backend(...)`),
   GB10-scoped. *Risk:* bit-identity gate (reduction-order), same discipline as the existing TF32-off rule.

4. **Capture the AR decode step as a CUDA Graph, bucketed by cohort width (`with_cuda_graph` + `gpu_graph_id`).**
   *What:* after #1 makes per-stride shapes static, `.with_cuda_graph(true)` on the ORT CUDA EP and one
   `gpu_graph_id` per batch bucket. *Why:* removes per-stride kernel-launch overhead on the hundreds-of-strides
   AR loop — the tch backend already proved **1.04×–1.20×** (csm → RTF<1) on the analogous launch-bound seam.
   *Where:* `ep.rs` + the codec-AR batcher's run-options. *Risk:* invalid without #1 (growing KV breaks the
   fixed-address precondition); `G7` already tracks the shape-bound-vs-dynamic-batch tension.

5. **Land the cheap, zero-export CUDA conv/stream knobs and measure (the LOW-effort sweep).**
   *What:* `prefer_nhwc(true)` + `fuse_conv_bias(true)` for conv-heavy STT front-ends (moonshine/parakeet) and
   vocoders; `cudnn_conv_algo_search=HEURISTIC` to cut first-inference warmup on the cold-load budget;
   trial `do_copy_in_default_stream=false` to overlap the per-stride H2D/D2H with compute as a **bridge before
   `G1`**. *Why:* all one-line, all accuracy-neutral (layout/algo/stream only), all already exposed in
   `ort` rc.12. *Where:* `ep.rs` CUDA EP builder, behind env flags like the existing `WAAV_ORT_TF32`. *Risk:*
   minimal; gate each on a bit-identity + wall-clock measurement on GB10 + a discrete-GPU sanity check.

---

## Sources

1. [CUDA Execution Provider — onnxruntime.ai](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html)
2. [Graph Optimizations — onnxruntime.ai](https://onnxruntime.ai/docs/performance/model-optimizations/graph-optimizations.html)
3. [Quantization — onnxruntime.ai](https://onnxruntime.ai/docs/performance/model-optimizations/quantization.html)
4. [Transformers optimizer — onnxruntime.ai](https://onnxruntime.ai/docs/performance/transformers-optimization.html)
5. [Threading / Tune Performance — onnxruntime.ai](https://onnxruntime.ai/docs/performance/tune-performance/threading.html)
6. [TensorRT Execution Provider — onnxruntime.ai](https://onnxruntime.ai/docs/execution-providers/TensorRT-ExecutionProvider.html)
7. [Device tensors / IOBinding — onnxruntime.ai](https://onnxruntime.ai/docs/performance/device-tensor.html)
8. [Memory consumption / arena — onnxruntime.ai](https://onnxruntime.ai/docs/performance/tune-performance/memory.html)
9. [OrtCUDAProviderOptions C struct reference — onnxruntime.ai](https://onnxruntime.ai/docs/api/c/struct_ort_c_u_d_a_provider_options.html)
10. [End-to-End AI for NVIDIA PCs: CUDA & TensorRT EPs in ORT — NVIDIA dev blog](https://developer.nvidia.com/blog/end-to-end-ai-for-nvidia-based-pcs-cuda-and-tensorrt-execution-providers-in-onnx-runtime/)
11. [Olive: AI Model Optimization Toolkit for ORT — onnxruntime.ai/olive](https://onnxruntime.ai/olive)
12. [Olive docs (passes: quantize/fuse/auto-opt) — microsoft.github.io/Olive](https://microsoft.github.io/Olive/)
13. [ORT v1.25.0 release (opset-24 Attention on CUDA, flash-attn head-sink) — github.com/microsoft/onnxruntime](https://github.com/microsoft/onnxruntime/releases/tag/v1.25.0)
14. [ORT 1.17 release blog (CUDA 12, Phi-2 MHA/GQA/RotaryEmbedding kernels) — onnxruntime.ai/blogs](https://onnxruntime.ai/blogs/ort-1-17-release)
15. [Attention contrib ops discussion (MHA vs Attention) — github.com/microsoft/onnxruntime/discussions/15325](https://github.com/microsoft/onnxruntime/discussions/15325)
16. [CUDA Graphs perf issue / IOBinding requirement — github.com/microsoft/onnxruntime/issues/12977](https://github.com/microsoft/onnxruntime/issues/12977)
17. `ort` rc.12 Rust API surface (verified locally): `src/ep/cuda.rs`, `src/ep/tensorrt.rs`,
    `src/session/builder/{impl_options.rs,impl_config_keys.rs}`, `src/session/run_options.rs` (gpu_graph_id).
18. WaaV: `BATCHING-ANALYSIS-SYNTHESIS.md` (`G1` host-KV bottleneck, `G5` q4f16-host-bound, `G7`/`G8`),
    `INFER_PERF.md` (IoBinding #1 change; SDPA-pin 40–135×; never-FlashInfer-on-sm_12x),
    auto-memory: `waav-infer-cuda-graph-fanout`, `waav-voxtral-accuracy` (q4f16 byte-identical), `waav-gb10-oom`.
