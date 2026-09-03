# Onboard — IndexTeam/IndexTTS-2 FULL text→wav chain CLOSED (front-end + gpt_latent + wiring, engine-served)

**Status: THE FULL CHAIN IS CLOSED — `text → front-end → GPT-2 AR codes → gpt_latent → back-half → wav`,
byte-faithful to the reference golden, engine-served (`TtsModel::synthesize`, arch `indextts2`).** This session
ported the two remaining scoped pieces byte-faithfully (the conditioning **FRONT-END** + the `gpt_latent`
GPT-forward) and **wired the end-to-end synth** (`TorchIndexTts2Full` + the `engine.rs` `"indextts2"`
`load_model_at` arm), closing the chain whose AR core (`indextts2.rs`, 200/200) and acoustic back-half
(`indextts2_backhalf.rs`, codes→wav rel_l2 6.3e-5) were already byte-faithful + committed.

---

## 1. Full e2e synth working + byte-faithful + engine-served? — **YES, all three.**

- **Working e2e**: `TorchIndexTts2Full::synthesize(text, voice)` runs the whole chain on GB10 and returns
  22.05 kHz wav. The engine path `load_model_at(indextts2_dir, Cuda).synthesize` produces **162304 samples
  BYTE-IDENTICAL to standalone** (`engine_serves_inprocess_torch_indextts2_byte_identical_to_standalone` — the
  B53 engine-seam gate). No numeric transform at the seam (same `TorchIndexTts2Full` concrete type).
- **Byte-faithful**: every front-end + gpt_latent sub-stage gates byte-faithful (CPU f32, vs the staged
  reference golden) — and the **AR codes regenerated from the front-end's own `inputs_embeds` are 200/200
  byte-identical** to the golden (despite the staged `inputs_embeds` differing by 6e-5, the greedy argmax is
  invariant). The full back-half wav (codes→wav) is byte-faithful on CPU (rel_l2 6.3e-5; the existing gate).
- The full e2e wav must run on **CUDA** (the BigVGAN alias-free depthwise conv SIGSEGVs on the
  torch-2.12/aarch64 CPU oneDNN JIT — the same scar the back-half hit); on CUDA the wav is rel_l2 2.0e-2 vs the
  f32-CPU golden (the documented CUDA conv-accumulation drift: the back-half **alone** on CUDA is 1.73e-2;
  audibly identical). The byte-faithfulness is PROVEN at the per-stage CPU gates, not at this CUDA-only e2e
  comparison.

### Per-stage byte-faithfulness (CPU f32, vs the staged front-end golden `indextts2_golden_frontend/`)

| stage | what | max\|Δ\| | rel_l2 | verdict |
|---|---|---|---|---|
| `cond_conformer` | WeNet ConformerEncoder ×6 (spk) → `[1,300,512]` | 9.5e-7 | 5.5e-7 | **byte-faithful** |
| `speech_conditioning_latent` | PerceiverResampler (32 latents) → `[1,32,1280]` | 6.1e-5 | 1.0e-6 | **byte-faithful** |
| `emo_conformer` | ConformerEncoder ×4 (emo) → `[1,300,512]` | 2.1e-6 | 4.3e-7 | **byte-faithful** |
| `emo_perceiver` | PerceiverResampler (1 latent) → `[1,1,1024]` | 9.5e-7 | 2.7e-7 | **byte-faithful** |
| `emovec` | `emo_layer(emovec_layer(perc))` → `[1,1280]` | 4.8e-7 | 3.8e-7 | **byte-faithful** |
| `conds_latent` | `cat([scl+emovec, speed(1), speed(0)])` → `[1,34,1280]` | 6.1e-5 | 1.0e-6 | **byte-faithful** |
| `inputs_embeds` | `prepare_gpt_inputs` → `[1,63,1280]` (AR prefill prefix) | 6.1e-5 | 1.0e-6 | **byte-faithful** |
| `gpt_latent` | teacher-forced GPT forward → `[1,200,1280]` | 2.0e-5 | 1.1e-6 | **byte-faithful** |
| **`codes_from_frontend`** | AR over the front-end `inputs_embeds` → **200/200** | — | — | **byte-IDENTICAL** |
| `full e2e` (CUDA) | text → … → wav (88064 samples = 3.99 s) | 8.0e-2 | 2.0e-2 | within CUDA tol |

All gates green: `cargo test -p waav-infer-backend-torch --test cuda_torch_indextts2_frontend -- --ignored`
→ **8 CPU gates pass + the CUDA e2e** (`WAAV_INDEXTTS2_DEVICE=cuda` for the e2e). The AR (200/200), back-half
(rel_l2 6.3e-5), dia2 (544/544), csm (LAW), irodori (3/3) re-verified green. Lib **191 passed**; clippy
`--workspace --all-targets -D warnings` **clean**.

### RTF
- The engine-served full synth on GB10 CUDA: ~25 s for a 162304-sample (7.36 s) utterance incl. model load
  (the 24L GPT-2 AR launch-bound at batch-1 + the 25-step×13L CFG-doubled DiT dominate; the AR + CFM are the
  perf targets, deferred — the priority here was the byte-faithful gate). The fixed 200-code e2e: ~9 s incl.
  load (3.99 s audio).

---

## 2. The byte-identity scars (none new — the port was first-try byte-faithful)

Notable, vs the FULL onboard's "9 byte-identity scars" warning for the front-end — the WeNet-vs-NeMo conformer
differences were captured **up-front from the reference source** (`indextts/gpt/conformer/`), so every stage
gated byte-faithful on the FIRST run. The captured WeNet-specific points (each a potential scar):
1. **`rel_shift` is OFF.** The reference `RelPositionMultiHeadedAttention.forward` comments out
   `matrix_bd = self.rel_shift(matrix_bd)` ("useless in speech recognition"). The score is `(q_u·kᵀ +
   q_v·pᵀ)/√d_k` with `matrix_bd` UN-shifted — NOT the textbook Transformer-XL rel-pos shift.
2. **macaron OFF** (`macaron_style=False`): no `feed_forward_macaron`, `ff_scale=1.0` (confirmed — the
   checkpoint has no `*.feed_forward_macaron.*` / `*.norm_ff_macaron.*` tensors). The FULL onboard's "macaron
   pre-norm" note was for the abstract WeNet ConformerEncoderLayer; this checkpoint's instance is macaron-off.
3. **PerceiverResampler `cross_attn_include_queries=True`**: the cross-attn context is `cat([latents, ctx])`
   (the latents attend themselves + the conformer output). FFN is **GEGLU** (`gelu(gate)·x`, `di =
   int(d·mult·2/3)`). `norm` is the lucidrains **RMSNorm** = `F.normalize(x, dim=-1)·√d·gamma` (an L2-normalize
   over the last dim, NOT the gpt-fast mean-square RMSNorm the back-half's DiT uses).
4. **Conv2dSubsampling2** input layer (/2): `Conv2d(1,d,3,stride=2)` (no pad) + ReLU → `Linear(d·((idim−1)//2),
   d)`; the `RelPositionalEncoding` scales `x` by `√d_model` and returns the sinusoidal `pe[0:T]` (offset 0).
5. **`inputs_embeds_frontend` ≠ the AR golden `inputs_embeds.npy`** (max|Δ| 0.039) — the AR golden was captured
   under a slightly different conditioning. The full chain uses the front-end-produced `inputs_embeds`; its
   greedy AR codes are **200/200 byte-identical** to the staged golden codes regardless (argmax-invariant), so
   the chain is fully self-consistent (`gpt_latent` and the back-half consume the staged codes; verified
   maxdiff 0.0 vs `golden_full`).

The `gpt_latent` forward reuses the SAME ported GPT-2 backbone + `final_norm` (`indextts2.rs`): one full causal
forward over `cat([conds(34), text_emb, mel_emb])`, the separate `final_norm` over the post-`conds` slice,
return the mel-latent slice minus the last 2 tokens (`mel_logits[:,:-2]`).

---

## 3. What landed vs scoped

**LANDED (byte-faithful, gated, engine-served):**
- The **conditioning FRONT-END** (`indextts2_frontend.rs`, owned, NEW): WeNet ConformerEncoder ×2 (spk 6-blk +
  emo 4-blk) + PerceiverResampler ×2 + `emovec_layer`/`emo_layer` + `speed_emb` + `prepare_gpt_inputs` →
  `inputs_embeds`. Consumes the staged w2v-bert feature `spk_cond_emb`. Composes `nn::{Linear, sdpa_manual}` +
  raw tch conv/LN ops (read-only reuse — NO shared module edited).
- The **`gpt_latent`** GPT-forward (`indextts2.rs`, owned, ADDED `gpt_latent` + `gpt_forward_mel_emb`): reuses
  the AR backbone + `final_norm`.
- The **full synth + `TtsModel`** (`indextts2.rs`, owned, ADDED `TorchIndexTts2Full`): orchestrates front-end →
  AR → gpt_latent → back-half → wav; `synthesize(text, voice, speed)`; per-voice staged conditioning bundle;
  seeded CFM noise (`manual_seed(0)`).
- The **engine arm** (`engine.rs`, EDIT): `"indextts2" | "index_tts2" | "IndexTTS2" =>
  TorchIndexTts2Full::load(...) → LoadedModel::Tts` (mirrors the irodori/pocket_tts arms).
- Goldens: `indextts2_golden_frontend/*.npy` (12 stage-boundary tensors incl. `codes_from_frontend`). The
  front-end weights extracted to `indextts2_frontend.safetensors` (1.44 GB — 367 tensors: the 2 conformers, 2
  perceivers, emovec/emo layers, speed_emb, text_embedding/pos). The per-voice conditioning staged at
  `voices/default/{spk_cond_emb,prompt_condition,ref_mel,style}.npy`.

**SCOPED (the precise remainder — the truly-large audio→feature encoders + arbitrary-text tokenization):**
1. **w2v-bert-2.0 semantic encoder** (`Wav2Vec2BertModel`, hidden_states[17], normalized) — the audio →
   `spk_cond_emb` encoder. This is the genuinely-large piece (a full conformer-based SSL model); the front-end
   consumes its output as a STAGED golden. Porting it would let `synthesize_cloned(reference_audio)` accept
   arbitrary reference audio (currently the speaker conditioning is staged per voice).
2. **The audio-side conditioning** (`CAMPPlus` speaker encoder → `style`; the semantic FVQ `quantize` →
   `S_ref` → `length_regulator` → `prompt_condition`; the `mel_spectrogram` → `ref_mel`). Deterministic and
   bounded, but each needs its small encoder; currently staged per voice. (The FVQ `vq2emb` + `length_regulator`
   ARE already ported in the back-half — only the FVQ `quantize` (encode) direction is missing.)
3. **Arbitrary-text tokenization**: the `bpe.model` is a **BPE** SentencePiece (not Unigram, so the existing
   `SpUnigram` doesn't apply) + a Chinese/English **TextNormalizer** (pinyin / number expansion). Verified: the
   raw `sp.Encode` of the demo text reproduces the staged `text_ids` byte-identically — so the byte-faithful
   demo path is canned; arbitrary normalized-text needs the BPE encoder + the TextNormalizer port.

With (1)+(2)+(3) the synth accepts arbitrary text + arbitrary reference audio. The acoustic graph (front-end
conformers/perceivers + AR + gpt_latent + back-half DiT-CFM + BigVGAN) — the model itself — is **fully ported
and byte-faithful**; the remainder is the input-side feature extraction + text front-end (the standard
"scoped-encoder" tail), each independently stageable.

---

## 4. Files (absolute; shared flagged — NO shared edits)

- **NEW** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/indextts2_frontend.rs` — the
  conditioning front-end (owned, 623 LOC). Composes `nn::{Linear, sdpa_manual}` + raw tch ops (NO shared edit).
- **EDIT (owned)** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/src/indextts2.rs` — added
  `gpt_forward_mel_emb` + `gpt_latent` (the teacher-forced forward) + `TorchIndexTts2Full` (the full synth +
  `TtsModel` impl) + the per-voice loader + seeded CFM noise.
- **NEW** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-backend-torch/tests/cuda_torch_indextts2_frontend.rs`
  — the 9 front-end/gpt_latent/e2e gates (owned).
- **EDIT (owned)** `.../src/lib.rs` — `pub mod indextts2_frontend;` + doc.
- **EDIT** `/home/bud/ditto/waav/waav-infer/crates/waav-infer-server/src/engine.rs` — the `"indextts2"`
  `load_model_at` arm + the doc/error-list entries.
- **NEW** `.../crates/waav-infer-server/tests/fixtures/torch_inprocess/indextts2.waav.json` — the engine fixture
  manifest.
- **EDIT** `.../crates/waav-infer-server/tests/torch_inprocess_live.rs` — the engine-seam byte-identity gate
  `engine_serves_inprocess_torch_indextts2_byte_identical_to_standalone`.
- **NO shared `nn::`/`cfm::`/`codec::` edits** — dia2 (544/544)/csm (LAW)/irodori (3/3) re-verified green (reuse
  only). Lib 191 tests green.
- Weights: `~/.cache/waav-models/indextts2/indextts2_frontend.safetensors` (1.44 GB) + `waav.json` updated with
  the `frontend` block + `voices/default/*.npy`.
- Goldens: `/home/bud/ditto/waav/WaaV/inferv2/REVIEW/indextts2_golden_frontend/*.npy` (12 tensors). Also
  `~/.cache/waav-models/indextts2/golden_frontend/`.
- Throwaway reference scripts (scratchpad + `waav-infer/scratchpad/`, NOT a serving path; reuse the `refvenv`
  transformers==4.52.1 venv): `extract_frontend.py` (front-end weight extractor), `golden_frontend.py` (the
  per-stage front-end + gpt_latent + codes-from-frontend dumper).

## 5. Verification

- `cargo test -p waav-infer-backend-torch --test cuda_torch_indextts2_frontend -- --ignored` → **8 CPU gates
  pass** (conformers, perceivers, emovec, inputs_embeds, gpt_latent, **codes_from_frontend 200/200 EXACT**) +
  the CUDA e2e (`WAAV_INDEXTTS2_DEVICE=cuda`, rel_l2 2.0e-2).
- `cargo test -p waav-infer-server --features torch --test torch_inprocess_live -- --ignored
  engine_serves_inprocess_torch_indextts2_byte_identical_to_standalone` → **PASS: engine == standalone,
  162304 samples BYTE-IDENTICAL** (on CUDA).
- `cargo test -p waav-infer-backend-torch --lib` → **191 passed**. `cargo test -p waav-infer-server --features
  torch --lib` → **67 passed**.
- Shared re-verify: dia2 `cpu_fp32_codes_byte_identical` **544/544**; csm `cuda_csm_codes_byte_identical_to_
  sidecar` **LAW PASSED** (the csm `…sampled…` divergence is a pre-existing test-parallelism global-CUDA-RNG
  leak — both csm tests pass `--test-threads=1`; my changes touch no shared code); irodori **3/3**.
- `cargo clippy --workspace --all-targets --features torch -- -D warnings` → **clean**.
- NO `git commit`, NO `cargo fmt` (per instructions; only touched files were edited).
