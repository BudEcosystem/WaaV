# ONBOARD — Zyphra/Zonos2 (HARD-tier: MoE codec-AR TTS) — BYTE-FAITHFUL ✓

**Model**: `Zyphra/Zonos2` — Zyphra's latest TTS, a **Mixture-of-Experts (MoE) codec-AR transformer** that
predicts 9 parallel DAC audio codebooks/step from UTF-8 byte text tokens + an optional ECAPA-TDNN speaker
embedding, decoded to 44.1 kHz by the **descript/dac_44khz** codec. Triage tier: **HARD** (TRIAGE_DISPOSITION
§F "new codec-AR TTS stack").

**Verdict (one line)**: **PORTED + BYTE-FAITHFUL** — a REAL Zonos2 synth on GB10, greedy codes **288/288
byte-identical** to the reference golden over 32 frames (f32 LAW), full AR→DAC pipeline live (audible audio),
library 187/187 + clippy `--all-targets -D warnings` clean, engine-wired. The port reimplements the MoE+EDA-router
backbone in `zonos2.rs` reusing the shared `nn::` primitives + the **descript/dac_44khz `codec::dac::DacDecoder`
reused VERBATIM** (the same codec `dia` serves). One real bug found + fixed via f32-CPU bisection (a non-causal
attention mask). No shared `nn::`/`codec::` math touched → dia2/csm byte-identity preserved (187 lib tests green).

---

## 1. HfApi verification (done — the disposition's arch guess refined)

- `Zyphra/Zonos2` (+ alias `Zyphra/ZONOS2`) **EXISTS**, **ungated** (`gated=False`), **apache-2.0**. Files:
  `model.pth` (**15.34 GB**, a torch pickle — NOT safetensors), `params.json`, `README.md`, assets.
- This is the **v2** model, distinct from the dense `Zyphra/Zonos-v0.1-{transformer,hybrid}` family (those ship
  `config.json` + `model.safetensors`). The disposition's "transformer/hybrid (some variants SSM/Mamba)" note
  applied to **v0.1**; **Zonos2 is a dense-attention MoE** — NO SSM/Mamba (confirmed from `params.json` + the
  GitHub reference `github.com/Zyphra/ZONOS2`, package `zonos2`).
- The reference serving stack is **Mini-SGLang** + flashinfer + sgl_kernel + a Triton fused-MoE — **none run on
  GB10** (Blackwell aarch64 sm_12x; flashinfer is a hard no). So the golden is a **standalone eager-PyTorch
  reference** (faithful re-expression of `models/zonos2.py`, no flashinfer/sgl/triton) — reference-ONLY, never a
  serving path [[waav-infer-no-venv-wrap]].

### Architecture (params.json + `models/config.py` derivation — all VERIFIED vs the checkpoint header)
28 layers · hidden 2048 · GQA 16q/4kv × head_dim 128 · dense/MoE intermediate 3072 · RoPE θ=10000 **interleaved**
(flashinfer `is_neox=False`) · RMSNorm eps 1e-5 (QK-norm eps 1e-6) · 9 codebooks · audio_vocab 1026 · text_vocab
519 · **softcap 15.0** · **MoE**: sonic, **16 experts**, router_dim 128, **top-1** (layer 26 **top-2**), **EDA
router**, MoE on layers **3..=26** (dense on 0..2 + 27) · speaker: ECAPA-TDNN→LDA 2048→1024→proj 1024→2048 ·
codec: **descript/dac_44khz** (9 cb, upsampling [8,8,4,2], hop 512) — IDENTICAL to dia's.

Non-standard ops (each ported + verified): **MoE feed-forward** (EDA router: down_proj → +router_states·scale →
rmsnorm_eda → GELU·GELU·linear → f32-softmax → balanced top-k `scores = prob − bias`; SwiGLU experts
`down(silu(w1·x)·(w3·x))` from the sonic `w13` de-interleave); **attention** `wq`/fused `wkv`/`wo` with
**QK-RMSNorm** (eps 1e-6, no affine) × learnable per-head `temp.abs()` on q + **headwise sigmoid gating**
(`o *= sigmoid(gater(x))`); **fused-residual RMSNorm**; `multi_embedder` SUMs 9 audio + 1 text; `multi_output`
ONE linear → `[9,1026]` then **softcap**.

---

## 2. Acquire (done)

- **Weights**: `model.pth` (15.34 GB) → `~/.cache/waav-models/zonos2/` (Xet transfer, ~30 min).
- **Convert** (`scratchpad/convert_zonos2.py`): `model.pth` → `model.safetensors`, normalizing the keys per the
  reference `weight.py` (`.parametrizations.X.original`→`.X`; drop `.router.ent_denom`/`.normalized_entropy` —
  this checkpoint had 0 of each, already clean). 507 tensors. Dumped `keys.txt` → **all 507 names + shapes
  cross-checked against the port** (wkv `[2,512,2048]`, experts.w13 `[16,6144,2048]`, w2 `[16,2048,3072]`,
  temp `[1,16,1]`, gater `[16,2048]`, multi_output `[9234,2048]` — 0 unexpected, 0 missing).
- **Codec**: NO new acquisition — `~/.cache/huggingface/hub/models--descript--dac_44khz` is **already cached**
  (dia uses it). Zonos2's reference `dac` 44 kHz model == the HF `descript/dac_44khz` re-serialization (dia's
  byte-faithfulness proves the math is identical).
- `~/.cache/waav-models/zonos2/waav.json` = `{"runtime":{"backend":"torch-inprocess","architecture":"zonos2",
  "dtype":"bfloat16"},"task":"tts"}`.

---

## 3. Port — reuse vs new

**Files (⚠ = SHARED, also touched by the concurrent S2S agent — edited with re-read/retry):**
| file | change | reuse |
|---|---|---|
| `crates/waav-infer-backend-torch/src/zonos2.rs` | **NEW** (~960 LOC incl. bisection probes) — the model | reuses `nn::{Linear, RmsNorm, rms_norm_decomposed, Rope, InvFreq, Square}` + **`codec::dac::DacDecoder` (the dia DAC, VERBATIM mapping)** |
| `crates/waav-infer-backend-torch/src/lib.rs` ⚠ | +`pub mod zonos2;` (1 line) | — |
| `crates/waav-infer-server/src/engine.rs` ⚠ | +`"zonos2"\|"ZONOS2"` dispatch arm + `TorchZonos2` import + err-list entry (mirrors the `dia` arm) | — |
| `crates/waav-infer-backend-torch/tests/cuda_torch_zonos2.rs` | **NEW** — the live gates (codec smoke / synth / step0 codes / byte-identical LAW / RTF + the f32-CPU bisection probes) | — |

**Reuse maximized — what is SHARED (NO shared-lib edit):**
- **The codec is 100% reused** — `Self::load_dac` is dia's `load_dac` weight-mapping verbatim building the SHARED
  `codec::dac::DacDecoder`. **NO `codec/` or `nn/` change** (so dia2/csm byte-identity is structurally preserved).
- `nn::Rope::from_inv_freq` (HALF-table) + `apply_interleaved_full` = flashinfer's `is_neox=False` rope, reused.
- `nn::rms_norm_decomposed(_, None, …)` = the no-affine QK-norm; `nn::RmsNorm::fused` = the flashinfer
  `rmsnorm`/`fused_add_rmsnorm` weight path; `nn::Linear::matmul` = every projection.

**What is genuinely NEW (model-specific, in zonos2.rs — no shared analog):** the MoE feed-forward (`MoeFfn`:
EDA router + sonic w13 de-interleave [UNIT-TESTED] + per-token top-k SwiGLU), the QK-temp-gated attention
(`Attn`: `temp.abs()` q-scale + headwise sigmoid gate, not expressible via the shared `Attention` enum), and the
codebook-delay prompt + greedy AR loop with the delayed-EOS countdown + post-EOS trim.

**Speaker conditioning is SCOPED** (not in the gate): the gate runs WITHOUT a speaker embedding (deterministic —
the reference skips injection when `speaker_embedding is None`). The LDA+projection injection + the ECAPA-TDNN
(ResNet293) embedder is a bounded clone follow-up.

**NO per-venv serving path** — the throwaway eager-PyTorch reference is golden-only.

---

## 4. Byte-faithful gate (DONE — PASS) + the RCA

**THE LAW = greedy (argmax) codes byte-identical to the reference golden, f32** (the chaotic MoE-AR greedy chain
is sub-ULP-sensitive; the reference's OWN bf16-vs-f32 greedy codes diverge — confirmed — so f32 is the
deterministic gate, exactly like dia2/misotts).

- **Result**: `zonos2_greedy_codes_byte_identical` → **288/288 codes match over 32 frames, first-div None**.
  Step-0 per-codebook greedy codes byte-identical (`zonos2_step0_codes`). Step-0 final-norm hidden **maxΔ=9e-6**.
- **Codec smoke** (`zonos2_codec_decode_smoke`): random codes → 20480 samples (= 40 frames × 512 hop), peak 0.75,
  finite — the shared `codec::dac::DacDecoder` decodes descript/dac_44khz through the zonos2 mapping correctly.
- **Synth smoke** (`zonos2_synth_smoke`): full AR → DAC → **24576 samples (0.56 s @ 44.1 kHz), rms 0.0038**
  (audible). End-to-end pipeline live.
- **Reference golden** (`scratchpad/zonos2_golden.py`): standalone eager-PyTorch `Zonos2ForCausalLM`, greedy,
  the codebook-delay prompt, dumps `codes_greedy.npy [n_frames,9]` + per-layer/router bisection probes +
  `hidden0`/`step0_logits`. Both golden + port forced to the **full-FP32 floor (TF32 OFF)**.

### The bug found + fixed (f32-CPU bisection, the prescribed RCA)
Initial gate: 53/288. Bisection (`zonos2_layer_bisection_vs_reference` + `zonos2_layer1_full_per_row`) localized
the divergence to **row 0 (the first position)** at layer 1: maxΔ 8.29 while all other rows were ~2e-6 — a
hallmark of a **broken causal mask** (position 0 wrongly attending all 49 positions). Root cause: `causal_mask`
built it as `ones.triu(1) · −inf` then `nan_to_num(nan=0, posinf=−inf, neginf=0)` — the `neginf=0` arg
**converted every −inf back to 0**, zeroing the mask → fully non-causal attention. Fix: `Tensor::full(−inf).triu(1)`
(the reference's exact `triu(full(−inf), 1)`), unit-tested (`causal_mask_is_causal`). After the fix: layer-1
maxΔ 2e-6, step-0 hidden 9e-6, **288/288 byte-identical**.

---

## 5. Perf (RTF)

`zonos2_rtf` (bf16, 96 frames): **RTF ≈ 258** — high, because the correctness-first port **full-seq-prefills
every AR step** (no KV cache: step N re-runs the whole growing sequence through all 28 layers, and the MoE
expert sum loops per-token in Rust). This is a **perf lever, NOT a correctness gap**: a ring-KV decode (like
csm/dia2) + a batched/scatter MoE are the obvious next optimizations. The byte-faithful greedy correctness is
fully established at the f32 floor.

---

## 6. Build / tests / clippy (all green)

- **`cargo test -p waav-infer-backend-torch --lib`** → **187/187** (6 new zonos2 unit tests: `moe_layer_membership`
  [3..=26 MoE, layer 26 top-2], `shear_roundtrip`, `byte_ids`, `softcap_bounds`, `sonic_w13_deinterleave`,
  `causal_mask_is_causal`; +181 pre-existing untouched).
- **clippy** `-p waav-infer-backend-torch --all-targets -D warnings` + `-p waav-infer-server --features torch
  --all-targets -D warnings`: **clean**.
- **server** builds + dispatches `"zonos2"|"ZONOS2"`.
- NO `nn::`/`codec::` shared-math change → dia2 (608/608) + csm (4000/4000) byte-identity structurally preserved;
  the ONNX-core `REGISTERED_ARCHITECTURES` count is untouched (zonos2 is torch-inprocess, dispatched in engine.rs).

---

## 7. What landed vs scoped

**LANDED (byte-faithful, this session):** the full Zonos2 port — MoE+EDA router, QK-temp-gated attention,
interleaved RoPE, fused-residual RMSNorm, softcap head, codebook-delay AR loop, the descript/dac_44khz codec
reused verbatim; **greedy codes 288/288 byte-identical** vs the golden; full AR→DAC synth live; lib 187/187 +
clippy clean; engine-wired; one real bug RCA'd + fixed.

**SCOPED (precise):**
- **KV-cache + batched MoE perf** — the #1 lever (RTF 258 → the obvious ring-KV + scatter-MoE rewrite, byte-
  identity preserved). Correctness-first full-prefill landed; perf is the deliberate next step.
- **Speaker/voice cloning** — the ECAPA-TDNN (ResNet293) → LDA → projection `index_copy` path (the projection is
  trivial; verifying it needs the separate `Zyphra/Zonos-v0.1-speaker-embedding` ResNet + a reference clip).
- **Production conditioning** — greedy emits EOS-at-frame-0 without the server's richer (sampling + rate/quality)
  conditioning; the gate/synth use `WAAV_ZONOS2_IGNORE_EOS=1` to exercise a representative fixed-length run.

---

## 8. Exact files

- `crates/waav-infer-backend-torch/src/zonos2.rs` — **NEW**, the model.
- `crates/waav-infer-backend-torch/tests/cuda_torch_zonos2.rs` — **NEW**, the live gates + bisection probes.
- `crates/waav-infer-backend-torch/src/lib.rs` ⚠ — `+pub mod zonos2;`.
- `crates/waav-infer-server/src/engine.rs` ⚠ — `+"zonos2"|"ZONOS2"` dispatch arm + import + err-list.
- `~/.cache/waav-models/zonos2/{model.safetensors, waav.json}` — the converted weights + manifest.
- Throwaway (scratchpad, reference-only): `zonos2_golden.py`, `convert_zonos2.py`, `zonos2-ref/`.

**RETURN**: PORTED + **BYTE-FAITHFUL** (greedy 288/288 vs the f32 golden, full AR→DAC synth live, lib 187/187 +
clippy clean, engine-wired). One real bug (non-causal mask) RCA'd via f32-CPU bisection + fixed. Codec reused
verbatim (dia's descript/dac_44khz). RTF ≈ 258 (correctness-first full-prefill; KV-cache is the scoped perf
lever). No shared `nn::`/`codec::` math changed → dia2/csm byte-identity preserved.
