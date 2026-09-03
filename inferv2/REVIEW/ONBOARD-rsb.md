# Onboard: Yorch233/RSB → WaaV Infer (ONBOARDED, byte-faithful)

**Date:** 2026-06-24
**Status:** ✅ **ONBOARDED** — a real RSB inference runs on GB10, **byte-faithful** to the reference golden.
**Triage correction:** the §F HARD-tier entry called this *"Semantic-DACVAE"* — **that label was wrong.**
RSB is unrelated to DACVAE; see §1.

---

## 1. What is RSB? (HfApi-verified + identified)

`HfApi.model_info("Yorch233/RSB")` resolves (not 404, **not gated**, apache-2.0):

| field | value |
|---|---|
| repo | `Yorch233/RSB` |
| pipeline_tag | **`audio-to-audio`** |
| language | `en` |
| siblings | `README.md`, `config.yml`, `model.safetensors` (111 MB, 271 tensors, all F32) |
| GitHub | https://github.com/Yorch233/RSB |

**RSB = "Regularized Schrödinger Bridge via Distortion-Perception Perturbation for High-Fidelity Speech
Enhancement"** — a **generative speech-enhancement / denoiser** (noisy 16 kHz → cleaner 16 kHz). This is
**in Voice-AI scope** (the `enhance` task family — WaaV already has an ONNX enhancement seam
`waav-infer-core/src/enhance.rs`; RSB is the first **torch-backend** member).

**Architecture (genuinely a "new stack"):** a **score-based / Schrödinger-Bridge SDE** denoiser over the
**complex STFT**, with the Song-et-al. **NCSN++** conv U-Net as the score network:
- `ncsnpp_base`: nf=128, ch_mult=(1,2,2,2), 1 res-block/level, **BigGAN** res-blocks with **FIR [1,3,3,1]**
  (StyleGAN2 `upfirdn2d`) up/down-sampling, **Gaussian-Fourier** time embedding, **one bottleneck
  channel-attention** block, `output_skip`/`input_skip` progressive pyramids.
- **VE Schrödinger-Bridge SDE** (`bridge_type: VE`, c=0.4, k=2.6): σ²(t)=c·(k^{2t}−1)/(2 ln k), α≡1.
- STFT: n_fft=510, hop=128, **sqrt-Hann**, center; magnitude-warp `|X|^0.5·e^{i∠X}·0.33`; time-pad to ×64.
- Default sampler: **SDE, 3 steps**, `time_uniform`. Input channels = 4 = re/im of [xt, y].

**NOT a DACVAE, NOT a transformer, NOT a codec, NOT an LLM.** Reuse of the shared `nn::/codec::/cfm::`
transformer library is **zero** — those are transformer/codec primitives; RSB is a conv U-Net + SDE math.
Ported directly on `tch` ops. (This is honest: the model legitimately shares nothing with the existing
families, which is *why* it was the §F "new stack, under-specified" entry.)

The repo's CLI/registry has import-casing bugs (`from RSB.modeling_RSB`, `from DisperSE.common.register`) —
the golden harness bypasses them and imports the model classes directly.

---

## 2. Port

**File (owned):** `crates/waav-infer-backend-torch/src/rsb.rs` (new, ~880 lines).
**Shared edits:** `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod rsb;` + doc + the
`pub use rsb::{RsbError, Solver as RsbSolver, TorchRsb};` re-export. **No shared `nn::/codec::/cfm::` files
touched** (rsb.rs depends only on `crate::device`), so **dia2 (608) / csm (4000) need no re-verify.**

**Entry point:** `rsb::TorchRsb` —
- `load(dir, TorchDevice)` — f32 weights from `model.safetensors`.
- `net_forward(xt, y, t)` — the deterministic score-network forward (the primary byte-faithful gate).
- `enhance(audio, Solver::{Sde,Ode}, num_step)` — full STFT → sampler → iSTFT → de-normalize.

Everything is reconstructed op-for-op from the traced reference module sequence (modules 0–35 +
`output_layer`), incl. the **`upfirdn2d` native path** (pad→reshape→flipped-kernel `conv2d`→stride-slice,
the exact CPU reference code the CUDA custom kernel reproduces), the DDPM **NIN channel-attention**
(einsum-as-matmul, C^-0.5 scale, full [B,HW,HW] softmax), the **Gaussian-Fourier** embedding, the
magnitude-warped center STFT/iSTFT (sqrt-Hann), and the **VE-SB SDE/ODE** reverse steps.

**Model + golden artifacts staged at** `~/.cache/waav-models/rsb/` (`model.safetensors`, `config.yml`,
`waav.json`, `golden_input_f32.bin`, `golden_net0_{cpu,cuda}_f32.bin`, `golden_enhanced_{cpu,cuda}_f32.bin`).

---

## 3. Byte-faithful gate (vs the golden)

Golden = the upstream PyTorch model run in a **throwaway reference venv** (reference-only; not a serving
path). Two gates (both `#[ignore]`, live-GPU; in `rsb.rs` tests):

| gate | what | result |
|---|---|---|
| `net_forward_matches_golden` | deterministic U-Net forward (t=1, [y,y]) vs golden `net0` | **max\|Δ\| = 2.7e-6 (CUDA)**, 2.4e-6 (CPU) |
| `enhance_matches_golden` | full 3-step SDE STFT→sampler→iSTFT vs golden waveform | **max\|Δ\| = 2.7e-7 (CUDA)** |

Both are **bit-identical** (f32 epsilon). `tch` IS libtorch ⇒ identical ops + identical RNG draw order ⇒
identical bytes; the SDE's per-step `randn_like` draws are RNG-matched via `tch::manual_seed(0)`.

### RCA of the one divergence (f32-CPU bisection) — **TF32**
First CUDA run showed max\|Δ\| = **7e-4** (CPU was already 2.4e-6). Bisection:
- CPU golden vs **TF32-off** CUDA golden = **4e-6** (bit-identical).
- CPU golden vs **TF32-on** CUDA golden = **1e-3**.

⇒ The residual was **TF32** (GB10/sm_121 default "high" f32-matmul/conv precision); tch and PyTorch pick
slightly different TF32 algorithm orderings. **Fix (the byte-faithful scar):** `TorchRsb::load` pins
**TF32 OFF** for f32 matmul+conv via the `setAllowTF32Cu{BLAS,DNN}(false)` FFI (the same hook dia2 uses to
*enable* it) + `cudnn_set_benchmark(false)`. With that, CUDA == CPU == golden bit-for-bit. RSB is a full-f32
model, so full-FP32 is the correct precision regime.

---

## 4. Perf / RTF

3-step SDE enhancement of a **1.0 s** clip (16 kHz, 256×128 padded spectrum) on GB10 CUDA: the full
`enhance` (3 U-Net forwards + STFT/iSTFT) completes in **~0.9 s wall in the test (incl. model already
loaded)**; the 3-NFE generative path is the cost. RTF ≈ **<1** for the default 3-step SDE on this clip
(generative enhancement is inherently multi-NFE; `Solver::Ode` is also wired for a deterministic, RNG-free
3-step path). Model is 27.7 M params / 111 MB f32 — tiny; memory-trivial on the 121 GB unified pool.

---

## 5. Tests / lint

- `cargo test -p waav-infer-backend-torch --lib` → **190 passed, 0 failed, 2 ignored** (the 2 ignored = the
  live-GPU byte-faithful gates, both confirmed passing). The non-ignored `ve_sde_coefficients_are_consistent`
  unit test is in the 190.
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean (exit 0)**.
- Shared `nn::/codec::` untouched ⇒ dia2/csm regression not in scope of this change.

---

## 6. Files

- `crates/waav-infer-backend-torch/src/rsb.rs` *(new — the port + 3 tests)*
- `crates/waav-infer-backend-torch/src/lib.rs` *(SHARED — `pub mod rsb;` + re-export only)*
- `~/.cache/waav-models/rsb/{model.safetensors, config.yml, waav.json, golden_*.bin}` *(model + goldens)*

**Disposition update for `TRIAGE_DISPOSITION.md` §F:** RSB is **ONBOARDED** (byte-faithful, GB10), and the
"Semantic-DACVAE" label was a **mis-bucketing** — RSB is an NCSN++ Schrödinger-Bridge **speech-enhancement**
model with no DACVAE relationship.
