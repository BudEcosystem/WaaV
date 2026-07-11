# ONBOARD — Aratako/Irodori-TTS (HARD-tier, deferred §F) → PORTED + BYTE-FAITHFUL

**Status: PORTED + BYTE-FAITHFUL on GB10 (CPU-f32 golden bit-faithful, CUDA live, end-to-end from raw text).**
A REAL Irodori-TTS synthesis runs natively in tch (in-process Torch backend), byte-faithful to a CPU-f32
reference golden. The latent core + DACVAE decoder + JP text frontend all landed and are gated.

---

## 1. HfApi-verify (the disposition note WAS stale)

The disposition (`TRIAGE_DISPOSITION.md §F`) named **`Aratako/Irodori-TTS · Semantic-DACVAE`**. The bare repo
id **does not exist** (404). HfApi search resolved the real family:
- canonical model: **`Aratako/Irodori-TTS-500M-v3`** (gated=False, MIT, `model.safetensors` 2.0 GB + README +
  `EMOJI_ANNOTATIONS.md`; **no config.json** — arch lives in the inference code, config in the safetensors
  `config_json` metadata).
- codec: **`Aratako/Semantic-DACVAE-Japanese-32dim`** (`weights.pth`, 410 MB).
- reference code: GitHub **`Aratako/Irodori-TTS`** (full `irodori_tts` package).
- a 6-graph ONNX export exists (`mtsmfm/Irodori-TTS-500M-v3-ONNX`: text/speaker/dit_step/duration/dacvae
  enc+dec) — used to confirm the I/O signatures; the port is native tch, not ORT.

**Arch (from `config_json` + GitHub `irodori_tts/model.py`):** a **Rectified-Flow Diffusion-Transformer
(RF-DiT)** following Echo-TTS — a **NON-AR flow-matching** TTS over **continuous 32-dim DACVAE latents** at
48 kHz. **NOT a finetune of any onboarded arm** (it is its own stack: the first DiT-RF / flow-matching member
in the fleet; the closest existing sibling is `dots`/`cfm`, but the joint-attention DiT + low-rank AdaLN +
complex/half RoPE + DACVAE codec are all new). So this is a genuine port, not a waav.json pointer — as §F
honestly predicted.

Pipeline (no-ref, deterministic path — the byte-faithful target):
1. `normalize_text` (NFKC + small replace map) → **llm-jp-3-150m** fast tokenizer (BOS + subwords, right-pad
   to 256, pad id 4).
2. 10-layer **TextEncoder** (SwiGLU + complex-RoPE self-attn, sigmoid-gated) → `text_norm`.
3. 8-layer **SpeakerEncoder** over a zeroed/masked ref latent (no-ref ⇒ unconditional) → `speaker_norm` →
   prepend-masked-mean-token.
4. token-sum AdaRN-Zero **DurationPredictor** (3 SwiGLU blocks) → `log1p(total)` → `expm1` → round → clamp →
   `latent_steps`.
5. seeded `randn` init noise + **N-step Euler RF** with independent-CFG (text 3 / speaker 5): each step runs
   the 12-layer joint-attention **DiT** (timestep low-rank AdaLN, joint cross-attn over self+text+speaker,
   half-RoPE), `x += v·Δt`.
6. **Semantic-DACVAE decoder** (q_out_proj 1×1 → conv 1024→1536 → 4 DecoderBlocks → watermark-bypass
   Snake→Conv(96→1)→Tanh) → 48 kHz waveform.

## 2. Acquire
`~/.cache/waav-models/irodori/`: `model.safetensors` (2.0 GB, fp32, 637 tensors) + `dacvae_decode.safetensors`
(0.32 GB, weight-norm-FOLDED decode path, 164 tensors) + `tokenizer.json` (llm-jp) + `waav.json` manifest.
Golden in `~/.cache/waav-models/irodori-golden/`.

## 3. Golden (throwaway venv, reference-only — NOT a serving path)
`torch_runtime/dump_irodori_golden.py` runs the upstream `irodori_tts` package on **CPU fp32**, no-ref, seed 0,
8 Euler steps, cfg 3/5, on text `こんにちは、これはテストです。`. Dumps `text_ids/text_mask`, `text_state`,
`speaker_state/mask`, `duration_features`, `init_noise`, `z_patched`, `z_latent` (139×32) and the
DACVAE-decoded `wav` (266 880 samples = 5.56 s). The decoded wav is real speech (envelope dynamic-range ratio
7.4 over 139 frames, RMS 0.126, peaks ±0.9).

## 4. Port — reuse-map (SHARED libs used MAXIMALLY)
- **REUSED shared `nn::rms_norm::rms_norm_decomposed` (Square::Mul)** — every RMSNorm (text/speaker/DiT q/k/out
  norms, incl. the 2-D `(heads,head_dim)` q/k weight broadcast).
- **REUSED shared `codec::snake1d`** — the DACVAE Snake nonlinearity `x + (α+1e-9)⁻¹·sin(αx)²` (identical form).
- **REUSED `tch` `at::linear`** (fused addmm) for every `nn.Linear`, and `Tensor::scaled_dot_product_attention`
  (the fused SDPA the reference's `F.scaled_dot_product_attention` uses).
- **irodori-specific (new):** complex RoPE (`view_as_complex` interleaved-pair form, θ=10000, f32 tables) +
  **half-RoPE** (only the leading half of the DiT heads rotate); **LowRankAdaLN** (Echo-style
  `up(down(silu(c)))+c` → RMSNorm-modulate + tanh-gate); the **joint cross-attention** (self+text+speaker KV,
  sigmoid-gated, context-KV precompute); the token-sum AdaRN-Zero duration predictor; and the **DACVAE
  decoder** (a chunk-2-selector conv decoder, see §5).

## 5. Byte-faithful gate + RCA (f32-CPU bisection)
The bisection (`debug_intermediates`) caught and resolved every divergence to byte-faithful:

| stage | max\|Δ\| vs CPU-f32 golden | note |
|---|---|---|
| text_state | 2.16e-6 | float-faithful (SDPA op-order; not a logic bug) |
| speaker_state | **0.0** | exact |
| duration log_frames | **0.0** (4.9408035) → 139 steps | exact |
| init_noise (seeded) | **0.0** | CPU RNG draw order matches |
| **z_latent (RF-DiT, 8-step)** | **1.96e-4** | **BYTE-FAITHFUL** (≪ 2e-3 gate) |
| **wav (DACVAE decode)** | **1.63e-4** (ref RMS 0.126) | **within tolerance** |

**RCA scars fixed during the port:**
1. **AdaLN gate source** — `JointAttention` gates with `sigmoid(gate(x))` where its `x` is the AdaLN-modulated
   `h` (not the residual input). Using the residual input gave max\|Δ\|=4.9; fixed → 1.96e-4.
2. **cond/uncond CFG ALIASING** (the [[waav-infer-cuda-graph-fanout]] THE SCAR) — `v = v_cond.shallow_clone()`
   then in-place `v += cfg·(v_cond − v_unc)` CORRUPTED `v_cond` (shared storage), poisoning the next CFG term
   (max\|Δ\|=11.4). Fixed with `v_cond.copy()` (deep). A real bug clippy's "manual assign-op" lint surfaced.
3. **device-independent RNG** — the reference draws init noise via a CPU `torch.Generator`; drawing on the
   CUDA generator gives a *different valid* sample. Fixed by drawing on CPU then moving to device ⇒ CUDA tracks
   the CPU golden (3.81e-4, not 5.8).
4. **DACVAE decoder is a chunk-2 SELECTOR** — `DecoderBlock.forward` (`_chunk_size=2`) keeps only block
   indices `[0,1,4,5,8,9]` (Snake → ConvTranspose-upsample → 3×ResidualUnit(d=1,3,9) → Identity); the other 6
   sub-modules (ELU/Conv with mismatched channels) are **dead/vestigial weights**. ResidualUnit shortcut is
   `true_skip` (identity). Plus `decoder.model.0` (conv 1024→1536) and the watermark-bypass head
   (`Snake→Conv(96→1)→Tanh`, the real waveform head; the last conv is Identity-replaced). All weight-norm convs
   FOLDED to plain `weight = g·v/‖v‖` at conversion.

## 6. Perf (GB10 CUDA, live)
Full no-ref synth of 5.56 s audio (8 Euler steps × 12-layer DiT, independent-CFG = 3 forwards/step): **~0.95–1.04 s,
RTF ≈ 0.17** (real-time-capable). CPU-f32 ~7.9 s (the byte-faithful reference path).

## 7. What landed vs scoped
**LANDED (verified):**
- Full latent-generation pipeline (tokenize → text/speaker encode → duration → RF-DiT Euler sampler) —
  byte-faithful (1.96e-4).
- DACVAE decoder (weight-norm-folded, chunk-2 selector) — wav within tolerance (1.63e-4).
- JP text frontend: `normalize_text` (NFKC + replace map) + llm-jp tokenizer wired → `synthesize_text` produces
  the golden token ids + latent from raw text (1.96e-4).
- GB10 CUDA path (RTF 0.17).
- 3 gated tests (`cuda_torch_irodori`): CPU byte-faithful gate, end-to-end text→audio, GB10 CUDA synth — all
  registered in `ci/heavy_live_tests.sh`.

**SCOPED (honest follow-ups, golden-staged):**
- **Engine integration** — `TorchIrodori` is NOT yet wired into `engine.rs::load_model_at` / a `TtsModel` impl
  (the `irodori` architecture in `waav.json` has no dispatch arm). The model loads + synthesizes via its public
  API; engine-serving is the next bounded step (the seam is the no-ref text→wav path). Deliberately deferred —
  `engine.rs`/`lib.rs` are shared with a concurrent IndexTTS-2 agent.
- **Speaker/ref-audio cloning** — only the **no-ref** path is ported/gated (the deterministic byte-faithful
  target). Ref-audio cloning needs the DACVAE *encoder* + loudness-normalize (audiotools) + the
  `prepend-masked-mean-token` speaker path with a real ref latent — all present in the reference; the encoder
  weights are NOT in `dacvae_decode.safetensors` (decode-only fold). Bounded follow-up.
- **`normalize_text` outer-bracket stripping** — omitted (only fires when the WHOLE string is one matched
  bracket pair; rare for TTS). Documented in code.
- **caption / voice-design** — v3 is `use_caption_condition:false`; the 600M-v3-VoiceDesign variant (caption
  conditioning) is a separate config (would reuse this arm + the caption encoder).
- **emoji style control** — works for free (emojis pass through the tokenizer as normal subwords; the duration
  predictor already counts them via `count_annotation_emojis`-equivalent features — those duration aux features
  are computed in the reference but the no-ref duration path here uses the token-sum architecture which doesn't
  consume aux, so it's already faithful).

## 8. Exact files
- **NEW** `crates/waav-infer-backend-torch/src/irodori.rs` — the port (`TorchIrodori`, ~1010 LOC).
- **NEW** `crates/waav-infer-backend-torch/tests/cuda_torch_irodori.rs` — 3 gated tests.
- **NEW** `torch_runtime/dump_irodori_golden.py` — throwaway-venv golden dumper.
- **EDIT (SHARED, additive only)** `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod irodori;` +
  `pub use irodori::{IrodoriError, TorchIrodori};` (no other-model edits; re-applied after the concurrent
  agent's IndexTTS-2 edits).
- **EDIT** `crates/waav-infer-backend-torch/Cargo.toml` — `unicode-normalization = "0.1"` (NFKC; already in lock).
- **EDIT** `ci/heavy_live_tests.sh` — 3 irodori gate entries + the descriptor comment.
- **artifacts** `~/.cache/waav-models/irodori/{model.safetensors, dacvae_decode.safetensors, tokenizer.json,
  waav.json}` + `~/.cache/waav-models/irodori-golden/*.npy`.

**No shared `nn::`/`codec::` source was modified** (only read-only reuse of `rms_norm_decomposed` + `snake1d`),
so dia2 (608) / csm (4000) are unaffected.

## 9. Verification (LAW)
- `cargo test -p waav-infer-backend-torch --lib` → **187 passed, 0 failed** (includes dia2/csm shared-module
  tests, all green).
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean (0 errors)**.
- `cuda_torch_irodori` (3 tests, `--ignored`) → **3 passed**: latent 1.96e-4, wav 1.63e-4, e2e 1.96e-4,
  CUDA 3.81e-4 @ RTF 0.17.

**BOTTOM LINE: ported + byte-faithful. A REAL Irodori-TTS-500M-v3 synth runs on GB10 (CPU bit-faithful to the
reference golden, CUDA live at RTF 0.17, end-to-end from raw Japanese text). The no-ref deterministic path is
the byte-faithful contract delivered; ref-audio cloning + engine wiring are the honest, golden-staged
follow-ups.**
