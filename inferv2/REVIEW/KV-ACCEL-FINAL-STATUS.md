# KV-ACCEL — COMPREHENSIVE FINAL STATUS (device-resident KV + ORT accel catalog)

**Date:** 2026-06-25. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified pool), aarch64.
**Tree:** `waav-infer` @ committed HEAD `073620d` (Phase 0–6) **+ the on-disk uncommitted Phase 7
accel-catalog work** (6 modified files; left on disk, NOT committed; no `cargo fmt`, per coordinator discipline).
**Plans followed:** `KV-ACCEL-INTEGRATION-PLAN.md` (§3.2 SlotId ring, §6 multi-quant, §7 accel catalog, §9 gates),
`ORT-PERF-FEATURES.md` (the ranked catalog: levers 1 fuse+serialize / 3 SDPA-pin / 5 conv knobs).
**Predecessor:** `KV-IMPL-PHASE56-STATUS.md` (Phase 0–6 = device-KV fp32 SHIP + q4f16 NO-GO + fused/CUDA-graph RETIRED).
**Env:** `source gb10-env.sh` (ORT 1.27 CUDA EP, `ort` rc.12, `CARGO_BUILD_JOBS=6`, 48 GiB arena cap). Live
gates ran process-isolated `--test-threads=1`, ONE model set at a time; `free -g` checked before each (≥20 GiB
free throughout, no OOM, no box-kill).

---

## 1. Headline verdict

**Workspace is GREEN. No regression. KV-ACCEL ships at its proven scope, now WITH the Phase-7 accel catalog.**

The KV-ACCEL program ends in two cleanly-separated deliverables:

1. **Device-resident KV (Phase 0–6, committed `073620d`):** the **fp32 per-slot device-resident KV ring** is
   byte-identical and **measured 2.34× faster than host-KV at B=24** (crossover knee B≈8). This is the proven,
   shipped decoder-level lever. The q4f16/F16 device-default flip and the fused-single-run + CUDA-graph remain
   **DEFERRED behind one shared blocker** (the `ort` rc.12 public-IoBinding in-place `past==present` alias limit).
2. **ORT accel catalog (Phase 7, on disk):** **offline fuse+serialize** (lever 1), the **SDPA-pin** (lever 3),
   and the **conv knobs** (lever 5) are implemented, env-gated, GB10-scoped, each **bit-identity-gated**. The
   measured **process-wide-safe set (`WAAV_ORT_SDPA` + `WAAV_ORT_CONV_ALGO`)** delivers a real wall-clock win on
   the STT/flow cohorts (**whisper −38%, supertonic flow −79%**) at **zero accuracy cost**; the per-graph-unsafe
   knobs **clean-reject** rather than corrupt.

This final-regression run **re-verified every gate green** (full workspace ×2 feature configs + the live
byte-identity gates: dia2 544/544 + 608/608, csm, `host_vs_device_kv_oracle`, the Phase-7 SDPA/conv/fuse gates,
the Phase 0–6 device-KV set, the production host-KV path). The single documented RED
(`f16_device_kv_codes_identical_to_host_kv_ragged`) stays `#[ignore]`'d off the merge-gate list — the **expected
intrinsic F16 divergence**, not a regression.

---

## 2. FINAL regression — what is GREEN (verified THIS run)

### 2.1 Build / lint — both feature configs (forced recompile of all 5 touched crates)

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` (default) | **GREEN** (full recompile after `touch`) |
| `cargo clippy --workspace --all-targets --features torch -- -D warnings` | **GREEN** (full recompile after `touch`) |

### 2.2 Deterministic workspace suites (`cargo test --workspace -- --test-threads=1`)

| Suite | passed | failed | ignored | exit |
|---|---|---|---|---|
| default features | **1184** | **0** | 161 | 0 |
| `--features torch` | **1184** | **0** | 180 | 0 |

(The +19 `ignored` under `--features torch` are the torch-only live-GPU gates; all `#[ignore]`'d live-GPU gates
are run separately in §2.3. Counts grew vs the Phase-5/6 status's 1175 by the **+5 Phase-7-lever-1 `opt_cache`
unit tests** [all GREEN: `opt_cache_off_by_default`, `opt_cache_cold_miss_serializes_and_is_ep_keyed`,
`opt_cache_hit_loads_cache`, `opt_cache_roundtrip_bit_identical`, `opt_cache_does_not_recache_a_cache`] plus the
Phase-7 deterministic conv/sdpa parser/reject twins.)

### 2.3 Live byte-identity gates (CUDA, process-isolated, ONE model set at a time)

| Gate | Crate / target | Result | Wall |
|---|---|---|---|
| `device_kv::device_ping_pong_two_buffer_bit_identical_to_host_run` (Phase-2 GROWING export) | backend-ort `--lib` | **GREEN** | 3.4 s |
| `device_kv::static_export_device_kv_bit_identical_to_host_run` (Phase-4.0 STATIC fp32 share) | backend-ort `--lib` | **GREEN** | 4.8 s |
| `tts::chatterbox::tests::host_vs_device_kv_oracle` (**THE Phase-4 fp32 oracle**, ragged mid-finish) | core `--lib` | **GREEN** | 83.1 s |
| `tts::chatterbox::tests::sdpa_pin_codes_identical_or_clean_reject` (Phase-7 lever 3) | core `--lib` | **GREEN** | 76.6 s |
| `tts::chatterbox::tests::conv_pin_codes_identical` (Phase-7 lever 5, fuse_conv_bias+conv_algo) | core `--lib` | **GREEN** | 78.6 s |
| `tts::chatterbox::tests::prefer_nhwc_rejected_on_chatterbox_gqa` (Phase-7 lever 5, clean-reject) | core `--lib` | **GREEN** | 83.2 s |
| `tts::supertonic::tests::supertonic_flow_maxdelta_zero_under_sdpa_and_conv_flags` (Phase-7 all-arch) | core `--lib` | **GREEN** | 2.6 s |
| `whisper_transcript_identical_under_sdpa_and_conv_flags` (Phase-7 STT all-arch) | server `--test perf_bench` | **GREEN** | 8.4 s |
| `tts::chatterbox::tests::live_ragged_batched_forward_bit_identical_and_scales` (**PRODUCTION host-KV path**) | core `--lib` | **GREEN** | 311.6 s |
| **dia2** `cpu_fp32_codes_byte_identical` (**544/544**) | backend-torch `--test cuda_torch_dia2` | **GREEN** | (in 67.8 s set) |
| **dia2** `cuda_bf16_codes_byte_identical` (**608/608**, B25 cross-precision LAW) | backend-torch | **GREEN** | 67.8 s set |
| **dia2** `cuda_torch_dia2` (envelope / ASR sidecar parity) | backend-torch | **GREEN** | 67.8 s set |
| **csm** `cuda_csm_codes_byte_identical_to_sidecar` (dual-AR greedy, all frames × codebooks) | backend-torch `--test cuda_torch_csm` | **GREEN** | (in 47.6 s set) |
| **csm** `cuda_csm_rtf` | backend-torch | **GREEN** | 47.6 s set |

Memory stayed bounded (20–32 GiB free) across every process-isolated gate; no OOM, no box-kill.

### 2.4 The Phase-5 NO-GO witness gate (expected RED — NOT a regression, NOT re-run as a green gate)

| Gate | Result | Status |
|---|---|---|
| `f16_device_kv_codes_identical_to_host_kv_ragged` (q4f16/F16-KV cell oracle) | **RED (expected)** | `#[ignore]`'d with explicit NO-GO attr; off the merge-gate list |

The gate carries `#[ignore = "PHASE-5 NO-GO PROBE: known-RED on GB10 (F16 padded-static vs exact-growing GQA
reduction divergence, KV-ACCEL §7) — run manually to reproduce; NOT a green gate"]`. It was **NOT** re-run as a
green gate this pass (re-running it would only re-reproduce the documented intrinsic F16 divergence). The fp32
device cell at the same depth is byte-identical (the `host_vs_device_kv_oracle` GREEN above), proving the device
ring carry-forward is sound and the divergence is purely the F16 non-associative reduction over the
MAX_SEQ-padded static buffer vs the exact-length growing buffer.

---

## 3. DELIVERED (shipped scope, byte-identical, no regression)

### 3.1 fp32 device-resident KV ring — 2.34× @ B24, byte-identical (Phase 0–6, committed `073620d`)

- **What:** a SlotId-keyed, statically-allocated-once device KV ring (`KvResidencyRegime::DeviceKvOrt`,
  `run_device_kv` backend-ort sibling, the static `past_present_share_buffer` export `language_model_share.onnx`,
  prefill→device scatter, recycle-zero, advance-on-Ok mid-batch-reject safety). Gated to **exactly one cell:**
  `(chatterbox, static-share export, is_gb10, fp32)`.
- **Proven:** byte-identical (max|Δ|=0 on decoded codes) across the 24-cell accuracy × perf × limits matrix
  (B∈{1,2,4,8,16,24} × {SHORT/MEDIUM/LONG/EXTREME}) **and** the ragged mid-finish `host_vs_device_kv_oracle`
  vs a `force_host_kv` reference. Re-verified GREEN this run.
- **Measured perf (the host-KV re-stream-elimination win):** device/host per-tick ratio
  0.93×@B1 → 1.05×@B2 → 1.0×@B4 → 0.84×@B8 → 0.58×@B16 → **0.43×@B24 (= 2.34× faster, 1494ms→638ms)**.
  Crossover knee ≈ B8; gap still widening at B24. The mechanism: the host path rebuilds + re-streams a fresh
  `[B,H,max_past,D]` KV buffer per layer per stride (cost ∝ B²); the device-resident SlotId ring carries KV
  across strides with no host bounce.
- **Limits (re-confirmed by the matrix gate):** over-MAX_SEQ prefill **clean-rejects** typed
  (`HostSync "exceeds the static ring MAX_SEQ 1024"`, retriable:false, no OOM); admission byte-budget shrinks an
  over-large `MAX_SLOTS=4096`→`admitted=204` cleanly (240 MiB/slot F32 under the 48 GiB arena cap).

### 3.2 Phase-7 accel catalog — measured gains, bit-identical (on disk)

**Lever 1 — offline fuse + serialize (`eval/make_optimized_onnx.py` + `backend-ort` `with_optimized_model_path`
opt-cache).** Implemented + bit-identical:
- **whisper-base** `encoder_model.onnx`: **727 → 290 nodes (−60%)**; fused-encoder transcript **token-for-token
  identical** on CPU+CUDA (producer gate + live CLI); steady-state **~1.04× (3.5%)**. The merged-decoder is left
  un-fused (its `If` control-flow blocks the transformer optimizer). **Shipped:** whisper `waav.json` encoder is
  repointed to `onnx/encoder_model.opt.onnx` (verified on disk).
- **chatterbox** `language_model.onnx`: **340 → 308 nodes** (already exporter-fused GQA/SkipSimplifiedLN; the
  pass adds QuickGelu ×30 / SiLU→QuickGelu −32 nodes), fused codes byte-identical (17-stride argmax).
- **Cold-start serialization** (`WAAV_ORT_OPT_CACHE`, EP+mtime-keyed, beside-model): warm-loads correctly +
  bit-identical (the `opt_cache_roundtrip_bit_identical` unit gate GREEN), but **NO measurable cold-start win**
  on chatterbox/whisper (~32–37 s load unchanged) — their load bottleneck is **external-data I/O + constant-fold,
  not graph re-fusion**, and folded-initializer persistence corrupts shape-inference-bound constants so that key
  is intentionally not set. Honest finding: the structural node-count win is real; the cold-start amortization
  the catalog hypothesized does NOT materialize on these two graphs.

**Lever 3 — SDPA-pin (`WAAV_ORT_SDPA`, env-gated, GB10-scoped, default OFF).** `ep.rs` `provider()` CUDA arm →
`with_attention_backend`. Pins ONLY ORT's safe cuDNN-flash | efficient | flash trio; **TRT-fused / FlashInfer
tokens are explicitly rejected** (the `sm_12x` ban). LIVE byte-identical: chatterbox codec-AR codes identical
under the pin alone (81 frames), supertonic flow maxΔ=0, whisper transcript token-identical; contract also
accepts a clean typed reject. The contract is correctness-safety + the warmup win below — at the whole-graph
chatterbox level the unpinned default already auto-selected a comparable kernel (the INFER_PERF 40–135× is a
per-attention-op micro-bench, not a whole-graph multiplier).

**Lever 5 — conv knobs (per-knob env flags, GB10-scoped, default OFF).** Split by per-knob safety:
- `WAAV_ORT_CONV_ALGO=heuristic` (`with_conv_algorithm_search`) = **PROCESS-WIDE-SAFE**, byte-identical on
  chatterbox+supertonic+whisper (selects only among math-equivalent algos).
- `WAAV_ORT_FUSE_CONV_BIAS` = **PER-GRAPH-ONLY**: byte-identical on chatterbox (81 frames) but BREAKS supertonic
  convnext pwconv (`cudnnAddTensor CUDNN_STATUS_BAD_PARAM`) → clean-reject.
- `WAAV_ORT_PREFER_NHWC` = **PER-GRAPH-ONLY**: BREAKS chatterbox GQA growing-KV (`seqlens_k out of range [0,0)`)
  AND supertonic text_encoder attention Reshape → **CLEAN-rejects, never silently wrong**
  (`prefer_nhwc_rejected_on_chatterbox_gqa` gate).
- `do_copy_in_default_stream` stays **DELETED** (removed from `ort` rc.12 — the input design's tombstone).

**Net process-wide-safe set = SDPA pin + conv_algo, measured live on GB10:**
- **whisper STT transcript 1207.7 ms → 745.3 ms (−38.3%, token-identical).**
- **supertonic flow 0.579 s → 0.124 s (−78.6%, maxΔ=0).**
- chatterbox codec-AR per-stride: SDPA pin ~flat (default already dispatched a comparable fused-attn backend);
  fuse_conv_bias+conv_algo −1.4% per stride.
The big STT/flow wins come from HEURISTIC cutting the EXHAUSTIVE cuDNN warmup search — accuracy-neutral, real,
and re-verified GREEN this run on all four touched archs (chatterbox codes, supertonic flow, whisper transcript).

---

## 4. DEFERRED + WHY (all behind ONE shared blocker: the `ort` rc.12 public-IoBinding in-place-alias limit)

The single root blocker for the next tier is precise and verified: **`ort` rc.12's PUBLIC `IoBinding` cannot
alias one `Value` to both a `past.*` input and a `present.*` output in the SAME run** — `bind_input` borrows
`&Value`, `bind_output` consumes an owned `Value`, and the in-place-alias APIs are `pub(crate)`. Consequences:

### 4.1 The q4f16 / F16-KV device-default flip — DEFERRED (NO-GO)

- **Why:** the F16 CUDA-GQA reductions over the **MAX_SEQ-padded static buffer differ from the exact-length
  growing buffer** (a non-associative-F16 reduction-order effect; the fp32 cell at the same depth is
  byte-identical, so it is intrinsic to F16, not a wiring bug). The CPU-EP graph-level growing-vs-static-q4f16 is
  byte-identical, confirming the graphs are equivalent; only the CUDA F16 reduction tree diverges.
- **Witness:** `f16_device_kv_codes_identical_to_host_kv_ragged` RED at the longest slot (slots 0–2 identical).
- **Precise unblock (any ONE):** (a) **exact-length / bucketed device buffers** (non-MAX_SEQ-padded) so the F16
  GQA reduction tree matches the growing graph — this ALSO kills the §5 padded-attention tax; OR (b) an
  **fp32-KV static device export** the q4f16 weights ride; OR (c) **pinning the GQA backend** to the same F16
  reduction tree. Re-greens the gate → flip the one-line `waav.json` `device_kv.share` key.
- **The seam is READY:** the q4f16 static-share artifact (CPU-EP byte-identical), the CUDA-only + file-gated
  resolver (`weight_path_device_kv`, deterministic gate GREEN), and the F16 ring machinery all ship behind the
  resolver; production `waav.json` intentionally carries **no `device_kv` key** (verified: keys
  `architecture / _comment / weights`, `language_model = {precision: q4f16}`) → production stays on the proven
  growing-q4f16 HostKv path.

### 4.2 The fused single-run + CUDA-graph (the ~30× hypothesis) — DEFERRED (RETIRED in Phase 6)

- **Why no win:** the fused B-row single run was proven byte-identical but delivered **NO reliable perf gain**
  over the Phase-4 per-slot device path (re-measured ~2.0–2.26×@B24, straddling the per-slot 2.34× in the GB10
  noise band). On the across-run static-share ping-pong the static export **re-allocates a fresh `present.*`
  device slab every stride** (the same alias limit), so the single fused launch saves only B−1 kernel launches
  per tick — swamped by the per-stride present realloc + the MAX_SEQ-padded GQA compute. The genuine
  device-resident win is the eliminated host KV re-stream, already captured by the per-slot path; fusion adds
  nothing on top.
- **Why CUDA-graph is blocked:** `enable_cuda_graph` requires fixed input/output **ADDRESSES**; the
  re-allocating `present.*` changes the output address run-to-run (fixed SHAPES are met, fixed ADDRESSES are
  not). True capture needs the same-run in-place `past==present` alias — the same `pub(crate)` limit.
- **The ~30×@B64 plan hypothesis is FALSE on this ORT static-share export.** It presumed the tch
  single-fused-batched-run with a same-run in-place `past==present` alias (zero per-stride realloc + one launch +
  CUDA-graph). Without the public alias, both fused and per-slot are capped by the SAME limiter.
- **Precise unblock:** a **same-run in-place `past==present` persistent fixed-address binding** — either a
  **vendored / `pub(crate)`-elevated `ort` alias** (an ort-fork/patch) OR an **output-reuse refactor** that
  registers a persistent fixed-address binding. This is the SINGLE prerequisite that both restores the fused run
  as a real win AND satisfies CUDA-graph's fixed-address precondition — the only path toward the ~30× hypothesis.

### 4.3 The MAX_SEQ-padded-attention tax (the capping limiter) — DEFERRED

The static MAX_SEQ=1024-padded buffer forces the GQA to attend the FULL MAX_SEQ per row regardless of true
context (~200 at MEDIUM) — a fixed ~5× padded-attention tax that the device-residency saving only outweighs once
the host path's super-linear re-stream dominates (high B). The fix is the **same exact-length / bucketed device
buffers** as §4.1 — it both kills the tax and re-greens the F16 cell. Deferred behind the same alias limit.

### 4.4 Lever-1 cold-start serialization win — DEFERRED (does not materialize on these graphs)

The opt-cache warm-loads bit-identically but yields no cold-start win on chatterbox/whisper (load is
external-data-I/O-bound, not fusion-bound). A genuine cold-start win would need to amortize the external-data
load itself, which the transformer-optimizer serialization does not address. Kept off-by-default; the structural
fused export remains the deliverable.

---

## 5. The honest end-to-end perf picture (decoder-level proven vs serve-loop NOT realized)

The brutal honest truth, separating decoder-test scope from live serve-loop scope:

- **At the decoder-test level (the Phase-4 oracle / 24-cell matrix):** the per-slot device-KV path is
  byte-identical and **2.34× faster than host-KV at B24** (crossover B≈8). This is the proven, shipped lever.
- **At the live serve-loop level:** the intended concurrency throughput scaling was **NOT realized
  end-to-end** through the multiplexed batched device serve path. The full-system probe found: single SHORT
  streams succeed (RTF ~0.66, byte-identical, transcribe-exact); but the multiplexed batched device branch
  **does not advance >1 slot** (n=2/4 hang → 30 s watchdog shed; n=8 yields empty 44-byte WAVs; n≥16 crashes the
  single `codec-ar-mux` thread → all subsequent codec-AR requests 500 until restart). The over-MAX_SEQ single
  stream **clean-rejects** typed (§4.7 guard, no OOM, loop survives), but the broader batched-concurrency limit
  behavior is **NOT clean** (a hard availability failure, not a graceful shed).
  **Therefore: the 2.34×@B24 stands ONLY at the decoder-test level; it is NOT yet realized through the live
  serve loop.** Production wiring of the device branch through `step_slots_batched` on `(chatterbox, CUDA,
  is_gb10)` is the remaining integration step — it is gated behind restoring the q4f16 default (§4.1) and is the
  honest reason the device path is NOT flipped on in production today. Production runs the proven host-KV path
  (re-verified GREEN, §2.3), which serves correctly at all batch widths.
- **The Phase-7 accel catalog DOES land real whole-system wins TODAY, accuracy-neutral:** the process-wide-safe
  `WAAV_ORT_SDPA` + `WAAV_ORT_CONV_ALGO` set cuts whisper STT −38% and supertonic flow −79% (warmup-search
  change), with the conv per-graph-unsafe knobs clean-rejecting. These are env-gated + default OFF, ready to
  enable per-deployment after the bit-gates (already GREEN).

**Net:** the device-KV program delivers a **proven, byte-identical 2.34×@B24 decoder lever** (fp32 cell) and a
**ready-but-gated** serve integration; the **accel catalog delivers real, accuracy-neutral STT/flow wins now**.
The two highest-value next steps (q4f16 device default + the ~30× fused/CUDA-graph) are blocked by a single,
precisely-identified `ort` rc.12 public-API limitation whose unblock is an **ort-fork/patch (or an exact-length /
non-padded device-buffer refactor)** — both well-scoped, neither requiring any accuracy compromise.

---

## 6. Files (Phase-7, on disk, NOT committed; no `cargo fmt`)

**Modified (6, uncommitted vs HEAD `073620d`):**
- `crates/waav-infer-backend-ort/src/ep.rs` — Phase-7 lever 3 (`sdpa_backend_pin`, GB10-scoped, sm_12x ban) +
  lever 5 (`conv_knobs`: prefer_nhwc / fuse_conv_bias / conv_algo, per-knob env-gated) wired into the CUDA EP
  `provider()` arm; deterministic parser/reject tests.
- `crates/waav-infer-backend-ort/src/lib.rs` — Phase-7 lever 1 opt-cache (`with_optimized_model_path`,
  `WAAV_ORT_OPT_CACHE`, EP+mtime-keyed beside-model) + the 5 `opt_cache_*` unit gates.
- `crates/waav-infer-core/src/tts/chatterbox.rs` — Phase-7 chatterbox bit-gates
  (`sdpa_pin_codes_identical_or_clean_reject`, `conv_pin_codes_identical`, `prefer_nhwc_rejected_on_chatterbox_gqa`).
- `crates/waav-infer-core/src/tts/supertonic.rs` — Phase-7 all-arch gate
  (`supertonic_flow_maxdelta_zero_under_sdpa_and_conv_flags`).
- `crates/waav-infer-server/tests/perf_bench.rs` — Phase-7 STT all-arch gate
  (`whisper_transcript_identical_under_sdpa_and_conv_flags`).
- `ci/heavy_live_tests.sh` — registers the 5 Phase-7 live gates with the process-wide-caveat doc.

**New (untracked):**
- `eval/make_optimized_onnx.py` — the offline fuse+serialize producer (transformer-optimizer O2, fp16 OFF).
- `ci/phase_c_model_sweep.sh`, `docs/` — auxiliary.

**Artifacts on disk:**
- `~/.cache/waav-models/whisper-base-onnx/onnx/encoder_model.opt.onnx` (Phase-7 fused encoder, 727→290 nodes) —
  whisper `waav.json` encoder repointed to it (verified).
- `~/.cache/waav-models/chatterbox-onnx/onnx/language_model.opt.onnx` (Phase-7 fused LM, 340→308 nodes).
- `~/.cache/waav-models/chatterbox-onnx/onnx/language_model_share.onnx` (Phase-4 fp32 static-share, the proven
  byte-identical device-KV cell) — unchanged.
- `~/.cache/waav-models/chatterbox-onnx/onnx/language_model_q4f16_share.onnx` (Phase-5 q4f16 static-share + F16
  KV) — the NO-GO cell, ready behind the resolver.

**Production `waav.json`** (chatterbox): untouched — `language_model = {precision: q4f16}`, **no `device_kv`
key** → production stays on the proven growing-q4f16 HostKv path.

**Regression logs (this run):** `scratchpad/p7_regr_default.log`, `p7_regr_torch.log`,
`p7_tts__chatterbox__tests__host_vs_device_kv_oracle.log`, `p7_*sdpa*/`p7_*conv*/`p7_*prefer_nhwc*`,
`p7_supertonic_flow.log`, `p7_whisper_sdpa.log`, `p7_device_kv__*.log`, `p7_dia2.log`, `p7_csm.log`,
`p7_prod_ragged.log` (session scratchpad).

---

## 7. Recommended next steps (sequenced, both behind the same single unblock)

1. **Land the same-run in-place `past==present` persistent fixed-address binding** — a vendored /
   `pub(crate)`-elevated `ort` alias OR an output-reuse refactor. This SINGLE change simultaneously: (a) unblocks
   exact-length / bucketed device buffers → re-greens `f16_device_kv_codes_identical_to_host_kv_ragged` → flip
   the one-line `waav.json` `device_kv.share` key (q4f16 device default); (b) kills the MAX_SEQ-padded-attention
   tax; (c) gives the fused run a realloc-free win; (d) satisfies CUDA-graph's fixed-address precondition (the
   only path toward the ~30× hypothesis).
2. **Wire + harden the production serve path** to route `step_slots_batched`'s device branch through the ring on
   `(chatterbox, CUDA, is_gb10)`, fixing the multiplexed batched-device serve-loop failures (hang/empty/crash at
   n≥2) so the proven decoder-level 2.34×@B24 is realized end-to-end with graceful shed, not a `codec-ar-mux`
   thread crash.
3. **Enable the process-wide-safe accel set per deployment** (`WAAV_ORT_SDPA` + `WAAV_ORT_CONV_ALGO`) — the
   STT/flow −38%/−79% wins are bit-gated GREEN and ready; keep the per-graph-unsafe conv knobs (prefer_nhwc /
   fuse_conv_bias) off process-wide (they clean-reject, never corrupt).
