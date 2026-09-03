# Onboarding: kyutai/hibiki-zero-3b — **PORTED + BYTE-FAITHFUL** ✅

**Date:** 2026-06-24 · **Triage:** HARD · **Status:** 🟢 **PORTED, GOLDEN-VERIFIED, csm/dia2 NON-REGRESSED.** The
Moshi-class full-duplex S2S translation model is live in the Torch backend, byte-faithful to the moshi
reference golden on every deterministic surface (Mimi encode, Mimi decode, the greedy backbone+depformer
duplex trajectory), and the shared-codec edit it required is proven safe (csm 4000/4000 + dia2 608/608 still
byte-identical). `--lib` green (181/181) and clippy `--all-targets -D warnings` clean.

This finishes the scoped port in `ONBOARD-hibiki.md`. The prior agent had landed the full implementation on
disk (~1.2k LOC, compiles clean) and captured the golden; it died on a transient API rate-limit before running
the verify + writing this report. This session captured (re-captured, deterministically) the golden, ran the
byte-identity gate, and ran the csm/dia2 regression gates — all green, no divergence to RCA.

---

## 1. Was it ported + verified? — **YES.**

| Gate | What it proves | Result |
|---|---|---|
| **G1 — Mimi encode byte-identical** | `MimiEncoder.encode(source_wav)` codes == golden `source_codes` | **0 / 128 mismatches** (8 frames × 16 cb) ✅ |
| **G2 — Mimi decode within tolerance** | native-Mimi decoder remap `decode(codes)` vs golden recon PCM | **corr = 1.000000, max_abs_diff = 0.000000** ✅ (far above the ≥0.99 bar) |
| **G3 — greedy duplex codes byte-identical** | the backbone + weights-per-step depformer + multistream-delay greedy trajectory's target codes == golden `target_audio_frames` | **0 / 96 mismatches**; text tokens `[-1,-1,3,3,3,3,3,3]` == golden ✅ |
| **G4 — `DuplexStep` seam, real S2S turn** | `HibikiDuplexModel::model_out` read-while-emit emits the 16-cb target frame each tick | 128 codes emitted over 8 frames through the seam ✅ |
| **DBG — encode-latent localization** | SEANet / encoder-transformer / downsample latent vs golden | corr = 1.0; SEANet max_abs **6e-8**, transformer **6e-8**, latent **2.5e-7** (the f32 floor) ✅ |
| **DBG — raw depformer (pre-undelay)** | the RAW generated codes per outer step vs golden `dbg_raw_audio` | **0 / 128 mismatches** ✅ |

> Run: `source gb10-env.sh && cargo test -p waav-infer-backend-torch --test torch_hibiki -- --ignored --nocapture --test-threads=1`
> (CPU f32 — the golden's regime; gates without an accelerator.)

**No divergence existed to RCA.** The port reuses dia2's proven backbone/depformer machinery, and every
hibiki-delta surface (the Mimi **encoder**, the native-moshi weight remap, the dual-stream duplex interleave)
landed byte-faithful on the first verified run — the encoder latent and the raw depformer codes both match the
moshi reference to the f32 floor.

---

## 2. The S2S turn + byte-faithfulness + RTF

- **The turn.** Each 80 ms frame: `mimi.encode(user_audio[1920]) → src_codes[16]` (the always-modeled user
  stream) → scatter into the Moshi multistream circular cache (channels 17..32, delay `[0,2,2,…]`) → 28-layer
  fused-QKV GQA backbone → greedy text token (channel 0) → 6-layer weights-per-step depformer → 16 greedy
  target audio codes → `mimi.decode(target_codes[16]) → 24 kHz waveform`. Reading the user codes while emitting
  the target text + 16 audio codes every frame is the duplex contract, implemented on the real
  `waav_infer_runtime::DuplexStep` seam (`HibikiDuplexModel::model_out(channel, frame_idx, user_in) →
  StepOutput::codec_per_codebook(16)`), with per-channel session isolation (circular cache + backbone/depformer
  KV keyed by `ChannelId`).
- **Byte-faithfulness:** greedy (temp 0) codes are deterministic → byte-identical vs the moshi golden on every
  deterministic surface (encode codes, decode PCM, the backbone+depformer trajectory). The f32 regime is the
  faithfulness target; CUDA bf16 tracks the reference's own bf16 floor (as documented for dia2/csm).
- **RTF:** **22.8** on CPU f32 (0.640 s audio in 14.6 s wall) — the *faithfulness* regime, not the perf regime.
  CUDA bf16 is the serving target (dia2, the same backbone family, measures RTF 5.0 on GB10 CUDA bf16; hibiki's
  3B backbone is comparable). CPU f32 RTF is expected to be ≫1 and is not a serving claim.

---

## 3. csm / dia2 re-verification — **NO REGRESSION**

The port edited the shared `codec::mimi` (a new `MimiConfig.interleaved_rope: bool` field, the
`mimi_24khz_native()` constructor, and `MimiLayer::forward` made `pub(crate)` with a RoPE-flavour branch). The
default constructor `mimi_24khz()` sets `interleaved_rope: false`, preserving the **exact** original
`apply_positions` (rotate-half) path that csm/dia2 use — the native interleaved path (`true`) is only taken by
hibiki's `mimi_24khz_native()`. Confirmed empirically:

| Model | Gate | Result |
|---|---|---|
| **csm** | `cuda_csm_codes_byte_identical_to_sidecar` (CUDA bf16) | GREEDY codes **BYTE-IDENTICAL** — 125 frames × 32 cb = **4000/4000** ✅ |
| **dia2** | `cpu_fp32_codes_byte_identical` (CPU fp32) | **544/544** byte-identical to the sidecar ✅ |
| **dia2** | `cuda_bf16_codes_byte_identical` (CUDA bf16) | **608/608** byte-identical to the CUDA sidecar (RTF 5.04) ✅ |

csm exercises the shared `MimiDecoder` + `MimiLayer` directly (its audio decode), so its 4000/4000 pass is the
direct proof the shared-codec edit is safe.

---

## 4. Exact files

**The partial (landed by the prior agent, now verified — NEW):**
- `crates/waav-infer-backend-torch/src/hibiki.rs` (941 LOC) — the full-duplex `step`/`step_frame` seam +
  `Backbone`/`BackboneLayer` (28-layer fused-QKV GQA, no q/k-norm, `expand_repeated_kv` GQA) +
  `Depformer`/`DepLayer` (6-layer, dep_q=16, 9 weight groups, per-step LayerNorm heads) + `load_*` (native-moshi
  weight remap, encoder/decoder) + `run_duplex_greedy_full` (the greedy trajectory) + `HibikiDuplexSession` +
  `HibikiDuplexModel: waav_infer_runtime::DuplexStep`.
- `crates/waav-infer-backend-torch/src/codec/mimi_encoder.rs` (249 LOC) — the NET-NEW `MimiEncoder` (SEANet
  downsampling conv encoder + encoder transformer + `downsample` replicate-pad strided conv + `RvqQuantize`
  nearest-centroid `cdist→argmin` encode), the mirror of the existing decoder.
- `crates/waav-infer-backend-torch/tests/torch_hibiki.rs` (391 LOC) — the acceptance gate (G1–G4 + the two
  localization diagnostics + RTF).

**Shared edits (additive, default-preserving — MODIFIED, +34/−3):**
- `crates/waav-infer-backend-torch/src/codec/mimi.rs` — `MimiConfig.interleaved_rope` field +
  `mimi_24khz_native()` ctor + `MimiLayer::forward` RoPE-flavour branch (default `false` = unchanged rotate-half).
- `crates/waav-infer-backend-torch/src/codec/mod.rs` — `pub mod mimi_encoder` + re-exports.
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod hibiki`.
- `crates/waav-infer-backend-torch/Cargo.toml` — add `waav-infer-runtime` dep (for the `DuplexStep` seam; no
  cycle — `-runtime` does not depend on `-backend-torch`).

**Golden (captured deterministically, reference-only — at `WaaV/inferv2/REVIEW/hibiki_golden/`):**
`source_wav / source_codes / source_recon_pcm / target_text_tokens / target_audio_frames / target_pcm.npy` +
`meta.json` (n_q=16, dep_q=16, 8 frames, frame_len=1920) + the `dbg_*` localization dumps. Captured via the
THROWAWAY `/tmp/capture_hibiki_golden.py` + `/tmp/hibiki_golden_venv` (moshi 0.2.13, validation-only, NOT a
serving path). Re-running the capture is bit-stable (`text_tokens [-1,-1,3,3,3,3,3,3]` reproduced).

---

## 5. The one remaining (serving-integration, NOT a faithfulness) item

The **model + the `DuplexStep` seam are complete and byte-verified**. The only un-landed piece from the original
8-step plan is **step 7 — server-engine arch registration** (`engine.rs` `"hibiki" => …` + a generic S2S model
registry). This is a *serving wiring* concern, not part of the FINISH/LAW: there is currently no generic S2S
model registry in `engine.rs` for any duplex model — the existing `CodecArDuplexModel` is likewise driven via
the `full_duplex_bench` driver (which consumes the same `DuplexStep` trait `HibikiDuplexModel` implements and
G4 exercises), not auto-registered in the engine. Wiring hibiki into a server endpoint would be the same effort
as wiring the first S2S endpoint generally, and is the documented next step — it does not affect the
byte-faithfulness result.

---

## Verdict

**PORTED + VERIFIED.** hibiki-zero-3b is byte-faithful to the moshi golden on every deterministic surface
(Mimi encode 0/128, Mimi decode corr 1.0, greedy duplex codes 0/96, raw depformer 0/128, encoder latent to the
2.5e-7 f32 floor), the real `DuplexStep` S2S turn runs through the runtime seam, and the shared-codec edit is
proven non-regressing (csm 4000/4000, dia2 608/608 + 544/544). `--lib` 181/181 green; clippy
`--all-targets -D warnings` clean. The LAW is met.
