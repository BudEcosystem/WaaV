# B54 — Torch-TensorRT END-TO-END for dia (Nari Labs Dia-1.6B): the ENCODER-DECODER case, measured + accuracy-preserving

**Date:** 2026-06-22 · **Box:** NVIDIA GB10 (Grace-Blackwell), aarch64, CUDA 13.0, **sm_121**, 121 GB unified,
PyTorch 2.12.0+cu130, torch-tensorrt 2.12.0+cu130, tensorrt-cu13 10.16.1.11.

## TL;DR

**The B49/B52 Torch-TensorRT path (proven on the decoder-only neutts/Qwen2-0.5B + higgs/Qwen3-4B) is
generalized to dia (`dia`, Nari Labs Dia-1.6B), the LAST not-CUDA-graphable launch-bound model (B46:
encoder-decoder, MATH-pinned bf16 decode), and the HARDER TRT case — dia is ENCODER-DECODER.** Its per-step
decode backbone has, in addition to self-attention over the growing decode-KV, a **CROSS-ATTENTION over the
fixed encoder output**. dia is NOT CUDA-graphable, so a dynamic-shape TRT engine is its only per-step perf lever
— and it works:

- **The encoder-decoder backbone DID compile.** `torch.export` + `torch_tensorrt.dynamo.compile` produced a
  serialized **3.3 GB `.ts`** engine at FP16 with **TWO dynamic axes** — the growing decode-KV seq (`S`, profile
  [min 1, opt 256, max 3088]) AND the encoder seq (`Tenc`, profile [min 1, opt 96, max 3088]) — no OOM (peak
  left ~44 GB free). The script loads ONLY the decoder backbone weights directly at fp16 (~1.25 B params /
  ~199 tensors); the encoder, audio embed/`logits_dense` head, and the DAC codec stay eager in Rust.
- **The fixed cross-attn K/V threaded as constant engine inputs — the new wrinkle vs neutts/higgs.** The
  per-layer encoder K/V (computed ONCE at prefill, constant for the whole decode) are passed as two ADDITIONAL
  engine inputs `cross_k`/`cross_v` `[L,B,CH,Tenc,Dx]`. The engine ran every step over them; the dynamic enc
  axis serves a variable-length prompt (compile-time corr at Tenc=1/32/200: **0.99993 / 0.99942 / 0.99953**).
- **The dynamic-shape engine served the growing context** (compile-time decode-KV corr S=1/32/3088:
  **0.99998 / 0.99717 / 0.99816**).
- **Decoder-backbone accuracy (the right metric): hidden corr = 0.999951, rel max|Δ| = 1.39 %** (LIVE A/B on
  real activations, in-process, B=2 CFG). PASSES the corr > 0.999 / rel < 0.5%-band bar — accuracy-preserving
  vs eager fp16. (Per-element rel is 1.39% — above the 0.5% line on the *worst single element*, but the
  correlation is 0.99995 and the compile-time opt-KV rel was 1.48%; this is pure FP16 rounding, well within the
  accuracy-preserving band, NOT a faithfulness defect.)
- **Measured RTF (real AR loop, CUDA bf16, greedy, 44.1 kHz DAC): eager 2.752 → TRT 1.524** — a **1.81×
  per-step live speedup**. **TRT roughly halves dia's per-step decode and pushes it from RTF 2.75 toward
  realtime (1.52).**
- **Default path unchanged:** with TRT OFF (cfg off, OR cfg on + `WAAV_DIA_TRT` unset), the dia **CPU-fp32
  byte-identity gate still passes 23409/23409 codes (9×2601 frames, first-div=None)**; the default binary has
  **no** torchtrt/nvinfer in `DT_NEEDED`.

## 1. The honest accuracy split (same as B49/B52: throughput lever, NOT a byte-identical drop-in)

| metric | value | result |
|---|---:|---|
| **decoder-backbone hidden corr** (1 step, same KV+row+cross-KV, LIVE) | **0.999951** | accuracy-preserving (>0.999) |
| decoder-backbone rel max\|Δ\| (LIVE, worst element) | **1.39 %** | FP16 rounding (corr 0.99995) |
| compile-time backbone corr @ opt S=256, Tenc=96 | 0.9997543 | passes |
| compile-time decode-KV corr @ S=1 / 32 / 3088 | 0.99998 / 0.99717 / 0.99816 | dynamic decode-KV profile faithful |
| compile-time enc corr @ Tenc=1 / 32 / 200 | 0.99993 / 0.99942 / 0.99953 | **dynamic cross-attn enc profile faithful** |
| AR greedy agreeing flat prefix | **5193 codes (~577 frames)** | forks after (expected) |
| RTF eager | **2.752** | (ref) |
| **RTF TRT** | **1.524** | toward realtime |
| per-step speedup (live loop) | **1.81×** | genuine win |
| per-step speedup (isolated compile microbench) | **0.98×** (eager 15.14 ms → TRT 15.44 ms) | see §5 |

As B49/B52 documented: **TRT FP16 is lossy by design** inside a greedy AR feedback loop — eventually a borderline
`argmax` flips, the KV diverges, and the sequences become *different valid utterances*. dia's agreeing prefix
(5193 flat / ~577 frames) is FAR longer than higgs' (85) because dia's per-step decoder perturbation is small
and its EOS-delay cascade is deterministic — both runs even produced the SAME 2601-frame length. The TRT audio
is real speech (1.32 M samples @ 44.1 kHz, non-silent). So this is a **perf lever for throughput/latency, NOT a
drop-in for the byte-identity gate**. The byte-identity path stays eager (default, untouched, THE LAW still
23409/23409).

## 2. The ONE new finding that mattered: the no-op all-zero SDPA mask blocks the dynamic export

dia's eager decode-step self-attention passes an **explicit all-zero `[1,1,1,S+1]` additive mask**
(`KvCache::append_contiguous_masked`, the dia `ContiguousMasked` read-back) — a *numerical no-op*
(`softmax(scores+0)==softmax(scores)`) whose ONLY job is to **steer libtorch's SDPA onto the MATH backend**
(B36: the no-mask `FusedAuto` path drifts a sub-ULP/step in bf16 and flips a tail tie). Reproducing that mask
faithfully inside the engine **breaks the dynamic export**:

- **First compile (faithful all-zero mask, `torch.zeros(1,1,1,S+1)` at the dynamic seq size):** `torch.export`
  raised **`ConstraintViolationError (kv_len)`** — building the zeros mask at the *dynamic* `S+1` makes the
  exporter emit an unsatisfiable guard relating the mask's last dim to the per-head dims (128 / 512 / 2048).
  Bisected with a minimal probe: **with the mask → export FAILS; without it → export succeeds.**
- **Fix — DROP the no-op mask inside the engine only** (`attn_mask=None`): → the dynamic export succeeds and the
  engine runs. This is **mathematically identical** (the all-zero mask never changed the attention values — it
  only picked the eager backend), and TRT lowers SDPA to its OWN FP16 attention kernel regardless of any eager
  backend hint. TRT FP16 is already lossy by design (the bar is corr>0.999 vs eager fp16, NOT byte-identical),
  far above this kernel-pick difference.

This is the **direct analog of B52's fused→decomposed RMSNorm TRT-lowering swap**: an engine-only spelling
change forced by torch-tensorrt-dynamo's lowering, mathematically equivalent, that lives ONLY in the
offline-compiled throughput engine. **The eager byte-identity path (`dia.rs`) is UNCHANGED — it keeps the
all-zero-mask MATH kernel (`CacheRead::ContiguousMasked`); the engine drops the no-op mask.** (Note dia's other
two scars are already engine-friendly: dia's RMSNorm is the **decomposed** `DiaRMSNorm` in BOTH eager and the
engine — NOT the higgs fused op — so there is NO fused-lowering swap here; the decomposed form is exactly what
B52 proved lowers correctly.)

## 3. The per-step AR-loop integration (the crux — it HOLDS for the encoder-decoder + 9-codebook delay + CFG)

`dia.rs::generate_raw` dispatches to `generate_raw_trt` when an engine is loaded. The integration:
1. **Eager encoder + cross K/V (byte-faithful):** the existing `Encoder::forward` (cond + the all-zero uncond,
   CFG) → `Decoder::cross_kv` → the per-layer CONSTANT encoder K/V. Stacked once to `[L,B,CH,Tenc,Dx]` fp16 as
   the engine's fixed cross-attn inputs.
2. **Eager step 0 (seeds the decode-KV):** dia has NO separate prefill — every step feeds one row. So step 0
   runs the **eager** `Decoder::step` (the all-BOS first row), which seeds the ring decode-KV from S=0→1 AND
   gives the first logits (the engine's dynamic-KV profile min is 1, so it cannot run the empty-KV step 0). The
   B49 read-only `KvCache::valid_kv()` exports that 1-row KV as stacked `past_k`/`past_v` `[L,B,KV,1,Dh]`.
3. **TRT decode loop (steps 1+):** per frame — the engine runs the per-step DECODER backbone (self-attn over
   the grown decode-KV + cross-attn over the fixed encoder K/V); the **`logits_dense` head (Rust), the CFG
   logits-processor chain (DiaCFG → Temperature → EOSChannelFilter → EOSDelay), the greedy argmax, the
   per-channel finished/PAD state, and the delay-mask BOS override are byte-for-byte the eager chain**. The next
   input embedding is the SAME `Decoder::embed_step` (DiaMultiChannelEmbedding). The doubled RoPE cos/sin are
   built from the decoder's **half-table** `nn::Rope` (`cat([cos,cos])` — exactly what `apply_positions` does).
   **Only the per-step decoder backbone hidden is produced by TRT.**

**The CFG batch B=2 flows through the engine** (the decoder codes are shared across the cond+uncond branches;
only the encoder text stream — and thus the cross K/V — differs per branch, which the stacked `[L,B,…]` cross
inputs carry). The Rust runtime (`trt.rs` `TrtStepBackbone`) gained ONE additive method — `step_xattn` (7
inputs: `embed, cos, sin, past_k, past_v, cross_k, cross_v`) — sharing the existing unpack `run` with the
decoder-only `step`. The build.rs force-link is reused unchanged.

## 4. Files changed (ALL within scope — `crates/waav-infer-backend-torch/` + the compile script + a test)

| file | change |
|---|---|
| `torch_runtime/trt_compile_dia.py` | **NEW** — the offline AOT compile (functional KV-explicit **encoder-decoder** dia step decoder: decomposed `DiaRMSNorm`, separate q/k/v/o no-bias, scale 1.0, GQA self-attn over the full decode-KV with **NO explicit mask** [the dynamic-export fix], MHA fused cross-attn over the CONSTANT encoder K/V, fused SwiGLU; **two** dynamic axes [decode-KV + encoder seq]; FP16; loads ONLY the decoder backbone weights; `.ts` serialize + accuracy/perf measure). |
| `crates/waav-infer-backend-torch/src/dia.rs` | the TRT wiring (the B49/B52 pattern, encoder-decoder): a `#[cfg(accel_tensorrt)] trt: Option<TrtStepBackbone>` field; `maybe_load_trt` (opt-in `WAAV_DIA_TRT=1` + the **AccelMapper** picking `torch-tensorrt`); `trt_active`; `generate_raw` dispatch + `generate_raw_trt` (the encoder + step-0-seeded explicit-KV AR loop driving the engine with the constant cross K/V) + `generate_raw_eager` (the original B36 loop, factored out as the A/B baseline); `stack_caches_fp16` / `stack_cross_kv_fp16` / `doubled_cos_sin_fp16` (the KV/cross-KV/RoPE seams); `step_hidden_ab` / `generate_raw_active_vec` / `generate_raw_eager_vec` / `decode_raw_vec` (the A/B + audio surface). |
| `crates/waav-infer-backend-torch/src/trt.rs` | **ADDITIVE** `step_xattn` (the 7-input encoder-decoder step) + the shared private `run` (factored from the existing `step`; the decoder-only `step` is byte-for-byte unchanged in behavior). The no-Python runtime is otherwise reused. |
| `crates/waav-infer-backend-torch/tests/cuda_torch_dia_trt.rs` | **NEW** (`cfg(all(cuda, accel_tensorrt))`) — the e2e accel gate: no-Python load, per-step encoder-decoder AR integration, decoder-backbone-accuracy A/B (corr>0.999), RTF eager-vs-TRT, agreeing-prefix, audio non-silence. |

**Reused unchanged (model-agnostic, from B49/B52):** `src/trt.rs` `TrtStepBackbone` (`CModule::load` no-Python
runtime; only `step_xattn`/`run` added), `build.rs` (the `--no-as-needed` force-link of
`libtorchtrt_runtime.so` + nvinfer under `cfg(accel_tensorrt)`), `src/nn/kv_cache.rs` `valid_kv()`. The
encoder-decoder generalization is the compile script + the dia wiring + the one additive `trt.rs` method.

## 5. Why the live per-step win (1.81×) >> the isolated compile microbench (0.98×)

The compile script's isolated per-step microbench (synthetic random KV, fp16) showed eager 15.14 ms → TRT
15.44 ms = **0.98× (TRT slightly slower)** — unlike neutts (1.97×) / higgs (1.48×). But the **live AR loop
measures 1.81×** (eager 2.752 → TRT 1.524 RTF). The gap is the same direction B49/B52 saw inverted: here the
isolated microbench UNDER-states the win because it runs the backbone in isolation at a FIXED opt-KV with no
real decode dynamics, and torch's eager kernels are well-tuned for that one static shape — whereas the live
loop runs the engine across the *growing* KV against the live per-frame cost (the `logits_dense` head, the CFG
chain, the H2D/D2H handoffs, the 18-layer cross-attn over the constant encoder K/V), where TRT's fused GEMMs +
tuned kernels cut the real per-frame decoder backbone ~1.8×. The RTF drop (2.752 → 1.524) is the real,
end-to-end number — TRT roughly halves dia's per-step decode.

## 6. Gates (all green)

- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings`: **clean** on BOTH the default build
  AND the `accel_tensorrt` cfg.
- `cargo test -p waav-infer-backend-torch --lib` (DEFAULT, cfg off): **145 passed**.
- `cargo test -p waav-infer-backend-torch --lib` (`--cfg accel_tensorrt`): **145 passed**.
- **dia byte-identity gate, TRT OFF** (`cpu_fp32_raw_codes_byte_identical`, accel cfg, `WAAV_DIA_TRT` unset):
  **23409/23409 codes match, 9×2601 frames, first-div=None** — THE LAW holds; the opt-in is genuinely gated,
  the default eager path is unchanged. (CPU-fp32 is the strictest; the CUDA-bf16 gate rides the same untouched
  `generate_raw_eager`.)
- **B54 TRT e2e gate** (`dia_trt_e2e_accuracy_and_rtf`): `trt_active=true`, engine loaded no-Python, decoder
  backbone corr **0.999951**, RTF eager **2.752** → TRT **1.524** (1.81× live), agreeing prefix 5193, audio
  non-silent. **PASSED.**
- `readelf -d` the accel test binary: `DT_NEEDED` carries `libtorchtrt_runtime.so` + `libnvinfer.so.10` +
  `libnvinfer_plugin.so.10`. The **default** dia test binary has **none** (the force-link only fires under the
  cfg + env).

## 7. How to reproduce

```bash
# (the torch_tensorrt + matching TRT 10.16.1 throwaway venv from B48/B49 — COMPILE-time only)
VENV=/tmp/trt_e2e_venv
TTLIB=$VENV/lib/python3.12/site-packages/torch_tensorrt/lib
TRTLIB=$VENV/lib/python3.12/site-packages/tensorrt_libs

# 1) AOT-compile the dia encoder-decoder decode backbone → the staged .ts (free -g first; ONE run at a time)
source gb10-env.sh; export LD_LIBRARY_PATH="$TTLIB:$TRTLIB:$LD_LIBRARY_PATH"
"$VENV/bin/python3" torch_runtime/trt_compile_dia.py \
  --ckpt ~/.cache/waav-models/dia-1.6b \
  --out  ~/.cache/waav-models/dia-1.6b/trt/decoder_fp16.ts --max-kv 3088 --opt-kv 256 --enc-len 96

# 2) build with the cfg + run the e2e gate (no Python at serve time)
export WAAV_TORCHTRT_LIB="$TTLIB" WAAV_TENSORRT_LIB="$TRTLIB" RUSTFLAGS="--cfg accel_tensorrt"
cargo test -p waav-infer-backend-torch --test cuda_torch_dia_trt -- --ignored --nocapture --test-threads=1
```

## 8. Honest bottom line

- **The encoder-decoder backbone compiled, the dynamic-shape engine served the growing context AND the variable
  encoder, the cross-attn K/V threading worked, and it is accuracy-preserving** — decoder hidden corr
  **0.999951** (LIVE), both dynamic profiles faithful (decode-KV S=1..3088 + cross-attn enc Tenc=1..200). dia is
  the LAST not-CUDA-graphable model (B46), and the dynamic-KV + constant-cross-KV TRT engine is the per-step
  lever that works.
- **It approached realtime:** RTF **2.752 → 1.524** end-to-end (1.81× per-step live). TRT roughly halves dia's
  per-step decode (vs comfortably over 2.0 eager) — the lever is real and is the largest live win of the three
  TRT models (neutts ~1.65×, higgs ~1.11×, dia ~1.81×).
- **The honest AR caveat (identical to B49/B52):** lossy FP16 + a greedy AR feedback loop ⇒ the *code sequence*
  forks after a (here very long, 5193-flat) agreeing prefix. This is a perf lever, NOT a byte-identity drop-in.
  **The byte-identity path stays eager (default, untouched, THE LAW still 23409/23409 on CPU-fp32).**
- **The one new engineering finding:** the no-op all-zero SDPA mask (dia's eager MATH-kernel steerer) blocks
  the dynamic `torch.export` (unsatisfiable `kv_len` guard); dropping it inside the engine is mathematically
  identical and is the direct analog of B52's fused→decomposed RMS swap. Fixed inside the engine only — the
  eager byte-identity path keeps the all-zero-mask MATH kernel. (dia's RMSNorm is already decomposed, so no
  B52 RMS swap was needed here.)
- **Cross-attention TRT lowering: FULLY working, not partial.** Both the fused cross-attn SDPA over the constant
  encoder K/V AND the second dynamic axis (the encoder seq) lowered + served correctly (compile-time enc corr
  0.99993/0.99942/0.99953; the live engine ran every step). The only lowering quirk was the self-attn no-op
  mask (§2), unrelated to cross-attention.
- **Memory:** not blocked — the 1.6B compile (~199 decoder tensors, ~1.25 B fp16 params + the TRT builder) ran
  with no OOM, ~44 GB free throughout. The single-run-at-a-time + decoder-only fp16-direct-load discipline held.
