# Onboard — IndexTeam/IndexTTS-2 (HARD-tier codec-AR TTS)

**Status: PORTED + BYTE-FAITHFUL** for the deterministic core (the GPT-2 autoregressive backbone → greedy mel
codes). The conditioning front-end and the acoustic back-half are **scoped follow-ups with the golden staged**
(both are genuine large multi-network ports; the AR core is gated *without* them via a staged `inputs_embeds`).

The triage's TRIAGE_DISPOSITION.md §F entry — "IndexTeam/IndexTTS-2 — new codec-AR TTS stack, a genuine
multi-day port" — is **accurate**: IndexTTS-2 is a **7-network pipeline**, not a config add. This session
ported + byte-faithfully gated the heart of it (the GPT-2 AR LM, the RNG-free deterministic stage that *is* the
proven byte-faithful cut), and precisely scoped the remainder with the golden staged.

---

## 1. HfApi verification (real repo, files, arch)

`IndexTeam/IndexTTS-2` — **public** (not gated), `pipeline_tag: text-to-speech`, langs en/zh, arxiv 2506.21619.
Files (HfApi `files_metadata`):
- `gpt.pth` (3484.7 MB) — the **GPT-2 AR LM** (`UnifiedVoice`) + conditioning encoders.
- `s2mel.pth` (1202.2 MB) — the **s2mel DiT flow-matching** acoustic model + length regulator + style encoder.
- `qwen0.6bemo4-merge/` (model.safetensors 1192.1 MB) — a **Qwen3 0.6B** emotion text-classifier
  (`Qwen3ForCausalLM`, hidden 1024, 28L, 16q/8kv) — only on the `use_emo_text` path.
- `bpe.model` (SentencePiece), `feat1.pt`/`feat2.pt` (spk/emo matrices), `wav2vec2bert_stats.pt`, `config.yaml`.
- Auxiliary (downloaded by `ensure_models_available`): `facebook/w2v-bert-2.0`, `amphion/MaskGCT` semantic
  codec, `funasr/campplus`, `nvidia/bigvgan_v2_22khz_80band_256x`.

Arch confirmed from `config.yaml` + the GitHub reference (`index-tts/index-tts`, `indextts/infer_v2.py`):
the full inference graph is

```
ref-wav → w2v-bert-2.0 (hidden_states[17], normalized)  →  spk_cond_emb / emo_cond_emb
        → semantic FSQ codec (quantize)                  →  S_ref
        → CAMPPlus speaker encoder                        →  style[1,192]
GPT (UnifiedVoice): ConformerEncoder×2 + PerceiverResampler×2 conditioning → GPT-2 AR LM → MEL CODES  ← [THIS PORT]
        → s2mel: gpt_layer + length_regulator + DiT-CFM (25 steps, cfg 0.7) →  mel
        → BigVGAN vocoder                                 →  22 kHz wav
```

---

## 2. Acquire

`~/.cache/waav-models/indextts2/` (full repo) + `~/.cache/waav-models/w2v-bert-2.0/` (for the golden
conditioning). NOTE: HuggingFace **xet** transfer was stalled on this box; the fix was `HF_HUB_DISABLE_XET=1
HF_HUB_ENABLE_HF_TRANSFER=0` → plain HTTPS, which downloaded reliably.

- `~/.cache/waav-models/indextts2/waav.json` — `{architecture:"indextts2", backend:"torch-inprocess",
  task:"tts", precision:"fp32", weights:"indextts2_gpt.safetensors"}`.
- `~/.cache/waav-models/indextts2/indextts2_gpt.safetensors` (1.98 GB) — the **extracted GPT-2 AR core**
  (296 tensors: `mel_embedding`, `mel_pos_embedding`, `gpt.h.{0..23}.*`, `gpt.ln_f`, `final_norm`, `mel_head`),
  extracted from `gpt.pth` by `scratchpad/extract_indextts2_gpt.py`.

## 3. Golden (throwaway reference venv — reference-only, NOT a serving path)

Reference IndexTTS-2 pinned `transformers==4.52.1` (the box's system 5.12.0 is ABI-incompatible with the
vendored `transformers_gpt2.py` internal imports). Built a **throwaway** `uv venv --python 3.12
--system-site-packages` (reuses the system torch 2.12+cu130 by appending the user-site to `sys.path` so the
pinned transformers still wins on the front), installed only `transformers==4.52.1 tokenizers==0.21.0 omegaconf
sentencepiece einops`. `scratchpad/indextts2_golden.py` loads `UnifiedVoice` (gpt.pth) + w2v-bert, builds the
speaker/emotion conditioning for a fixed ref-wav (`assets/kokoro_m1_sample.wav`) + fixed text, and runs the
GPT-2 AR in **greedy** (`do_sample=False, num_beams=1`, RNG-free) on CPU f32 (the reference `use_fp16=False`
regime). Dumped to `~/.cache/waav-models/indextts2/golden/` + committed to
`WaaV/inferv2/REVIEW/indextts2_golden/`:
- `inputs_embeds.npy` `[1,63,1280]` — the STAGED `[cond(32)+text]` prefix (the front-end output).
- `codes.npy` `[1,200]` — the greedy mel codes (**the byte-faithful gate target**).
- `step0_logits.npy` `[1,8194]`, `attention_mask.npy`, `spk_cond_emb.npy`, `emovec.npy`, `gpt_keyshapes.json`.

## 4. Port — `crates/waav-infer-backend-torch/src/indextts2.rs` (composes the shared `nn`)

The GPT-2 AR LM is a **canonical eager GPT-2** decoder (24 layers, hidden 1280, 20 heads, head_dim 64). The
port is the **cohere recipe** (learned absolute positions, LayerNorm, no RoPE) — maximal shared-lib reuse:
- **layers** = `nn::TransformerLayer` { `nn::Norm::Layer` (LayerNorm w+b, f32 variance) + `nn::Attention`
  (`Proj::Separate`, `ProjPrec::Native`, `RopeApply::None`, `Kernel::ManualMha`, scale 1/√64) + ungated
  `nn::Mlp` }. The combined-QKV `Conv1D` weight `[1280,3840]` is **split** along the out-dim into q|k|v and
  **transposed** to the `nn::Linear` `[out,in]` layout at load (Conv1D stores `[in,out]`).
- **the one shared-lib addition**: `nn::Act::GeluNew` (`x.gelu("tanh")` = HF `ACT2FN["gelu_new"]`, the GPT-2
  default `activation_function`). Purely additive (a new enum variant + match arm in `nn/mlp.rs`).
- **TWO final LayerNorms**: the transformer's own `gpt.ln_f` (the Backbone `final_norm`) THEN the separate
  `final_norm` (`GPT2InferenceModel.lm_head = Sequential(final_norm, mel_head)`), then `mel_head`.
- **AR loop** reproduces `GPT2InferenceModel.generate` greedy: prefill `[staged inputs_embeds ++
  (mel_embedding[start_mel]+mel_pos[0])]` with an explicit `[S+1,S+1]` causal additive mask (ManualMha = the
  GPT-2 eager `_attn`, applies only the explicit mask), then single-token `argmax` steps, stop at
  `stop_mel_token`.

### THE RCA (the one byte-identity scar caught + fixed)
First run: codes 0,1 matched, **diverged at code 2** (AR-compounding). f32-CPU bisection via a live position
probe (`scratchpad/probe_positions.py`, patching `LearnedPositionEmbeddings.get_fixed_embedding`) revealed the
reference's decode mel-position index sequence is **`2,3,4,…`** — position **1 is SKIPPED**. The reference's
`mel_position = attention_mask.shape[1] - mel_len` where `mel_len=63` (the staged prefix) but the prefill
`fake_inputs` carried an EXTRA start_mel token (prefill seq = 64), so after HF extends the mask to 65 the first
decode position is `65 - 63 = 2`. Fixed: the port's first decode position is **2**, not 1. → byte-identical.

## 5. Byte-faithful gate — PASS

`tests/cuda_torch_indextts2.rs :: indextts2_greedy_codes_byte_identical` (CPU f32):

```
OK: 200 greedy mel codes byte-identical to the IndexTTS-2 reference golden
test result: ok. 1 passed
```

**All 200 greedy mel codes are byte-identical** to the f32 reference golden. The deterministic GPT-2 AR core is
byte-faithful.

---

## What landed vs scoped

**LANDED (byte-faithful):**
- The GPT-2 AR backbone (24L/1280d/20h) → greedy mel codes, **byte-identical** to the reference golden.
- `nn::Act::GeluNew` shared-lib addition (re-verified: dia2 544/544 codes byte-identical post-change).

**SCOPED (follow-ups, golden staged so each is independently gateable):**
1. **Conditioning front-end** (produces the staged `inputs_embeds`): w2v-bert-2.0 (`Wav2Vec2BertModel`,
   hidden_states[17]) + the WeNet **ConformerEncoder ×2** (6-block spk + 4-block emo, conv2d-subsampling,
   rel-pos MHA, conv module, macaron FFN) + the **PerceiverResampler ×2** (cross-attn-include-queries, GEGLU,
   RMSNorm). All live in `gpt.pth`. The golden stages `spk_cond_emb.npy` (the w2v-bert output) → the port can
   be extended to consume that and reproduce `inputs_embeds.npy` byte-identically, then chain into the gated AR.
2. **Acoustic back-half** (mel codes → wav): the semantic FSQ codec (`vq2emb`) + `s2mel` (gpt_layer +
   length_regulator + the DiT flow-matching `cfm.inference`, 25 steps, cfg 0.7 — reuses the existing
   `cfm::` ODE family) + the **BigVGAN** vocoder (a NEW vocoder vs the existing `cfm::vocoder` HiFT/NSF; would
   be a clean `cfm::` addition). Deterministic (greedy codes → fixed mel → fixed wav), so each is golden-gateable
   on the staged codes.

## Files (absolute; shared flagged)

- **NEW** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/indextts2.rs` — the port (owned).
- **NEW** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_torch_indextts2.rs` — the
  byte-faithful gate (owned).
- **SHARED** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/nn/mlp.rs` — added
  `Act::GeluNew` (additive; dia2 re-verified byte-identical).
- **SHARED** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/lib.rs` — `pub mod indextts2;`
  + doc.
- Golden: `/home/bud/ditto/waav/WaaV/inferv2/REVIEW/indextts2_golden/{codes,inputs_embeds,step0_logits}.npy`,
  `meta.json`, `gpt_keyshapes.json`. Weights: `~/.cache/waav-models/indextts2/indextts2_gpt.safetensors` +
  `waav.json`.
- Throwaway reference scripts (scratchpad, NOT a serving path): `indextts2_golden.py`,
  `extract_indextts2_gpt.py`, `probe_positions.py`.

## Verification

- `cargo test -p waav-infer-backend-torch --lib` → **187 passed**.
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean**.
- dia2 `cpu_fp32_codes_byte_identical` → **544/544 byte-identical** (shared `nn::mlp` re-verified).
- csm re-verify: PENDING (a concurrent Irodori agent's mid-edit `nfkc` compile error in `irodori.rs` blocked the
  `--features cuda` build at report time; csm uses `Act::Silu` and is structurally unaffected by the additive
  `GeluNew` variant — re-run once the concurrent edit settles).

## Perf (RTF)

CPU f32, 200 mel steps ≈ 72 s (the byte-faithful regime; not the perf target). On GB10 CUDA the same f32 path
runs the 24×1280 GPT-2 AR launch-bound at batch-1; the standard `nn::Backbone` CUDA-graph decode path applies
(byte-identical replay) for a real perf number — deferred (the priority was the byte-faithful gate). The full
end-to-end RTF needs the scoped back-half (s2mel-CFM + BigVGAN).
