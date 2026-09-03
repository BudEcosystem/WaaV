# Onboard: Trendyol/Trendyol-TTS (Turkish VoxCPM2 diffusion-AR TTS)

**Status: ONBOARDED — config + weights ONLY, ZERO Rust changes.** Trendyol-TTS is a merged-LoRA
Turkish finetune of `openbmb/VoxCPM2`, an architecture WaaV Infer **already supports**
(`crates/waav-infer-core/src/tts/voxcpm2.rs` + the `"voxcpm2"` registry arm). The base seam was
onboarded from an ONNX mirror; Trendyol ships only PyTorch weights, so the only onboarding work was a
one-time **throwaway** ONNX export of Trendyol's merged safetensors into the 3 graphs the existing seam
consumes. Serving stays pure-Rust-ORT (no venv/pip in the serving path — the venv was export-only).

| | |
|---|---|
| Model | Trendyol/Trendyol-TTS — Turkish TTS, a **VoxCPM2** finetune (2B tokenizer-free continuous-latent flow-matching TTS on a MiniCPM-4 backbone, 48 kHz) |
| Arch | `voxcpm2` (config `"architecture":"voxcpm2"`) — IDENTICAL dims to base (28 base LM layers + 8 residual, patch 4, feat 64, audio_vae out 48 kHz). Merged LoRA (r=64, α=64) on LM + DiT q/k/v/o |
| Triage tier | MODERATE |
| Official repo | `Trendyol/Trendyol-TTS` — **ungated, VERIFIED EXISTS** (HfApi). Files: `config.json`, `model.safetensors` (4.58 GB), `audiovae.pth` (0.38 GB), `tokenizer.json` + configs, `merge_manifest.json`, `lora_adapter/` (provenance) |
| Base model | `openbmb/VoxCPM2` (base_model tag) — same file layout; the `ai4all8/VoxCPM2-ONNX` mirror that onboarded base VoxCPM2 is an ONNX export of `openbmb/VoxCPM2`, NOT Trendyol |
| Onboarding | **config + weights** — re-exported Trendyol's merged safetensors → 3 ONNX graphs via the `ai4all8/VoxCPM2-ONNX` export repo (throwaway venv). NO new Rust, NO registry change. `waav.json` `{"architecture":"voxcpm2"}` + tokenizer symlinks |
| Acquired | `~/.cache/waav-models/trendyol-tts/` (orig weights) + `.../onnx/` (the Trendyol-specific export) |
| Accuracy | **BYTE-IDENTICAL** — Rust-ORT latents == Python-ORT golden **8192/8192 exact, max\|Δ\|=0.000e0** (Turkish, 128 latent frames). Export also faithful to PyTorch: prefill max\|Δ\|≤4.8e-5, decode `pred_feat` 6e-6 / `stop_flag` exact |
| Live RTF | **GB10 CUDA 0.654** (5.28 s audio in 3.46 s, peak 0.969 / rms 0.18 — non-silent); Python-ORT CPU ref 1.82 |

---

## 1. HfApi verification (method step 1)

`HfApi.model_info("Trendyol/Trendyol-TTS")` → **exists, `gated=False`**, `pipeline_tag=text-to-speech`,
tags include `voxcpm2`, `lora`, `merged-lora`, `turkish`, `base_model:openbmb/VoxCPM2`. So the arch is a
**known base WaaV already supports** — NOT a genuinely-new stack.

`config.json` confirms `"architecture":"voxcpm2"` with dims byte-identical to the base
(`lm_config.num_hidden_layers=28`, `residual_lm_num_layers=8`, `patch_size=4`, `feat_dim=64`,
`audio_vae_config.out_sample_rate=48000`). `merge_manifest.json`: `artifact_type=merged_voxcpm2_lora`,
`base_model=openbmb/VoxCPM2`, LoRA r=64/α=64 on LM+DiT q/k/v/o, merged at `step_0002000`.

The safetensors header (577 tensors) carries the exact base-VoxCPM2 keys
(`base_lm.* ×254`, `residual_lm.* ×73`, `dit*`, `lm_to_dit_proj`, `feat_encoder.* ×112`) — the LoRA is
fully merged into the base weights, so it loads through the stock `VoxCPM2Model.from_local` with **no
key mismatch** (confirmed by a dry-run load: 35 s, 28+8 layers, patch 4, feat 64).

## 2. Acquire (method step 2)

`hf_hub_download` (HF_TOKEN, hf_transfer/Xet disabled — the high-perf path stalled in this env) →
`~/.cache/waav-models/trendyol-tts/`: `model.safetensors` 4.58 GB + `audiovae.pth` 0.38 GB + tokenizer.

## 3. The reuse decision (method step 3)

**Decision: config + weights, ZERO code.** WaaV already has the VoxCPM2 diffusion-AR seam
(`tts/voxcpm2.rs`) and the `"voxcpm2"` arch arm in `model.rs`. The seam consumes 3 ONNX graphs
(`voxcpm2_prefill.onnx`, `voxcpm2_decode_step.onnx`, `audio_vae_decoder.onnx`) + `tokenizer.json` via a
`waav.json` manifest. Trendyol ships only PyTorch, so the one onboarding step is re-running the SAME
export the base mirror used (`github.com/ai4all8/VoxCPM2-ONNX`) against Trendyol's merged weights — a
**throwaway-validation** use of a venv (`voxcpm` pip pkg, torch.onnx). The serving path is unchanged
pure-Rust-ORT; nothing pip/venv ships.

The export wrappers emit graphs with the EXACT I/O names the Rust seam feeds/reads (prefill:
text/text_mask/feat/feat_mask → dit_hidden/base_next_keys/…/prefix_feat_cond; decode_step:
dit_hidden/…/cfg_value → pred_feat/…/stop_flag; vae: z → audio), so the Trendyol graphs drop straight in.

## 4. Export (the only onboarding work)

Throwaway venv `--system-site-packages` (reuses host torch 2.12+cu130 / onnx 1.21 / ORT 1.26 /
onnxscript) + `voxcpm==2.0.3`. Ran the 3 exporters against `~/.cache/waav-models/trendyol-tts` →
`.../onnx/`:

- **AudioVAE** (enc+dec) — ONNX written OK; the `--validate` torch forward crashed with a torch-2.12
  CUDA `conv1d` "illegal immediate parameter" quirk (a host-torch kernel bug, NOT an export defect — the
  ONNX graph was already saved before validate). Decoder is the one the seam uses.
- **Prefill** — exported + **validation PASSED**: ONNX vs torch max\|Δ\| ≈ 3.1e-5 (dit_hidden),
  ≤4.8e-5 (all 6 outputs), `prefix_feat_cond` exact. The export is faithful to the Trendyol PyTorch model.
- **Decode step** (10 CFM Euler timesteps baked in) — TBD.

waav.json + tokenizer symlinks placed in `.../onnx/`. Final `.../onnx/` layout (drop-in for the seam):
`voxcpm2_prefill.onnx` (6.3M + 7.8G .data), `voxcpm2_decode_step.onnx` (20M + 8.1G .data),
`audio_vae_decoder.onnx` (1M + 175M .data), `waav.json`, `tokenizer.json`→`../tokenizer.json`.

## 5. Smoke + accuracy + RTF (live on GB10)

Reference golden (`/tmp/trendyol_golden.py`, Python-ORT, CPU) on the Trendyol Turkish prompt
*"Merhaba, bu WaaV Infer motorunda canlı çalışan Türkçe bir konuşma sentezi testidir."* → 40 prefill
tokens, AR stopped naturally at step 31 (32 patches), **5.12 s of 48 kHz audio**, CPU RTF 1.82. golden.json
(noise schedule + the [1,64,128] VAE-input latents) written into `.../onnx/`.

Then the production WaaV seam (`crates/.../tests/voxcpm2_live.rs`) pointed at the Trendyol dir:

1. **Byte-faithful accuracy gate (THE BAR).** Rust-ORT drove the diffusion-AR loop with the EXACT golden
   noise schedule. Prefill tokens matched the golden (tokenizer byte-identical for Turkish). Assembled
   latents: **8192/8192 exact, max\|Δ\|=0.000e0, rmse=0.000e0** vs the Python-ORT golden — full
   cross-runtime bit-identity on the Trendyol graphs.
2. **Real synthesis + GB10 RTF.** Production `synthesize` (PCG32-seeded Gaussian noise), CUDA EP:
   **5.28 s @ 48 kHz, RTF 0.654** (3.46 s synth), peak 0.969 / rms 0.18 — real, non-silent Turkish audio.
   Both `voxcpm2_live` tests pass (`test result: ok`).

## 6. Files (flag shared touches)

- **NO Rust source modified.** The `voxcpm2` arch arm (`crates/waav-infer-core/src/model.rs`), the seam
  (`crates/waav-infer-core/src/tts/voxcpm2.rs`), and the live test (`tests/voxcpm2_live.rs`) ALREADY
  existed from the base-VoxCPM2 onboarding — all mtimes predate this session. So the shared `model.rs` /
  `model registry` were **not touched** (zero collision risk with concurrent agents). `cargo test` /
  clippy are unaffected; the live `voxcpm2_live` tests pass against the Trendyol dir.
- **Acquired (new):** `~/.cache/waav-models/trendyol-tts/` (orig PyTorch weights) +
  `~/.cache/waav-models/trendyol-tts/onnx/` (the Trendyol-specific 3-graph export + waav.json + golden.json
  + tokenizer symlinks). The base `~/.cache/waav-models/voxcpm2/onnx/` is **untouched / separate**.
- **Throwaway (not shipped):** `/tmp/voxcpm_export_venv` (export venv, `voxcpm==2.0.3` + host torch),
  `/tmp/VoxCPM2-ONNX-export` (the ai4all8 export repo), `/tmp/trendyol_golden.py`. None on the serving path.

## 7. How to serve

```
source gb10-env.sh
VOXCPM2_DIR=~/.cache/waav-models/trendyol-tts/onnx VOXCPM2_CUDA=1 \
  cargo test -p waav-infer-core --test voxcpm2_live -- --nocapture --test-threads=1
```
or load `~/.cache/waav-models/trendyol-tts/onnx` (its `waav.json` selects the `voxcpm2` arm) through the
normal model registry — the same path as base VoxCPM2, now with the Turkish-finetuned weights.

## Disposition summary

**exists?** Yes — `Trendyol/Trendyol-TTS`, ungated. **arch?** `voxcpm2` (merged-LoRA Turkish finetune of
`openbmb/VoxCPM2`) — a stack WaaV **already supports**; REUSED the existing seam. **onboarded?** Yes,
config + weights, **zero Rust**. **accuracy:** byte-identical Rust-ORT vs Python-ORT golden
(8192/8192 exact, max\|Δ\|=0.000e0); export faithful to PyTorch (≤4.8e-5). **RTF:** GB10 CUDA **0.654**.
