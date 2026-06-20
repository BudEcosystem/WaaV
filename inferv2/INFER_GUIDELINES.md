# WaaV Infer — Engineering Guidelines & Code Patterns

**Status:** canonical · **Date:** 2026-06-18 · The standing rules every model/backend/scheduler PR follows. Distilled from the architecture (`INFER_ENGINE_V2.md`), the failure catalog (`INFER_FAILURE_CATALOG.md`), the deep designs (`design/L1-L7`), and the measured perf strategy (`INFER_PERF.md`). Where a rule cites a gate name, it's a RED-first test in `INFER_ENGINE_IMPL.md`.

---

## 0. The two invariants that override everything

1. **Accuracy is sacred.** The engine's default path is **exact / bit-faithful at the model's native precision**. No quantization, no speculative decoding, no approximate/linear/windowed attention, no pruning/distillation — by default. Any deviation is opt-in, per-model, gated by a `verify-vs-native` accuracy stamp (and for TTS, a perceptual/MOS check — WER-flat/MOS-crash is the silent-failure signature).
2. **Correctness under multi-tenancy is sacred.** The hardest bugs (exec-mask corruption, KV cross-contamination, slot leaks) are *invisible in single-stream tests* and only appear under concurrent load. Every per-slot mechanism is tested at `max_num_seqs≥4` with the bit-identical-to-serial gate.

**The one test that enforces #0.1 — the AR-compounding identity test (non-negotiable).** Run the full N-step AR loop and compare the **emitted integer codes** against the eager reference: they must be **IDENTICAL, not just close.** This catches precision loss that's invisible per-op but audible after compounding (the #2274 trap). It is the universal gate on every fusion/perf change. Mirror via mel/waveform `allclose` (CFM/diffusion) and WER-disagreement (STT).

---

## 1. The model-implementation contract (how to make a model batchable)

A model is batchable in WaaV's lockstep iff it satisfies the stepped seam below. This is **additive** — the 16-arm config-arch registry ("model = data") is untouched; one-shot models (kokoro/melo/whisper/supertonic/sensevoice) return `as_stepped() → None` and ride a micro-batch stage. **Only AR/codec-LM/duplex models** do this work.

```rust
// waav-infer-runtime/src/arstep.rs — the seam (additive; does NOT touch the coarse traits)
pub trait ArStepModel: Send {
    fn prefill(&mut self, slot: SlotId, cond: &Conditioning) -> Result<PrefixKey, InferError>;
    /// Advance ALL active slots ONE stride. The driver passes the exec-mask; the model MUST
    /// treat masked rows as no-ops (state frozen) and accept substituted init tokens.
    fn step(&mut self, batch: &SlotBatch) -> Result<Vec<StepOutput>, InferError>;
    fn reset_slot(&mut self, slot: SlotId);          // transactional fan-out [F3]
    fn kv_footprint_per_slot(&self) -> KvFootprint;
    fn stride_class(&self) -> StrideClass;
}
// LoadedModel gains: fn as_stepped(&mut self) -> Option<&mut dyn ArStepModel> { None }
```

**The per-arch reshape (four moves, none a kernel):**
1. **Decompose `generate()` into `prefill` + single-step `step`** — pull the AR loop *out* of the model into the scheduler.
2. **Swap the model's `cat`-append KV for the scheduler's ring-KV** — `scatter_set` at `(offset+delay) % ctx`; attention reads via a reconstructed logical-position mask (never assume a monotonic append; the ring wraps).
3. **Make every per-slot mutable tensor a `MaskedCell`** — the only mutator is `set_where(mask, new)`, so an ungated mutation *won't compile*. Plus pre-step init-token substitution for masked-or-warming rows.
4. **Route sampling through the graph-safe path** — argmax/gumbel in-graph, or multinomial outside the captured region; add the NaN→reject-frame guard; keep norms/RoPE/sampler/codec in fp32.

**Full-duplex** generalizes `ArStepModel` (K=1) to `DuplexStepModel`/`MultiStreamSlot{(role, delay_sign, ring)}` — K-lanes and D-codebooks fold as **inner dims, not batch axes**, so one CUDA-graph still covers the `[B,…]` forward. **Path-B (torch sidecar)** adds slot-keyed Python state (codec/sliding-window buffers keyed by slot-id, freed on reset) — else concurrent requests cross-talk.

---

## 2. The "masked ≠ absent" law (the central correctness discipline)

In lockstep, idle slots stay in the rectangular batch; **the dense kernel reads/writes every row.** Therefore:
- **(a)** Substitute a valid **init/BOS token** into masked-or-warming rows *before* embedding (`is_init |= ~exec_mask` → `where(is_init, initial, gathered)`). A masked row's embedding-lookup on a sentinel `-2`/stale value → CUDA illegal-memory/NaN that **kills the whole batch (all 64 users).**
- **(b)** Gate **every** per-slot mutation through one `where(exec_mask, new, old)` — offset, ring write-index, conv ring, RoPE phase, sampler RNG offset, partial-word buffers. A *single* ungated mutation = silent corruption that only appears when a stream idles-then-resumes under multi-tenant load.
- **Test:** `masked_row_gets_substituted_init_token`, `idle_then_resume_transcript_identical`.

**Slot recycling is transactional:** one `reset_slot(i)` fans out to *every* per-slot subsystem (KV pointers, conv rings, sampler, word buffers, **nested inner-solver latent — inner-before-outer**, codec window, host state). A monotonic `channel_id` drops any output/marker whose id ≠ the live occupant (cross-user contamination otherwise = a privacy bug).

---

## 3. The exact-performance rules (measured; see `INFER_PERF.md`)

**Perf = batching + memory-bandwidth physics + the right *exact* kernel. Not custom kernels.**

| Rule | Why | Gate |
|---|---|---|
| **Pin the SDPA backend** (cuDNN/flash per arch); **never auto-select**; **never FlashInfer on sm_12x** | math-backend fallback is 40–135× slower; FlashInfer has 3 compounding sm_12x failures incl. a GQA=16 crash; FA3/FA4 don't exist for sm_12x | `sdpa_backend_pinned_per_arch`, `flashinfer_excluded_from_sm12x` |
| **Keep KV on-device** — `StaticGraph::run_bound` (IoBinding) + a persistent KV buffer written at `cache_position`; never round-trip the cache host↔device per step | the stateless `run()` seam round-trips the whole KV every step: 13%→2× tax (grows with batch×ctx). **The #1 engine perf change.** | `kv_stays_on_device_across_steps`, `run_bound_output_bit_identical_to_run` |
| **Batch the AR step** — it's the headline lever; size slots at the **measured per-graph efficiency knee** (B≈16 for the real chatterbox codec-LM, ~1.8× peak; **NOT** the idealized 55×@64/~B64 — host-KV re-stream makes B=64 *slower* than per-slot on exported graphs, INFER_PERF.md §3 / INFER_PERF_VALIDATION.md §3a), not the KV wall | decode is bandwidth-bound (`t_step ≈ WeightBytes/bandwidth`, flat in B); batching fills idle compute for free — the lever the no-quant constraint leaves intact; the realistic per-graph win is ~1.8×, with 55×@64 only on a device-resident-KV re-export | (the lockstep batcher itself; live gate `live_headline_batched_scaling_matches_doc_curve`) |
| **Lay out KV at native kv_heads** (GQA); never MHA-expand | 5.5–6.9× attention + 7× concurrency, free | `ring_kv_laid_out_at_native_kv_heads` |
| **Reuse the deterministic-prefix KV** (radix cache, ~86% hit) | skips prefill bit-identically (~7× TTFA on cloned-voice/agents) | (R1 hybrid-KV) |
| **Tier kernels by batch** — CUDA-graph/torch.compile at edge/low-batch; eager at high-batch DC | graphs help @B1 (1.18–1.24×) but *hurt* @B32 (0.73×) — launch-overhead removal only pays when launch-bound | `cuda_graph_only_below_batch_knee` |
| **Zero D2H syncs in the per-step loop** — `dst.copy_(src)` not `fill_(item())`; `torch.where` not tensor-branches; no `.item()/.cpu()/.tolist()` | +14% tax in inner CFM/codec loops; also the prerequisite that *unlocks* graph capture | `zero_d2h_sync_during_decode` |
| **Pre-allocate all buffers** (no per-step malloc); cache CFM timestep schedules/masks | `cudaMalloc` is a device-wide sync; static addresses are required for graph capture | (persistent-buffer pattern) |
| **CPU compute speedup = bf16 with fp32-accumulate ONLY** (AMX-BF16 / Grace-BFMMLA via MLAS-SBGemm); never int8; ACL fast-math OFF | int8 = quantization (forbidden); bf16-fp32-accum is exact-equivalent to a bf16 model | `cpu_bf16_fp32_accumulate_only` |

**Fusion is allowed; losing the fp32 reduction is NOT.** Fuse linears + SDPA + elementwise (SiLU/GeGLU), but keep **RMSNorm variance + RoPE cos/sin reductions in fp32, and the final weight-multiply in NATIVE dtype** (the #42325 trap: fp32 weight-mul diverges 0.03–0.06/layer). Mechanism: `torch.compile(epilogue_fusion=False)` + fp32 custom RMSNorm/RoPE opaque to Inductor (Path-B); strongly-typed TRT-11 + `kTF32` unset (Path-A). **`gemm_dims_padded_to_8`** (zero-pad + −inf-mask) is exact and large.

---

## 4. Backend selection (per hardware × path)

**Path-A = Rust + ONNX Runtime** (the portability tier — CUDA/TensorRT/QNN/CoreML/OpenVINO/DirectML/XNNPACK via the `StaticGraph` seam). **Path-B = Python torch sidecar** (CUDA/ROCm/CPU only — *not* the portability path).

| Hardware | exact attention | graph/backend (fixed-shape step) |
|---|---|---|
| **GB10 / sm_121** | cuDNN-SDPA / FA2 (no FlashInfer) | Path-A: `ORT_ENABLE_ALL` + **IO-binding + persistent KV**; Path-B: `compile(reduce-overhead, dynamic=False, fullgraph=True)` or manual CUDA-graph |
| Hopper sm_90 | FA3-split-KV / cuDNN | + larger batch; FA3 prefill |
| MI300X CDNA3 | AITER / CK | hipBLASLt; AITER paged-decode |
| RTX (Ada/Blackwell) | built-in FA2 / cuDNN | VRAM-capacity slot cap |
| Grace / x86 / ARM CPU | fused `cpu_flash_attention` | MLAS-SBGemm bf16 / AMX; thread-pin 1/physical-core; NUMA-bind |
| variable encoder (any GPU) | (compute-bound) | Path-A: TensorRT-EP static `min=opt=max` + engine cache; Path-B: `compile(max-autotune, dynamic=True)` per bucket; **don't graph large batch** |

---

## 5. Streaming, lifecycle & ops rules (from the failure catalog)

- **Streaming is delta, never cumulative** (cumulative = O(N²); the #1 silent bug). Test `offline_concat == stream_concat` byte-identical. **End-of-stream is an explicit FINAL frame** (per-terminal, after-tail-drain), never inferred from silence; **cancelled ≠ completed** (distinct terminal — barge-in must be distinguishable).
- **Cooperative cancellation checked every frame** + RAII single-owner slot-free on any exit + a leak-reconciler (slots vs connections). **Frame-progress watchdog** (per-GPU "frames produced" heartbeat → fence+migrate on stall) is the only defense against silent GPU hangs. **`PR_SET_PDEATHSIG`** on the sidecar.
- **Numerics:** always-on NaN→reject-frame (not vLLM's emit-garbage); fp32 sampler/CFM/ODE math; `_SAMPLING_EPS=1e-5`/`_MAX_TEMP=1e-2`/≥1-survivor/NaN-safe `not(x<y)` pivot.
- **Admission rejects, never glitches** — schedulability `ΣU≤bound`, non-preemptible whole-stream fit, graded degradation (shed-LO → brownout → EDF+slack-drop → reject last), risk-EDF ordering.
- **Readiness = warmed + calibrated** (not process-up); the canonical fixed shape-bucket set is the audio-I/O↔model contract (defangs autotune/compile/graph-capture at once).
- **Transport:** media on UDP/QUIC, control on TCP; bounded drop-oldest send ring (never HWM=0); `TCP_NODELAY`; no proxy buffering/compression on the audio path.
- **Observability:** coordinated-omission-corrected histograms (bucket edge @0.08s); SM-efficiency not GPU-util; TTFA from first *playable* frame.

---

## 6. The forbidden list (rejected, with the reason)

- **Quantization / speculative decoding / approximate attention** — accuracy-impacting (the invariant). (Quant is allowed only as an opt-in, accuracy-gated, off-the-realtime-path DC throughput lever via vendored TensorRT/torchao — never the default.)
- **Fused RoPE+QKV (lossy-fused)** — rejected on *accuracy* (#2274 fp32 compounding), not perf.
- **FlashInfer on sm_12x** — 3 compounding failures + a GQA=16 crash.
- **Paged-KV / `CommonAttentionMetadata` / continuous-batching scheduler** for the AR steady-state — dead weight at fixed-cadence batch≈1 (the ring + lockstep is correct here).
- **Custom hand-written kernels** — none are required for the realtime target; reach for vendored kernels (TensorRT/cuDNN/AITER/MLAS) before ever writing one, and only for an off-path quantized-GEMM if ever.
- **CPU int8 / ACL fast-math** — quantization / fp32→bf16 downcast = accuracy loss.
- **Auto-selecting the SDPA backend** — can silently fall to the math backend (40–135× slower).
- **In-loop host syncs, per-step malloc, cumulative re-decode, busy-spin scheduler loops, fire-and-forget cancellation** — the catalog's recurring scars.
