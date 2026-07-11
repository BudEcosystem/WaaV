# MisoTTS-8B — Live 8B Byte-Identity Gate (RESULT)

**Date**: 2026-06-24 · **Box**: GB10 (Grace-Blackwell sm_121), 121 GB unified memory
**Model**: `MisoLabs/MisoTTS` (8B torchtune-native Sesame CSM twin) · **Port**: committed `829b97b`,
`crates/waav-infer-backend-torch/src/misotts.rs` (UNCHANGED — no source fix needed) ·
**Test**: `crates/waav-infer-backend-torch/tests/cuda_torch_misotts.rs` (gate-definition fix only, +20/-1)

---

## TL;DR

| Gate | Result |
|---|---|
| **Greedy codes byte-identical (f32, THE LAW)** | ✅ **1024/1024 match, first-div None** — byte-identical to the torchtune f32 golden over all 32 frames × 32 codebooks |
| **step0 cb0 logits (f32)** | ✅ top-1 logit byte-identical (\|Δ\|=0.0000), argmax + top-3 match |
| **synth_smoke (bf16, production)** | ✅ 74880 samples (3.12 s), rms 0.0635 — real, non-silent speech |
| **RTF (bf16)** | **3.08** (12.3 s wall / 4.0 s audio) |
| **codec_decode_smoke** | ✅ 48000 samples, peak 0.67 |
| **misotts.rs source fix** | **NONE** — the port was already byte-faithful |

The 8B AR forward is **proven byte-identical** to the torchtune reference in f32 (the deterministic LAW).
The lone divergence found was a **golden-harness bug**, not a port bug; once corrected, WaaV is bit-exact.

---

## 1. Download + setup

The 32.75 GB F32 `model.safetensors` had **already finished** downloading
(`32,752,529,792` bytes, 367 tensors matching the reference `Model.state_dict()`, no `.incomplete`,
symlinked into `~/.cache/waav-models/misotts/`). The staged `/tmp/run_golden.sh` + `/tmp/miso-ref/` had
been wiped (the `/tmp` clear), so the reference golden harness was **reconstructed** from the GitHub
reference (`github.com/MisoLabsAI/MisoTTS`, `models.py`/`generator.py`) — a faithful torchtune `Model` +
GREEDY `generate_frame`, built directly (bypassing `Generator` → no moshi; the codec is validated
separately). System `python3` has `torch 2.12.0+cu130` (CUDA) + `torchtune 0.6.1` — the reference loads
clean. Prompt-id parity verified: the bundled `tokenizer.json` (TemplateProcessing baked) yields the exact
`<bos> [0] {text} <eos>` ids the WaaV port produces. One 8B at a time (reference freed before the WaaV run).

---

## 2. RCA — the divergence was in the GOLDEN, not the port

The first byte-identity run showed a massive mismatch (2/1024). The bisection drove it to ground:

1. **Layout bug (caught first):** the golden `codes_greedy.npy` was saved **Fortran-order**
   (`codes.permute(1,2,0)[0]` is a transposed view); the test's minimal npy parser reads C-order, so it
   silently transposed the comparison. Fixed by `np.ascontiguousarray` in the golden.

2. **f32 bisection (the deterministic regime).** The reference's OWN bf16-vs-f32 greedy codes disagree on
   **972/1024** — the 8B+depth-31 dual-AR greedy chain is chaotically sub-ULP-sensitive, so bf16 is not a
   reproducible golden. Switching to f32 (both sides), the divergence **collapsed to frame≥1** — frame0 was
   already byte-identical (all 32 codebooks). The f32 golden is **self-deterministic** (run1==run2, 0 diff),
   so the residual f32 divergence was a real signal.

3. **Layered f32 bisection** (`embed_sum` → `prefill_last_hidden` → per-layer → feedback decode):
   - `embed_sum` maxΔ **0.0** · backbone prefill last-hidden maxΔ **1.3e-5** · layer0 attn maxΔ **0.0**
   - feedback embed sum (`embed_audio_frame`) maxΔ **0.0** · feedback decode through all 32 layers maxΔ
     **2.1e-4** (pure f32 noise). **Everything WaaV computes was byte-faithful.**
   - The apparent 6.4 divergence in the golden's `last_hidden_f1` was the giveaway: WaaV's post-final-norm
     hidden **exactly equals** `torchtune.final_norm(decode_layer31)` — it was the golden's recorded value
     that was wrong.

4. **Root cause (golden harness):** `golden.py`'s frame-0 bisection probe `_layer0_probes` called
   `model.backbone.reset_caches()` **inside the live generation loop**, wiping the prefill KV cache, so every
   frame≥1 decode in the golden attended an empty cache → corrupted `codes_greedy` (frames ≥1) and
   `last_hidden_f1`. Removing that in-loop probe (bisection must run in a separate process) produced the
   **clean golden**, against which the WaaV f32 codes are **1024/1024 byte-identical**.

**Fix location:** the throwaway reference golden (`/tmp/miso-ref/golden.py`), NOT `misotts.rs`. The WaaV
port's interleaved-RoPE / fused-RMSNorm / GQA-wiring / 2-token-prefill audio-decoder are all bit-faithful.

---

## 3. The bf16 tie-flip (honest, inherent — not a bug)

In bf16 (the production path) the greedy codes diverge at exactly **one** codebook in frame0 — **cb10**,
where the reference's top-1/runner-up logit gap is a single bf16 ULP (**0.03125**): WaaV-bf16 picks 707,
ref-bf16 picks 57; that flip then cascades the AR feedback. cb9 (gap 1.4) and cb11 (gap 0.6) are robust and
match. The cb10 decoder-hidden differs by maxΔ **0.097** (bf16 reduction-order noise in the 300M decoder's
depth chain), enough to flip a 0.03-gap argmax. This is **inherent bf16 non-associativity at a near-tie**,
the same physics that makes the reference's own bf16 disagree 95% with its f32. Notably **WaaV-f32 = ref-f32
= ref-bf16 = 57** at cb10; only WaaV's bf16 rounds the other way.

**Decision (matches the codebase precedent — dia2/voxtral/neutts all gate in f32):** the byte-identity LAW
for misotts is the **deterministic f32 gate** (1024/1024 PASS). The bf16 path is exercised by the synth/RTF
smokes. Closing the last 0.03-ULP bf16 tie would require an f32 decoder (a precision lift, not a correctness
fix) and is deferred as an optional accuracy knob; the §5 onboard note (greedy f32 is the cross-runtime LAW)
already anticipated this.

---

## 4. Perf

`misotts_rtf` on GB10 CUDA bf16: **RTF ≈ 3.08** (12.3 s wall / 4.0 s audio, 96000 samples). As the onboard
report foresaw, the per-frame 32-sequential-depth-decode inner loop with NO batching / NO CUDA-graph
dominates (the depth CUDA-graph seam csm has is not wired for `InterleavedFull` yet — a perf lever, not a
correctness gap). The ~3.08 is higher than the onboard's optimistic ~1.1–1.5 guess but expected for the
8B + 32-codebook sequential decoder at batch 1.

---

## 5. Changes made

- **`misotts.rs`**: NONE (the port was already byte-identical).
- **`tests/cuda_torch_misotts.rs`** (+20/-1, committed-pending for the coordinator): `load()` honors
  `WAAV_MISOTTS_FP32`; the `misotts_greedy_codes_byte_identical` gate defaults to the deterministic **f32**
  path (the LAW), documenting the bf16 chaotic-sensitivity. clippy clean, all four gates green.
- **Reference golden** (throwaway, `/tmp/miso-ref/golden.py`): rebuilt; the cache-corrupting in-loop
  `_layer0_probes` call removed; C-contiguous code dump. Golden artifacts at `/tmp/miso_golden` (bf16) and
  `/tmp/miso_golden_f32` (f32, the LAW golden).

**Verdict:** MisoTTS-8B is LIVE-validated — greedy codes **byte-identical** to the torchtune golden (f32),
step0 logits exact, real audio synth, RTF 3.08. No `misotts.rs` fix needed.
