# KV-ACCEL Device-Resident-KV — Phase 5–6 FINAL Regression + Status

**Date:** 2026-06-25. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified pool), aarch64.
**Tree:** `waav-infer` @ HEAD `66ed925` (the Phase 0–4 commit) + the on-disk **uncommitted** Phase 5–6 work
(5 modified files; left on disk, NOT committed; no `cargo fmt`, per the coordinator hand-off discipline).
**Plan followed:** `WaaV/inferv2/REVIEW/KV-ACCEL-INTEGRATION-PLAN.md` (§3.2, §6, §7-lever-3/4, §9).
Predecessor status: `KV-IMPL-PHASE4-STATUS.md` (Phase 0–4 = COMPLETE + PROVEN).
**Env:** `source gb10-env.sh` (ORT 1.27 CUDA EP, `ort` rc.12, `CARGO_BUILD_JOBS=6`, 48 GiB arena cap). Live
gates run process-isolated `--test-threads=1`, ONE model set at a time (GB10 OOM history). `free -g` checked
before each live gate (≥35 GiB free throughout).

---

## 1. Headline verdict

**Workspace is GREEN. No regression. The KV-ACCEL device-KV work SHIPS at its proven scope.**

- **Phase 5 (q4f16 / F16-KV cell) = implemented, NO-GO on the default flip.** The seam machinery is in place and
  the deterministic/artifact gates pass, but the live F16 ragged oracle
  (`f16_device_kv_codes_identical_to_host_kv_ragged`) is the documented **RED divergence witness**: an intrinsic
  non-associative-F16 CUDA-GQA padded-static-vs-exact-growing reduction-order effect at the longest slot. Per the
  plan §7 NO-GO trigger, the q4f16 device cell is **NOT** flipped as the default — production stays on the proven
  GROWING-q4f16 HostKv path (`waav.json` carries no `device_kv` key). This RED is **expected and correct**, not a
  regression: the gate is `#[ignore]`'d and removed from the green merge-gate list with a NO-GO comment.
- **Phase 6 (fused batched device-KV run + CUDA-graph) = built, proven byte-identical, then RETIRED.** The fused
  "B rows, one run" path was implemented and shown byte-identical, but it delivered **NO reliable perf win** over
  the Phase-4 per-slot device-KV path (re-measured fused-vs-host straddled the per-slot 2.34×@B24 in the GB10
  noise band), and CUDA-graph capture is **blocked** by the ort rc.12 public-API alias limit (the across-run
  static-share ping-pong re-allocates `present.*` at a fresh device address each stride → the cuda-graph
  fixed-ADDRESS precondition is unmet). The fused dead path + the `WAAV_ORT_CUDA_GRAPH` env knob were **removed
  from disk**; explanatory NO-GO comments remain in `chatterbox.rs`, `ep.rs`, and `heavy_live_tests.sh`.

**The delivered, shipped lever is unchanged from Phase 4: the per-slot device-resident KV ring (fp32 cell),
byte-identical, measured 2.34×@B24 — the host-KV re-stream-elimination win.** Phase 5–6 explored the two
candidate accelerations beyond it (the F16 default cell, the fused single-run + CUDA-graph) and converged on:
both are **deferred behind the same ort rc.12 public-IoBinding alias limitation** (no same-run in-place
`past==present`), and the F16 cell additionally needs exact-length (non-MAX_SEQ-padded) device buffers to kill
the padded-reduction divergence.

> **NOTE — divergence from the Phase 5/6 hand-off draft.** An earlier hand-off draft described the Phase-6 fused
> gates (`fused_batched_bit_identical_to_per_slot`, `cuda_graph_fused_replays_bit_identical`) and the
> `WAAV_ORT_CUDA_GRAPH` env knob as live on disk. They are **NOT** on disk in the regressed tree: after the
> re-measurement proved the fused path adds nothing over per-slot and CUDA-graph is API-blocked, the fused code
> path was **retired** (no production wiring, no win), leaving only the documented NO-GO comments. This status
> reflects the **actual on-disk state**, which is the converged outcome of those Phase-6 experiments.

---

## 2. FINAL regression — what is GREEN (verified this run)

### 2.1 Build / lint (both feature configs)

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -D warnings` (default features) | **GREEN** |
| `cargo clippy --workspace --all-targets --features torch -D warnings` | **GREEN** (force-rechecked after a `touch`) |

### 2.2 Deterministic workspace suites (`cargo test --workspace -- --test-threads=1`)

| Suite | passed | failed | ignored | exit |
|---|---|---|---|---|
| default features | **1175** | **0** | 156 | 0 |
| `--features torch` | **1175** | **0** | 175 | 0 |

(The extra `ignored` under `--features torch` are the torch-only live-GPU gates absent from the default build.
All `#[ignore]`'d live-GPU gates run separately below.)

**Deterministic Phase-0 + Phase-5 device-KV gates** (re-confirmed individually, GREEN):
- core (9): `arm_prefix_cache_declines_under_device_kv_ort`, `bf16_kv_graph_stays_hostkv_or_hard_errors`
  (the Phase-5 §6 GO-cond.3 — bf16/Other KV stays HostKv, F16 still enables the path),
  `device_kv_decoder_routes_through_device_seam_and_recycles`, `device_kv_step_disabled_on_cpu_and_rocm`,
  `device_kv_value_dtype_equals_graph_input_type`, `each_codec_ar_arch_default_regime_is_hostkv`,
  `kv_residency_regime_is_hostkv_for_f16_until_proven`, `kv_residency_regime_is_hostkv_off_cuda`,
  `prefix_cache_arm_decided_by_kv_residency_regime`.
- core model resolver (1): `model::tests::device_kv_share_selected_only_on_cuda_and_when_present` — the
  **Phase-5 SHIPPED-DEFAULT seam**: `weight_path_device_kv` selects the CUDA-only static-share variant ONLY when
  `device_kv.share[language_model]` is declared AND EP is CUDA AND the file exists; else the growing graph. Full
  truth table asserted (CUDA+present⇒variant, CUDA+absent⇒growing, CPU⇒growing, no-key⇒back-compat identical).
- backend-ort (1): `device_kv::device_kv_ring_is_send_not_sync` (the Send-not-Sync static assert).
- backend-api (2): `device_kv_spec_constructs_and_round_trips`, `device_kv_spec_ignored_by_host_backend_bit_identical`.

### 2.3 Live byte-identity gates (CUDA, process-isolated, ONE model set at a time)

| Gate | Result | Wall |
|---|---|---|
| backend-ort `device_kv::device_ping_pong_two_buffer_bit_identical_to_host_run` (Phase-2 GROWING export) | **GREEN** | (in 7.3 s pair) |
| backend-ort `device_kv::static_export_device_kv_bit_identical_to_host_run` (Phase-4.0 STATIC fp32 share export) | **GREEN** | 7.3 s pair |
| core `host_vs_device_kv_oracle` (Phase-4 fp32 device-KV vs `force_host_kv` ref, ragged mid-finish) | **GREEN** | 82.7 s |
| core `device_kv_accuracy_perf_matrix` (the 24-cell accuracy × perf × limits matrix, fp32 cell) | **GREEN** | 729.9 s |
| core `live_ragged_batched_forward_bit_identical_and_scales` (**PRODUCTION host-KV path**, ragged) | **GREEN** | 310.4 s |
| **dia2** `cpu_fp32_codes_byte_identical` (**544/544**) | **GREEN** | (in 67.4 s set) |
| **dia2** `cuda_bf16_codes_byte_identical` (**608/608**, the B25 cross-precision LAW) | **GREEN** | 67.4 s set |
| **dia2** `cuda_torch_dia2` (envelope/ASR sidecar parity) | **GREEN** | 67.4 s set |
| **csm** `cuda_csm_codes_byte_identical_to_sidecar` (dual-AR greedy, all frames × codebooks) | **GREEN** | (in 48.4 s set) |
| **csm** `cuda_csm_rtf` | **GREEN** | 48.4 s set |
| **q4f16 artifact** `q4f16_share_export_decodes_identical_to_growing_q4f16` (`eval/make_chatterbox_static_kv.py --verify`, CPU EP) | **GREEN** | — |

The q4f16 artifact verify (CPU EP, dtype-aware): `KV cache dtype = float16`, growing codes `[426×9]` ==
static codes `[426×9]`, **BYTE-IDENTICAL ✓**. This is the Phase-5 GO-cond.1 — the producer-form static export
is sound; the F16 divergence below is purely a CUDA-GQA padded-reduction effect, NOT a graph/wiring bug.

### 2.4 The Phase-5 NO-GO witness gate (expected RED — NOT a regression)

| Gate | Result | Detail |
|---|---|---|
| core `f16_device_kv_codes_identical_to_host_kv_ragged` (the q4f16/F16-KV cell oracle, `#[ignore]`'d) | **RED (expected)** | slot 3 (the LONGEST slot) diverges; slots 0–2 byte-identical |

Reproduced live: `panicked at chatterbox.rs:6858 … slot 3: q4f16 DEVICE-KV (F16 ring) codes MUST be
byte-identical …`. The longest slot shares the first ~32 codes then splits — the **intrinsic
non-associative-F16 CUDA-GQA reduction over the MAX_SEQ-padded static buffer vs the exact-length growing
buffer** (§6 — F16 has no lossless widening safety net; the fp32 cell at the SAME depth is byte-identical, so
the device ring carry-forward is sound; only F16 diverges). This gate is the deliberately-kept RED witness:
`#[ignore]`'d, off the green merge-gate list, with a NO-GO comment in `heavy_live_tests.sh`. It re-greens only
once Phase 6 makes the F16 cell byte-identical (an fp32-KV static device export, OR exact-length / non-padded
device buffers, OR a pinned GQA backend that takes the same F16 reduction tree).

---

## 3. Phase 5 — q4f16 / F16-KV device-KV cell

**STATE: implemented; NO-GO on the default flip (per plan §7); production stays on growing-q4f16 HostKv.**

### 3.1 What was built (all on disk, uncommitted)

1. **The q4f16 static-share artifact** `~/.cache/waav-models/chatterbox-onnx/onnx/language_model_q4f16_share.onnx`
   — the static `past_present_share_buffer` variant of the q4f16 export (MatMulNBits weights + **F16** KV,
   MAX_SEQ=1024; seq axis −1→1024 on all 60 past/present KV tensors; external data reused in place from
   `language_model_q4f16.onnx_data`, no repack). Produced via the **dtype-aware** `eval/make_chatterbox_static_kv.py`
   (its `--verify` now reads the KV dtype from the growing graph → F16 for q4f16, F32 for fp32 — graph-driven,
   never guessed). **GO-cond.1 GREEN** (CPU-EP plain-`run()` bit-identity, §2.3 / §2.4).
2. **The SHIPPED-DEFAULT EP-conditional resolver seam** (`crates/waav-infer-core/src/model.rs`):
   `Manifest.device_kv_share` + `weight_path_device_kv(dir, logical, stem, ep_is_cuda)` (CUDA-only + file-gated;
   falls back to the growing graph off-CUDA or when absent), wired into the chatterbox factory by probing the
   already-loaded `speech_encoder`'s `active_ep()`. Deterministic gate
   `device_kv_share_selected_only_on_cuda_and_when_present` GREEN. **Enabling the default is a one-line
   `waav.json` change** (`{"device_kv":{"share":{"language_model":"onnx/language_model_q4f16_share.onnx"}}}`)
   once the F16 cell is byte-identical.
3. **The F16 backend machinery** (already present from Phase 4's per-input-name dtype seam): F16 ring via
   `empty_kv_dtype_for`, F16 `seed_static_kv_row` scatter, mixed-dtype per-input bind (`inputs_embeds` F32,
   `attention_mask` I64, KV F16, `logits` F32). All in place and correct — the divergence is intrinsic to the
   padded-static-vs-exact-growing F16 geometry on the CUDA kernel, NOT the wiring.
4. **The deterministic companion gate** `bf16_kv_graph_stays_hostkv_or_hard_errors` GREEN (§6 GO-cond.3:
   Other/bf16 KV stays HostKv; F16 still enables the path).

### 3.2 Why NO-GO (the blocker)

`f16_device_kv_codes_identical_to_host_kv_ragged` FAILS on GB10 CUDA at the longest slot (§2.4). Root cause:
the CUDA GQA kernel's **F16 reductions over the MAX_SEQ-padded static buffer differ from the exact-length
growing buffer** — a non-associative-F16 reduction-order effect, not a wiring bug. Evidence it is intrinsic:
- CPU graph-level growing-vs-static-q4f16 is **byte-identical** for the full prompt (the artifact gate, §2.4).
- The fp32 device cell (`host_vs_device_kv_oracle` + the 24-cell matrix incl. EXTREME ~1000-tok) is
  byte-identical at depth → the device ring carry-forward is sound; F16 simply has no lossless widening net (§6).
- Python onnxruntime on aarch64 is CPU-only (no CUDA wheel), so the CUDA-specific divergence is observable only
  through the Rust `backend-ort` seam — the CPU verify confirms the graphs are equivalent, it cannot reproduce
  the CUDA reduction-order effect.

Per the plan §7 NO-GO trigger (the F16 cell diverges where fp32 did not → do NOT flip the q4f16 device cell):
production `waav.json` intentionally has **no `device_kv` key** (verified: keys = `architecture / _comment /
weights`, `language_model = {precision: q4f16}`), so production stays on the proven GROWING-q4f16 HostKv path.
The fp32 static-share device cell (`language_model_share.onnx`) IS byte-identical and remains the Phase-6
substrate.

---

## 4. Phase 6 — fused batched device-KV run + CUDA-graph (built → RETIRED)

**STATE: built + proven byte-identical, then RETIRED (no production wiring, no perf win); CUDA-graph
API-blocked. Documented NO-GO comments remain; the per-slot device cell stays as the shipped lever.**

### 4.1 The approach (the task's "whole ring / no-gather" sidestep)

Bind the whole active cohort as ONE fixed-shape `[B,H,MAX_SEQ,D]` static-share device run per decode tick — NOT
a same-run device gather, NOT an ort-fork. An empirical probe of the real ORT GroupQueryAttention kernel decided
the exact shape: the kernel **REJECTS batch>1 when `seq_len>1` with past given** ("batch_size must be 1 when
sequence_length > 1 and past context is given"), so PREFILL stays per-slot B=1, and only the DECODE strides
(`seq_len==1`, the throughput-dominant bulk) were to fuse into one run (each batch row attends independently
against its own mask-derived `seqlens_k`).

### 4.2 Why it was RETIRED (the measured truth)

The fused run was **byte-identical** to BOTH the per-slot device run and the provably-HOST reference on a ragged
cohort across decode strides (corroborated by a CPU onnxruntime probe of `language_model_share.onnx`: fused B=N
decode vs solo B=1 = maxabs diff 0.0 even with ragged history). But it delivered **NO reliable perf win** over
the Phase-4 per-slot device path:

| B | per-slot device-vs-host (Phase 4 §3.2) | fused-vs-host (re-measured) | net |
|---|---|---|---|
| 1 | 0.94× | ~0.72–0.74× | both ≤ host (padded-attn tax, no batch benefit) |
| 8 | 0.87× (1.15×) | ~0.74–0.91× | per-slot already crosses; fused still host-favorable |
| 16 | 0.59× (1.69×) | ~1.5–1.63× | both cross; ~same |
| 24 | 0.43× (**2.34×**) | ~2.0–2.26× (straddled 2.34× across runs) | **NO win over per-slot — within GB10 noise** |

**Root cause of the non-win:** on the across-run static-share ping-pong, the static export re-allocates a FRESH
`present.*` device slab every stride (ort rc.12's public IoBinding cannot alias one `Value` to both a `past.*`
input and a `present.*` output in one run — `bind_input` borrows `&Value`, `bind_output` consumes an owned
`Value`; the alias APIs are `pub(crate)`). Without the same-run in-place `past==present` alias, the single fused
launch saves only **B−1 kernel launches per tick** — swamped by the per-stride present realloc + the
MAX_SEQ-padded GQA compute (which scales with B identically for B×(B=1) launches and 1×(B-batch) launch). The
genuine device-resident win is the **eliminated host KV re-stream** (visible from B≈8–16), already captured by
the per-slot path; the fusion adds nothing on top.

### 4.3 CUDA-graph (§7 lever 4) — API-blocked

A `WAAV_ORT_CUDA_GRAPH=1 → CUDAExecutionProvider::with_cuda_graph(true)` knob was prototyped. On this ORT/rc.12
build `enable_cuda_graph` **rejected the fused binding** — the across-run static-share ping-pong allocates a
fresh `present.*` device target each stride, so output ADDRESSES change run-to-run, violating CUDA-graph's
fixed-input/output-ADDRESS precondition (fixed SHAPES are met; fixed ADDRESSES are not). True capture needs the
same-run in-place `past==present` alias (persistent fixed-address binding), which ort rc.12's PUBLIC IoBinding
cannot register (the same `pub(crate)` alias limit Phase 4 deferred). **CUDA-graph capture of the fused step is
not achievable on the public API today.** The env knob + fused gate were retired with the fused path; restore
both only WITH a persistent fixed-address binding (vendored ort alias or an output-reuse refactor).

### 4.4 What was removed vs what remains on disk

- **Removed from disk** (the retired fused dead path): `device_kv_spec_fused` / `device_kv_forward_batched` /
  `build_fused_seed` / `recycle_fused_ring` / `fused_cohort_decode_for_test` + the 2 fused gates
  (`fused_batched_bit_identical_to_per_slot`, `cuda_graph_fused_replays_bit_identical`) + the
  `WAAV_ORT_CUDA_GRAPH` → `with_cuda_graph(true)` env knob.
- **Remains on disk** (documented NO-GO): explanatory comments in `crates/waav-infer-core/src/tts/chatterbox.rs`
  (the `~30×` re-scope + fused-retired rationale, around `lm_forward_batched`), `crates/waav-infer-backend-ort/
  src/ep.rs` (the `enable_cuda_graph` retirement note in the CUDA EP builder), and `ci/heavy_live_tests.sh` (the
  Phase-5 F16 NO-GO note + the Phase-6 fused/cuda-graph retirement note).

---

## 5. FINAL measured perf verdict

**SHIP the per-slot device-resident KV ring (fp32 cell). MEASURED: 2.34× faster than host at B=24, crossover
knee at B≈8, gap still widening. The ~30×@B64 plan hypothesis is FALSE on this ORT static-share export.**

- The delivered lever is the **host-KV re-stream-elimination win**: the host path rebuilds + re-streams a fresh
  `[B,H,max_past,D]` KV buffer per layer per stride (cost growing super-linearly in B — the `chatterbox.rs`
  host wall), while the device-resident SlotId ring carries KV across strides with no host bounce. Crossover at
  B≈8, 2.34×@B24, widening.
- **The ~30×@B64 hypothesis is FALSE here.** It presumed the tch single-fused-batched-run with a same-run
  in-place `past==present` alias (zero per-stride realloc + one launch + CUDA-graph). On the ORT static-share
  export the public-API alias is unavailable, so the fused run re-allocates `present.*` each stride and fuses
  only launches — measured ~2.0–2.26×@B24, the SAME curve as per-slot. Both paths are capped by the SAME
  limiter; fusion adds nothing over per-slot.
- **The capping limiter:** the static MAX_SEQ-padded buffer forces the GQA to attend the FULL MAX_SEQ=1024 per
  row regardless of true context (~200 at MEDIUM) — a fixed ~5× padded-attention tax per row that the
  device-residency saving only outweighs once the host path's super-linear re-stream dominates (high B).
- **The next levers (both deferred behind the same ort rc.12 public-API alias limit):** (a) exact-length /
  bucketed device buffers to kill the MAX_SEQ-padded-attention tax (also fixes the Phase-5 F16 divergence); (b)
  a persistent fixed-address binding (same-run in-place `past==present`) to unlock CUDA-graph + realloc-free
  fusion. Neither is fusion-vs-per-slot.

### 5.1 Limits / extreme behavior (re-confirmed by the matrix gate, GREEN)

- **Over-MAX_SEQ single prefill (>1024 tok):** clean typed reject (`ChatterboxError::HostSync "exceeds the
  static ring MAX_SEQ 1024"`); no OOM, no box-kill.
- **EXTREME bucket (~1000-tok prefill, near MAX_SEQ):** byte-identical on every B 1..24; held the full ring
  depth with no overflow.
- **Admission byte-budget:** an over-large `MAX_SLOTS=4096` SHRANK cleanly to `admitted=204` (per_slot=240 MiB
  F32 under the 48 GiB arena cap); a budget under one slot admits 0 — no runaway alloc.
- **GB10 stability:** no box-kill at any point; resident set bounded; instances reused, slots
  `drop_slot`-recycled (§4.5 zero-on-recycle ring path).

---

## 6. Complete vs deferred

### Complete (shipped scope, byte-identical, no regression)
- Phase 0–4 (committed `66ed925`): the fp32 per-slot device-resident KV decoder — SlotId ring, prefill→device
  scatter, recycle-zero, advance-on-Ok — byte-identical across the 24-cell matrix + the ragged oracle, 2.34×@B24.
- Phase 5 **seam** (on disk): the q4f16 static-share artifact (CPU-EP byte-identical), the SHIPPED-DEFAULT
  EP-conditional resolver (CUDA-only + file-gated, deterministic gate GREEN), the F16 ring machinery, the
  bf16-stays-HostKv guard. Enabling the q4f16 default is a one-line `waav.json` change once the F16 cell greens.
- Phase 6 **byte-identity** (proven before retirement): the fused batched run was byte-identical to per-slot and
  to host on ragged cohorts (Rust + CPU-onnxruntime corroborated).

### Deferred (documented NO-GO / next-tier, all behind the same ort rc.12 public-API alias limit)
- **The q4f16 device default flip** — blocked by the F16 padded-static-vs-exact-growing CUDA-GQA reduction
  divergence (`f16_device_kv_codes_identical_to_host_kv_ragged` RED). Resolution: an fp32-KV static device
  export, OR exact-length (non-MAX_SEQ-padded) device buffers, OR pinning the GQA backend to the same F16
  reduction tree. Re-greens that gate → flip the one-line `waav.json` key.
- **The fused single-run + CUDA-graph** — RETIRED (no win over per-slot; CUDA-graph fixed-address precondition
  unmet by the re-allocating `present.*`). Restore only WITH a same-run in-place `past==present` alias (a
  non-public ort alias or a persistent-binding output-reuse refactor) that gives both the fixed-address
  precondition and the realloc-free win.
- **Production wiring of the device branch** (route `step_slots_batched`'s device branch through the ring on
  CUDA) — gated on restoring the q4f16 default; the fp32 cell is proven and ready behind the resolver seam.

---

## 7. Files (all on disk, NOT committed; no `cargo fmt`)

**Modified (5, uncommitted vs HEAD `66ed925`):**
- `crates/waav-infer-core/src/model.rs` — Phase-5 SHIPPED-DEFAULT resolver: `Manifest.device_kv_share` +
  `weight_path_device_kv` (CUDA-only + file-gated) + the chatterbox factory EP-probe wiring + the deterministic
  gate `device_kv_share_selected_only_on_cuda_and_when_present`.
- `crates/waav-infer-core/src/tts/chatterbox.rs` — Phase-5 F16 cell: `f16_device_kv_codes_identical_to_host_kv_
  ragged` (the `#[ignore]`'d RED witness) + `bf16_kv_graph_stays_hostkv_or_hard_errors` (deterministic) + the
  Phase-6 fused-retired / `~30×`-re-scope NO-GO comment block.
- `crates/waav-infer-backend-ort/src/ep.rs` — the `enable_cuda_graph` retirement note in the CUDA EP builder
  (the env knob was prototyped then dropped with the fused path).
- `eval/make_chatterbox_static_kv.py` — made `--verify` dtype-aware (reads KV dtype from the growing graph →
  F16 for q4f16, F32 for fp32) so the q4f16 artifact is bit-verified with F16 KV (the
  `q4f16_share_export_decodes_identical_to_growing_q4f16` gate).
- `ci/heavy_live_tests.sh` — the Phase-5 F16 NO-GO note + the Phase-6 fused/cuda-graph retirement note (no new
  green gates added; the F16 gate is deliberately OFF the merge-gate list).

**Artifacts:**
- `~/.cache/waav-models/chatterbox-onnx/onnx/language_model_q4f16_share.onnx` (Phase-5 static-share q4f16 + F16
  KV, MAX_SEQ=1024; external data reused in place from `language_model_q4f16.onnx_data`).
- `~/.cache/waav-models/chatterbox-onnx/onnx/language_model_share.onnx` (Phase-4 fp32 static-share; the proven
  byte-identical cell, the Phase-6 substrate) — unchanged.

**Production `waav.json`:** untouched — keys `architecture / _comment / weights`,
`language_model = {precision: q4f16}`, **no `device_kv` key** → production stays on the proven growing-q4f16
HostKv path.

**Regression logs (this run):** `scratchpad/p56_regr_default.log`, `scratchpad/p56_regr_torch.log`,
`scratchpad/p56_perf_matrix.log`, `scratchpad/p56_dia2.log`, `scratchpad/p56_csm.log`,
`scratchpad/p56_prod_ragged.log`.

---

## 8. Recommended next steps (sequenced)

1. **Un-block the q4f16 default** — produce an exact-length (non-MAX_SEQ-padded) or bucketed device buffer so the
   F16 GQA reduction tree matches the growing graph (also kills the §5 padded-attention tax), OR an fp32-KV
   static device export the q4f16 weights ride. Re-green `f16_device_kv_codes_identical_to_host_kv_ragged`, then
   flip the one-line `waav.json` `device_kv.share` key.
2. **Un-block CUDA-graph + realloc-free fusion** — land a same-run in-place `past==present` persistent
   fixed-address binding (a vendored/`pub(crate)`-elevated ort alias or an output-reuse refactor). This is the
   single prerequisite that both restores the fused run as a real win AND satisfies CUDA-graph's fixed-address
   precondition — the only path toward the ~30× hypothesis.
3. **Wire the production serve path** to route `step_slots_batched`'s device branch through the ring on
   `(chatterbox, CUDA, is_gb10)` once (1) restores the q4f16 default — the fp32 cell is proven and ready behind
   the resolver seam today.
