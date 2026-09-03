# WaaV Infer v2 — Realtime Voice Inference Engine Architecture

**Status:** v1.0 — all 10 research dimensions + 7 GB10 benchmarks integrated · **Date:** 2026-06-17 · **Device of record:** NVIDIA GB10 (Grace‑Blackwell, aarch64, CUDA 13)

> ⚠️ **The serving-layer thesis of this document is SUPERSEDED by `INFER_ENGINE_V2.md`** (the brutal-critique revision, 2026-06-17). v1.0's empirical foundation (§1, the 7 GB10 benchmarks) and HAL (§2) stand unchanged; v2.0 corrects five overreaches with primary-source evidence — **hybrid two-tier KV** (radix prefix-cache + ring suffix; "prefix sharing ~zero" in §4.3 was empirically false — 86% hittable), **variable-stride lockstep + a third execution class** (AR-outer + variable-NFE generative-inner), **deadline-graded degradation** (not pure reject-don't-glitch), **KV-length-aware prefill firewall + intra-node spatial P/D**, and a **production-hardening spine** (cell isolation, frame-progress watchdog, NaN→reject-frame, media-on-UDP/QUIC, coordinated-omission-honest observability). Read v2.0 first; this doc remains the substrate it builds on.

> The "vLLM for voice" — a realtime, accurate, portable, modular inference engine for voice models (STT / TTS / S2S, <10B params) that runs the *same* model contract from a **1000× B200 datacenter** down to a **GB10 edge box** and a **Hexagon phone**, config‑tiered, KISS, progressively downloaded. This document extends the existing WaaV Infer architecture (`INFER_SPEC.md`, `INFER_TORCH_RUNTIME.md`, `INFER_REUSE.md`) by taking the best practices from vLLM, vLLM‑Omni, SGLang‑Omni, and Kyutai/Moshi, and grounding every load‑bearing decision in **measured benchmarks on this hardware** plus a deep study of **hardware substrates × model paradigms × DAG structure × precision × sample‑rate**.

**Research base:** 10 deep source/literature studies (vLLM core, vLLM‑Omni + SGLang‑Omni, Kyutai/Moshi, current WaaV seams, model‑paradigm × batching, voice batching methodology + SLO/admission science, DAG + heterogeneous scheduling, sample‑rate/frame‑rate, hardware substrates, quantization/precision) + **7 GB10 benchmarks** (`/tmp/infer_engine_bench.md`; scripts `/tmp/bench_*.py`).

---

## 0. Executive thesis (the seven claims, each measured or sourced)

1. **Voice inference is frame‑synchronous, not throughput‑maximizing.** Every live stream emits exactly **one frame per fixed‑period tick** (12.5–150 Hz). This fixes the one unknown — per‑request token rate — that vLLM's continuous batching exists to manage. ⟹ the steady‑state batching primitive is **frame‑synchronous lockstep** (Moshi‑style: fixed slots, per‑stream exec‑mask over a rectangular batch, per‑slot ring KV), **not** continuous batching + paged‑KV. *(Kyutai source + §1.1.)*

2. **AR codec‑LM decode is the ideal batching workload and the GPU is idle at batch 1.** A SYNTHETIC GEMV‑only decode‑step microbenchmark on GB10 measured **flat latency from batch 1→64 (8.6→9.95 ms)** → an idealized **55× @64, 84× @128** roofline. **⚠️ MEASURED-ON-REAL-PATH CORRECTION (do not quote 55×@64 as a serving number):** that microbenchmark omits the host‑KV feedback that real exported codec‑LM graphs carry (e.g. chatterbox `language_model.onnx` takes the split‑KV as host `past_key_values.*` inputs and emits host `present.*` outputs the AR loop re‑streams *every stride* — `O(B·max_past·n_layers·2)` loop‑invariant work that grows with B). On the **real chatterbox 30‑layer MHA Llama, live on GB10 (CUDA EP, equal‑context, bit‑identical to per‑slot)** the lockstep‑batched speedup **RISES to a peak then REGRESSES**: B2 1.12× · B4 0.99× · B8 1.39× · **B16 1.81× (peak)** · B32 1.46× · **B64 0.95× (slower than per‑slot)**. So the real efficiency knee is **B≈16 (~1.8×), NOT B=64**, and 55×@64 does not reproduce on any exported codec‑LM graph that re‑streams host KV. Batching is still the headline accuracy‑preserving lever (it is the one the no‑quant constraint leaves intact), but size lockstep slots at the **measured per‑graph knee** (B≈16 for chatterbox), and treat 55×@64 as a kernel‑physics ceiling for a hypothetical device‑resident‑KV graph, not a shipped serving figure. *(§1.1; live re‑measurement gate `live_headline_batched_scaling_matches_doc_curve`; INFER_PERF_VALIDATION.md §3a/§7.)*

3. **Paradigm determines the batch profile — two families, therefore two batchers.** AR/RNNT/MTP decode is **memory‑bound → batches better than compute‑bound stages** (idealized roofline 55×@64; **real exported‑graph reality far lower — host‑KV re‑stream caps chatterbox at ~1.8× peak @ B≈16, regressing by B=64**, see point 2 + §4.3). Diffusion/flow/masked‑diffusion/encoder is **compute‑bound → sublinear batch** (measured **~10×@64** for a chunk‑CFM DiT step) with **fixed N‑step solves** that can blow the frame budget. Hybrids nest the two; the **nested per‑frame diffusion patch batches like AR (38×@64)** because a tiny latent is launch‑bound. *(§1.5, paradigm study. The 55×@64/38×@64/10×@64 figures are the synthetic‑microbenchmark roofline; on‑real‑graph AR batching is re‑measured in point 2.)*

4. **vLLM splits cleanly into BORROW vs AVOID.** Borrow the *patterns*: the `Platform` HAL, `CustomOp` once‑at‑load dispatch with a pure‑PyTorch fallback, the `ModelRegistry`/config‑arch dispatch, the op *math* (RMSNorm/RoPE/SiLU). Avoid the *serving core*: continuous‑batching scheduler, paged‑KV, the `CommonAttentionMetadata` (slot_mapping/block_table) contract, and **FlashInfer** — **documented ~2× end‑to‑end regression on sm_120/Blackwell aarch64 (our GB10)**. *(vLLM source study, §7.)*

5. **The model is a heterogeneous multi‑stage DAG, and the voice stages are deliberately lightened.** vLLM‑Omni and SGLang‑Omni *independently* converge on: a multi‑process stage pipeline, per‑stage scheduler tuned to its bottleneck, the AR backbone reusing batching but the codec / code‑predictor / CFM stages stripped to **plain SDPA, no‑KV re‑prefill, fp32 numerics, manual CUDA graphs, zero in‑loop GPU syncs, bs=1 fast‑path bypass**. Both engines are **CUDA‑only** — so WaaV's ORT‑EP/ggml portability is its real differentiator. *(omni‑engines study, §3, §7.)*

6. **Precision and sample‑rate are batching dimensions, not afterthoughts.** Measured: **fp8 is a DC‑throughput lever, not a batch≈1 latency lever** (fp8/bf16 GEMM = **0.62× at M=64** but **2.1× at M=4096** on Blackwell). KV‑quant is the **dominant concurrency lever for big‑KV models** (Moshi‑7B: 25→101 streams via int4) and irrelevant for small codec‑LMs. There are **two clocks**: sample‑rate (fidelity/transport) and frame‑rate (the batching clock); you **cannot lockstep‑mix streams on different frame‑rate clocks** → batch by **(model, frame‑rate) cohort**. *(§1.6, §5, precision + sample‑rate studies.)*

7. **One methodology config‑scales edge↔DC; the substrate sets the ceiling, not the design.** The *same* lockstep loop runs batch=1 (+CUDA graph) on edge and batch=64–128 on GB10 and far larger on B200. CUDA graphs help @batch‑1 (1.21×) but **hurt @batch‑32 (0.72×)** → kernels tier by batch size. **CPU (Grace ARM, no AMX, fp32) is ~24× slower and cannot serve a 0.5B codec‑LM in realtime** → the CPU/edge tier is for small/quantized models + feedforward stages, not 0.5B+ AR decode. *(§1.3, §1.7, §8.)*

---

## 1. Empirical foundation (measured on GB10, 2026‑06‑16/17)

All numbers from `Qwen2‑0.5B` codec‑LM class (neutts backbone: 896 hidden, 24 layers, 2 KV heads), bf16 unless noted. These ground every architectural choice.

### 1.1 AR decode‑step latency vs batch (THE keystone) — ctx=64
| batch | 1 | 2 | 4 | 8 | 16 | 32 | 64 | 128 |
|---|---|---|---|---|---|---|---|---|
| ms/step | 8.62 | 8.45 | 8.57 | 8.61 | 8.83 | 9.10 | **9.95** | 13.08 |
| throughput/stream | 1.0× | 2.0× | 4.0× | 8.0× | 15.6× | 30.3× | **55.4×** | **84.3×** |

**Flat batch 1→64.** GPU massively underused at batch 1 → frame‑synchronous step‑batching scales throughput near‑linearly. **One GB10 ≈ 64–128 realtime streams.**

### 1.2 Step latency vs KV‑context (frame budget over an utterance)
B=32 @ ctx{64,256,512,1024} = {9.17, 9.94, 12.32, 13.18} ms. **Benign** — even ctx 1024 @ batch 32 is 13.2 ms, under a 40 ms (25 Hz) budget. Voice AR is compute/latency‑bound long before KV‑capacity‑bound (the opposite of long‑context text LLMs) → size slots by the **compute crossover**, not the KV wall.

### 1.3 CUDA graph vs eager (launch‑overhead) — ctx=256
batch 1: eager 8.90 → graph 7.35 ms (**1.21× faster**); batch 32: eager 9.95 → graph 13.92 ms (**0.72×, slower**). ⟹ **kernels tier by batch size**: CUDA graphs for low‑batch/edge latency, eager/compile for high‑batch DC.

### 1.4 Mimi codec decode (shared vocoder stage) — 2 s audio
batch 1: 6.0 ms (RTF 0.003); batch 64: 230 ms (3.6 ms/stream). **Not the bottleneck** — already GPU‑efficient, ~1.7× batch efficiency. The AR LM is the stage that needs batching.

### 1.5 Diffusion/flow DiT denoiser step vs batch (the COMPUTE‑bound contrast)
- **Chunk CFM head** (D512/L12/T64, CosyVoice/dots.tts class): 1.75→11.0→22.0 ms @ B1/64/128; efficiency only **10×@64**; a **10‑step solve @B64 = 110 ms** (exceeds a 40 ms frame budget).
- **Per‑FRAME patch** (T4, nested AR‑inner): **38×@64** — batches like AR because tiny‑T is launch‑bound.
- **DDPM head** (D768/L16/T64, 25‑step, VibeVoice class): 25‑step solve = 90 ms@B1, **624 ms@B64**; collapses @B128.

⟹ chunk‑level diffusion must be **amortized over frames (chunked/lookahead)**, not run per‑frame; **nested AR+diffusion(small‑T)** recovers AR‑like batchability.

### 1.6 Quantization/precision on Blackwell
- **fp8 vs bf16 GEMM (TFLOPS):** M=64 → 46.1/28.5 (**0.62×**); M=2048 → 70.7/120.2 (1.70×); M=4096 → 83.6/176.8 (**2.11×**). fp8 is a **DC‑batch throughput lever, not a batch≈1 latency lever**.
- **KV bytes/stream → streams/40 GB:** Qwen2‑0.5B (2 kv_heads) 25 MB → 1589 (int4: 6357); Higgs‑7B‑class (8 kv_heads) 268 MB → 149 (int4: 596); Moshi‑7B (32 kv_heads, ctx3000) 1573 MB → **25 (int4: 101)**. **KV‑quant is the dominant concurrency lever for big‑KV models, irrelevant for small codec‑LMs.**

### 1.7 CPU (Grace ARM, 20 threads, fp32, no AMX) AR decode — the substrate contrast
~210 ms/step floor (vs 9 ms GPU = **~24× slower**); some thread‑parallel headroom but absolute latency **>> 80 ms frame budget → not realtime even at batch 1** for a 0.5B codec‑LM. ⟹ CPU/edge tier = small/quantized models + feedforward stages (codec/vocoder/encoder); AR‑codec‑LM realtime needs GPU (or x86 AMX‑int8).

---

## 2. Hardware Abstraction Layer (HAL) — substrate‑aware

**Design (locked):** borrow vLLM's two‑mechanism HAL *pattern* (not its code), keep WaaV's already‑live EP spine.

- **`Backend` trait** (≈150 Rust lines, mirrors vLLM `Platform`): runtime‑probe → exactly one active backend; methods `device_capability()`, `select_attn_kernel(head_size, dtype)`, `supports(paradigm, artifact, device)`, `pick_op_impl()`. Extends the existing pure‑data enums `EpKind{Cuda,TensorRt,Rocm,MiGraphX,OpenVino,Qnn,CoreMl,DirectMl,Xnnpack}` / `EpRequest` / `ActiveEp` — which already leak no `ort` types into `-core`/`-components` and are live‑proven on GB10 CUDA.
- **Per‑op `CustomOp`‑style dispatch**: pick the kernel **once at load** and cache it; `forward()` is a direct call (zero per‑call dispatch). The default chain is the portability backbone: a backend lacking a custom kernel falls to **`forward_native` (pure portable: ORT/ggml/ndarray)**. CPU is the guaranteed floor (P‑6); an accelerator problem is a degrade‑to‑CPU + telemetry event, never an `Err`.
- **Single policy point + entrypoint convention**: the `ort`↔EP mapping stays confined to one module (today's `backend-ort/ep.rs`, `auto_probe_order()` per‑OS); mirror vLLM's plugin entrypoint as a `waav.backends` group so third parties add a backend without forking.
- **Kernel routing (Blackwell‑specific, measured):** **never FlashInfer on sm_120 aarch64** (≈2× e2e regression — vLLM‑Omni's own Blackwell default routes to cuDNN/SDPA). Tier by batch (§1.3): **CUDA‑graph @ low‑batch/edge, eager/compile @ high‑batch/DC**. Do not vendor `vllm._C` (needs vLLM's CUDA build; breaks on aarch64/Blackwell).

**2.1 The one law (roofline).** Realtime voice is dominated by **AR decode at low batch**, which is **memory‑bandwidth‑bound**: decode latency ≈ (weight bytes streamed) ÷ (memory bandwidth), *independent of batch until the knee*. The roofline ridge point = peak‑compute ÷ peak‑bandwidth (FLOP/byte); since decode arithmetic intensity ≈ batch, the **compute‑bound knee sits at batch ≈ ridge‑point**. Two universal consequences: **(a) quantize weights (int8/int4/fp4) for near‑proportional decode speedup everywhere**; **(b) the ideal batch rises with a substrate's FLOPs:bandwidth ratio** — CPUs/NPUs (low ratio) saturate at batch 1–4; a B200 (high ratio) needs a *bigger* batch than an H200 just to be filled. This is why a 0.5B is flat‑to‑64 on GB10 (§1.1) — GEMV can't fill the SMs.

**2.2 Per‑substrate profile (the scheduler's batch‑knee + placement table).**

| Substrate | Memory | Ideal batch | Bottleneck | Shape | Favored voice paradigm |
|---|---|---|---|---|---|
| **CPU** (x86 AMX/AVX‑VNNI, ARM NEON/SVE2/I8MM) | DDR5 ~307–844 GB/s; deep cache; NUMA | **1–4** (declines past ~8) | DRAM bandwidth (decode) | **dynamic‑friendly** | edge / single‑stream, tiny STT (Moonshine), latency>throughput |
| **Hexagon/QNN** | **VTCM ~8 MB scratchpad** (HMX reads only VTCM) → LPDDR | **fixed =1** (AOT) | VTCM cap + fixed‑shape rigidity | **STRICTLY STATIC** (context binary/SoC) | static conv encoders/vocoders/KWS — **NOT AR** |
| **H200** (Hopper) | 141 GB HBM3e, **4.8 TB/s** | ~64–256+ | under‑occupancy at small batch | dynamic; CUDA‑graph + seqlen buckets | high‑concurrency batched STT/TTS, AR decode at scale |
| **MI300X** (CDNA3) | **192 GB HBM3, 5.3 TB/s**, 256 MB cache | ~128–256+ | under‑occupancy (capacity a non‑issue) | dynamic | **max co‑resident small models + huge KV** |
| **B200** (Blackwell) | 180–192 GB HBM3e, **~8 TB/s** | ~256–512+ | **severe** under‑occupancy at small batch | most launch‑sensitive (graphs essential) | hyperscale batched; wasteful for 1–4 streams |
| **RTX 4090/5090** | 24/32 GB GDDR, ~1–1.8 TB/s | tens (~16–64) | **VRAM capacity** then bandwidth | dynamic | few‑stream single‑box prosumer/edge |
| **NPU** (ANE/Intel/XDNA/TPU) | on‑chip SRAM + DRAM; INT8/INT4 native | **static =1** | static‑shape rigidity; quant on AR | **STATIC required** | static conv encoders + CNN vocoders — **BAD at AR** |
| **GB10** *(this box)* | **128 GB unified LPDDR5X, ~273 GB/s**; NVLink‑C2C ~600 GB/s coherent | small (bandwidth sets knee) | **shared ~273 GB/s** (CPU alone can't saturate) | GPU=dynamic; CPU/idle‑SMs=static | **heterogeneous zero‑copy DAG** (§3.4) |
| **Intel Core Ultra** (Lunar Lake) | 16/32 GB LPDDR5X, **~136.5 GB/s** | very small | shared bus, 3 engines contend | NPU static / Xe2 dynamic | zero‑copy DAG across CPU+Xe2+NPU (OpenVINO) |
| **Apple UMA** (M‑series) | 120 (M4) → 273 → 546 → ~800 (Ultra) GB/s | tier‑dependent | shared UMA bandwidth | ANE static / GPU dynamic | zero‑copy DAG across GPU+ANE+CPU |

**2.3 The recurring axis — static‑conv GOOD, dynamic‑AR BAD on fixed‑function engines.** A systolic/dataflow array trades registers/control/branching for MAC density, so a STT conv encoder maps perfectly while AR decode breaks the contract on four fronts (variable per‑token shape, growing KV, data‑dependent control flow, per‑token host round‑trip). **Both Qualcomm and Apple ship Whisper split exactly this way** (encoder on DSP/ANE, AR decoder on CPU/GPU). ⟹ **placement recipe: dynamic/AR → GPU; static conv (encoder, CNN vocoder, mel frontend) → NPU/iGPU/CPU‑AMX/idle‑SMs.**

**2.4 Substrate‑aware scheduler — three knobs.** (a) **Per‑substrate batch knee** (1–4 CPU/NPU; tens RTX; 64–512 datacenter; bandwidth‑set on unified). (b) **Per‑stage placement** on its best engine (§2.3, §3.4). (c) **A shared‑bandwidth arbiter** on unified‑memory boxes (§6) — *quantization is a universal win because decode is bandwidth‑bound on every substrate.*

---

## 3. The model as a heterogeneous stage‑DAG

A voice model is a **typed stage‑DAG declared as data** (mirrors vLLM‑Omni `StagePipelineConfig.input_sources` + SGLang‑Omni `Stage.get_next`), not a monolithic `forward()`. Each node owns **its own bounded queue + its own batched micro‑engine + its own batch policy**, so stage N+1 of request A pipelines against stage N of request B. The seam sits *above* the existing `SttModel`/`TtsModel` contract; the one seam that must change shape is the coarse `synthesize`/`transcribe` → a **stepped `ArStep`‑style contract** so the scheduler can batch across sessions.

### 3.1 Node/edge schema (extends `INFER_SPEC §8.2`)
A `StageNode` declares (TOML in the signed manifest, beside `exec_path`): `id`, `paradigm ∈ {ar, flow, diffusion, feedforward, codec_stream}`, `batch_policy ∈ {lockstep, micro_batch, streaming_window, inline}`, `substrate` hint (`gpu|npu|cpu|any` — a hint; the placer decides, §3.4), `stream ∈ {token, tensor}` egress granularity, `inputs[]`/`outputs_to[]` edge lists (`[]` = entry; `final_output` = egress), a `[stage.resource]` block (`kv_quota_tokens`, `bytes_touched`, `steps_per_second`, `workspace_bytes` — reference‑class priors, calibration overrides), and an optional `[stage.nested]` block (§3.3). Edges are **typed bounded channels** carrying `TokenFrame{codes,frame_idx}` (token‑streaming AR→codec), `LatentChunk{latent,chunk_idx,left_context}` (semantic→CFM→vocoder), or `WholeTensor` (encoder→decoder).

### 3.2 Decoupled per‑stage batching + pipeline parallelism
> *"Each stage runs its own scheduler tuned to its own bottleneck — never one batch loop across stages."* (SGLang‑Omni, verbatim; vLLM‑Omni runs each stage as a separate `EngineCore` subprocess.) The decisive concrete asymmetry: **AR stage wants `max_num_seqs ≥ 4` to pipeline; the codec stage typically uses `1`** — a uniform default *causes audio gaps* (vLLM‑Omni RFC #2568). This is exactly the bug the per‑stage duty ledger prevents (§6).

Three micro‑engine archetypes, each on a dedicated OS thread with thread‑affine device state + a bounded inbox:

| Archetype | paradigm | Batching | Maps to |
|---|---|---|---|
| **AR‑batch** | `ar`/`lockstep` | Fixed‑slot masked static batch (B_max slots, `StreamMask`, static shapes); synchronous tick at `steps_per_second`; gather→step→scatter | moshi‑server `batched_asr.rs`; vLLM‑Omni `OmniARScheduler` |
| **micro‑batch** | `flow`/`diffusion`/`feedforward` | Dynamic coalesce: drain inbox up to `max_batch_size` within ~2 ms deadline, one‑shot graph per **length bucket** | SGLang `SimpleScheduler`; encoders, CFM steps |
| **streaming‑vocoder** | `codec_stream`/`streaming_window` | Independent queue, windowed decode (left‑context + crossfade + dynamic first‑chunk TTFA ramp) | SGLang `HiggsStreamingVocoderScheduler` |

**Pipeline overlap is the payoff:** the AR thread lockstep‑ticks B streams while the codec thread independently micro‑batches their frames — stage N+1 of A runs while stage N of B runs. On one device this is temporal interleaving (keeps the codec fed, lets AR cross‑batch independently of codec batch size); on heterogeneous placement it is real parallelism (AR on GPU ∥ codec on NPU). **Back‑pressure:** bounded inter‑stage queues — a full downstream queue *parks* the upstream stage (never drops) → admission must test the **bottleneck stage**, not the AR stage (§6). The terminal **codec node is the one safe to offload** (CPU/other EP) and the highest‑value cross‑model dedup point (Mimi/DAC/HiggsV2 are shared decoders).

### 3.3 The nested case (AR‑outer + inner sub‑loop) — stays in‑forward
Both omni engines hit exactly the dots.tts/qwen3‑tts concern and resolve it identically: **the inner loop stays INSIDE one stage's single batched forward; it is NOT a cross‑process stage.** (SGLang‑Omni merges Talker+MTP into one stage because *"the per‑step latency would balloon"* if separated; vLLM‑Omni's `code_predictor_forward` is a literal in‑forward `for pos in range` loop with feedback written back in‑call.) A nested stage is **one `StageNode` with `[stage.nested]{inner_paradigm, inner_steps, inner_batch=fused}`** — the DAG sees one node. Two decisive properties:
1. **The inner loop is batched across the OUTER lockstep batch.** At outer frame *t*, all B active slots are at the same inner step *k* (frame‑synchronous), so the inner ODE/depth/diffusion step is a single batched kernel `[B,…]`. **My benchmark confirms this: the nested per‑frame patch batches 38×@64** (vs chunk‑diffusion's 10×@64) because the tiny latent is launch‑bound — *nesting is net‑positive precisely because the inner head is too small to saturate the GPU alone.*
2. **Schedulability folds the inner loop into the outer step time** `T_step = T_ar + inner_steps × T_inner` (calibration times the whole nested forward).

The dividing line — `fused` vs separate stage — is feedback tightness: **AR→code‑predictor is tight (fused per‑frame feedback) → one node; talker→chunk‑CFM→vocoder is loose (consumes completed chunks) → separate nodes.** So CosyVoice2 = 3‑node DAG (`ar_semantic → cfm_chunk → vocoder`); dots.tts = 2‑node (`ar_talker{nested cfm} → audiovae`). Same engine expresses both via data (P‑7).

### 3.4 Heterogeneous placement on shared‑memory systems (zero‑copy)
The stage placer adopts **ggml's `backend_sched` decision order** lifted to stage granularity: (1) **capability predicate** per substrate (`supports(paradigm, artifact, device)`); (2) **priority order with a guaranteed CPU fallback** (P‑6); (3) **follow the immovable weights** — pin each stage to where its (load‑once) weights are resident (AR's 3–6 GB on GPU → AR runs on GPU; codec's small weights on NPU → codec runs there); (4) **paradigm×substrate affinity** (AR→GPU, conv‑codec→NPU/CPU, encoder→NPU, CFM→GPU); (5) **current‑load tie‑break** via the duty ledger; (6) **boundary minimization** (each cross‑substrate edge is a cost). Manual `substrate` pin is never overridden.

**Zero‑copy contract (the GB10/UMA unlock):** ggml inserts a copy at a boundary *only* when the consumer can't view the producer's buffer type. **On coherent memory (GB10 NVLink‑C2C+ATS, Apple UMA, Intel integrated) every substrate advertises a `SharedHostBufType`, so the boundary crosses with ZERO copy — the same physical buffer is consumed directly** (the copy degenerates to a pointer alias). The handoff is a `ZeroCopyBuffer{ptr, buft, layout, owner, ready_event}` pass, not a DMA. On discrete GPUs it falls back to async copy + event sync + double‑buffering, copying only the live slice (the per‑frame `TokenFrame`).

**Contention guard (critical):** zero‑copy removes the *transfer* cost but not the *shared‑bandwidth* cost — GB10 has **one ~273 GB/s LPDDR ceiling shared by GPU+NPU+CPU; concurrent engines DIVIDE it**. So aggregate memory bandwidth is a **budgeted, schedulable resource** (§6): the typical win is *placement frees the GPU* (codec/encoder on NPU → more GPU bandwidth for AR streams), provided admission budgets the shared bandwidth so the split doesn't oversubscribe the one ceiling. Prefer to overlap a memory‑bound stage (AR decode) with a compute‑bound one (small conv‑codec); co‑locate + time‑share when both saturate bandwidth.

---

## 4. Voice‑native batching methodology (THE core deliverable)

### 4.1 The taxonomy + suitability matrix (one model sits in different boxes per stage)
Four batching methods differ on *when a batch forms* and *what frees a slot*. The reconciling insight: **a voice engine needs all three live primitives, disaggregated by stage.**

| Voice task / stage | Static‑naive | Dynamic (windowed) | Continuous (vLLM) | **Frame‑sync lockstep** |
|---|---|---|---|---|
| Codec‑AR TTS token‑gen (Orpheus, Voxtral‑TTS, CosyVoice talker) | ⚠️ | ⚠️ | 🟢 today's path | ✅ **best** (fixed 12.5–25 Hz emit) |
| Full‑duplex STS (Moshi‑class) | ⚠️ | ❌ | ❌ (no per‑frame EOS) | ✅ **only correct choice** |
| Frame‑sync STT (CTC, RNN‑T/TDT; parakeet, FastConformer‑T) | ⚠️ | 🟢 | ⚠️ | ✅ **best** (lockstep chunk‑batched encoder) |
| Token‑AR STT (Whisper‑AED, Voxtral, Qwen2‑Audio, SenseVoice‑LLM) | ❌ | ⚠️ | ✅ **best** (paged KV + admit/evict) | ❌ (text length ≠ frame count) |
| Flow / diffusion decode (F5, CosyVoice‑CFM, Matcha, Grad‑TTS) | 🟢 | ✅ **best** (length‑bucketed, fixed NFE) | ❌ (no KV) | ❌ |
| Masked‑parallel decode (SoundStorm, MaskGCT) | 🟢 | ✅ **best** (bucketed, K iters) | ❌ | ❌ |
| Codec / vocoder decode (Vocos/BigVGAN; SNAC/DAC/Mimi) | 🟢 | ✅ **best** (bucket + chunk‑stream, CUDA‑graphs) | ❌ | 🟢 (rides AR axis if co‑located) |
| **DC (1000s streams, B200)** | ❌ | ✅ non‑AR stages | ✅ token‑AR STT | ✅ AR steady‑state, big N |
| **Edge (1 stream, GB10)** | ✅ (N=1 trivially static) | ✅ | 🟢 | ✅ (N=1 degenerate lockstep) |

### 4.2 The two batchers + the nesting rule
- **Lockstep batcher** — AR codec‑LM, AR depth/MTP, RNNT/TDT, AR STT decode. Batch on the **stream axis**, fixed slots, per‑stream exec‑mask over a rectangular batch, per‑slot ring KV, advance one frame/tick, wall‑clock paced. Scaling is the headline accuracy‑preserving lever, but the **real‑graph speedup is bounded by host‑KV feedback**: on the live chatterbox codec‑LM it peaks at **~1.8× @ B≈16** then regresses (B=64 is *slower* than per‑slot), NOT the idealized 55×@64 — size slots at the **measured per‑graph knee** (B≈16 for chatterbox), not B=64 (point 2 of §1.1). 55×@64 is recoverable only by a graph that keeps KV device‑resident across strides (no host re‑stream). CUDA‑graph the step @ batch‑1/edge; eager/compile @ high‑batch/DC.
- **Step‑bucket batcher** — flow/CFM, diffusion, masked‑diffusion, STT encoder, and every **inner head**. Batch on the **same‑step axis**, **CFG folded into the batch (×2)**, bucket by `(model, latent‑shape, step‑schedule, CFG)`, run fixed N/K forwards, static CUDA‑graph per step.
- **Nesting rule** — a hybrid is a DAG where the outer AR node ticks the frame clock and **fans each tick's B hidden‑states into the inner node's step‑bucket batch (2B with CFG)**. Bottlenecks are complementary (bandwidth‑bound outer, compute‑bound‑but‑under‑occupied inner), so batching B feeds both.
- **Cohort key** — batch by `(model, frame_rate)`; **never lockstep‑mix clocks** (a 12.5 Hz and a 75 Hz stream have no common realtime tick). Same model ⟹ same frame‑rate ⟹ freely co‑batchable. Cohorts share the GPU **temporally** via the duty ledger, not within a fused step.

### 4.3 Per‑slot fixed ring KV, NOT paged (the biggest divergence from vLLM)
PagedAttention exists because text KV grows to an **unknown** EOS length (60–80% waste without paging) and prefixes are shared. **Voice is the opposite:** context is bounded (utterances are seconds; even ctx 1024 is benign, §1.2), slots are homogeneous (every stream needs the same fixed KV), and prefix sharing is ~zero. So a **fixed per‑slot ring/arena** has zero reservation waste, no per‑step block‑table gather, better locality, and **no allocation jitter** (jitter = frame‑deadline misses). Reach for paging *only* on the long‑variable‑transcript token‑AR STT path. Per §1.2, size slots by the **compute crossover, not the KV wall**.

### 4.4 The roofline + the realtime constraint
The hard constraint is **isochronous** (a step that overruns the frame period = an audible underrun):
```
step_time(N) + scheduling_overhead ≤ frame_period − jitter_margin     (target ~70–80% utilization, RMS‑safe)
step_time(N) ≈ max( WeightBytes / HBM_bandwidth ,  N · FLOPs_per_stream / compute_throughput )
                    └─ memory term (flat in N) ─┘   └─ compute term (grows with N) ─┘
N_max ≈ the N where the compute term reaches (frame_period − margin)
max_batch ≈ 0.8 · T_f / t_step ,   T_f = 1000 / frame_rate
```
Below the crossover, adding streams is ~free (the measured flat‑to‑64). The frame‑rate spread is **12×** (Mimi 12.5 Hz/80 ms is generous → batch 16–32 @0.5B; EnCodec‑48k 150 Hz/6.7 ms is sub‑realtime even at batch 1) — **low frame‑rate is the single biggest realtime‑throughput lever.** First‑audio = `frame_period + acoustic_delay·frame_period + step_time` (Moshi: 160 ms theoretical, ~200 ms measured).

### 4.5 The prefill firewall (a correctness requirement, not a tweak)
Voice "prefill" is small/bounded (a style embedding or short context), so the generation‑stall problem is largely absent in steady state — **but a prefill spike must not break the frame cadence.** Naive prefill+decode hybrid batching inflates per‑token TBT **up to 28.3×** (vLLM P99 TBT 1.76 s = **17–22 dropped frames** at an 80 ms budget = total dropout). **Rule: admit ≤1 new stream's prefill per K frames, and chunk any prefill exceeding one frame‑budget's tokens** (Sarathi‑Serve's token budget keyed on the audio frame deadline, not a text TBT SLO; keep chunk token counts **power‑of‑two** — 257 is ~32% slower than 256, tile quantization). Optionally disaggregate prefill to a side pool (DistServe‑style, ~20–30 ms KV transfer) at DC scale only.

### 4.6 Free token pacing — the frame‑sync bonus
Voice is consumed at ~3.3 tok/s; a continuous‑batching TTS over‑generates and wastes ~2.3× surplus GPU. **A frame‑synchronous loop paces at exactly the consumption rate by construction** — it gets Andes‑style "Token Pacer" benefit for free.

---

## 5. Precision & sample‑rate contracts

### 5.1 Sample‑rate & frame‑rate — the two clocks
**SR = fidelity/transport axis** (8 kHz PSTN, 16 kHz STT, 24 kHz codecs/TTS, 44.1/48 kHz HD); **FR = the AR step/batching clock** (one model step = one codec frame). A model declares two intrinsic constants `(sample_rate, frame_rate)`; the engine **derives** step budget (`T_f=1000/FR`), cohort key (§4.2), `samples_per_frame = SR/FR`, duty, and the resample chain. **Cohort batching is forced** (§4.2): can't lockstep‑mix frame‑rate clocks. **Codec‑decode + resample are post‑batch stages**, off the AR clock, CPU/NPU‑offloadable. **Resample contract:** ingress `any → 16 k`, egress `model‑SR → transport‑SR`; persistent per‑stream rubato (FFT fixed‑ratio default, sinc for fractional, **always anti‑alias when downsampling to 8 k**); telephony egress repacketizes to fixed **20 ms RTP** via a jitter buffer. Chunk sizing = `frames × (SR/FR)`; overlap‑add holdback in frames (zero for causal codecs like Mimi).

### 5.2 Precision — per‑component mixed, tiered by substrate & batch (locked direction)
- **Per‑component mixed precision is mandatory for accuracy.** The big LM GEMMs tolerate int8/fp8; **norms, RoPE, sampling, and the codec/vocoder MUST stay high‑precision** — quant noise compounds across AR frames (the documented vLLM‑Omni reason for fp32 RMSNorm/RoPE + fp32 codec; the funasr int8‑decode‑divergence lesson).
- **Precision tiers like kernels** (§1.6): fp8/mxfp4 help only the **compute‑bound DC‑batch** regime (2.1× @ large M) and *hurt* at batch≈1 (0.62× @ M=64) → **bf16 for edge/batch‑1, fp8/mxfp4 for DC/large‑batch.** **KV‑quant scales concurrency for big‑KV models** (Moshi‑7B 25→101 streams int4), irrelevant for small codec‑LMs.
- **The master constraint — format must match the substrate (or the memory win becomes a latency LOSS).** The **ORT CUDA‑EP cannot run int8/4‑bit GEMM** (`MatMulInteger`/Q‑DQ silently partition to the CPU EP → measured **12 ms fp → 232 ms int8**; the fused `GroupQueryAttention` op is fp16/bf16/float‑only; the quantized‑KV PR was closed unmerged). On GB10, reaching int8/fp8/fp4 tensor cores via ONNX needs the **TensorRT EP** (static, S8S8 only) or the **torch sidecar tier** (owns kernels via torchao/native fp8 — the natural home for GPU quant). Per‑substrate native support (condensed): CUDA fp16/bf16/tf32 always, **fp8 Hopper+/Blackwell, mxfp4/nvfp4 Blackwell**, int8 tensor‑cores‑yes‑but‑not‑via‑ORT‑CUDA; AMX bf16/int8 (~8× VNNI); AVX‑VNNI int8; **Hexagon HMX int8/int16 + true int4**; ANE fp16/int8. This is why WaaV's int8 Voxtral was validated **on CPU**.
- **Two orthogonal levers stack on the fixed‑slot scheduler:** **weight‑quant buys per‑token latency** (memory‑bound decode: int4 ≈4×, int8/fp8 ≈2× cheaper) → widens the flat batch region; **KV‑quant buys streams** (int8/fp8 ≈2×, int4 ≈2.5–3.5× realized) → raises the slot ceiling. (GQA is the biggest KV lever *before* quant — Qwen3‑0.6B's 8 KV heads make its KV/token ~9× a 0.5B's.) On 8 GiB edge a 3B model is *only practical quantized*; on 80 GiB DC, weights are a rounding error → quant is a pure KV/throughput lever.
- **Runtime quant abstraction (KISS — a minimal extension of the live `Manifest`, which already has a `precision` token + per‑component `weights` map):** (a) load **published** quantized checkpoints as‑is (AWQ/GPTQ/GGUF/fp8/int8 — manifest selects the variant, zero‑code); (b) per‑component precision via `component_precision{logical→prec}` (Path‑A) / a `keep_high_precision` ignore‑list + runtime `quantization=` (Path‑B torch) — **defaults per‑architecture encode the rule above so norms/RoPE/codec/head stay high‑precision with zero user config**; (c) **per‑substrate** `by_substrate{ep→precision}` resolved against the active EP (`$WAAV_PRECISION` → `by_substrate[ep]` → `precision` → fp32) so an int8 file never lands on ORT‑CUDA. Mirror vLLM‑Omni's `ComponentQuantizationConfig` (longest‑prefix `{prefix→config}` router + ignore‑list) — ~150 Rust/torch lines via torchao/bnb; skip paged‑KV/custom‑kernels/MXFP4‑Ascend.
- **The accuracy gate (promote the existing offline harnesses `stt_eval.py`/`tts_roundtrip.py` to a load‑time, fail‑closed gate):** verify a quant variant vs the `reference_precision` on fixtures before it serves, persist a `verified{substrate,precision,metric}` stamp (production load = cheap stamp‑check). **The TTS gate MUST include a perceptual/MOS check** — a text‑only (WER) gate passes the exact bugs WaaV hit (the WER‑flat/MOS‑crash AR‑drift signature). Unverified ⇒ refuse or fall back to `reference_precision` + emit `waav_quant_gate_failed`.

---

## 6. Scheduler & resource model (extends `INFER_SPEC §8`)

**Per‑stage SLOs:** the session SLO (TTFA p90 ≤ rated budget; streaming viability ≥ 99.9%) decomposes into per‑stage budgets `T_step(stage,B) ≤ S·(1000/steps_per_second)`, `S≈0.8`. The **bottleneck stage** (often the CFM/codec/vocoder, *not* the AR stage) is the binding constraint — so every stage carries its own SLO + duty entry.

**SLO‑aware admission (reject, don't glitch)** — a two‑level test, now per‑substrate + bottleneck‑aware:
```
ADMIT a realtime stream IFF:
 (1) ∀ stage s: free slot ∧ KV‑quota+codec‑window+workspace reservable ∧ active(s) < calibrated max(s)
 (2) ∀ SUBSTRATE d: Σ_{realtime stages on d} duty(stage) ≤ S      duty = T_step(B_active) × tick_rate   (measured)
 (3) on UNIFIED memory: Σ_{all stages on the shared pool} bandwidth_duty ≤ S·ceiling     (the §3.4 contention guard)
 ⇒ REJECT if admitting breaks the frame budget on ANY stage (esp. the BOTTLENECK). Typed 429/503 + Retry‑After.
   NEVER admit‑and‑degrade (P‑4). (2) is per‑substrate (NPU & GPU stages don't share compute); (3) is per‑shared‑pool.
```
**Per‑substrate duty ledger** (extends §8.3c): one compute‑duty ledger per substrate + one shared bandwidth ledger across substrates on a coherent pool; every stage type is in the relevant sum (AR‑only admission the codec can't sustain is the exact bug this prevents). Calibration (§8.3b) measures `T_step(B_active)` per stage per substrate under synthetic co‑load, persisted keyed `sha256 × device × driver × warm‑set`. The torch‑sidecar reports its footprint+duty at handshake.

**Graceful overload (productized):** deadline‑aware admission (Clockwork/Niyama/SCORPIO) — **reject/relegate to a degraded queue, don't drop frames for everyone** (Niyama: 8.6% vs 80% SLO violations at 50% overload). Drift response (FR‑S3b): sustained p99 breach on the bottleneck stage → stop admitting → shed Batch → only then shed newest Realtime ≤1/tick, 60 s hysteresis. **Shed is the backstop; admission is the mechanism.** **DC spill/rebalance:** Llumnix‑style **constant‑time (~20–30 ms) append‑only KV migration** moves a stream between replicas without a glitch. Priority `Realtime > Batch` applies per stage (Sarathi piggyback of Batch into leftover budget). Barge‑in/cancel is a control message that jumps every stage's queue and frees the stream's slot/KV/window within ≤1 tick.

---

## 7. Borrow vs build

| BORROW (replicate pattern / port math) | ADAPT (take idea, shrink) | AVOID (don't pull / don't inherit) |
|---|---|---|
| vLLM `Platform` HAL + `CustomOp` once‑at‑load dispatch + pure‑`forward_native` fallback (§2) | vLLM token‑budget step loop *only if* batching a few concurrent streams | vLLM continuous‑batching scheduler + `KVCacheManager`/`BlockPool` (~5,200 LOC multi‑tenant machinery) |
| vLLM `ModelRegistry`/config‑arch dispatch (WaaV already does this) | `precision`/`quantization` knob over original safetensors | **Paged‑KV + `CommonAttentionMetadata` (slot_mapping/block_table)** — reusing any vLLM attention `Impl` adopts the whole block‑pool |
| Op **math** from `ir/ops`/`forward_native`: RMSNorm/RoPE/SiLU (fp32 norm/RoPE for fidelity) | cuDNN‑vs‑FA‑vs‑SDPA routing table per arch | **FlashInfer** — paged+ragged+plan() overhead; ~2× e2e regression on sm_120 aarch64 |
| omni **stage patterns**: declarative DAG, per‑stage scheduler, in‑forward nesting, delta‑streaming + overlap‑add, bs=1 fast‑path bypass (§3, §4) | omni zero‑copy relay → WaaV `SharedHostBufType` (§3.4) | Compiled `vllm._C`/`_moe_C` (needs vLLM CUDA build; breaks aarch64/Blackwell) |
| Moshi **lockstep primitives**: `StreamMask`, `ScatteredKvCache` ring KV, CUDA‑graphed fixed‑shape step, marker/flush (§9) | ggml `backend_sched` placement order (§3.4) | HTTP/grammar/OTel/Ray/multimodal/distributed dep stack (~60 pkgs of serving weight) |
| Existing WaaV seams (KEEP): `StaticGraph`, EP HAL, `SttModel`/`TtsModel`, torch sidecar, protocol, components, server safety floor | Triton decoupled (1:N) transaction policy + FINAL marker; BLS = control‑flow‑in‑node | Continuous batching as the steady‑state AR default (it's for variable‑length text) |

**Both omni engines are CUDA‑only → WaaV's ORT‑EP/ggml portability (Path‑A ONNX + Path‑B torch sidecar) is the differentiator.** Borrow their *model defs*, not their backend.

---

## 8. Config tiers (edge ↔ DC, same DAG)

**One manifest DAG; the engine picks the execution mode from config + load — the edge never pays for DC machinery.**

| Mode | When | Stage execution | Batching | Scheduler/ledger |
|---|---|---|---|---|
| **Inline** | `mode=edge` OR single‑stream, no co‑tenant | All stages **inline on the calling thread** in DAG order (`batch_policy→inline`); nested loops still in‑forward | None (B=1) | **None** — no queues, no tick loop, no admission, no ledger |
| **Pipelined‑single** | 1..handful streams, one model | Per‑stage threads + bounded queues (pipeline overlap), B_max=1 | No cross‑request batch | Light: queues + watchdog; memory ledger only |
| **Stage‑batched‑pipelined** | `mode=dc` OR many streams OR multi‑model | Full §3.2 decoupled micro‑engines | Per‑stage decoupled batching (AR lockstep B_max>1; codec micro‑batch) + §3.4 placement + zero‑copy | Full §6: per‑substrate duty ledger, bottleneck admission, drift response |

`mode=auto` (default): start Inline/Pipelined‑single, **promote to Stage‑batched lazily when a 2nd concurrent stream arrives or a co‑tenant model loads** (the ledger spins up on demand). The DAG, stages, nested loops, and placement hints are **identical across modes — only the executor differs** (Inline calls the same stage‑forward with B=1; there is no second implementation). This is the internal‑scheduler analog of `INFER_SPEC`'s proven T1‑sidecar/T2‑in‑proc config‑selected topology. Same one binary scales **GB10 (273 GB/s, latency‑only, N=1, bf16+CUDA‑graph) ↔ B200 (~8 TB/s, big lockstep N, fp8/mxfp4, push toward compute‑bound)** — only the batch ceiling and precision tier change.

---

## 9. Kyutai‑systems support (the 9‑item checklist to run them all)

To run *all* Kyutai systems (Mimi, Moshi full‑duplex, STT, TTS, Hibiki — which exceed the upstream Rust server, where full‑duplex Moshi is served one‑conversation‑per‑process):

1. **Frame‑synchronous lockstep scheduler** — fixed B slots, per‑stream boolean exec‑mask over a rectangular batch, admit‑into‑free‑slot + backpressure, per‑slot reset, 2 ms idle sleep, **wall‑clock pacing**. (The headline new scheduler.)
2. **Per‑stream rotating KV cache by scatter indices** — one `(B,H,context,D)` tensor, per‑stream `offset`/`index` (mod context), `scatter_set`, per‑stream causal+window mask; masked streams are no‑ops. (Port `ScatteredCacheBuilder`.)
3. **Streaming codec runtime (Mimi)** — causal conv + transformer streaming state, batched `encode_step`/`decode_step` + per‑slot reset, 24 kHz, 1920‑sample frames, Split‑RVQ.
4. **RQ‑Transformer with a depth/codebook decoder** — temporal transformer (1 fwd/frame) + Depformer (K fwds/frame, weights‑per‑step), summed multi‑codebook input embeddings. (The second new primitive.)
5. **Multistream interleave + delay engine** — K=2Q+1 streams with per‑stream integer delays, write/read at `(offset+delay)%context`; one knob (text↔audio delay sign) switches STT/TTS/S2S/translation.
6. **Full‑duplex I/O contract** — per frame: ingest user Mimi tokens into input slots, emit Moshi tokens from output slots, simultaneously; barge‑in = the user stream is always modeled.
7. **Extra heads** — generic per‑step linear heads for semantic‑VAD / end‑of‑turn.
8. **Protocol** — WS + binary, 80 ms/1920‑sample/24 kHz blocks, opus+PCM, the **marker/flush** end‑of‑stream primitive, `used/total_slots` for autoscaling.
9. **Static‑shape acceleration** — CUDA‑graph (or ORT IO‑binding) the fixed‑shape temporal + depth step; warmup 2–3 steps; persistent KV; 4/8‑bit quant for edge. Portability seam = identical safetensors weights + identical streaming‑step contract per backend (CUDA/Metal/CPU), exactly as Moshi does across PyTorch/Candle/MLX.

---

## 10. KISS / progressive‑evolution roadmap

The engine extends today's WaaV Infer (backend seams correct + live; serving discipline is the greenfield) in milestones, each shippable and each gated by acceptance criteria, never building DC machinery before a named trigger (P‑2):

- **M2 — Stepped seam + lockstep scheduler (the unlock).** Introduce the `ArStep`‑style stepped contract beside the coarse `synthesize`/`transcribe`; build the fixed‑slot masked lockstep batcher + per‑slot ring KV + wall‑clock tick (Inline + Pipelined‑single modes). Path‑A one‑shot models ride a micro‑batch stage; Path‑B torch sidecar exposes a multi‑session step verb. **Accept:** GB10 serves ≥16 concurrent codec‑AR streams at RTF<1 within the frame budget; single‑stream edge unchanged. *(Grounds: §1.1, §4.2.)*
- **M3 — Stage DAG + step‑bucket batcher + streaming egress.** Manifest `[[stage]]` schema; decoupled per‑stage micro‑engines + pipeline overlap; the step‑bucket batcher (CFG‑folded, length‑bucketed) for CFM/diffusion/encoder; convert `synthesize` to incremental streaming + TTFA ramp + real barge‑in. **Accept:** a 3‑node CosyVoice2 DAG and a 2‑node nested dots.tts DAG both stream first‑audio sub‑300 ms; the codec stage no longer head‑of‑line‑blocks AR. *(§3, §4.1.)*
- **M4 — Heterogeneous placement + per‑substrate duty ledger.** The `StagePlacer` (ggml decision order) + `SharedHostBufType` zero‑copy on GB10 + the per‑substrate compute ledger + shared‑bandwidth budget + bottleneck‑stage admission + calibration lifecycle. **Accept:** codec/encoder placed on NPU/CPU frees GPU bandwidth for ≥1.3× more AR streams on GB10; admission rejects rather than glitches at saturation. *(§3.4, §6.)*
- **M5 — Full‑duplex S2S + DC scale.** The RQ‑Transformer depth decoder + multistream/delay engine + full‑duplex I/O (run all Kyutai systems, §9); Stage‑batched mode at B200 scale with Llumnix‑style KV migration spill/rebalance + fp8/mxfp4 DC precision tier. **Accept:** Moshi full‑duplex served lockstep‑batched (exceeding upstream); one methodology config‑scales GB10↔B200. *(§8, §9.)*

**The north star:** one binary, one model‑contract, one batching methodology — frame‑synchronous lockstep for the AR spine, step‑bucket for the generative heads, nested in a heterogeneous stage‑DAG, cohort‑batched by frame‑rate, precision‑ and substrate‑tiered, reject‑don't‑glitch — config‑scaling from a phone to a B200 fleet, KISS and progressively downloaded.
