# Onboarding: kyutai/hibiki-zero-3b-pytorch-bf16 (Moshi-class FULL-DUPLEX S2S translation)

**Date:** 2026-06-23 · **Triage:** HARD · **Status:** 🟡 **SCOPED + architecture fully verified + golden-capture path stood up; PORT NOT YET LANDED** — the precise blocker is an in-flight 6.3 GB weight download (classic-HTTP, xet stalled) + the substantial net-new surface (Mimi **encoder** + native-moshi Mimi weight remap + dual-stream full-duplex interleave). Reuse mapping is exact and the port plan is complete. This mirrors the s2-pro first-pass pattern (scoped → ported).

## TL;DR — what hibiki IS, and why it is HARD

Hibiki-Zero is a **3B Moshi-class full-duplex speech-to-speech (S2S) simultaneous-translation** model (FR/ES/PT/DE → EN), running at a 12.5 Hz frame rate. Verified live via `HfApi` + the reference `moshi`/`kyutai-labs/hibiki-zero` source:

- **Native moshi checkpoint format** (NOT HF-transformers). 3 files: `hibiki-pytorch-…safetensors` (6.3 GB, the `LMModel`), `mimi-pytorch-…safetensors` (385 MB, the Mimi codec **with encoder + decoder + quantizer**), `tokenizer_spm_48k_multi6_2.model` (SentencePiece, `text_card=48000`). `config.json` `model_type: "hibiki"`.
- **Architecture = the Moshi multistream RQ-Transformer** (the same family this codebase already ports as **dia2** = Nari's repackaged-moshi sibling):
  - **Backbone:** 28-layer GQA `StreamingTransformer`, `dim=2048`, `num_heads=16`, `kv_repeat=2` (→ 8 KV heads), `head_dim=128`, `rms_norm_f32`, RoPE `max_period=20000`, `hidden_scale=6`, `gating="silu"`. **Fused QKV** (`self_attn.in_proj_weight`), **no q/k-norm** in the backbone.
  - **Depformer (depth decoder):** 6 layers, `depformer_dim=1024`, `dep_q=16` audio codebooks, `depformer_weights_per_step=true` with a 16-entry `weights_per_step_schedule` (per-step `depformer_in.{g}` / `linears.{cb}` projections), `depformer_kv_repeat=1`.
  - **Delay pattern** `[0,0,2,2,…]` (33 entries: 1 text + 32 audio streams) — the Moshi acoustic-delay interleave.
  - **Mimi codec** (native format): SEANet conv **encoder** + 8-layer encoder transformer + split RVQ (`rvq_first` semantic + `rvq_rest` acoustic, `input_proj`/`output_proj` 1×1 convs) → codes; and the symmetric **decoder** → 24 kHz waveform.
- **Full-duplex mechanism** (the reference loop, `hibiki_zero/inference.py` + `moshi.models.LMGen`):
  ```python
  codes  = mimi.encode(user_audio_frame)        # SOURCE Mimi codes (the always-modeled user stream)
  tokens = lm_gen.step(codes[:, :, c:c+1])       # ONE duplex step: feed user codes → [text_tok, target_audio(dep_q=16)]
  pcm    = mimi.decode(tokens[:, 1:])            # TARGET audio out (continuous), tokens[0,0,0] = target text token
  ```
  i.e. **read user-audio codes while emitting target text + target audio every 80 ms frame** — exactly the `DuplexStepModel::step(&SlotBatch{user_in}) → DuplexStepOutput{model_out}` seam.

**Why HARD (vs a normal codec-AR TTS onboard):** three net-new pieces stacked on the proven seam — (1) a **Mimi ENCODER** (does not exist anywhere in the repo — only the decoder does), (2) the **native-moshi Mimi weight layout** (fused QKV, `rvq_first`/`rvq_rest`, `decoder.model.N.conv.conv`) which differs from the transformers-`MimiModel` layout the shared `codec::MimiDecoder` ports, and (3) the **dual-stream full-duplex interleave** (source-codes-in + target-text/audio-out) wired through the real `DuplexStepModel` seam.

## Architecture verification (HfApi-first + reference source)

| Fact | Source | Value |
|---|---|---|
| repo files / dtype | `HfApi.model_info` | 3 weight files, bf16; `pipeline_tag=audio-to-audio`, `model_type=hibiki` |
| backbone dims | `config.json` | dim 2048 · 28 layers · 16 heads · kv_repeat 2 · max_period 20000 · gating silu · rms_norm_f32 |
| depformer | `config.json` | depformer_dim 1024 · 6 layers · dep_q 16 · weights_per_step + 16-schedule |
| delay pattern | `config.json` `delays` | `[0,0,2,2,…]` (33 ch) |
| Mimi has encoder | `safetensors` keys (inspected the 385 MB file) | `encoder.model.N.conv` + `encoder_transformer` (80 keys) + `quantizer.rvq_first/rvq_rest` (100 keys) — **all present** |
| Mimi native layout | `safetensors` keys | fused `self_attn.in_proj_weight [1536,512]`; `decoder.model.N.conv.conv`; `rvq_first.vq.layers.0._codebook.{embedding_sum,cluster_usage}` |
| LMModel param names | moshi `models/lm.py` | `emb.{cb}` · `text_emb` · `transformer.layers.N` · `out_norm` · `text_linear` · `depformer_in.{g}` · `depformer.layers.N` · `depformer_emb.{cb}` · `linears.{cb}` |
| layer params | moshi `modules/transformer.py` | `self_attn.in_proj_weight` (fused) · `out_proj` · `norm1`/`norm2` · `gating` (SiLU) · `layer_scale_*`=Identity (config layer_scale null) |
| duplex step contract | `hibiki_zero/inference.py` | `lm_gen.step(src_codes[:,:,c]) → tokens[1, 1+dep_q, 1]`; `tokens[:,1:]→decode`, `tokens[0,0,0]=text` |

## Reuse vs new (the LAW: reuse `nn::` / `codec::Mimi` / the real `DuplexStepModel`)

| Component | WaaV reuse | New work |
|---|---|---|
| 28-layer GQA backbone | ✅ `nn::Backbone` + `nn::TransformerLayer` (`Proj::Fused`, GQA via `enable_gqa`, `Mlp::swiglu` SiLU, `RmsNorm` rms_norm_f32, `Rope` `InvFreq` θ=20000), driven exactly as **dia2's `Backbone::step`** | weight-name map (stock-moshi fused `in_proj`, no q/k-norm) — a thin loader variant of dia2's `load_backbone` |
| Depformer (6 layers, dep_q=16, weights_per_step) | ✅ dia2's `Depformer` is the SAME mechanism (`depformer_in[group]`, per-stage `linears`, `WEIGHTS_SCHEDULE`, ring-KV reset per frame) | re-parameterize: 6 layers / 16 stages / hibiki's 16-entry schedule + `depformer_emb`/`depformer_text_emb` |
| Multistream embed (text + 32 audio, delay) | ✅ dia2's `Backbone::embed` (text_emb main/second + Σ audio_embeds) + the host delay buffer + un-delay logic | dual-stream **source+target** variant (hibiki reads source-audio codes into the SAME multistream embed; dia2 only writes target) |
| Mimi **decoder** (codes → 24 kHz) | ✅ `codec::MimiDecoder` (the math is identical) | **weight remap** native→shared layout: fused `in_proj`→split q/k/v; `rvq_first`/`rvq_rest`→`semantic`/`acoustic`; `decoder.model.N.conv.conv`→`decoder.layers.N.conv` |
| Mimi **encoder** (24 kHz → codes) | ⚠️ partial — reuses `codec::{MimiConv, ResBlock, MimiLayer}` primitives | **NET-NEW `codec::MimiEncoder`**: SEANet conv encoder (downsample 2/4/5/6/8) + encoder transformer + RVQ **quantize** (`input_proj` → nearest-codebook → indices). The codebooks/conv primitives exist; the encode *forward* + RVQ *quantize* (vs the existing dequant) are new. |
| Full-duplex step | ✅ the **real `DuplexStepModel` seam** (`waav_infer_core::s2s` + `waav_infer_runtime::duplex` `SlotBatch`/`DuplexStepOutput`/`resolve_turn`/`TurnHead`); `StepOutput::codec_per_codebook` already carries dep_q=16 | a new `s2s::HibikiDuplexModel` impl: `step` = `embed(source_codes ⊕ prev_target) → backbone → text_linear + depformer(16) → DuplexStepOutput{model_out: 16-cb frame}` |
| Sampling (temp 0.8, top_k 250) | ✅ the dia2/csm `sample_token` pattern (libtorch `multinomial`, seedable) | greedy for the golden; sampled for serving |
| Tokenizer | SentencePiece (`tokenizer_spm_48k…model`) | `sentencepiece`-rs (already a dep) — text stream is OUTPUT (translation), not required for the audio S2S turn |

**Net:** ~70 % is a direct re-parameterization of the **dia2** port (same moshi family, same backbone/depformer/delay/Mimi-decoder machinery, already byte-identical on GB10). The genuinely new surface is the **Mimi encoder** (~250 LOC, weights present) + the **native-Mimi weight remap** (~80 LOC) + the **dual-stream full-duplex `DuplexStepModel` glue** (~300 LOC) + the loader.

## What was done this session

1. **HfApi-verified** the repo + arch; pulled `config.json`, README, the 385 MB Mimi safetensors, the SentencePiece tokenizer to `~/.cache/waav-models/hibiki-zero-3b/`. Inspected the Mimi keys → confirmed **encoder + decoder + split-RVQ present, native-moshi layout**.
2. **Reference-source audit** (moshi `lm.py` / `transformer.py` + `hibiki_zero/inference.py`) → captured the EXACT `LMModel` state-dict scheme, the fused-QKV / gating / kv_repeat=2 layer shape, and the full-duplex `mimi.encode → lm_gen.step → mimi.decode` contract.
3. **Mapped the reuse surface exactly** against dia2 (the moshi sibling: 28 layers / dim 2048 / 16 heads / delay-pattern / depformer / Mimi — all already in-tree and byte-identical on GB10), csm, and the **real `CodecArDuplexModel` / `DuplexStepModel` S2S seam** (task #63) + the half-duplex `lfm2_audio` S2S.
4. **Stood up the golden-capture path** (THROWAWAY venv, validation-only per the no-venv rule): `moshi==0.2.13` + `sphn` + `sentencepiece`; the reference sample (`leon.wav`, FR→EN); and `/tmp/capture_hibiki_golden.py` — greedy (temp 0) `mimi.encode → LMGen.step trajectory → mimi.decode` → writes `WaaV/inferv2/REVIEW/hibiki_golden/{source_codes,target_text_tokens,target_audio_frames,target_pcm}.npy + meta.json`. This is the byte-faithfulness target the port verifies against, AND an end-to-end validation of the model.

## The precise blocker (why not yet ported live)

- **Weight acquisition:** the 6.3 GB `hibiki-pytorch` safetensors is the gate. The HF **xet** path **stalled hard** (`read_bytes:0`, blob frozen at 402 MB) under bandwidth contention with a concurrent agent's download; restarted on the **classic-HTTP path** (`HF_HUB_DISABLE_XET=1`) which is progressing but throttled (~2–3 MB/s shared). Until the safetensors lands, neither the live tch load nor the reference golden (which loads the same checkpoint) can run.
- **Port size:** even with ~70 % dia2 reuse, the net-new Mimi **encoder** + native-Mimi **remap** + dual-stream **full-duplex glue** is a multi-stage effort on the scale of the dia2 port (B18→B23→B25, multiple byte-identity scars) plus a brand-new encoder and the first real *translation* S2S — not a single-session onboard. The right call (LAW) is the honest scoped-with-golden deferral, exactly as **s2-pro** was first scoped then ported.

## Port plan (the path to PORTED + byte-faithful, in dependency order)

1. **Acquire** `hibiki-pytorch-…safetensors` → `~/.cache/waav-models/hibiki-zero-3b/` (download in flight).
2. **Capture the golden** (`/tmp/capture_hibiki_golden.py`, ready) once weights + venv land → `hibiki_golden/`. **First live validation of the model.**
3. **`codec::MimiEncoder`** (new) — SEANet conv encoder + encoder transformer (reuse `MimiConv`/`ResBlock`/`MimiLayer`) + RVQ **quantize** (the `input_proj`→argmin-codebook encode, the mirror of the existing `RvqDequant`). Gate: `encode(wav)` codes == golden `source_codes.npy` (byte-identical, deterministic).
4. **Native-Mimi decoder remap** — load the 385 MB native file into `codec::MimiDecoder` via a name/shape adapter (fused→split QKV, `rvq_first/rest`→`semantic/acoustic`). Gate: `decode(golden codes)` corr ≥ 0.99 vs golden `target_pcm`.
5. **`hibiki.rs`** (new, the dia2 twin) — `Backbone` (28-layer, fused-QKV, no q/k-norm loader) + `Depformer` (6-layer, 16-stage, weights_per_step) + the multistream **dual-stream** embed/delay. Gate: greedy backbone prefill hidden Δ==0 / frame-0 `target_audio_frames` byte-identical vs golden (the dia2 byte-identity discipline + TF32/bf16 scars).
6. **`s2s::HibikiDuplexModel : DuplexStepModel`** — `step(&SlotBatch)`: `user_in` = source Mimi codes → multistream embed → backbone → `text_linear` + depformer(16) → `DuplexStepOutput{ model_out: codec_per_codebook(16), eot via TurnHead }`. Wire `n_codebooks()=16`. **Smoke:** a real speech-in → speech-out duplex turn through the engine (`leon.wav` FR → EN audio), reusing the `full_duplex_bench`/`CodecArDuplexModel` harness.
7. **Register** arch `hibiki` (a tch-backed loader, sibling of dia2) + `lib.rs` `pub mod hibiki`. **RTF** on GB10 (reference claims 3× real-time batched on one H100).
8. `cargo test -p waav-infer-backend-torch --lib` green + clippy clean; re-verify dia2/csm unchanged (shared `codec::`/`nn::` touches additive only).

## Acceptance (the LAW, to be met on PORTED)

- **A real S2S turn:** `leon.wav` (FR) → EN target audio + timestamped EN text, through the `DuplexStepModel` engine seam.
- **Accuracy:** byte-faithful where deterministic — `MimiEncoder` codes Δ==0 vs golden, backbone frame-0 target codes byte-identical (greedy), Mimi decode corr ≥ 0.99. (The sampled trajectory tracks the reference's own bf16 floor, as documented for dia2/csm/s2-pro.)
- **RTF** measured on GB10.

## Files (planned) / artifacts (this session)

- **Acquired:** `~/.cache/waav-models/hibiki-zero-3b/{config.json, mimi-…safetensors, tokenizer_spm…model, README.md}` (+ the 6.3 GB main safetensors, in flight).
- **Golden harness (ready):** `/tmp/capture_hibiki_golden.py`, `/tmp/hibiki_sample_leon.wav`, throwaway `/tmp/hibiki_golden_venv` (moshi 0.2.13).
- **Planned NEW:** `crates/waav-infer-backend-torch/src/codec/mimi_encoder.rs` (or `codec/mimi.rs` extension), `crates/waav-infer-backend-torch/src/hibiki.rs`, `crates/waav-infer-core/src/s2s/hibiki_duplex.rs`.
- **Planned SHARED (additive) touches:** `codec/mod.rs` (`MimiEncoder` export + native-layout adapter), `nn/` (none expected — primitives suffice), `lib.rs` (`pub mod hibiki`), `waav-infer-server/src/engine.rs` (register `hibiki`), `waav-infer-core/src/s2s/mod.rs` (`pub mod hibiki_duplex`).
- **Golden (planned):** `WaaV/inferv2/REVIEW/hibiki_golden/{source_codes, target_text_tokens, target_audio_frames, target_pcm}.npy + meta.json`.

## Verdict

**SCOPED, with the architecture fully verified, the reuse mapping exact, the golden-capture path stood up, and a complete dependency-ordered port plan.** The model is a Moshi-family full-duplex S2S translator — ~70 % a re-parameterization of the already-byte-identical **dia2** port, plus three genuinely net-new pieces (Mimi **encoder**, native-Mimi weight remap, dual-stream full-duplex `DuplexStepModel` glue). The single hard blocker to landing it live this session is the in-flight 6.3 GB weight download (xet stalled → classic-HTTP, throttled); the port itself is a multi-stage effort on the dia2 scale. This is the s2-pro pattern (scoped now, ported next).
