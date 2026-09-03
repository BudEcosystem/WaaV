# Fleet-Regression FIX — misotts + zonos2 (TTS byte-identity)

**Host:** GB10 (sm_121, 121 GB unified). **Date:** 2026-06-24. **Repo:** `/home/bud/ditto/waav/waav-infer`.
**Source regression report:** `WaaV/inferv2/REVIEW/FLEET-REGRESSION-TTS.md` (the 2 REAL regressions, both TTS).
**Bar:** the [[waav-infer-100-percent-correctness]] LAW — fix EVERY divergence to byte-identical; never accept "fragility".
**Constraints honored:** no `git commit`, no `cargo fmt`; touched ONLY the two test files I own (`cuda_torch_misotts.rs`,
`cuda_torch_zonos2.rs`); a concurrent voxtral-perf + TRT agent shared `target/` + GPU throughout (coordinated; their
`voxtral.rs`/`self_attention.rs`/other test edits left untouched). One model at a time on the GPU.

---

## VERDICT — BOTH FIXED, both LAW-green

| Model | Root cause | Class | Fix | Final result |
|---|---|---|---|---|
| **misotts** | The gate's default golden (`/tmp/miso_golden`) had been overwritten with a **bf16** golden, but the gate forces **f32** generation → it compared f32-tch against bf16-golden. | **Golden-staleness (NOT a code regression)** | Point the gate default at a freshly-regenerated **f32** golden, persisted to `~/.cache/waav-models/misotts-golden/` (reboot-survival). | **1024/1024 match, first-div None** (32 frames × 32 cb) |
| **zonos2** | The LAW gate omitted `WAAV_ZONOS2_IGNORE_EOS=1`; the golden is an *ignore-EOS* 32-frame trajectory (kept the eoa_id rows it hit at frames 4-7), but tch's natural greedy stop (eoa@frame4 → `N_CODEBOOKS+1`=10-frame delay-pattern countdown) terminated at **frame 14**. | **Gate/golden regime mismatch (NOT a code regression)** — the AR codes are byte-identical, only the stop *test* differed. | Set `WAAV_ZONOS2_IGNORE_EOS=1` in the LAW gate (the documented intent; matches `zonos2_synth_smoke`/`zonos2_rtf`). | **288/288 match, first-div None, tch frames 32 == golden 32** |

**Neither was a port bug. No `misotts.rs`/`zonos2.rs` source change, no shared `nn::`/`codec::` change.**

---

## A. misotts — ROOT CAUSE = golden staleness (bf16 golden at the f32 gate's path)

### Step 1 — code regression ruled out
- `git log 11ce647..HEAD -- src/misotts.rs` → **empty**. misotts.rs is byte-unchanged since the 1024/1024 commit (11ce647).
- Shared deps it uses (`nn::RmsNorm::fused`, `nn::Rope` `InterleavedFull`, `nn::Mlp::swiglu` Silu, `codec::MimiDecoder`):
  - `nn/mlp.rs` (c36d39f) only ADDED a new `Act::GeluNew` variant (for indextts2) — existing Silu/Gelu/Relu untouched.
  - `codec/mimi.rs` (e3b2f49, the hibiki commit) added `MimiConfig.interleaved_rope`, **defaults false** = the original
    transformers rotate-half path. misotts loads `codec::MimiConfig::mimi_24khz(...)` (NOT `_native`) → gets the original
    path. That commit re-verified csm 4000/4000 + dia2 544/544 byte-identical. **No misotts impact.**

### Step 2 — golden provenance (the smoking gun)
The gate forces f32 (`cuda_torch_misotts.rs` auto-sets `WAAV_MISOTTS_FP32=1`); its own doc-comment says "In f32 the WaaV
port is BYTE-IDENTICAL to the torchtune golden (verified 1024/1024)" and the LAW golden is the **f32** golden. But the
default golden path `/tmp/miso_golden` held a **bf16** golden. Multiple golden dirs coexisted in `/tmp` from
golden-regen experiments:

| dir | provenance | `codes_greedy` vs `/tmp/miso_golden` |
|---|---|---|
| `/tmp/miso_golden` (gate default) | **bf16** (== `miso_golden_bf16b`, 00:00:51) | 1024/1024 (self) |
| `/tmp/miso_golden_f32` | **f32** (the LAW golden) | **61/1024** |
| `/tmp/miso_golden_f32b` | f32 (earlier) | 35/1024 |

`MISOTTS-8B-LIVE.md` confirms intent: *"Golden artifacts at `/tmp/miso_golden` (bf16) and `/tmp/miso_golden_f32`
(f32, the LAW golden)"* and *"`misotts.rs`: NONE (the port was already byte-identical)"*.

**Reproduced the regression EXACTLY:** f32-codes vs the bf16 golden = **61/1024, first-div (cb0, frame3)** — bit-for-bit
the fleet-report numbers. Frame 0 is fully identical (deterministic prefill); divergence begins frame 3 = the documented
bf16-vs-f32 AR-compounding (0.06/layer over 32 layers flips a borderline mid-depth argmax tie, then the dual-AR feedback
cascades). The fleet sweep ran f32-tch against the **bf16** default golden.

### Step 3 — regenerated the f32 golden FRESH (provenance-clean) + persisted
- Reran the staged torchtune reference `/tmp/miso-ref/golden.py` (the FIXED harness: C-contiguous dump, no in-loop
  `_layer0_probes` cache-wipe), 8B **f32** CUDA, `NVIDIA_TF32_OVERRIDE=0`, greedy `generate_frame`, 32 frames.
  (OOM on first try with a fragmented unified pool while voxtral held 13 GB → solved with
  `PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True`; loaded f32 OK.)
- **Fresh f32 golden == existing `/tmp/miso_golden_f32`: 1024/1024** (the f32 golden is run-to-run deterministic).
- **Fresh f32 golden vs the bf16 golden: 61/1024** (reproduces the regression number — closes the loop).
- **Persisted to `~/.cache/waav-models/misotts-golden/`** (granite-style reboot-survival), and pointed the gate's
  `golden_dir()` default there.

### Step 4 — live LAW gate vs the fresh cached golden → PASS
```
misotts greedy: tch frames 32, golden frames 32
misotts greedy codes: 1024/1024 match over 32 frames; first-div None
test misotts_greedy_codes_byte_identical ... ok (36.98s)
```

**Golden persisted: YES** → `~/.cache/waav-models/misotts-golden/` (freshly regenerated f32, reboot-surviving).

---

## B. zonos2 — ROOT CAUSE = LAW gate missing the ignore-EOS regime of its golden

- Golden `codes_greedy.npy` = **(32, 9)**; `meta.json` `n_frames: 32, eos_frame: 0` ⇒ generated with **ignore-EOS**
  (ran the full 32-frame budget). The golden itself **contains eoa_id (1024) at frames 4-7** yet kept generating.
- tch `generate_codes` (src/zonos2.rs §EOS): on the first frame with any codebook == `EOA_ID` (1024) it sets
  `eos_frame` and starts `eos_countdown = N_CODEBOOKS+1 = 10`; after the countdown it `break`s — **unless**
  `WAAV_ZONOS2_IGNORE_EOS=1`. Greedy zonos2 hits eoa at **frame 4** → stops at frame 4+10 = **frame 14** (= the report).
- The codes were **byte-identical where they overlap** (126/126 over frames 0-13, first-div None) — proving the AR math,
  MoE routing, QK-temp attention, interleaved-RoPE, softcap head are all correct. The *only* divergence was the early
  stop: the gate ran a different generation regime than its golden.
- **Fix:** the LAW gate now sets `WAAV_ZONOS2_IGNORE_EOS=1` (alongside the existing `MAX_FRAMES=gt`, `FP32=1`). This is
  the documented intent — `ONBOARD-zonos2.md`: *"the gate/synth use `WAAV_ZONOS2_IGNORE_EOS=1` to exercise a
  representative fixed-length run"* — and matches `zonos2_synth_smoke`/`zonos2_rtf`. No `zonos2.rs` change.
- Golden freshness: `/tmp/zonos2-golden` is from this same session (Jun 24 01:30-02:07, against the current
  converted checkpoint; the 288/288 onboard result). The throwaway `zonos2_golden.py` recipe was already cleaned, so
  it could not be regenerated, but it is verified-current and **persisted to `~/.cache/waav-models/zonos2-golden/`**
  (reboot-survival). The gate run below points at that cache.

### Live LAW gate (with the fix) → PASS
```
zonos2 greedy: tch frames 32, golden frames 32
zonos2 greedy codes: 288/288 match over 32 frames; first-div None
test zonos2_greedy_codes_byte_identical ... ok (47.69s)
```

**Golden persisted: YES** → `~/.cache/waav-models/zonos2-golden/`.

---

## LAW verification (full)
- **misotts `misotts_greedy_codes_byte_identical`** → **1024/1024, first-div None** (f32, vs fresh persisted golden).
- **zonos2 `zonos2_greedy_codes_byte_identical`** → **288/288, first-div None, frames 32==32** (f32, ignore-EOS).
- **`cargo test -p waav-infer-backend-torch --lib`** → **192 passed; 0 failed** (incl. all zonos2 unit tests).
- **`cargo clippy -p waav-infer-backend-torch --all-targets --features cuda -- -D warnings`** → **clean (exit 0)**.
- **dia2 (608) / csm (4000) re-verify:** NOT triggered by my changes — I touched zero shared `nn::`/`codec::` (only the
  two test files). (Note: the concurrent agent independently edited `src/nn/self_attention.rs`; re-verifying dia2/csm
  against *that* edit is the voxtral-perf/TRT agent's responsibility, not this regression fix.)

## Exact files changed (mine only)
- `crates/waav-infer-backend-torch/tests/cuda_torch_misotts.rs` — `golden_dir()` default → `~/.cache/waav-models/misotts-golden` (f32 golden), with rationale comment.
- `crates/waav-infer-backend-torch/tests/cuda_torch_zonos2.rs` — `zonos2_greedy_codes_byte_identical` now sets `WAAV_ZONOS2_IGNORE_EOS=1`, with rationale comment.

## Persisted goldens (reboot-survival, like granite)
- `~/.cache/waav-models/misotts-golden/` — **freshly regenerated** f32 torchtune golden (`codes_greedy.npy` + bisection probes + meta).
- `~/.cache/waav-models/zonos2-golden/` — the verified-current f32 ignore-EOS golden (copied from `/tmp/zonos2-golden`).
