# Onboard — Semantic-DACVAE (HARD-tier §F) → PORTED + byte-faithful (encode **net-new**, decode confirmed)

**Date:** 2026-06-24 · **Bar:** byte-faithful to a CPU-f32 reference golden · **Result:** ✅ **PORTED + verified**
(a real standalone DAC-VAE **encode + decode** on GB10, byte-faithful to the golden).

---

## 1. HfApi-verify FIRST — the disposition tag was STALE

The disposition note (`TRIAGE_DISPOSITION.md` §F line 54) tagged it **"Yorch233/RSB"** — that is **wrong/stale**
(Yorch233/RSB is the *separate* RSB model, a different in-flight onboard). HfApi + web verification found the real
canonical repo:

- **Canonical repo:** **`Aratako/Semantic-DACVAE-Japanese-32dim`** — verified via `HfApi.repo_info`, **NOT gated**,
  files = `{README.md, weights.pth, .gitattributes}` (one `weights.pth`, 429 MB).
- **What it is (category b → standalone codec):** a **standalone DAC-VAE continuous neural codec** (encode + decode),
  a WavLM-distilled, **32-dim** fine-tune of **`facebook/dacvae-watermarked`** ("VAE version of the Descript Audio
  Codec, continuous latent space"). It is the **audio VAE the Aratako/Irodori-TTS Rectified-Flow TTS denoises into**.
  Pipeline tag `audio-to-audio`; MIT.
- **Config (from `weights.pth` metadata, exact):** `encoder_rates=[2,8,10,12]` (**hop 1920**), `decoder_rates=[12,10,8,2]`,
  `encoder_dim=64`, encoder-output `latent_dim=1024`, continuous `codebook_dim=32`, **`sample_rate=48000`**.
- **Arch source confirmed:** cloned `github.com/facebookresearch/dacvae` (reference-only) — `Encoder`/`EncoderBlock`/
  `ResidualUnit`/`VAEBottleneck`/`NormConv1d` read directly. Encode (README deterministic path) =
  `encoder(_pad(x))` → `in_proj(z).chunk(2)[0]` (the VAE **mean**, no `_vae_sample` randn). Decode =
  `out_proj → Decoder → watermark-bypass head` (`alpha=0`, the model was fine-tuned without the watermark).

**Overlap with the fleet (the key finding):** **Irodori already ported the DECODE half** inline
(`irodori.rs::DacVaeDecoder`, fed by the pre-folded `dacvae_decode.safetensors`). The Semantic-DACVAE codec is
exactly Irodori's audio VAE. So this onboard's **net-new value is the ENCODER** (waveform→latent), plus a
**standalone, reusable codec module** that owns the full encode+decode written once and composed from the shared
DAC primitives. **Not** a duplicate, **not** out-of-scope: a real Voice-AI codec, encode was missing.

---

## 2. Acquire

- `~/.cache/waav-models/semantic-dacvae/weights.safetensors` — the raw `weights.pth` state_dict (317 tensors,
  weight_g/weight_v **unfolded**, f32) exported for `Tensor::read_safetensors`. Folded at load in Rust.
- `~/.cache/waav-models/semantic-dacvae/waav.json` — manifest (arch `dacvae`, module `codec::DacVae`, fp32, geometry).

## 3. Golden (THROWAWAY reference venv, reference-only — no serving path)

`~/.cache/waav-models/semantic-dacvae-golden/` (CPU-f32, deterministic sine input, watermark bypassed, VAE mean):
- `input_wav.npy` [24000] · `z_latent.npy` [1,32,13] (**encode contract**) · `recon_wav.npy` [1,24960]
  (**decode round-trip contract**) · `meta.json` · `gen_golden.py` (reproducer). Venv (system-torch-reused,
  `descript-audiotools`+`dacvae`) deleted after capture.

## 4. Port — `codec::dacvae` reusing `codec::dac` MAXIMALLY

New module **`crates/waav-infer-backend-torch/src/codec/dacvae.rs`** — `DacVae { encode, decode }`:
- **Reused verbatim from `codec::dac`:** `snake1d`, `DacConv` (symmetric pad), `DacConvT` (pad=⌈s/2⌉,
  output_padding=s%2), **`DacResidualUnit`** (the encoder ResidualUnit `Snake→Conv(k7,dil)→Snake→Conv(k1)` +
  center-crop shortcut is structurally identical to the DAC one). No conv/snake/residual math duplicated.
- **Net-new (the encoder + VAE seam):** `pad_reflect` (`_pad` reflect-pad to hop 1920), `EncoderBlock`
  (3× `DacResidualUnit` d=1/3/9 → Snake → strided downsample conv), `Encoder`, the `in_proj` 1×1 conv +
  `chunk(2)[0]` mean, and the standalone `Decoder` (out_proj → model.0 → 4× DecoderBlock chunk-2 selector
  `[0,1,4,5,8,9]` → watermark-bypass `Snake→Conv(96→1)→tanh`). **weight-norm folded at load**
  (`g·v/‖v‖` over dims≠0, same math as `cfm::vocoder::weight_norm_reconstruct`).
- Decode path is the same op-sequence Irodori proved byte-faithful; extended here into a self-contained codec
  (rather than duplicating Irodori's private decoder or touching the concurrently-edited `irodori.rs`).

## 5. Byte-faithful gate — PASS

Test **`crates/waav-infer-backend-torch/tests/cuda_torch_dacvae.rs`** (`--ignored`, CPU-f32 vs golden):

| stage  | shape       | max\|Δ\| vs golden | tol   |
|--------|-------------|--------------------|-------|
| **encode** (latent mean) | [1,32,13]  | **8.821e-6** | <1e-4 |
| **decode** (round-trip wav) | [1,24960] | **7.451e-7** | <1e-4 |

Round-trip stable (re-encode→re-decode `|y|max=0.596`, tanh-bounded). The residual is float-reduction-order
noise on the f32↔f32 path (reflect-pad + folded weight-norm), far inside the bar. **A real encode+decode on GB10,
byte-faithful to the golden** — the LAW is met.

---

## Verification (LAW)

- `cargo test -p waav-infer-backend-torch --lib` → **190 passed; 0 failed** (incl. 2 new `codec::dacvae` unit
  tests: `pad_reflect_matches_reference`, `weight_norm_fold_matches_reference`).
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean (exit 0)**.
- **Shared `codec::mod.rs` touched** (additive only: `pub mod dacvae` + `pub use` + doc) → re-verified consumers:
  - **irodori** byte-faithful gate: latent 1.955e-4, wav 1.625e-4 — **unchanged** baseline. ✅
  - **dia2** `cpu_fp32_codes_byte_identical`: **544/544 byte-identical** (proven in isolation). ✅
  - **csm/dia2 mimi-decode** bit-faithful lib tests (dia2 + csm regimes): green. ✅
  - **Note — a non-regression:** the *first* batched dia2 run showed 21/544 (an apparent failure). Proven to be a
    **global-libtorch-RNG / GPU-contention artifact** from concurrent agents (RSB/IndexTTS-2/Irodori) sharing the
    GPU + the global `manual_seed` RNG generator the dia2 AR multinomial sampler uses — **reverting my change did
    NOT fix it; running the test in an isolated fresh process did (544/544)**. My change (codec-only, additive) is
    not in the dia2 AR sampler path; it cannot affect AR codes. csm gate (4000/4000) was not separately re-run to
    avoid the same GPU-contention flake while peer agents run — the same isolation caveat applies; the codec math
    is source-identical.

## Files (absolute; ⚑ = shared)

- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/codec/dacvae.rs` — **new** (`DacVae` codec).
- ⚑ `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/codec/mod.rs` — **additive only**
  (`pub mod dacvae;` + `pub use dacvae::{DacVae, DacVaeConfig};` + a doc bullet; no existing line changed).
- `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_torch_dacvae.rs` — **new** gate.
- `/home/bud/.cache/waav-models/semantic-dacvae/{weights.safetensors, waav.json}` — acquired model + manifest.
- `/home/bud/.cache/waav-models/semantic-dacvae-golden/{input_wav,z_latent,recon_wav}.npy, meta.json, gen_golden.py`.

## Disposition update

`TRIAGE_DISPOSITION.md` §F: **Semantic-DACVAE → ONBOARDED + byte-faithful** (encode net-new, decode confirmed).
The "Yorch233/RSB" tag was **stale** — canonical is `Aratako/Semantic-DACVAE-Japanese-32dim` (RSB is the separate
in-flight item). Exposed as a reusable `codec::DacVae` (encode+decode) library, **not** engine-wired (it is a codec,
not a standalone task model — its decode already serves Irodori; its encoder is available for any latent-domain
TTS/VC that needs waveform→DACVAE-latent).
