# TORCH212-MIGRATION — Phase 1: torch 2.11 → 2.12 re-link + byte-identical golden re-validation

> **Scope:** PHASE 1 ONLY — move the Rust build/runtime from torch 2.11 to **torch 2.12** and
> RE-VALIDATE every byte-identical golden under 2.12. **No TRT wiring** (that is Phase 2, gated on this
> checkpoint). Done in the **isolated worktree** `/home/bud/ditto/waav/waav-infer-torch212`
> (branch `torch212-migration`, off `waav-infer-v2-build` HEAD `6c282b3`). The main checkout
> `/home/bud/ditto/waav/waav-infer` (another agent, torch 2.11) was NOT touched.
>
> **Box:** NVIDIA GB10 (sm_121, cap 12.1), aarch64, 121 GB unified pool.
> **Date:** 2026-06-27.

---

## 1. Why (one line)

The no-Python `.ts` TensorRT path is **torch-2.12-only** (proven in TRT-ENV-SETUP.md §3: a 2.12-compiled
`.ts` cannot link/load in a torch-2.11 runtime — the `torch::headeronly::Tag` ABI break). To unblock the
Throughput tier we must move the build to torch 2.12 **and** prove the byte-identical foundation survives
there first. The box was historically torch 2.12, so this is a revert-to-known-good, not new ground.

---

## 2. Environment / re-link recipe

- **2.12 venv (already built):** `/home/bud/torch212_trt_venv` — torch **2.12.0+cu130**, torch_tensorrt
  2.12.1, tensorrt 10.16.1.11. torch/lib = `…/lib/python3.12/site-packages/torch/lib` (has
  `libtorch_cuda.so`, `libc10_cuda.so`).
- **New env file (the ONLY repo addition):** `gb10-env-212.sh` — a copy of `gb10-env.sh` whose **only delta**
  is hard-pinning the 2.12 venv's `torch/lib` at the front of `LD_LIBRARY_PATH`. It keeps
  `TORCH_CUDA_VERSION=cu130`, `LIBTORCH_BYPASS_VERSION_CHECK=1`, `LIBTORCH_USE_PYTORCH=1`, the ORT-CUDA dylib,
  the CUDA-13.1 compat dir, and the OOM guardrails (`CARGO_BUILD_JOBS=6`, `RUST_TEST_THREADS=4`) — all
  identical to the 2.11 env.
- **Usage:** `source /home/bud/torch212_trt_venv/bin/activate && source ./gb10-env-212.sh` (activating the
  venv first makes `python3` resolve to the 2.12 torch, which the torch backend's `build.rs` also probes to
  force-link `libtorch_cuda`).

---

## 3. Build outcome (against torch 2.12)

**CLEAN BUILD — zero code changes, zero dependency changes.**

- `cargo build -p waav-infer-backend-torch -p waav-infer-server --features cuda --tests` → **Finished, exit 0**
  (from-scratch, ~1m21s on 6 jobs; all 66 backend-torch test binaries produced).
- **tch was NOT bumped.** `tch = "0.20"` (torch-sys 0.20.0) is unchanged in `Cargo.toml` and `Cargo.lock`.
  `git status` shows the **only** new file is `gb10-env-212.sh`; no Cargo edits.
- **What made 2.12 link with tch 0.20 (which "expects" libtorch 2.11):** `LIBTORCH_BYPASS_VERSION_CHECK=1`
  alone. torch-sys 0.20's `build.rs::version_check()` early-returns `Ok(())` when that env var is set
  (verified at source: `torch-sys-0.20.0/build.rs:156-167`), so the 2.11≠2.12 version-string gate is
  bypassed. The C++ bindings torch-sys compiles (`libtch/*.cpp`) are **ABI-compatible 2.11→2.12** — the
  `torch::headeronly::Tag` rename (TRT-ENV §3) only affects **torch_tensorrt's** `libtorchtrt_runtime.so`,
  which is NOT linked in Phase 1. So the standard libtorch binding surface tch uses is unaffected.
- **CUDA initialises under 2.12:** `cuda_smoke::cuda_is_available_and_matmul_exact` →
  `is_available=true, device_count=1`, softmax/matmul exact. The B16 `--no-as-needed` force-link (build.rs)
  keeps `libtorch_cuda` in `DT_NEEDED` under 2.12 exactly as under 2.11 — no `LD_PRELOAD` needed.

**Net: the only artifact required to build+run on torch 2.12 is the env file. No tch/dep bump.**

---

## 4. Golden re-validation table (torch 2.12)

Method: run each gate process-isolated in the worktree, `free -g` first, one model at a time, under
`gb10-env-212.sh` + the 2.12 venv. Goldens are the sidecar references captured 2026-06-22/24 under the
**torch 2.11** build venv, so GREEN = "torch 2.12-tch matches the 2.11-captured sidecar golden" = the kernel
numerics did not move across the version bump for that op sequence.

| Model | Gate | Expected | Result (torch 2.12) | Verdict |
|---|---|---|---|---|
| **dia2** | `cpu_fp32_codes_byte_identical` | 544/544 | **544/544**, first-div None | **GREEN** |
| **dia2** | `cuda_bf16_codes_byte_identical` (B25 LAW) | 608/608 | **608/608**, first-div None | **GREEN** |
| **dia2** | `dia2_graph_capture_vs_eager_trace` (CUDA-graph A/B) | 1188/1188 | **1188/1188**, capture==eager bit-faithful | **GREEN** |
| **csm** | `cuda_csm_codes_byte_identical_to_sidecar` | greedy bit-exact | **125 frames × 32 cb byte-identical**; step0 cb0 logit 10.1250==golden | **GREEN** |
| **voxtral** | `cuda_torch_voxtral_vs_ort` (strict EN) | 100% char | **100.0% char-identity** (tch-CUDA == ORT); soft bf16-vs-q4 clip 82.4% (by design) | **GREEN** |
| **cohere** | `cuda_torch_cohere_vs_ort` | 100% | **100.0% char-sim**, identical transcript | **GREEN** |
| **ark** | `cuda_torch_ark_byte_identical` | 100% | **100.0% char-identity** vs sidecar golden | **GREEN** |
| **cosyvoice3** | `cuda_torch_cosyvoice3` (LLM token byte-id) | 123/123 | **123/123 AR speech-tokens byte-identical** (first-div None); CFM mel max\|Δ\|0.0049, vocoder corr 0.853, e2e determinism 119460/119460 — all within the same tolerance gates as 2.11 | **GREEN** |
| **dia2** | `dia2_tch_force_solo_codes_identical_ragged` (Fork-A1, CUDA bf16) | batched==solo | **5 rows / 5504 codes / max\|Δ\|=0**; D2 slots 0,4 byte-identical (slot-independent RNG) | **GREEN** |
| **qwen3-tts** | `qwen3_tch_force_solo_codes_identical_ragged` (Fork-A1, CUDA f32) | batched==solo | **4 rows / 2704 codes / max\|Δ\|=0** (frames 12/51/35/71) | **GREEN** |

Notes:
- dia2 CUDA bf16: load 5.36s | AR-gen 4.74s | synth 7.23s | audio 1.52s | RTF 4.76 (perf unchanged; the gate
  is correctness).
- voxtral 2nd clip (`rag_physics`) is an intentionally-**soft** bf16-torch-vs-q4-ORT comparison (82.4%), not
  a byte-identity gate; the strict EN clip is the byte-identity arm and is 100%.
- cosyvoice3 CFM/vocoder stages are **tolerance** gates by design (ODE integrator + ORT-CUDA estimator, not
  bit-exact); only the AR speech-token sequence is the strict byte-identity arm, and it is 123/123.
- **Force-solo cell choice:** the two Fork-A1 oracles default to CPU-f32 (the "cleanest deterministic" cell).
  The CPU-f32 dia2 run was **functionally correct but overran on CPU** (loads dia2 ×5 + AR loops; reached
  solo[3]=87 frames in ~6 min before being switched). Both were re-run in the **CUDA cell**
  (`WAAV_BATCH_DEV=cuda`: dia2 bf16, qwen3 f32) — the serving-precision cell and a **stronger** test (it
  exercises bf16 + sampling + the D2 content-keyed-RNG batched==solo path). Both `max|Δ|=0`. These oracles
  are tch-vs-tch self-consistency (batched ring == solo B=1), so they validate that 2.12's batched execution
  stays bit-self-consistent; they are version-independent by construction and confirmed exact under 2.12.

**Drift / re-baseline summary: NONE.** Zero gates drifted; zero goldens were re-baselined; zero real breaks.
Every 2.11-captured sidecar golden is reproduced byte-identically by torch-2.12-tch. So there was no need to
re-run any sidecar dumper under 2.12 — the kernels (fused RMSNorm, SDPA priority, CUDA-graph capture, bf16
multinomial draw order, greedy argmax) all produce identical numerics across the 2.11→2.12 bump on GB10/sm_121.

---

## 5. Verdict — Phase 2 (TRT wiring): **GO**

**10/10 gates GREEN under torch 2.12. Zero drift. Zero re-baseline. Zero real breaks.**

Every byte-identity gate that defines the foundation is byte-identical under torch 2.12:
dia2 (CPU-fp32 544/544, CUDA-bf16 608/608, CUDA-graph 1188/1188), csm (125×32), voxtral (100% strict),
cohere (100%), ark (100%), cosyvoice3 (LLM 123/123), and both Fork-A1 force-solo oracles (dia2 & qwen3,
max|Δ|=0). The build is clean with **no tch bump** — `LIBTORCH_BYPASS_VERSION_CHECK=1` + the existing B16
force-link are sufficient; the only repo addition is `gb10-env-212.sh`.

torch 2.11→2.12 preserves every kernel's numerics on GB10/sm_121, so **the byte-identical foundation is SOUND
on torch 2.12**. Phase 2 (wiring the no-Python `.ts` TensorRT Throughput path, which is torch-2.12-only) is
cleared to proceed in this worktree.

### Caveats / what Phase 1 did NOT cover
- Only the **named** goldens were re-validated (the make-or-break set). The remaining torch-backend gates
  (dia, granite, neutts, higgs/higgs_v2, dots, indextts2, misotts, s2_pro, vibevoice, etc.) were not re-run
  here; given 10/10 of the representative set are byte-identical with no drift, they are expected GREEN, but a
  full `ci/heavy_live_tests.sh` sweep under `gb10-env-212.sh` should be run before merging the 2.12 move.
- This is a **worktree-local** validation; nothing was committed. The main `waav-infer` checkout (torch 2.11)
  was untouched.
- Phase 2 will additionally link `libtorchtrt_runtime.so` (2.12) — the `torch::headeronly::Tag` ABI surface
  that is the whole reason for the 2.12 move. That link is **not** exercised by Phase 1 (no TRT gate was run);
  it is the first thing Phase 2 must prove.
