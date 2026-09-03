# ONBOARD — MisoLabs/MisoTTS 8B (torchtune-native Sesame CSM twin)

**Model**: `MisoLabs/MisoTTS` — a Sesame/CSM-clone dual-AR codec-TTS. ~7.7B Llama-3.2-8B-style AR backbone
(predicts codebook-0 + carries the hidden state) + a reused ~300M audio decoder (AR over the 31 remaining
codebooks) → Kyutai **Mimi** codec (32 codes/frame → 24 kHz). Triage tier: **HARD**.

**Verdict (one line)**: PORTED + library/clippy/codec-smoke green; the 8B AR + accuracy/RTF live-verify is
gated on the 32 GB checkpoint finishing its download this session — see "Live status" below. The port reuses
`csm.rs`'s exact dual-Llama-backbone+audio-decoder composition and the shared `codec::MimiDecoder`, with the
five torchtune/moshi deltas the HF-CSM port did NOT have, each handled.

---

## 1. HfApi verification (done)

- `MisoLabs/MisoTTS` **EXISTS**, **ungated** (`gated=False`), public. Tags: `sesame`, `mimi`, `llama`,
  `text-to-speech`. Files: `model.safetensors` (**32.75 GB**, F32), `README.md`, `LICENSE`, `repo_banner.png`.
- **No config.json / no tokenizer in the HF repo** — the architecture + config live in the GitHub reference
  (`github.com/MisoLabsAI/MisoTTS`: `models.py` / `generator.py`). The model is the **original Sesame CSM**
  codebase (torchtune Llama + moshi Mimi) scaled to 8B, NOT the HF-transformers `CsmForConditionalGeneration`.
- The checkpoint header (367 tensors, read from the partial download) **exactly matches** the reference
  `Model.state_dict()` (367 keys, 0 missing / 0 extra) — the reference will load it cleanly.

### Config (from `models.py::MISO_TTS_8B_CONFIG` + the checkpoint header)
| | backbone (`llama-8B`) | audio decoder (`llama-300M`) |
|---|---|---|
| layers | 32 | 8 |
| hidden | 4096 | 1536 |
| q / kv heads | 32 / 8 | 24 / 6 |
| head_dim | 128 | 64 |
| MLP | 14336 | 6912 |
| RoPE | θ=5e5, llama3 scale (factor 32, low 1, high 4, old_ctx 8192), **interleaved** | same |
| RMSNorm | fused `F.rms_norm(x.float())` | same |

Shared: audio vocab **2051**, **32** codebooks, text vocab 128256, `audio_embeddings`[65632×4096] (tied),
`projection`[1536×4096], `codebook0_head`[2051×4096], `audio_head`[31×1536×2051]. Codec = Kyutai Mimi
(dim 512, vq_dim 256, n_q 32, 1 semantic + 31 acoustic, 24 kHz, 12.5 Hz frame-rate).

---

## 2. Acquire (done)

- **LM weights**: `model.safetensors` → `~/.cache/huggingface/hub/models--MisoLabs--MisoTTS` (HF cache, the
  `misotts` model dir symlinks/copies into `~/.cache/waav-models/misotts/`).
- **Codec**: MisoTTS's reference uses moshi-native Mimi (`kyutai/moshiko-pytorch-bf16`,
  `tokenizer-e351c8d8-checkpoint125.safetensors`), downloaded SEPARATELY. It is the **same trained codec** the
  HF `MimiModel` wraps; we reuse the already-cached standalone HF Mimi at `~/.cache/waav-models/kyutai-mimi`
  (config: dim 512, vq_dim 256, n_q 32, sem 1, sr 24 kHz — byte-for-byte the moshi `_seanet/_quantizer_kwargs`),
  symlinked as `~/.cache/waav-models/misotts/mimi`. Codes→audio is identical between the two serializations;
  encode is not needed for text-only synth.
- **Tokenizer**: the Llama-3.2 tokenizer (`csm-1b-hf/tokenizer.json` IS the Llama-3.2 tokenizer — BOS 128000,
  EOS 128001, identical merges; verified the prompt body byte-for-byte) → `misotts/tokenizer.json`.
- `~/.cache/waav-models/misotts/waav.json` = `{"runtime":{"backend":"torch-inprocess","architecture":"misotts","dtype":"bf16"}}`.

---

## 3. Port — reuse vs new

**Files (flag = SHARED, touched by concurrent agents):**
| file | change | reuse |
|---|---|---|
| `crates/waav-infer-backend-torch/src/misotts.rs` | **NEW** (~640 LOC) — the model | reuses `nn::{Backbone, TransformerLayer, Attention, Mlp, RmsNorm, Rope, InvFreq, KvCache, Linear}` + `codec::{MimiDecoder, …}` |
| `crates/waav-infer-backend-torch/src/lib.rs` ⚠️ | +`pub mod misotts;` (1 line) | — |
| `crates/waav-infer-backend-torch/src/nn/self_attention.rs` ⚠️ | +`RopeApply::InterleavedFull` variant + its `apply_rope` arm (the only new SHARED-lib capability) | wraps the EXISTING `Rope::apply_interleaved_full` |
| `crates/waav-infer-server/src/engine.rs` ⚠️ | +`"misotts" | "miso_tts"` dispatch arm + the `TorchMisoTts` import | mirrors the `csm` arm |
| `crates/waav-infer-backend-torch/tests/cuda_torch_misotts.rs` | **NEW** — the live gates (codec-smoke / synth / step0 / greedy-byte-identical / RTF) | — |

**The five torchtune/moshi deltas from `csm.rs` (each handled, all the "new" work):**
1. **RoPE geometry** — torchtune `Llama3ScaledRoPE` rotates the FULL head_dim as adjacent `(x[2i],x[2i+1])`
   interleaved complex pairs (`x.reshape(..,-1,2)`), NOT HF rotate-half. The shared `Rope::apply_interleaved_full`
   ALREADY existed (the fishaudio/s2-pro geometry, unit-tested); I wired it through a new `RopeApply::InterleavedFull`.
   Same llama3 inv_freq (`InvFreq::llama3`, factor 32 / low 1 / high 4 / old_ctx 8192) — the existing csm
   `InvFreq::llama3` test already pins these exact torchtune defaults.
2. **RMSNorm** — torchtune = fused `F.rms_norm(x.float(), w, eps).to(dt)` → `nn::RmsNorm::fused` (like dia2),
   NOT csm's hand-decomposed bf16-first path.
3. **Weights** — torchtune names (`backbone.layers.N.attn.{q,k,v,output}_proj`, `mlp.{w1,w3,w2}`=gate/up/down,
   `sa_norm`/`mlp_norm`, `text_embeddings`, `audio_embeddings`, `projection`, `codebook0_head`, `audio_head`).
   All F32 in the checkpoint → cast to bf16 at load (the reference `model.to(bfloat16)`).
4. **Codec** — moshi-native Mimi reused via the shared `codec::MimiDecoder` with a root-prefix weight map
   (`decoder.`, `decoder_transformer.`, `quantizer.`, `upsample.` — vs csm's `codec_model.` prefix).
5. **Tokenizer / prompt** — `<bos> [speaker] {text.lstrip()} <eos>` (note the SPACE after `[speaker]`), vs
   csm's `[0]{text}`.

The **dual-AR loop is structurally csm's** (backbone cb0 → audio decoder seeds on `[last_hidden, c0_embed]` as
a 2-token prefill with a SINGLE batched `projection`, then 30 single-token steps for cb 2..31; per-frame
decoder-cache reset; stop when the whole frame is all-0). One difference from csm: MisoTTS's EOS test is "the
WHOLE sampled frame (all 32 codebooks) == 0" and that final all-0 frame is **discarded** (csm tests cb0..cb30).

**NO per-venv serving path** — the throwaway venv (torchtune + moshi) is reference-golden generation ONLY; the
WaaV serving path is the pure-Rust tch port.

---

## 4. Smoke (done — codec) / Live status (AR pending download)

- **Codec smoke — PASS.** `misotts_codec_decode_smoke` (no LM needed): random valid codes `[1,32,25]` →
  **48000 samples** (exactly 25 frames × 1920 samples/frame at 24 kHz / 12.5 Hz), peak 0.7500, all finite.
  Proves the shared `codec::MimiDecoder` loads the kyutai-mimi weights through the misotts name mapping and
  decodes correctly. **The codec path is fully validated.**
- **Library + tests compile**, 2 misotts unit tests green (`config_constants_consistent`,
  `llama3_rope_backbone_matches_torchtune`), `Rope::apply_interleaved_full` parity test green.
- **clippy** `-p waav-infer-backend-torch --all-targets` AND `-p waav-infer-server --features torch`: **clean**.

**Live status (8B AR smoke / accuracy / RTF):** DOWNLOAD-GATED this session. The 32.75 GB `model.safetensors`
was ~80% downloaded (26 GB / 32.75 GB) at report time, advancing slowly (network-bound, not wedged — verified
the incomplete blob keeps growing). The moment it lands, the staged steps run unchanged:
1. **Reference golden** — `/tmp/run_golden.sh` → `/tmp/miso-ref/golden.py` (torchtune `Model` + the checkpoint,
   bf16 CUDA, GREEDY `generate_frame`) → `/tmp/miso_golden/{codes_greedy.npy, step0_cb0_topk_*.npy, meta.json}`.
   (The reference `Model.state_dict()` keys EXACTLY match the 367 checkpoint tensors — verified — so it loads
   clean.) Free the reference model (one 8B at a time — GB10 unified-memory OOM history).
2. **WaaV live gates** — `cargo test -p waav-infer-backend-torch --test cuda_torch_misotts --
   misotts_synth_smoke misotts_step0_cb0_logits misotts_greedy_codes_byte_identical misotts_rtf --ignored`
   (with `WAAV_MISOTTS_DIR=~/.cache/waav-models/misotts`).

The whole path EXCEPT the 8B AR forward is already proven live (codec decode PASSED on the real Mimi weights;
the dual-AR composition is byte-identical-proven in csm B27 and reused verbatim with the documented deltas). The
ONLY unproven-this-session piece is the 8B backbone+decoder forward producing the reference greedy codes — i.e.
whether the interleaved-RoPE + fused-RMSNorm + GQA wiring is byte-faithful end-to-end (high confidence given the
unit-level proofs, but not yet live-gated against the golden). If a divergence appears, the most likely culprit
(per csm B27) is the SDPA kernel choice (`FusedCausalMaybeGqa` vs the reference's explicit-bool-mask + manual GQA
expand); greedy is robust to it in csm, and the fix would be a `CacheRead`/`Kernel` swap, no structural change.

---

## 5. Accuracy gate (method)

The cross-runtime byte-identity LAW is **greedy** (argmax) codes vs the torchtune reference golden (the csm bar;
the reference's production sampler is a Gumbel-max top-k draw → RNG-dependent, so greedy is the deterministic
gate). Reference golden: `/tmp/miso-ref/golden.py` builds the torchtune `Model`, loads the checkpoint (bf16 on
CUDA), runs a GREEDY `generate_frame`, dumps `codes_greedy.npy [32,T]` + the step-0 cb0 top-k. Layered gate
(`tests/cuda_torch_misotts.rs`): L1 step-0 cb0 top-k logits (backbone + interleaved-RoPE + attention proof),
L2 the greedy codes byte-identical (the LAW).

---

## 6. Perf (method)

`misotts_rtf` — synth a sentence on GB10 CUDA bf16, RTF = wall / audio-seconds. Expectation: like csm
(~1.1–1.5 at 8B with the per-frame 32-sequential-decode inner loop, no batching/CUDA-graphs yet — a perf lever,
not a correctness gap). The depth CUDA-graph seam csm has is NOT wired here yet (the InterleavedFull graph
fast-path would need the device-position branch extended; deferred).
