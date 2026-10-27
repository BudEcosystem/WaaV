# vLLM-Omni vs WaaV Infer — Architecture, Optimizations, Gaps & Roadmap to Beat the Best

**Date:** 2026-07-01
**Article analyzed:** https://vllm.ai/blog/2026-06-23-vllm-omni-tts ("Engineering TTS Inference in vLLM-Omni")
**Source analyzed:** `github.com/vllm-project/vllm-omni` @ HEAD `e4a2d36` (2026-07-01, tag v0.22.0, ~341K LOC Python) — cloned to `/home/bud/ditto/serverless/vllm-omni`
**WaaV analyzed:** `/home/bud/ditto/waav/waav-infer` (~200K LOC Rust, 14 crates)
**Method:** 9 parallel deep-read agents (5 vs WaaV Infer, 4 vs vLLM-Omni source) + article + vLLM-Omni GitHub roadmap/bug-tracker. Every WaaV claim below is file:line-verified against real code; the vLLM-Omni claims are verified against its real source (V3 audited the blog's claims line-by-line).

---

## 0. Executive verdict

**WaaV Infer has the better *bones*; vLLM-Omni has the better *breadth and streaming ergonomics*.** WaaV is a voice-native, single-process, multi-runtime, byte-identical engine built from scratch for frame-synchronous audio. vLLM-Omni is a sprawling set of TTS adapters bolted onto the text-LLM vLLM engine, whose steady state (PagedAttention + continuous batching + one-vLLM-engine-per-stage) is the *wrong* regime for voice and carries a brutal memory tax — but which ships a mature streaming-chunking UX and a large model zoo that WaaV has not yet matched.

**Net position:** WaaV wins the architecture argument and the correctness argument decisively. It loses on **realized first-audio latency (streaming TTFP)**, on **CUDA-graph coverage/bucketing**, and on the **design-vs-wired gap** (WaaV has *superior* machinery that isn't plugged into the live path). Closing three P0 items makes WaaV strictly better than vLLM-Omni for the voice mission.

| | WaaV Infer | vLLM-Omni |
|---|---|---|
| **Substrate** | Single-process, in-process libtorch (tch) + ORT, GPU-resident handoff | Multi-process: **one full vLLM engine per pipeline stage**, CPU-serialized `/dev/shm` handoff |
| **Memory (0.6B 2-stage TTS)** | ~1 CUDA context, shared allocator | **22 GB** (2× CUDA ctx + 2× KV @0.90 each), vs 2.6 GB in-process — their own bug #2318 |
| **Correctness bar** | **Byte-identical** to reference engine, gated in CI | None; blog demonstrably oversells (§3) |
| **State safety under dynamic batching** | **Compile-time** (`ChannelId`+`MaskedCell`; illegal mutation won't compile) | Runtime convention (dict-keyed + hooks); **Higgs is actually row-keyed and fragile** (§3.4) |
| **Streaming first-audio (TTFP)** | Plumbing world-class, **but no shipped model emits audio early** → TTFA = whole-utterance | **Mature tunable chunk-knob taxonomy → 64 ms TTFP** (Qwen3-TTS only) |
| **Hardware reach** | ORT: CPU/CUDA/TRT/ROCm/OpenVINO/QNN/CoreML/DirectML (Path-A); tch: CUDA (Path-B) | CUDA-first; ROCm/XPU/NPU/MUSA in progress |
| **Model zoo** | ~46 arch families / 60+ checkpoints | ~12 TTS + full image/video diffusion + S2S |
| **Kernels** | Zero custom kernels; hand-rolled byte-identical CUDA graphs | Zero CUDA/C++; Python + **14 Triton kernels** + torch CUDA-graph |

---

## 1. What vLLM-Omni is

A **separate** repo (not a feature inside vLLM) that sits **on top of** mainline vLLM. It hosts ~12 TTS models (Qwen3-TTS, VoxCPM2, Fish Speech S2 Pro, Higgs Audio V3, CosyVoice3, MOSS-TTS, Ming-TTS, GLM-TTS, IndexTTS2, Voxtral-TTS…), a full image/video **diffusion** subsystem, and experimental full-duplex. Our local `serverless/vllm-src` is mainline vLLM and had **none** of this TTS code.

### Core thesis (their central bet)
> **"TTS is a pipeline, usually with multiple model stages."** Don't apply one fixed optimization recipe to every model. Decouple the **latency-bound** stage (*Talker* = single-token AR decode) from the **throughput-bound** stage (*Code2Wav* = parallel decoder) so each batches **independently**, and let a **Connector** chunk codec tokens at its own cadence to hit sub-second Time-To-First-Audio-Packet (TTFP).

### How it's actually built (V1 + V4, file:line-verified)
- **Each stage = its own OS process + its own vLLM engine + its own scheduler + its own KV cache**, spawned via `multiprocessing` (one process per stage×replica×DP-rank). A pipeline is a **frozen DAG declared in code** (`config/stage_config.py` `PipelineConfig`, validated for entry-stage/no-self-loop/refs-exist) plus a **thin deploy YAML** of knobs (devices, gpu_mem_util, max_num_seqs, connectors).
- **Stage-aware scheduling is structural** because stages are separate processes: `LLM_AR → OmniARScheduler` (delegates to vLLM continuous batching, ~1 tok/seq/step, latency-bound) vs `LLM_GENERATION → OmniGenerationScheduler` (a one-shot fast path that schedules the whole prompt in one forward and **finishes on the first step**, throughput-bound). Each has independent `max_num_seqs`/token-budget/KV/device.
- **Connector = CPU-mediated, non-zero-copy** by default: codec codes are a `torch.long` tensor but cross the boundary as **msgpack bytes of a CPU tensor** (`.detach().cpu()...tobytes()` → `torch.frombuffer`), transported via `SharedMemoryConnector` (`/dev/shm` + `fcntl.flock`, ~4 memcpies), overlapped via **daemon threads** (not CUDA streams). Source literally says `TODO: enable zero-copy`. GPU-direct RDMA (Mooncake/Mori/Yuanrong) is opt-in for multi-node PD-disaggregation.
- **Head-side Orchestrator** (`AsyncOmniEngine`) is a single busy-poll loop round-robin-polling every stage×replica at 1 ms.

---

## 2. The optimization catalog (understood in full)

The article's 20 techniques, grouped. All are **verified present in code** (V2/V3), with the **reality-check from V3 in bold** where the blog oversells.

**Scheduling / batching**
1. Stage separation + independent per-stage batching (Talker vs Code2Wav). ✅ real (separate processes).
2. CFM/LocDiT **decode-tail batching** across requests (their diffusion runs B=2 under CFG — too small). ✅ real; **all-or-nothing** — any prefill/multi-token request drops the whole step to the per-request path.
3. **Frame-count-bounded** DAC batching (cap frames/forward). ⚠️ real but **thresholds default 0 → one unbounded group** (off by default).
4. Async chunk overlap (connector transfer ∥ decode). ✅ real (daemon threads).

**Launch-overhead**
5. **Whole-forward `torch.compile` (fullgraph=False)** → −71% `cudaLaunchKernel`. ⚠️ **bypassed on GPU** — CUDA graphs are used instead; `dynamic=` not found; the compile path only runs on non-CUDA.
6. **CUDA-graph shape bucketing** (`bisect_left` → nearest padded (batch,frames) bucket, 81% hit). ✅ real — but on the **Code2Wav vocoder**, plus an **LRU graph cache (max 4) with eviction** for IndexTTS2 DiT.
7. **Local-MLP graph** (graph only `post_attn_norm+mlp`, attention eager). ✅ real and it's the **default**; "beats PIECEWISE" is **unproven spin** — it keeps a 36-layer Python loop + 36 `replay()`s + ~72 `copy_`s per step.

**Sync elimination**
8. Remove `.item()` GPU→CPU syncs (~2400/req in VoxCPM Euler loop → GPU `.copy_()` broadcast). ✅ real **for the ODE loop**; default step still does a batched `stop_mask.cpu()` + coalesced audio D2H per emit.
9. **GPU-resident decode state machine** (Higgs: last_codes/has_codes/delay-count/EOC-countdown/done-flags on GPU, batched `torch.where`/`masked_fill_`). ✅ **solid** — the one unambiguously excellent Higgs optimization.

**Precision (mixed, per-component)**
10. **fp32** code predictor / **fp16** DAC. ⚠️ fp32 is **conditional** (only when model is fp16; default is **bf16** with fp32 islands); fp16 DAC ships **off** (defaults fp32, autocast disabled).

**Kernels**
11. Fish **q_len=1 Triton decode kernel** (head_dim=128, block16, GQA, split-K >1024, graph-capture-safe). ✅ real, and the **crown jewel** — but it's on the **Slow AR** (blog implies Fast AR), and `head_dim==128`/`block==16` are literals (rigid point-solution).

**Memory / complexity**
12. **VAE sliding-window decode** (O(N²)→O(N)). ✅ real (12-frame pad, patch-size new frames); **re-decodes the 12 pad frames every call** (~50% redundant VAE FLOPs at default).
13. Trailing-text offset tracking (`_TRAILING_TEXT_COMPACT_MIN_FRAMES=64`). ✅ **clean, no oversell**.
14. Precomputed disallowed-mask + `masked_fill`. ✅ **clean, no oversell**.

**Preprocessing**
15. Speaker-embedding GPU caching + batched mel/STFT. ⚠️ GPU mel ✅ but **batch-1 per request** ("batched" oversold); + a needless `.item()` in `AttentiveStatisticsPooling`.
16. Batched "Stage-0" preprocessing. ✅ real.

**IPC**
17. **Tensor-payload codec transfer** (replaces `list[int]`). ⚠️ wired but **default-off** (`fish_speech_tensor_codes=False`); fallback still `.tolist()`s + per-frame `.item()`.

**Multi-codebook**
18. Fused `[N×V,D]` embedding + offset lookup + MusicGen delay `[0..7]` + BOC/EOC. ✅ **real and clean** (Higgs).

**Dead code (blog claim with zero runtime effect):**
19. Fish Fast-AR `torch.compile(fullgraph=False, dynamic=True)`. ❌ **DEAD** — targets `self.model.forward` while decode calls `forward_one`; `warmup_compile` skipped; `_compiled_model_fwd` never called.

**Headline benchmarks (H20):** Qwen3-TTS +61.5% throughput / −41% median E2E / −26% P99 TTFP; VoxCPM2 +172% audio throughput; Higgs 2.70×; Qwen3-TTS "production" RTF 0.34 / TTFP 131 ms.

---

## 3. Per-axis comparison (the matched axes)

Verdict is WaaV's position **relative to vLLM-Omni**.

| # | Axis | WaaV verdict | One-line |
|---|------|--------------|----------|
| A | Stage decomposition / heterogeneous pipeline | **AT-PAR (design ahead, not wired)** | WaaV's `StageNode`+`Paradigm` DAG taxonomy is richer than vLLM's implicit split, but is pure-logic with zero live callers; live serving is per-model, internal stages. |
| B | Stage-aware independent batching (latency vs throughput) | **AT-PAR** | Both split AR-latency from decode-throughput; vLLM does it via separate processes (heavy), WaaV via lockstep-AR + step-bucket batchers (in-process, lighter) — but WaaV's cross-request flow batching is live only for one-shot models. |
| C | **Dynamic-batch state safety** | **AHEAD** | WaaV enforces request-identity at **compile time** (`ChannelId`/`MaskedCell`/`is_live`); vLLM relies on dict-keying + hooks and **Higgs is actually row-keyed/fragile**. |
| D | **Streaming first-audio (TTFP)** | **BEHIND** ⬅ #1 gap | WaaV has superb delta-streaming plumbing but **no shipped model emits audio early**; vLLM streams first packet in ~64 ms via tunable chunk knobs. |
| E | Connector chunk-knob taxonomy | **BEHIND / ABSENT** ⬅ #1 gap | vLLM: `initial_codec_chunk_frames` + load-adaptive dynamic-IC + `left_context` + `right_holdback` + ref-code-first-chunk. WaaV: one **inert** hardcoded `F6_DECODE_CADENCE_FRAMES=8`. |
| F | Stage-to-stage transfer format | **AT-PAR / AHEAD** | vLLM CPU-serializes through `/dev/shm` (never zero-copy); WaaV keeps codes/mel **device-resident** in hot models (dia2 one bulk D2H/frame, cosyvoice3 mel resident), in-process Rust. |
| G | Async intra-request stage overlap | **BEHIND** | vLLM overlaps transfer∥decode via threads; WaaV runs AR-step + decode serially on one lockstep thread (its parallelism is cross-request, orthogonal). |
| H | CUDA-graph **shape bucketing** | **BEHIND** | vLLM: `bisect_left` nearest-padded bucket + LRU graph cache + eager fallback. WaaV: exact-shape single-slot batch-1 capture; **batched serve disables the backbone graph** rather than bucketing it. |
| I | Local / partial (sub-region) graph | **AT-PAR** | **Both independently arrived at "eager backbone + local sub-decoder/MLP graph"** — WaaV (dia2 backbone eager + 31 depformer graphs), vLLM (Higgs post-attn-norm+mlp). |
| J | Launch-overhead attack | **AT-PAR mechanism / BEHIND coverage** | WaaV hand-rolled byte-identical graphs (−13.6% dia2 solo, **4 models** graphable); vLLM `torch.compile` more general but oversold/bypassed. WaaV can't use Dynamo (tch). |
| K | GPU↔CPU sync elimination | **AT-PAR (leader on diffusion, laggard on coverage)** | WaaV's omnivoice/dots diffusion loops are **fully sync-free** (matches VoxCPM technique); but **12 WaaV models still round-trip per step** (cosyvoice3 copies a ~6761-wide `Vec<f64>` to host **every AR step**). |
| L | GPU-resident decode state | **AT-PAR / AHEAD** | Both GPU-resident; WaaV's is `ChannelId`-keyed (safe); vLLM Higgs is row-keyed (fragile) + leaks state on abort. |
| M | Attention kernel specialization | **BEHIND** | vLLM ships Fish q_len=1 Triton decode kernel + real cuDNN-SDPA pin (215→11 ms). WaaV's "SDPA-pin" is **arg-steering only** (tch exposes no SDP-backend selector; all collapse to FusedAuto). |
| N | Precision policy | **AHEAD** | WaaV: per-graph precision + **lossy-quant admission gate behind a passing accuracy stamp** + native-bf16 measured quality-equivalent. vLLM: oversold conditional fp32. |
| O | Multi-codebook (delay/fused-embed) | **AT-PAR** | Both handle it (WaaV dia2/csm 32-codebook depformer; vLLM Higgs fused `[N×V,D]` + delay `[0..7]`). |
| P | Model onboarding / adapter framework | **AHEAD (design) / AT-PAR (ecosystem)** | WaaV: config-arch dispatch + `waav.json` manifest → **0 lines for a known arch**. vLLM: model class + frozen-DAG code + deploy YAML + hand-written stage_input_processor. Python velocity offsets for novel archs. |
| Q | Backend / accel pluggability | **AHEAD** | WaaV: 3 runtimes (ORT/tch/TRT) + 7-vendor `AccelBackend` registry, models target **one** runtime (not double-implemented). vLLM: CUDA-only torch+Triton. (WaaV caveat: non-NVIDIA accel is selection-real, execution-stubbed.) |
| R | Engine architecture fit | **AHEAD (fit) / BEHIND (hardening)** | WaaV: fixed-slot + lockstep + per-slot ring-KV + cohort-by-frame-rate + risk-EDF — the correct primitive for frame-sync voice, deliberately rejecting paged/continuous. vLLM: years of scale-hardening + kernel ecosystem WaaV lacks. |
| S | Diffusion step-caching lever | **BEHIND (by choice)** | vLLM has 4 lossy step-caches (MagCache/TeaCache/…); WaaV's byte-identity bar forbids them. Adopt only behind a WER/MOS gate. |
| T | Disaggregated multi-node serving | **BEHIND** | vLLM has PD-disaggregation (Mooncake/Mori/Yuanrong) as a DAG edge; WaaV is single-box (sidecar), by design. |

### 3.4 The Higgs state-safety inversion (why WaaV's approach wins)
The blog's marquee "learning" is that a row-cursor overlap design was *rejected* as "structurally unsafe under dynamic batching." **V3 found the opposite in the code:** Higgs' GPU state is **keyed by batch row**, resets fresh requests by a **row mask**, and *actively uses* a row cursor (`_postprocess_cursor`, "one row per request per step") — it relies on the very row-overlap invariant the blog claims to have rejected, and it **leaks `higgs_v3_emitted_frames[request_id]` on abort** (popped only on `finished`). WaaV makes this class of bug **impossible at compile time**: state is `(row, ChannelId)`-keyed, `is_live()` drops stale output for a recycled slot, and an ungated per-slot mutation *won't compile* (`MaskedCell`). This is the single clearest "WaaV is doing it better — keep going" datapoint.

---

## 4. vLLM-Omni's own admitted weaknesses (from its roadmap + bug tracker)

- 🔴 **8.5× memory blowup from the 2-stage design** (#2318, closed w/o clear fix): 0.6B → **22 GB** because each stage sizes KV as `total_mem × gpu_mem_util` with **default 0.90 per stage** and **no auto-division**. In-process serving does it in 2.6 GB. **WaaV's single-process substrate erases this entirely — the biggest strategic moat.**
- 🔴 **Streaming isn't universal** — the beautiful chunk-knob TTFP story is essentially **Qwen3-TTS-only**; "non-streaming is currently the default for many models" (roadmap #2115 Theme 1, P0, incomplete).
- 🟠 **Composability is an unbuilt RFC** ("any text model → chunker → any TTS decoder", "unresolved design questions"); **duplex/multi-turn is P1 and not native** ("single-turn unidirectional").
- 🟠 **Known memory leaks in async-chunk mode**; not validated for 1000+ requests; edge cases (codec errors, malformed ref audio, OOM) unhandled; **quality metrics (UTMOS/spk-sim) not yet in CI**.
- 🟠 Still *planning* (RFC #2136): quantization, prefix caching, KV CPU-offload, Model-Runner-V2, disaggregated serving, multi-hardware. **1 of 31 tracked roadmap issues complete.**
- 🟠 **Blog oversell** (V3): several headline optimizations are default-off (fp16 DAC, tensor-payload, bounded-batching, FSQ batching, async audio copy), dead code (Fish compile), bypassed (VoxCPM compile), or misattributed (Qwen3 fp32/bucketing).

---

## 5. Where WaaV is already better — **CONTINUE**

1. **Dynamic-batch state safety** — compile-time `ChannelId`/`MaskedCell`; categorically ahead of vLLM's runtime convention (and its actually-fragile Higgs). *(C, L)*
2. **Byte-identical correctness bar** — a differentiated discipline vLLM has no equivalent of, and which its own blog demonstrably lacks. *(N)*
3. **Single-process, in-process substrate** — no per-stage engine duplication, no CPU-serialized handoff, GPU-resident inter-stage tensors. Erases vLLM's 22 GB bug and its serialization latency floor. *(F, R)*
4. **Multi-runtime portability** — ORT Path-A reaches CPU/edge/ROCm/OpenVINO/QNN/CoreML that vLLM cannot; models target one runtime, not two. *(Q)*
5. **Architectural fit for realtime voice** — lockstep + ring-KV + cohort-by-frame-rate + risk-EDF is the right primitive; vLLM retrofits a text engine whose continuous-batching/paging is the wrong regime. *(R)*
6. **Partial/local CUDA graph as the default** — WaaV arrived at "eager backbone + local sub-decoder graph" structurally, the same conclusion vLLM reached for Higgs. *(I)*
7. **Sync-free diffusion loops** — omnivoice/dots already match vLLM's headline VoxCPM sync-elimination. *(K, partial)*
8. **Precision policy** — per-graph precision + accuracy-gated quant admission is more principled than vLLM's oversold conditional fp32. *(N)*

**Do NOT adopt from vLLM-Omni:** the full-engine-per-stage multiprocess model (the 22 GB tax); lossy diffusion step-caching (unless MOS-gated); `torch.compile`-as-primary (impossible in Rust anyway, and their own path is bypassed/fragile with silent eager fallback); TensorRT-as-throughput (WaaV's measured TRT win is near-break-even / sometimes negative — vLLM's "no TRT" is the more defensible call).

---

## 6. Roadmap to beat the best — **LEARN**

Ranked by realized-value. Every borrowed idea must pass WaaV's accuracy gate (byte-identical where deterministic; WER/MOS where a streaming approximation is introduced).

### P0 — the gaps that lose head-to-head demos
1. **Build the streaming-TTFP path (axes D, E, G).** This is the #1 gap: WaaV's realized first-audio == whole-utterance latency for the shipped fleet, while vLLM hits 64 ms.
   - Steal the **chunk-knob taxonomy**: `initial_codec_chunk_frames` (smaller first chunk) + **load-adaptive dynamic initial-chunk** (small@low-load→fast TTFA, large@high-load→amortize) + `codec_left_context_frames` (lookback for cross-chunk continuity) + `codec_right_holdback_frames` (delay-pattern right-context) + ref-codes-on-first-chunk-only.
   - Engage the **already-built-but-dead** `decode_committed_prefix` seam (`arstep.rs:615`) on the **causal-CONV tail** (SEANet, proven by vibevoice's conv VAE) where prefix-decode *is* byte-identical-by-construction — zero philosophy change. **⚠️ Correction (superseded by `SOTA_IMPLEMENTATION_PLAN.md`):** an earlier version of this line said "SNAC/Mimi causal = byte-identical" — that is **wrong**. The **Mimi decoder is empirically non-causal** (`arstep.rs:608`: `decode(body[..p]) ≠ decode(body)[..p]` at sample 0, from `_get_extra_padding_for_conv1d` right-pad + float non-associativity). Only the causal-conv sub-tail qualifies for TIER-0; the Mimi transformer / S3Gen / DAC / CFM decoders are **TIER-2 WER/MOS-stamped**, classified by a per-codec prefix-equality probe — never asserted causal.
   - For **non-causal** vocoders (chatterbox S3Gen, cosyvoice3 CFM), introduce a **streaming/left-context decode path gated by a perceptual (WER/MOS/SI-SDR/speaker-sim) bar** instead of byte-identity — the one place to consciously relax the bit-identity law, for the *streaming* path only, **gate-first** (publish the pass/refuse table before wiring the seam), keeping byte-identical as the default/verification mode.
   - Add a real **SSE/chunked-transfer HTTP** (pcm-only — wav RIFF can't stream byte-identically) + overlap decode with the next AR tick on the **single default stream** (tch has no `record_stream` — this is CPU-launch/host-sync-stall overlap, **not** GPU concurrency).
2. **Close the design-vs-wired gap (axes A, B).** WaaV already has *superior* machinery that isn't live:
   - Wire the **step-bucket flow batcher** (`cohort.rs`/`subbucket.rs`, byte-identical-tested) into the **12 streaming hybrid models** whose flow head is still per-slot (self-labeled "P1-PARTIAL", `engine.rs:407`). This is the exact small-batch decode-tail win vLLM's CFM batching targets — WaaV built it and left it unplugged.
   - Wire the **heterogeneous stage-DAG** (`dag/stage.rs` `Paradigm` taxonomy) for cascades that currently sequence internally.

### P1 — measurable perf, low risk
3. **Extend sync-free decode to the 12 round-tripping models (axis K).** The primitive exists (proven on dia2/omnivoice/dots). Highest-ROI hotspots:
   - `cosyvoice3.rs:331` — a full `Vec<f64>` of the ~6761-wide sorted prob dist copied to host **every AR step** (host-side nucleus cumsum → move on-device).
   - `s2_pro.rs:1371` — ~10 scalar D2H+H2D round-trips per frame (perfect fit for `sample_token_on_device` + on-device `index_select`).
   - `zonos2.rs:449` (~23 MoE host reads/frame), `csm`/`misotts` (32 syncs/frame), `qwen3_tts` (16/frame).
4. **Add CUDA-graph shape bucketing + widen coverage (axes H, J).**
   - Add `bisect_left` nearest-padded-bucket capture + an **LRU graph cache with eviction** (vLLM's IndexTTS2 pattern) to replace WaaV's exact-shape single-slot capture and dots' re-capture-on-`total` thrash.
   - Keep the backbone graph **on** under batched serve for graphable models instead of disabling it.
   - Steal the reusable, lossless graph idioms: **RNG-buffer-refill before replay** (fresh noise, no baked RNG), **scalar-placeholder runtime controls** (cfg/sigma/temp tunable without recapture), the `is_current_stream_capturing()→eager` nested-capture guard.
5. **Steal the Fish graph-capture-safe protocol for the paged token-AR STT path (axis M).** The *design* (CPU-precomputed `seq_lens_cpu_upper_bound`, static-worst-case grid at capture, real seq-len dereferenced only inside the kernel, both length-regimes launched unconditionally with self-select) is Triton-independent — it's how to CUDA-graph a paged decode-attention without an `.item()` sync. Directly applicable to WaaV's `paged_kv`/`prefix_cache` STT carve-out.

### P2 — hygiene & moat
6. **Consolidate per-model tuning** — move the scattered `WAAV_<MODEL>_BATCHED`/`_PRECISION` env knobs into the `waav.json` manifest / a single policy table (this was literally vLLM's whole v0.22 refactor).
7. **Publish the memory moat** — benchmark WaaV single-process (≈one CUDA context) vs vLLM-Omni's 22 GB for a 2-stage 0.6B model. This is a killer, defensible differentiator worth a blog of our own.
8. **Optionally** add a **MOS-gated** diffusion step-cache (DBCache-style live-residual, *not* MagCache open-loop) as an opt-in `Throughput` lever — but only behind the accuracy stamp, never default.

---

## 7. One-paragraph answer to "are we doing it better?"

**Yes — where it counts architecturally, and we should keep going.** WaaV's from-scratch, voice-native, single-process, byte-identical, multi-runtime engine is the *correct* foundation, and vLLM-Omni's own #2318 (22 GB for 0.6B) plus its actually-row-keyed Higgs state prove the parts WaaV rejected were the right things to reject. **But** vLLM-Omni has genuinely solved the thing our users feel first — **sub-second first audio** — via a mature connector chunk-knob taxonomy we haven't built, and it has more models and more hardware backends shipping. The path to beating the best is narrow and clear: **build the streaming-TTFP path (P0.1), wire the superior machinery we already wrote but left dormant (P0.2), and extend our proven sync-free decode across the fleet (P1.3)** — then WaaV is strictly better than vLLM-Omni on every axis that matters for voice.
