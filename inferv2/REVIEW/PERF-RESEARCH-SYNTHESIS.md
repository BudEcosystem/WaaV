# WaaV Infer — ONNX Perf Research: KV‑Cache Fix + Acceleration Catalog (SYNTHESIS)

**2026‑06‑25.** Synthesizes the three cited research reports — `KV-FIX-RESEARCH.md`, `ORT-GENAI-RESEARCH.md`,
`ORT-PERF-FEATURES.md` — plus direct verification of the `ort 2.0.0-rc.12` API and the WaaV `backend-ort` source.
Goal: the **best way to fix the Path‑A host‑KV problem**, and a **ranked catalog of other ONNX acceleration
methodologies**. Every external claim is cited in the source reports.

---

## Part 1 — The KV‑cache fix (the #1 lever: ~1.8× → ~30×)

### The problem (verified in source)
The chatterbox codec‑AR LM (`language_model.onnx`, 30‑layer Llama, GQA, kv_heads=16, head_dim=64) passes the KV
cache as **named host graph inputs AND outputs, round‑tripped every decode stride**:
- `lm_forward_batched` (`tts/chatterbox.rs:683‑839`) **rebuilds the padded `[B,16,max_past,64]` KV on the host every
  layer every step**, then reads all 60 `present.*` back via `to_f32_vec()` → `O(B·layers·heads·seq·dim)` host
  copies + a full H2D/D2H of the cache **per step**.
- WaaV **already has IoBinding** (`backend-ort/lib.rs::run_bound`) **but it binds outputs to `CUDA_PINNED`‑class
  *host* memory** (lib.rs:403) and re‑feeds host inputs each step — so even `keep_on_device("present.*")` measured
  **0.77× (slower)**. It never carries step‑N's *device* output into step‑N+1's *device* input. **That is the bug.**

Result: Path‑A scaling caps at **1.77×@B16 and regresses to 1.06×@B64**; the device‑resident tch probe hits
**~30×@B64** purely because its KV stays on the GPU.

### The fix — two parts, both already supported by `ort` rc.12 (no FFI)
Verified: `ort 2.0.0-rc.12` (api‑24 ⇒ ORT ≥1.24) exposes `IoBinding`, `bind_input(&Value)`, `bind_output(Value)`,
`run_binding → SessionOutputs`, a CUDA `Allocator`, and `Tensor::new(&cuda_alloc, shape)` (device‑resident `Value`).

1. **Runtime — device IoBinding ping‑pong** (removes the host round‑trip): keep the 60 KV tensors as **device
   `Value`s**; bind step‑N's `present.*` device output as step‑N+1's `past.*` device input (ORT can alias a bound
   output buffer to the next input). Extend WaaV's `backend-ort` to carry device `Value`s through the bound loop
   instead of host `Vec`s + `CUDA_PINNED` outputs.
2. **Export — buffer‑sharing GQA `past_present_share_buffer`** (removes the per‑step device realloc): re‑export the
   LM so `present` writes **in place** into a pre‑allocated static `max_length` `past` buffer (BNSH
   `[B,16,MAX_SEQ,64]`). Chatterbox currently emits a *growing* `[B,H,past+1,D]` buffer → a one‑time re‑export via
   the GroupQueryAttention contrib‑op (the same op `builder.py` / ORT‑GenAI use as the reference producer).

### Plan (both KV agents agree)
- **Phase 1 (shippable, no re‑export):** device‑KV IoBinding ping‑pong against the *current* growing‑buffer export →
  removes the host transfer (partial gain). Flip a new `KvResidencyRegime::DeviceKv` on for chatterbox.
- **Phase 2 (the full ~30×):** the `past_present_share_buffer` static re‑export, shipped as a `waav.json` variant →
  removes the device realloc too. Matches the device‑resident curve.
- **Accuracy: byte‑identical throughout** — same GQA kernel/math; `seqlens_k`/`total_sequence_length` select the
  same write index as WaaV's existing LEFT‑align identity. Gate with the current bit‑identity + AR‑compounding
  tests, and (the chunked‑prefill/B23 playbook) gate the decoded **codes**, not just closeness, for the bf16 path.
- **Risks:** `max_length` pre‑alloc vs the GB10 unified‑pool arena → reuse the `gpu_mem_limit`/`SameAsRequested`
  cap (950d491); coexisting the static buffer with WaaV's ragged LEFT‑aligned KV → bucket batch widths for CUDA‑graph.

### onnxruntime‑GenAI — verdict: borrow the technique, don't adopt the library
ORT‑GenAI's "continuous decoding" is **single‑stream incremental, NOT continuous batching**; its batch is
**static/rectangular/padded** (WaaV's `CodecArBatcher` already does true **ragged mid‑flight add/evict** — adopting
it would be a *downgrade*). It has **no paged attention**, **no Rust binding**, and its text‑token loop has **no
analog** for WaaV's multi‑codebook delay‑pattern head / CFG / codec‑vocoder / ragged multiplexing. It **does** prove
the buffer‑sharing‑GQA device‑resident‑KV technique above — use it as the reference, port the technique into WaaV's
own batcher (keeps the bit‑identity guarantee + the audio glue).

---

## Part 2 — Ranked catalog of other ONNX acceleration methodologies

**Already used in `backend-ort`** (verified): the IoBinding bound loop, the CUDA `gpu_mem_limit` arena cap +
`SameAsRequested` (GB10 anti‑frag), `use_tf32=0` (batch‑invariance), full CPU‑tier threading (one‑thread‑per‑physical
‑core + affinity + intra‑op spin, inter‑op=1), EP auto‑fallback, int8‑never guards, Level‑3 graph opt.

**The opportunity surface (NOT used) — all one‑line builder calls in `ort` rc.12, so the cost is *validation*, not
new FFI.** Top‑5 do‑next, ranked impact × ease, all accuracy‑preserving:

| # | Lever | Mechanism | Expected | Depends on |
|---|---|---|---|---|
| 1 | **Device‑resident ring‑KV (the Part‑1 fix)** | IoBinding ping‑pong + buffer‑sharing GQA | **~1.8× → ~30×** | — (prerequisite for #3,#4) |
| 2 | **Offline fuse + serialize at onboarding** | `optimize_model`/Olive (fp16 OFF), save the optimized graph | un‑fused onnx‑community exports are the win; shrinks the STT encoder + AR step; kills per‑cold‑start re‑prepacking | independent |
| 3 | **Pin ORT FlashAttention / efficient SDPA** | `sdpa_kernel` on the fused attention nodes (**never FlashInfer** on sm_12x) | INFER_PERF measured SDPA‑pin **40–135×** on the attention op | needs #2 |
| 4 | **CUDA‑graph capture of the AR step** | `enable_cuda_graph`, bucketed by `gpu_graph_id` | tch backend proved **1.04–1.20×** | gated on #1 (static shapes) |
| 5 | **CUDA conv/stream knob sweep** | `prefer_nhwc` / `fuse_conv_bias` / `cudnn_conv_algo_search=HEURISTIC` / `do_copy_in_default_stream=0` | cheap, accuracy‑neutral; helps the codec/vocoder convs | independent (env‑flag gated) |

**Honest negatives (don't chase):** int8 PTQ is closed (forbidden by the CUDA‑EP + CPU‑tier guards); 4‑bit/q4f16
compute is **negative on the host‑KV path** (only pays off *after* #1); parallel‑execution‑mode + `with_memory_pattern`
on the AR LM are correctly **OFF** (they hurt the dynamic‑KV linear graphs); the ORT **TensorRT‑EP is largely
redundant** with WaaV's Path‑B Torch‑TensorRT for AR models.

---

## Part 3 — Recommended implementation roadmap (dependency‑ordered)
1. **G1 — device‑resident KV** (Part 1): Phase 1 ping‑pong → Phase 2 buffer‑sharing re‑export. *The single biggest
   lever; unblocks #3 and #4.* Byte‑identical, gated.
2. **Offline fuse + serialize** (#2): independent, do in parallel — pure onboarding‑time win, helps both STT and TTS.
3. **SDPA/FlashAttention pin** (#3) once #2 lands the fused graph.
4. **CUDA‑graph the AR step** (#4) once #1 gives static shapes.
5. **Conv/stream knob sweep** (#5): cheap, env‑gated, measure‑and‑keep.

Each lands behind a flag, is measured on **GB10 + aarch64 CPU**, and must hold the **byte‑identical** bar (gate the
decoded codes/transcript, not just numeric closeness) before it's defaulted on.
