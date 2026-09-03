# PORTED: ZzWater/ViiTorVoice-NAR → WaaV Infer (byte-faithful, in-engine)

**Status: DONE. PORTED + BYTE-FAITHFUL vs the seed-0 golden (codes 0/948, wav maxΔ=0). Wired as a
`TtsModel` + engine arch arm. `cargo test -p waav-infer-backend-torch --lib` green (179/179, incl. 4 new
viitorvoice unit tests); clippy `--lib`/`--test cuda_torch_viitorvoice` clean (`-D warnings`).**

ViiTorVoice-NAR is now a registered in-process Torch model (config arch `ViiTorVoice` /
`model_type: omnivoice`): a 12-codebook masked-diffusion NAR TTS — a bidirectional Qwen3-0.6B backbone
(fp32 ONNX) + 32-step MaskGIT/LLaDA unmasking + DualCodec @25Hz ONNX decoder. It is an architectural
SIBLING of the already-done `omnivoice` but architecturally distinct enough to need its own model file.

---

## Did it port? YES — byte-faithful, live-gated on GB10

`cargo test -p waav-infer-backend-torch --test cuda_torch_viitorvoice --features cuda -- --ignored`:

```
loaded viitorvoice in 4.71s
gate1 prompt ids: got 34 ids (astart=34 T=79), want 34          ✅
gate2 codec maxΔ = 0e0                                            ✅ (DualCodec ONNX byte-identical)
gate3 masked-diffusion codes: 0 / 948 differ                     ✅ THE LAW (seed-0 byte-identical codes)
gate4 full-synthesis wav maxΔ = 0e0  → RTF 1.935; peak 0.4626    ✅ (full e2e byte-identical)
test result: ok. 1 passed
```

Byte-faithful vs the golden `/tmp/vv_ref_out/{codes.npy,wav.npy}` (re-derived + confirmed bit-identical
across runs first): **codes 0/948 disagreement, wav maxΔ=0.0**. The deterministic-NAR regime held
exactly as predicted (one RNG = the per-step finite-Gumbel `rand_like`, seeded; tch IS libtorch ⇒
identical MT19937 ⇒ identical Gumbel ⇒ identical reveal order ⇒ identical codes).

---

## RTF

**GB10 RTF ≈ 1.94** (6.12 s for 3.16 s audio), DOMINATED by the **CPU-EP ONNX backbone** (32 forwards):
ORT has no CUDA-EP wheel reachable on this aarch64 box (`get_available_providers() = [Azure, CPU]`), so
the backbone + codec run CPU EP — exactly the golden's path. The embeddings/heads/gumbel/scoring run on
CUDA (tch). This matches the report's measured python-CPU RTF 2.19 (slightly faster here: 1.94). The
report's projected GB10-GPU RTF ~0.33 needs a CUDA-EP ORT build (a future lever; the Rust `ort` crate CAN
do CUDA EP where a wheel exists — `OrtModel::load_ep` already supports it, just swap `EpRequest::Cpu`).

---

## What was REUSED vs NEW

**Reused (verbatim, unchanged):**
- `waav_infer_backend_ort::OrtModel` — the ORT codec/backbone hybrid seam (the SAME pattern `neutts` /
  `cosyvoice3` use). Drives BOTH the fp32 backbone ONNX and the DualCodec decoder ONNX (CPU EP).
- The `RuleDurationEstimator` char-weight table + the masked-diffusion *recurrence shape* mirror the
  `omnivoice`/`cfm::masked` sibling (same structure: log_softmax → forbid MASK → argmax/max → layer
  penalty → gumbel → forbid-revealed → flat topk).

**NEW (this model's distinct glue, in `viitorvoice.rs`):**
- The **split sem/acoustic embed+head**: `semantic_embedding[16387]` (row-0 ids) + `sum_c
  acoustic_embedding(ids[1:] + c·1027)` over 11 codebooks, masked; two untied heads
  `semantic_head→16387` + `acoustic_head→11·1027` (reshaped `[1,11,s,1027]`). Verified byte-identical to
  the reference at step 0 (embed/hidden/heads/scores all matched).
- The **finite-Gumbel split-head stepper** (GS=0, single forward): `-log(-log(u+1e-10)+1e-10)` — a
  VALUE-DEPENDENT Gumbel, distinct from omnivoice's DEGENERATE all-NaN `clamp_min` form, so
  `cfm::masked::apply_gumbel` is deliberately NOT reused.
- The **34-point reveal schedule** (`schedule_34`): the upstream `_build_mask_token_schedules` quirk —
  `_get_time_steps(num_step = NSTEP+1=33)` itself builds `linspace(0,1,num_step+1=34)` (a DOUBLE `+1`)
  ⇒ a `linspace(0,1,34)` warp, NOT omnivoice's `linspace(0,1,33)`. **THIS WAS THE SOLE BYTE-FAITHFULNESS
  BLOCKER** (see below); `cfm::masked::schedule` is therefore NOT reused (it diverges from this golden).
- The **DualCodec silence-pad decode**: `[12,49]` silence prefix+suffix (25 frames each) → split
  `semantic[1,1,P]` + `acoustic[1,11,P]` int64 → ONNX → trim `start=25·960`, `len=T·960−4`.
- The prompt build (`<|lang_start|>None…<|text_start|>text<|text_end|>`, each id ×12 + masked target).

---

## The one real blocker (found + fixed): the schedule double-`+1`

The first live run gave **867/948 codes wrong**, despite step-0 internals (embed, backbone hidden, heads,
scores, gumbel `u` draw, AND even the step-0/1/2 reveal positions) all being BYTE-IDENTICAL to the
reference. The divergence was traced to the **reveal schedule**: my initial port reused
`cfm::masked::MaskedDiffusion::schedule` (omnivoice's `linspace(0,1,33)`), but the golden's schedule is
`linspace(0,1,34)` — upstream `_build_mask_token_schedules` calls `_get_time_steps(num_step=NSTEP+1)` and
`_get_time_steps` adds another `+1` inside (`linspace(.., num_step+1)`). The reveal COUNTS differ from
step 0 (`[3,4,4,…]` vs `[4,4,4,…]`), so the whole reveal cascade diverged. Replacing the reuse with a
local `schedule_34` (34-point warp, unit-tested against the reference vector) → **0/948**.

This is genuinely the model's behavior (confirmed in upstream `generation.py:1530-1533`), not a script
bug — so `cfm::masked::schedule` correctly stays untouched (it is omnivoice's, which DOES use 33 points).

---

## Exact files

**NEW:**
- `crates/waav-infer-backend-torch/src/viitorvoice.rs` (816 lines) — the `TorchViitorVoice` model
  (`TtsModel`): split embed/head, ONNX backbone via ORT, finite-gumbel 12-cb stepper, `schedule_34`,
  DualCodec silence-pad decode, `RuleDurationEstimator`. 4 unit tests (pcm, duration→T=79, char-weight,
  `schedule_34`).
- `crates/waav-infer-backend-torch/tests/cuda_torch_viitorvoice.rs` (181 lines) — the 4-gate live
  acceptance test (prompt ids / codec parity / codes-THE-LAW / full-synth), mirroring
  `cuda_torch_omnivoice.rs`.

**SHARED FILE TOUCHES (flagged — concurrent canary-qwen agent also edits these; re-read+retried on the
detected lib.rs change, my edits preserved):**
- `crates/waav-infer-backend-torch/src/lib.rs` — added `pub mod viitorvoice;` (+ doc) + `pub use
  viitorvoice::{TorchViitorVoice, ViitorVoiceTorchError};`. ONLY additive; did not touch any other line.
- `crates/waav-infer-server/src/engine.rs` — added the `"viitorvoice" | "ViiTorVoice"` arch arm + the
  `TorchViitorVoice` import + the bail-message arch list. ONLY additive.

**Shared compute seams NOT touched (so omnivoice byte-identity is structurally preserved):**
`cfm/masked.rs`, `nn/backbone.rs`, `codec/` — all unchanged. omnivoice's gate depends only on those +
its own file, none of which I modified; the 179-test lib suite is green.

**Acquired artifacts (no commit):**
- `~/.cache/waav-models/viitorvoice-nar/waav.json` — the manifest (`{backend:torch, architecture:
  viitorvoice, dtype:fp32}`).
- `~/.cache/waav-models/viitorvoice-nar/assets/dualcodec_silence_2s.npy` — a **REQUIRED portable
  sidecar** (a one-time `np.save(torch.load(dualcodec_silence_2s.pt)["tokens"])`): the `.pt` is a torch
  pickle dict (not pure-Rust-readable; the codebase prefers `.npy`/`read_safetensors` over the legacy
  unpickler), so `load_silence` reads this `.npy` with a bare i64 npy reader. **Onboarding note: this
  sidecar must ship/convert alongside the model** (the loader errors with a clear message if missing).

---

## NO per-venv serving path

Per [[waav-infer-no-venv-wrap]]: the model is served entirely in-process (tch + the Rust `ort` crate).
The only python used was the throwaway reference `/tmp/vv_ref_synth.py` (golden derivation + the silence
`.pt`→`.npy` one-time convert) — no venv/pip on the serving path.

---

## Loose threads / follow-ups (none block the port)

1. **CUDA-EP ORT** would drop RTF from 1.94 → ~0.33 (report's projection). `OrtModel::load_ep` already
   supports `EpRequest::Explicit(Cuda)`; gated only on a reachable CUDA-EP ORT dylib on this box.
2. **The silence `.npy` sidecar** is a manual convert today; a loader fallback that unpickles the `.pt`
   (or shipping the `.npy` in the model card) would remove the manual step.
3. **Voice cloning / local editing** (the aligner + w2v-bert + encode-ONNX) — out of scope here (the
   zero-shot text→speech path is complete + byte-faithful); a clean future extension on the same seams.
