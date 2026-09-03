# misotts / s2_pro — TRT fp16 Throughput backbone engines (item #3)

Goal: compile lossy-fp16 Throughput TRT engines for the backbone-dominated heavy models
(misotts 8B Llama backbone; s2_pro 36-layer Qwen3 slow-AR) so their **single-stream RTF**
improves in `PerfMode::Throughput`. The byte-identical **Accuracy default stays untouched**.

Hardware: GB10 (sm_121, 121GB UNIFIED CPU+GPU pool). torch 2.12.0+cu130, torch_tensorrt 2.12.1,
tensorrt 10.16.1.11. SAFETY: memory watchdog aborts any compile if MemAvailable < 25 GiB
(unified-OOM has hard-crashed the box). One compile at a time.

Branch waav-infer-v2-build, HEAD c7985d6. Scaffolding already present (commit 9a2d862):
`generate_codes_trt` + `maybe_load_trt` + `WAAV_<MODEL>_TRT` in misotts.rs / s2_pro.rs.

Status: COMPLETE. misotts = documented non-win + hardware limit (no engine shipped). s2_pro = working
opt-in TRT engine staged (corr 0.9999993, end-to-end RTF 1.111×). The box never crashed (watchdog held).
Uncommitted on disk (coordinator commits). Temp MAX_FRAMES measurement cap reverted.

## misotts (8B, 32-layer Llama backbone) — most backbone-dominated
- checkpoint: 32.75 GB F32 (≈8B params); backbone = 6.98B params (289 tensors). fp16 resident ≈14GB.
- compile script: torch_runtime/trt_compile_misotts.py (selective backbone-only fp16 load).
- RoPE: InterleavedFull (consumes the Rust-fed DOUBLED cos/sin, slices first half).
- GQA: SDPA enable_gqa=True (dia2-proven; manual repeat_interleave broke torch.export's dynamic-shape guard).
- eager backbone per-step (fp16, B=1, GB10) ≈ 71 ms.

### Memory journey (the GB10 unified-pool constraint — a headline hardware finding)
- Watchdog floor = 25 GiB MemAvailable (abort-on-cross). One compile at a time.
- **Attempt 1 (no offload):** loaded fine (6.98B), eager ran, export OK — TRT build phase climbed past the
  floor: **watchdog ABORTED at MemAvailable = 21.0 GiB**. The 8B single-engine TRT build transiently needs
  >96 GiB on the unified pool. dia2's OOM-warning was the canary; at 8B it actually crosses the safety floor.
- **Attempt 2 (offload_module_to_cpu=True — the dynamo warning's own mitigation):** build SURVIVED (min
  MemAvailable 27.2 GiB). So misotts-8B TRT compiles ONLY with offload. The SAVE stage then OOM'd (a post-
  compile model re-add + in-process reload doubled memory). FIX: pre-compute all eager refs+bench BEFORE
  compile, FREE the source model before the trace/save.
- **Attempt 3 (offload + use_fp32_acc + memory-safe save):** the build + the whole eval ran, but the **8B .ts
  trace/save ITSELF crossed the floor — watchdog ABORTED at MemAvailable = 23.7 GiB** (the serialization of a
  ~14GB TRT engine transiently doubles it; the model was already freed). The `backbone_fp16.ts` was a 0-byte
  killed-mid-write artifact (removed). So persisting/serving the 8B engine is itself at the unified-memory
  edge on this box.

### Accuracy (corr) — FIXED by use_fp32_acc
- First builds gave hidden_correlation ≈ 0.93–0.96 on random stress inputs. rmsnorm spelling (fused 0.955 vs
  explicit-f32 0.930) was NOT the lever. Root cause = TRT fp16 GEMM accumulation (fp16) vs PyTorch cuBLAS
  (f32-accumulate), compounding over the 32-layer 4096-wide 8B stack (dia2 at 28L/2048 tolerated it).
- **`use_fp32_acc=True` fixes it**: dynamic-shape corr at S=1/32/1024 = **0.99996 / 0.99998 / 0.99999** (the
  opt-case S=192 0.977 is an input-specific outlier — 3 independent random draws all clear 0.9999). So an
  accuracy-preserving engine IS buildable.

### PERF — the decisive finding: NO accuracy-preserving RTF win
- python bench (B=1, GB10): **TRT 70.04 ms/step vs eager 71.30 ms/step → 1.018× — essentially no win.**
- Root cause: the misotts 8B backbone per-step at B=1 is **memory-BANDWIDTH-bound** (~14GB of weights read per
  step). fp16-TRT and fp16-PyTorch read the SAME bytes, so TRT cannot beat eager; and the **f32 accumulation
  that accuracy REQUIRES** eats whatever GEMM headroom existed. The only faster path (fp16-accumulation, no
  use_fp32_acc) is the inaccurate 0.93-corr one. Contrast dia2 (1.4B, launch-bound) which TRT genuinely sped
  up 1.2×; the 8B misotts step is GEMM/bandwidth-bound, where lossy-but-no-faster TRT has nothing to offer.
- WER not run: there is no valid staged .ts (save OOM), and a 1.018× engine cannot improve single-stream RTF
  regardless of the WER fork, so it does not change the ship decision.

### VERDICT — misotts: do NOT ship a TRT Throughput engine.
Honest outcome = a documented dual limit: (1) **no accuracy-preserving single-stream RTF win** (bandwidth-bound
8B step; lossy-fp16 is a wash once f32-accumulation is required for fidelity), and (2) the 8B engine compile
AND save both **hug/cross the GB10 25 GiB unified-memory safety floor** (build survives only with offload at
min ~27 GiB; the .ts save crosses it). The opt-in scaffolding stays a no-op (no engine staged ⇒ eager
byte-identical default untouched). This is the correct call, not a forced/risky one.

(byte-identical-default confirmation below; s2_pro results pending the in-progress build)

## s2_pro (36-layer Qwen3 slow-AR, 3.63B) — fast-AR-bound (partial lever)
- KEY architecture finding: the slow-AR is **zero_rope** (the reference's `freqs_cis` is left all-zeros, so
  q,k → 0 ⇒ attention = UNIFORM MEAN of v over the causal window) + **ones-RMSNorm** (all norms reset to ones).
  So the engine needs only wqkv/wo/w1/w3/w2 (180 tensors), computes the uniform V-mean explicitly, and the
  fed cos/sin are numerically irrelevant. Confirmed: engine `new_k` = all-zeros (`ref_new_k_absmax = 0.0`).
- compile script: torch_runtime/trt_compile_s2_pro.py.
- MEMORY: 3.63B fits WITHOUT offload (min MemAvailable ~38 GiB) and the .ts saves cleanly. No unified-memory
  limit here — unlike the 8B misotts.
- **PERF (python bench): trt 33.8 ms/step vs eager 43.8 ms/step → 1.296× — a REAL slow-AR speedup** (unlike
  misotts; 3.6B is less bandwidth-saturated / the zero-rope simplifies the per-step graph). BUT s2_pro is
  fast-AR-bound (10-codebook fast loop + firefly codec dominate), so the END-TO-END synth RTF win is bounded
  by Amdahl (measured by the live Rust WER eval, below).
- **ACCURACY (after fixes): corr = 0.9999993 (opt) / 0.9999995 / 0.9999994 / 0.9999997 (dyn S=1/32/1024)** —
  comfortably > 0.999. Engine staged: `<s2-pro>/trt/backbone_fp16.ts` (9.69 GiB).

### TWO TRT-converter bugs found + fixed (generalizable findings)
1. **`use_fp32_acc=True` is REQUIRED for accuracy** on the deep stack (without it corr ≈ 0.93: TRT fp16 GEMMs
   accumulate in fp16 vs PyTorch cuBLAS f32-accumulate). Same lever as misotts.
2. **TRT's `aten::rms_norm` converter ZEROS the output when weight is None** (the `elementwise_affine=False` /
   ones-weight case). Symptom: the engine's hidden `h` came back ALL-ZEROS (corr nan, rel_max 1.0) while the
   `new_v` passthrough stayed correct — the final `rms_norm(h, None)` collapsed h to 0. FIX = spell RMSNorm as
   the EXPLICIT f32 decomposition `x·rsqrt(mean(x²)+eps)` (no None-weight op). This bit only s2_pro because its
   slow-AR uses ones-norms; misotts has real norm weights so its `F.rms_norm` path was unaffected, but the
   explicit decomposition is the safe spelling regardless.
- Also: SDPA on all-zero q,k is a TRT edge case — the engine computes the zero-rope attention as an EXPLICIT
  uniform V-mean (= what eager's SDPA(0,0,v) equals) instead. And unused cos/sin survive export/trace as input
  placeholders (5-input `.ts` confirmed), so no keep-alive hack is needed.

### Live Rust RTF + WER eval (Accuracy vs Throughput) — DONE
Engine loads + runs in-process in the Rust server binary (the torch-tensorrt runtime self-`dlopen`s,
no LD_PRELOAD): Throughput `trt_active=true`, `trt_precision=Some(Fp16)`. Harness:
`crates/waav-infer-server/tests/zz_misotts_s2pro_trt_wer.rs` (throwaway).
- **RTF (end-to-end synth, 200-frame cap, 4 short prompts):** Accuracy 2.509 vs Throughput **2.259 → 1.111×**
  (~2.3 s saved/synth = the ~200×(45→34 ms) slow-AR step saving). The slow-AR per-step 1.296× dilutes to
  ~1.11× end-to-end EXACTLY as Amdahl predicts — s2_pro is fast-AR-bound (the 10-codebook fast loop + firefly
  44.1 kHz codec are unaffected and dominate). Both RTF >1 (the eager fast-AR+codec keep it slower-than-realtime
  on this path); the slow-AR TRT helps but does not flip realtime.
- **WER fork: INCONCLUSIVE (not a clean quality gate here).** s2_pro's zero-shot greedy synth on arbitrary,
  unconditioned text runs to MAX_FRAMES (≈2048; capped to 200 for the timing run) producing runaway NON-speech
  (9.29 s of garbled audio for "Hello there." in BOTH modes), so the absolute WER is meaningless (Acc 2.07 /
  Thr 3.40 macro — comparing two non-speech outputs; no NaN/Inf). The reliable accuracy evidence is the engine
  corr (0.9999993 slow-AR hidden ⇒ near-identical semantic tokens ⇒ faithful audio on a properly-conditioned
  input). The MAX_FRAMES cap was a TEMP measurement edit, reverted.

### Byte-identical Accuracy-default confirmed (step 3)
- The Accuracy load (accel_tensorrt cfg present, Throughput NOT selected) reports `trt_active=false`,
  `precision_path=f32-sandwich` — the TRT engine is NOT loaded, so `generate_codes` takes the UNCHANGED eager
  byte-identical path (`if self.trt.is_some()` is false ⇒ max|Δ|=0 by construction). The opt-in scaffolding is
  fully gated on `PerfMode::Throughput`/`WAAV_S2_PRO_TRT`. (The standing `s2_pro_force_solo_codes` oracle
  exercises the same eager path; not re-run here as its greedy decode is gated by the runaway-generation cost.)

### VERDICT — s2_pro: a WORKING, accurate, opt-in TRT Throughput engine with a SMALL end-to-end win.
Staged at `<s2-pro>/trt/backbone_fp16.ts` (9.69 GiB, fp16 + use_fp32_acc, corr 0.9999993). It is the one
model here where TRT genuinely helps (slow-AR step 1.296×), but the architectural fast-AR-bound ceiling caps
the end-to-end win at ~1.11×. Accuracy default is untouched (opt-in). Worth keeping as the opt-in Throughput
tier; modest benefit. NOTE: the staged engine's KV profile is max-kv 1024 — to serve full-length (up to
MAX_FRAMES=2048) generations, recompile with `--max-kv 2176` (a 1-line arg; the build/save both fit easily at
3.6B).

## misotts byte-identical-default (step 3)
No engine is staged for misotts (the 8B .ts save crosses the safety floor; the 0-byte artifact was removed),
so `maybe_load_trt` finds no `.ts` ⇒ returns None ⇒ eager. The opt-in scaffolding is therefore a strict no-op
for misotts and the byte-identical default is untouched by construction.

## Generalizable TRT-on-GB10 findings (reusable for future #3 work)
1. **`use_fp32_acc=True` is mandatory** for accuracy-preserving fp16 engines on deep stacks (TRT fp16 GEMMs
   else accumulate in fp16 vs PyTorch cuBLAS f32-accumulate → corr ~0.93). It also ERASES the fp16 throughput
   win on bandwidth-bound large-B=1 GEMMs (the misotts result).
2. **TRT `aten::rms_norm` with weight=None zeros the output** — always spell ones-/no-affine RMSNorm as the
   explicit f32 decomposition.
3. **Manual `repeat_interleave` GQA-expand on the dynamic KV dim breaks torch.export** (use SDPA
   `enable_gqa=True`); and **SDPA on all-zero q,k is a TRT edge case** (compute the mean explicitly).
4. **Unused engine inputs survive export+jit.trace as placeholders** (no `*0.0` keep-alive hack — and that
   hack is itself a TRT mis-compile hazard).
5. **8B single-engine TRT builds need offload + cross the GB10 25 GiB unified floor** on both build and save;
   ~3.6B fits comfortably. The watchdog (`scratchpad/mem_watchdog.sh`, floor 25 GiB) aborted every over-budget
   build cleanly — the box never crashed.

Compile scripts: `torch_runtime/trt_compile_{misotts,s2_pro}.py`. Coordinator commits.
