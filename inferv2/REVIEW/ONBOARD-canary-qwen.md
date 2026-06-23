# Onboarding: nvidia/canary-qwen-2.5b — ONBOARDED (byte-faithful, live-verified)

**Date:** 2026-06-23
**Model:** `nvidia/canary-qwen-2.5b` (NeMo SALM — FastConformer speech encoder → linear projector → Qwen3-1.7B LLM decoder w/ speech-LoRA, greedy ASR)
**Status:** ✅ ONBOARDED. Live transcribe on GB10 CUDA, **0.0% WER vs the NeMo reference** (byte-identical), RTF ~0.12–0.16.
**Triage tier:** was HARD; turned out a clean win because NVIDIA ships an HF safetensors export (no `.nemo` unpack needed).

---

## 1. HfApi verification (FIRST, per method)

- `nvidia/canary-qwen-2.5b` **EXISTS, ungated, public** (`gated=False`, `private=False`), `library=nemo`, `pipeline_tag=automatic-speech-recognition`, `base_model=Qwen/Qwen3-1.7B`, license cc-by-4.0.
- **Critical finding that flipped the triage:** the HF repo ships **`model.safetensors` (5 GB bf16) + `config.json`** alongside the `.nemo`. The stale `waav.json` ("blocked-pending-portable-reimplementation, .nemo only") was written before this HF export existed. **No `.nemo` unpack, no NeMo serving runtime, no per-venv path** — the safetensors load directly (directive #4 satisfied).
- Weight inventory (from the safetensors header): 1718 tensors in 3 groups — `embed_tokens` (1), `llm.*` (421, the Qwen3-1.7B decoder **with a PEFT speech-LoRA**: q_proj/v_proj carry `base_layer` + `lora_A`/`lora_B`), `perception.*` (1296, the FastConformer encoder + the baked mel featurizer + the `proj` linear). `__metadata__` declares `lm_head.weight == embed_tokens.weight` (**tied head**).

### Architecture (config.json)
- **Encoder** = NeMo `ConformerEncoder`: `subsampling=dw_striding factor 8`, `d_model=1024`, `n_layers=32`, `n_heads=8`, `conv_kernel=9`, `conv_norm=batch_norm`, `self_attention_model=rel_pos` (Transformer-XL relative-position MHA), `xscaling=false`, `feat_in=128`.
- **Subsampling** (`pre_encode`) = depthwise-separable dw-striding: `Conv2d(1→256,k3,s2) → ReLU → [DWConv2d(k3,s2,g=256) → PWConv2d(256→256,k1) → ReLU]×2` (verified from conv.{0,2,3,5,6} shapes) → flatten freq → `out` Linear `4096→1024`.
- **Projector** = `IdentityConnector` (no-op) → a single `perception.proj` Linear `1024→2048`.
- **Decoder** = Qwen3-1.7B (28 layers, hidden 2048, 16 q / 8 kv heads × head_dim 128, q/k RMSNorm, SwiGLU, RoPE θ=1e6) + the speech-LoRA (r=128, α=256 → **scale 2.0**, on q_proj/v_proj).
- **Preprocessor** = NeMo `AudioToMelSpectrogramPreprocessor`: n_fft 512, win 400, hop 160, 128 mels, `normalize=per_feature`, log, dither 1e-5 (no-op at eval). The **mel filterbank ships baked into the weights** (`perception.preprocessor.featurizer.fb [1,128,257]` + `featurizer.window [400]`).

---

## 2. Acquire / weights path

- Weights live in the HF cache; symlinked into the WaaV model dir:
  `~/.cache/waav-models/canary-qwen-2.5b/{model.safetensors, config.json}` → HF snapshot.
- The Qwen3 tokenizer (canary ships none — NeMo wraps the Qwen2 tokenizer) was fetched from `Qwen/Qwen3-1.7B`:
  `~/.cache/waav-models/canary-qwen-2.5b/{tokenizer.json, tokenizer_config.json, vocab.json, merges.txt}`.
- `waav.json` rewritten: `{"architecture":"canary_qwen","modality":"stt","runtime":"torch-inprocess","precision":"bf16"}`.

---

## 3. Integration (REUSE-first, the LLM-decoder-ASR template)

New module `crates/waav-infer-backend-torch/src/canary_qwen.rs` (`TorchCanaryQwen`, impl `SttModel`):

- **Qwen3 decoder: REUSED the higgs-stt stack verbatim** — `nn::Backbone` + `build_qwen3_layer` (separate q/k/v/o, per-head q/k fused RMSNorm, RoPE apply_start, `FusedCausalGqa` SDPA, SwiGLU, `KvCache`), the f32 first-max-tie-break greedy argmax, the scatter-into-prompt decode loop. The only delta: **LoRA-merge q_proj/v_proj at load** (`W_eff = W_base + 2.0·(B@A)`, folded into a plain `nn::Linear`), and the **tied lm_head** (= embed_tokens).
- **Conformer conv module: REUSED the granite idiom** — `LN → pointwise1 → GLU(1) → depthwise(k9, batch_norm) → SiLU → pointwise2` (kernel 9 vs granite's 15; symmetric pad).
- **New FastConformer glue** (composes `nn::LayerNorm`/`nn::Linear`/`nn::Mlp::ungated`/`sdpa`):
  - dw-striding depthwise-separable Conv2d subsampling;
  - **Transformer-XL rel-pos MHA** (`linear_pos` + learned `pos_bias_u`/`pos_bias_v`, `matrix_ac + rel_shift(matrix_bd)`);
  - the descending-position sinusoidal `RelPositionalEncoding`;
  - macaron block `x += ½FF1; x += MHA; x += Conv; x += ½FF2; norm_out`.
- **Mel**: NeMo per-feature log-mel computed with the model's OWN baked-in `featurizer.fb`/`window` via the libtorch STFT (granite pattern — no filterbank reconstruction drift), then per-mel-bin `(x-mean)/std` normalization.
- **Prompt** (traced live from NeMo's `apply_chat_template`): `<|im_start|>user\nTranscribe the following: <|audioplaceholder|><|im_end|>\n<|im_start|>assistant\n` = ids `[151644,872,198,3167,3114,279,2701,25,220, <LOCATOR>, 151645,198,151644,77091,198]`. Hardcoded as `PROMPT_PREFIX`(9) + N audio embeddings + `PROMPT_SUFFIX`(5).

### Files touched
- **NEW** `crates/waav-infer-backend-torch/src/canary_qwen.rs` (the model).
- **NEW** `crates/waav-infer-backend-torch/tests/cuda_torch_canary_qwen.rs` (the live WER/RTF gate).
- **SHARED (flagged)** `crates/waav-infer-backend-torch/src/lib.rs` — added `pub mod canary_qwen;` + `pub use canary_qwen::{CanaryQwenError, TorchCanaryQwen};`.
- **SHARED (flagged, concurrent ViiTorVoice agent active)** `crates/waav-infer-server/src/engine.rs` — added `TorchCanaryQwen` to the torch import list, the `"canary_qwen"|"canary-qwen"|"salm"` dispatch arm → `LoadedModel::Stt`, and the arch name to the error list. (Touched around the ViiTorVoice agent's `TorchViitorVoice`/`viitorvoice` edits; minimal, non-overlapping.)

---

## 4. Smoke / 5. Accuracy / 6. Perf — LIVE on GB10 CUDA (bf16)

Reference = **NeMo `SALM.from_pretrained('nvidia/canary-qwen-2.5b').generate()`** (throwaway venv, validation only — NOT a serving path). Two LibriSpeech sample clips (the canary README widget utterances).

| clip | dur | WaaV transcript | WER vs NeMo golden | WER vs LibriSpeech GT | RTF |
|------|-----|-----------------|--------------------|-----------------------|-----|
| sample1 | 13.7s | "going along slushy country roads … immediately afterwards." | **0.0%** | **0.0%** | 0.159 |
| sample2 | 14.2s | "Before he had time to answer a much encumbered Vera … black red gamecock." | **0.0%** | 5.1%* | 0.115 |

\* The single sample2 GT "diff" is `gamecock` (canary) vs `GAME COCK` (LibriSpeech's two-word spelling) — a compound-word tokenization artifact, **not** a recognition error; NeMo itself emits "gamecock", so the WaaV arm is byte-identical to the reference.

- **Accuracy: byte-identical to NeMo** on both clips, including punctuation and casing (the model is a punctuated/cased ASR: "He'll", "Sunday", "Vera", "?"). 0.0% WER vs the NeMo golden = the bf16-CUDA forward is faithful end-to-end (mel → dw-striding → 32 rel-pos conformer blocks → projector → LoRA-merged Qwen3 → tied-head greedy).
- **Perf:** RTF **0.12–0.16** on GB10 (≈6–8× faster than realtime); model load 5.0 s; peak memory healthy (no OOM, one model at a time).

### Faithfulness scars handled (the 100%-correctness playbook)
fused RMSNorm/LayerNorm/GLU; bf16-on-CUDA dtype regime (= NeMo's `torch_dtype=bfloat16`); TF32 global context; RoPE f32 inv_freq tables; greedy f32 argmax with first-max tie-break; LoRA merge at load; tied lm_head; baked-fb mel (no filterbank drift); the exact NeMo chat-template prompt ids.

---

## 7. Tests

- `cargo test -p waav-infer-backend-torch --lib canary_qwen` → **4/4 green** (argmax tie-break, dw-striding shape, rel_shift shape, rel_pos_encoding center).
- `cargo clippy -p waav-infer-backend-torch --lib --features cuda` → **clean** (no canary_qwen warnings/dead code).
- `cargo build -p waav-infer-server --features torch` → **clean** (engine dispatch wired).
- `cargo test … --test cuda_torch_canary_qwen --features cuda -- --ignored` → **green** (0.0% avg WER, asserts non-empty + WER < 15%).
- (The shared lib was intermittently broken by the concurrent ViiTorVoice agent's in-flight edits to `viitorvoice.rs`; rode through with a re-read+retry loop. My module never had a build error after the conv-subsampling fix.)

---

## RETURN

**Onboarded?** YES — byte-faithful, live-verified. Reused the **higgs-stt Qwen3 decoder template** (verbatim + LoRA-merge) and the **granite conformer-conv idiom**; added FastConformer glue (dw-striding subsampling + Transformer-XL rel-pos MHA + NeMo per-feature baked-fb mel).
**WER:** 0.0% vs NeMo reference (both clips, byte-identical); 0.0% / 5.1%* vs LibriSpeech GT (*= a compound-word spelling, not an error).
**RTF:** 0.12–0.16 on GB10 CUDA bf16.
**Exact files:** new `canary_qwen.rs` + `tests/cuda_torch_canary_qwen.rs`; shared touches (flagged) `backend-torch/src/lib.rs` + `server/src/engine.rs`.
**Blocker:** none — the `.nemo`-export concern was moot (HF ships safetensors).
