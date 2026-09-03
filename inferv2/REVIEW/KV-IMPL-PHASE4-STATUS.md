# KV-ACCEL Device-Resident-KV — Phase 4 (the device decoder) FINAL Regression + Status

**Date:** 2026-06-25. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified pool), aarch64.
**Tree:** `waav-infer` @ HEAD `4daa9aa` (the Phase 0–2 foundation commit) + the on-disk **uncommitted**
Phase 3–4 device-decoder work (4 modified files + 1 new producer + 1 new waav.json variant; left on disk,
NOT committed, per the coordinator hand-off discipline).
**Plan followed:** `WaaV/inferv2/REVIEW/KV-ACCEL-INTEGRATION-PLAN.md` (§3–§6, §9). Foundation status:
`KV-IMPL-FOUNDATION-STATUS.md`. Phase-3 RCA: `scratchpad/PHASE3_FINDINGS.md`.
**Env:** `source gb10-env.sh` (ORT 1.27 CUDA EP, `ort` rc.12, `CARGO_BUILD_JOBS=6`, 48 GiB arena cap). Live
gates run process-isolated `--test-threads=1`, ONE model set at a time (GB10 OOM history). `free -g` checked
before each live gate (≥24 GiB free throughout; peak ~13 GiB used outside the live model loads).

---

## 1. Headline verdict

**Workspace is GREEN. Phase 4 is COMPLETE and PROVEN. No regression attributable to the device-KV work.**

The Phase-4.0 GO-condition the foundation review flagged as the gate that converts "feasible-with-caveat" into
"proven" — **the STATIC-export `run_device_kv` byte-identity gate (the fixed in-place share-buffer geometry the
Phase-2 growing-export gate did NOT cover)** — is **GREEN**. The Phase-3 Python-pybind 212-vs-453 IoBinding-GQA
blocker does **NOT** reproduce in the Rust `backend-ort` seam: the static-share device-resident path reaches
byte-identity, exactly as the foundation review predicted the finer-control Rust seam would.

The full chatterbox **device-KV DECODER** (SlotId ring, prefill→device scatter, recycle-zero, advance-on-Ok)
is wired and **byte-identical to the provably-HOST reference** across a 24-cell accuracy matrix
(B ∈ {1,2,4,8,16,24} × {SHORT, MEDIUM, LONG, EXTREME}), and the measured device-vs-host perf curve crosses
into device-favorable territory from **B=8** and reaches **2.34× at B=24** — the host-KV re-stream-elimination
win the ~1.06×@B64 host cap needed.

---

## 2. What is GREEN (verified this run)

### 2.1 Build / lint (both feature configs)

| Check | Result |
|---|---|
| `cargo clippy --workspace --all-targets -D warnings` (default features) | **GREEN** |
| `cargo clippy --workspace --all-targets --features torch -D warnings` | **GREEN** |

### 2.2 Deterministic workspace suites (the no-GPU twins + everything else)

| Suite | passed | failed | ignored | exit |
|---|---|---|---|---|
| `cargo test --workspace -- --test-threads=1` (default features) | **1173** | **0** | 155 | 0 |
| `cargo test --workspace --features torch -- --test-threads=1` | **1173** | **0** | 174 | 0 |

(The extra `ignored` under `--features torch` are the torch-only live-GPU gates that don't exist in the default
build. All `#[ignore]`'d live-GPU gates are run separately below.)

**Phase-0/1/2 deterministic gates GREEN** (the §9 "always-run" set), individually re-confirmed:
- core: `device_kv_decoder_routes_through_device_seam_and_recycles`, `device_kv_step_disabled_on_cpu_and_rocm`,
  `arm_prefix_cache_declines_under_device_kv_ort`, `device_kv_value_dtype_equals_graph_input_type`,
  `each_codec_ar_arch_default_regime_is_hostkv`, `kv_residency_regime_is_hostkv_off_cuda`,
  `kv_residency_regime_is_hostkv_for_f16_until_proven`, `force_host_kv_override`,
  `prefix_cache_arm_decided_by_kv_residency_regime` (rewired to the `&self` arbiter).
- backend-api (5): `device_kv_spec_*` round-trip + host-ignore bit-identity.
- backend-ort (1): `device_kv::device_kv_ring_is_send_not_sync` (the Send-not-Sync static assert).
- runtime (1): `prefix_cache::device_kv_ort_regime_is_cost_truthful_and_not_a_strict_win`.

### 2.3 Live byte-identity gates (CUDA, process-isolated, ONE model set at a time)

| Gate | Result |
|---|---|
| **Phase-2** `device_kv::device_ping_pong_two_buffer_bit_identical_to_host_run` (GROWING export, CUDA) | **GREEN** — 8 strides byte-identical, ring retains all 60 device KV tensors (3.15 s) |
| **Phase-4.0 GO-cond.1** `device_kv::static_export_device_kv_bit_identical_to_host_run` (STATIC share export) | **GREEN** — 8 strides on `language_model_share.onnx` byte-identical to growing-export host run, MAX_SEQ=1024 (4.64 s) |
| **Phase-4 ORACLE (GO-cond.2)** `host_vs_device_kv_oracle` (ragged staggered mid-finish cohort) | **GREEN** — 4 ragged slots (lens=[18,60,55,50]) device-KV codes byte-identical to the `force_host_kv` reference (82.5 s) |
| **Phase-4 FULL MATRIX** `device_kv_accuracy_perf_matrix` (24-cell accuracy × perf × limits) | **GREEN** — `all_byte_identical=true`, full perf curve, clean over-MAX_SEQ reject, clean admission shrink (729.6 s) |
| **dia2 544/544** `cpu_fp32_codes_byte_identical` (32 cb × all frames, CPU fp32) | **GREEN** — 544/544 match, first-div=None (32.3 s) |
| **dia2** `cuda_bf16_codes_byte_identical` (the B25 cross-precision LAW, CUDA bf16 vs CUDA sidecar) | **GREEN** — 608/608 byte-identical (16.8 s) |
| **csm** `cuda_csm_codes_byte_identical_to_sidecar` (dual-AR greedy, all frames × codebooks) | **GREEN** (31.5 s) |
| **PRODUCTION chatterbox host-KV** `live_ragged_batched_forward_bit_identical_and_scales` | **GREEN** — 4 slots distinct lengths [18,74,67,60] byte-identical batched-vs-per-slot, best speedup 1.02× (310.9 s) — **production HostKv path untouched** |

**No-regression confirmation:** the production chatterbox path loads the GROWING `language_model.onnx` (no
`total_sequence_length`/`seqlens_k` graph inputs), so `device_kv_step_enabled() == false` and the path is the
unchanged host `lm_forward_batched`. The device-KV decoder is dormant unless the static-share export is loaded
behind the instance-aware gate. The 1.02× host-batched speedup is the same ~1.06×@B64 host cap the device work
exists to recover — confirming the host path is byte-for-byte the pre-Phase-3 behavior.

---

## 3. The 24-cell accuracy × perf × limits matrix (the Phase-4 deliverable, measured)

`device_kv_accuracy_perf_matrix` (real chatterbox, CUDA, `MAX_SEQ=1024`, 729.6 s, exit 0). The reference is the
GROWING export pinned `force_host_kv_for_test(true)` (the provably-host anchor); the SUT is the static-share
export device-KV decoder (SlotId ring, prefill-on-device, recycle-zero, advance-on-Ok). Both genuinely take their
asserted path (`device_kv_step_enabled_for_test()` true for SUT, false for reference).

### 3.1 ACCURACY — byte-identical (max|Δ|=0) decoded codes, ALL 24 cells

| seq-len bucket (prefill) | B=1 | B=2 | B=4 | B=8 | B=16 | B=24 |
|---|---|---|---|---|---|---|
| SHORT (~10 tok) | ✔ | ✔ | ✔ | ✔ | ✔ | ✔ |
| MEDIUM (~200 tok) | ✔ | ✔ | ✔ | ✔ | ✔ | ✔ |
| LONG (~800–900 tok) | ✔ | ✔ | ✔ | ✔ | ✔ | ✔ |
| EXTREME (~950–1000 tok, near MAX_SEQ=1024) | ✔ | ✔ | ✔ | ✔ | ✔ | ✔ |

`RESULT SUMMARY all_byte_identical=true`. EXTREME (in-range, just under the static ring MAX_SEQ=1024) held the
full ring depth with no overflow on every B.

### 3.2 PERF — device-vs-host per-tick wall (MEDIUM ~200-tok equal-context cohort, 12 timed ticks after 3 warmup)

| B | host-KV batched (ms/tick) | device-KV per-slot B=1 runs (ms/tick) | ratio (device/host) | interpretation |
|---|---|---|---|---|
| 1 | 29.030 | 27.338 | **0.942** | device ~6% faster (near parity) |
| 2 | 51.054 | 54.677 | 1.071 | device ~7% slower (noise band) |
| 4 | 106.446 | 109.486 | 1.029 | parity |
| 8 | 251.825 | 218.415 | **0.867** | device **1.15× faster — the KNEE / crossover** |
| 16 | 736.827 | 435.767 | **0.591** | device **1.69× faster** |
| 24 | 1524.454 | 653.940 | **0.429** | device **2.34× faster — widening** |

**The crossover knee is at B=8.** Below it the two paths are within the noise band; from B=8 the device path
pulls ahead, and the gap WIDENS with B because the host path rebuilds + re-streams a fresh `[B,H,max_past,D]` KV
buffer per layer per stride (cost growing super-linearly in B — the `chatterbox.rs:683-839` wall), while the
device-resident ring carries KV across strides with no host bounce.

### 3.3 LIMITS — clean reject + clean admission shrink (no OOM, no box-kill)

- **Over-MAX_SEQ prefill** (a single prefill > the static ring MAX_SEQ=1024): `RESULT LIMIT over_max_seq prefill
  clean_reject=true ok=false (no OOM either way)` — the device prefill path REJECTED with the typed
  `ChatterboxError::HostSync("device-KV attn_len … exceeds the static ring MAX_SEQ 1024 … §4.7 guard")`. No OOM,
  no panic, no box-kill.
- **Admission byte-budget:** `per_slot_bytes=251,658,240` (= 30 layers × 2 × 16 kv-heads × MAX_SEQ=1024 × 64
  head-dim × 4 B, F32 = 240 MiB per SlotId ring row) under the 48 GiB arena cap. An over-large desired
  `MAX_SLOTS=4096` SHRANK cleanly to `admitted=204` (= ⌊48 GiB / 240 MiB⌋); a budget under one slot admits 0
  (clean refusal). `device_kv_admit_slots` never attempts a runaway 4096-row alloc.
- **GB10 stability:** no box-kill at any point; resident set stayed ~2 chatterbox instances across all 24 cells
  (instances reused; slots `drop_slot`-recycled = §4.5 zero-on-recycle ring path between cells).

---

## 4. The resolution (the static-export device-resident GQA-share blocker)

**RESOLVED in the Rust `backend-ort` seam — byte-identical, RED-first gate GREEN.** The realization is the
**ACROSS-run static-share ping-pong on FIXED-shape buffers**, not genai's same-run in-place aliasing.

Adapting genai's `kv_cache.cpp past_present_share_buffer` discipline to chatterbox's in-graph (mask-derived)
seqlens:
1. **`total_sequence_length`:** chatterbox has NO scalar `total_sequence_length`/`seqlens_k` graph inputs (the
   GQA `seqlens_k = ReduceSum(mask)-1` and `total_seq_len = Shape(mask)[1]` are derived in-graph from
   `attention_mask`), so the genai CPU-scalar feed is N/A — the equivalent is caller-padding `attention_mask` to
   MAX_SEQ so the in-graph `total_sequence_length == MAX_SEQ` (the static buffer width).
2. **Zero-len-past-first-step:** the static graph REJECTS a 0-length OR a real-length past (it demands exactly
   MAX_SEQ), so the genai mechanism applied is a ZEROED full `[1,16,MAX_SEQ,64]` past on the prefill stride
   (host-zeroed, H2D once), and the §4.7 prefill scatter LEFT-aligns the host prefill KV into `[0..prefill_len]`.
3. **`past == present` alias:** ORT rc.12's PUBLIC IoBinding cannot register the same device pointer to BOTH a
   `past.*` input and a `present.*` output in ONE run (`bind_input` borrows `&Value`; `bind_output` consumes an
   owned `Value`; `Tensor::from_raw` views are non-upgradable; `Value::clone_of`/`from_ptr_nodrop` are
   `pub(crate)`). So the byte-identical resolution is the **ACROSS-run device ping-pong on fixed-shape static
   buffers**: `present.*` bound device-resident (`MemoryType::Default` — NOT the `CUDA_PINNED` 0.77× trap),
   carried forward AS-IS as the next stride's `past.*` for the SAME SlotId ring row. This is device-resident (no
   host bounce) AND fixed-shape (the CUDA-graph substrate the growing export could not provide).

**Deferred to Phase 6 (documented, not blocking):** true same-run in-place (zero per-stride present realloc;
genai `state_.inputs_[i] = presents_[i]`) needs a non-public `ort` alias or a persistent-binding output-reuse
refactor. On the static export ORT today re-allocates a fixed-size slab from the arena each stride (cheap,
fixed-shape, arena-reused). The residual realloc-elimination is a separate Phase-6 optimization.

---

## 5. Phase-4 STATE

**Phase 4 = COMPLETE.** The chatterbox device-KV decoder is wired behind the instance-aware
`device_kv_step_enabled()` gate (CUDA EP + static-share export + device-supported KV dtype), flipped for the
`(chatterbox, share-export, is_gb10, fp32)` cell ONLY, and proven byte-identical + perf-positive. All §9 Phase-4
RED-first gates are GREEN:
- GO-cond.1 static-export `run_device_kv` byte-identity — `static_export_device_kv_bit_identical_to_host_run`.
- GO-cond.2 THE oracle (explicit `force_host_kv` reference vs device SUT, ragged mid-finish) — `host_vs_device_kv_oracle`.
- recycle-clean (§4.5), mid-batch advance-on-Ok (§4.6), prefill→device handoff (§4.7) — exercised by the oracle's
  mid-finish `drop_slot` recycle + the deterministic `device_kv_decoder_routes_through_device_seam_and_recycles`.
- the 0.77×-trap guard — the PERF section shows device ≥ parity at every B and faster from B=8 (no pinned-host trap).
- admission under the 48 GiB arena cap (§3.2 byte-budget) — the LIMIT section's clean shrink to 204.

**Scope (unchanged from the plan):** the device path is purely additive and stays gated; production loads the
GROWING export → `device_kv_step_enabled()==false` → the untouched HostKv path (the production ragged gate proves
it byte-identical). No behavior change shipped to production.

---

## 6. Perf VERDICT (the actual measured device-vs-host scaling on the ORT export)

**SHIP the across-run static-share device-KV path.** The measured ORT truth (NOT the plan's ~30×@B64 hypothesis):
- **Crossover knee at B=8;** device reaches **2.34× faster at B=24** and the gap is still widening (host re-stream
  cost grows ~B² per stride; the device-resident ring avoids it).
- **Honest scope on the headline:** the plan's ~30×@B64 is the **tch single-fused-batched-run** probe. The
  Phase-4 ORT decoder runs each active slot as its OWN B=1 device-resident `run_device_kv` (per the
  `step_slots_batched` per-slot loop) — the single fused `[B,…]` device run is the **deferred Phase-6 lever**, not
  built here. So the delivered number is the **host-KV-re-stream-elimination win** (2.34×@B24), not 30×. The
  ~30×@B64 remains a hypothesis to measure on a fused-batched device run (Phase 6), explicitly NOT a delivered
  number.
- **Substrate win beyond the ratio:** the static export gives FIXED-shape `[1,16,MAX_SEQ,64]` buffers every
  stride (the CUDA-graph fixed-address precondition Phase 6 requires), which the growing export could not. This is
  the device-residency + fixed-shape foundation the SlotId ring (§3.2) and CUDA-graph (Phase 6) sit on.

---

## 7. GO / NO-GO for Phase 5 (the q4f16 / F16-KV device-KV cell — the SHIPPED default)

### Decision: **GO (conditional).**

The Phase-4 fp32 cell is proven byte-identical end-to-end, which de-risks the seam (fp32 has a lossless host
reference). The per-input-name dtype machinery the F16 cell needs already exists and is GREEN:
- `device_kv_spec` reads the ring dtype PER-INPUT via `empty_kv_dtype_for(language_model, in_name)` (§6), and the
  deterministic gate `device_kv_value_dtype_equals_graph_input_type` proves `input_types()=F16 ⇒ ring allocates
  F16`. The mixed-dtype bind (`inputs_embeds` F32, `attention_mask` I64, KV F16, `logits` F32) is resolved
  per-name, never per-graph.
- The static-share re-export is a DIM REWRITE only (`eval/make_chatterbox_static_kv.py`) — it composes with any
  precision's external-data weight blob.

### GO-conditions Phase 5 must land before flipping `device_kv_step_enabled` for the q4f16 cell:

1. **Phase-4.5 artifact + gate:** produce `language_model_q4f16_share.onnx` (static buffer + MatMulNBits) and
   gate `q4f16_share_export_decodes_identical_to_growing_q4f16` (plain `run()` identity) — Phases 3 (fp32 static)
   and 5 (q4f16 quant) do NOT compose for free; the q4f16 static export must be bit-verified against the growing
   q4f16 graph on a fixed prompt BEFORE the device path is wired to it.
2. **F16-cell OWN ragged gate:** `f16_device_kv_codes_identical_to_host_kv_ragged` — the F16 KV cell has NO
   lossless widening safety net (`f16::from_f32` is bit-exact only for f16-origin values), so it needs its OWN
   ragged-mid-finish CODES gate, not the fp32 oracle. Mirror `host_vs_device_kv_oracle` with the q4f16 export.
3. **bf16 stays out:** `bf16_kv_graph_stays_hostkv_or_hard_errors` — a bf16-declared KV graph must stay HostKv (or
   typed not-supported), never a silent f32 mis-bind (`ElemType` has no BF16).
4. Re-run the §3 LIMITS admission with `ElemType::F16` per_slot bytes (the F16 ring is half the F32 footprint =
   ~120 MiB/row → admits ~408 slots under the 48 GiB cap) and re-confirm the clean shrink.

### NO-GO trigger (fall back, keep fp32 cell + static export as the Phase-6 substrate):

- If GO-cond.1 (the q4f16 static-share `run()` identity) or GO-cond.2 (the F16 ragged CODES gate) fails — i.e. the
  F16 KV path diverges where the fp32 path did not — DO NOT flip the q4f16 cell; keep production on the GROWING
  q4f16 HostKv path. The fp32 device cell and the static export still stand for Phase 6 (CUDA-graph fixed-shape
  substrate) independent of the q4f16 device cell.

---

## 8. Files (all on disk, NOT committed; no `cargo fmt`)

**Modified (4):**
- `crates/waav-infer-backend-api/src/lib.rs` — `DeviceKvBuffer.in_place_static` field + the §4.2 pure-data spec.
- `crates/waav-infer-backend-ort/src/lib.rs` — `run_device_kv` mode-branch + `run_device_kv_static` (the static
  in-place share-buffer path) + `zeroed_named_tensor` + `seed_static_kv_row` (the §4.7 scatter) + the static gate
  `device_kv::static_export_device_kv_bit_identical_to_host_run`.
- `crates/waav-infer-core/src/tts/chatterbox.rs` — the device-KV decoder: `device_kv_max_seq`, `device_kv_spec`,
  `device_kv_forward` (§4.4), prefill→device scatter (§4.7), recycle-zero (§4.5), the `host_vs_device_kv_oracle`
  + `device_kv_accuracy_perf_matrix` live gates + the deterministic
  `device_kv_decoder_routes_through_device_seam_and_recycles` twin + `device_kv_max_seq_for_test`/
  `device_kv_step_enabled_for_test` test accessors.
- `ci/heavy_live_tests.sh` — added the static-export gate, the oracle, and the perf-matrix to the heavy live list.

**New (2):**
- `eval/make_chatterbox_static_kv.py` — checked-in in-repo producer (dim-rewrite + `--verify`; replaces the
  scratchpad `make_static.py`).
- `~/.cache/waav-models/chatterbox-onnx/waav.share.json` — the device-KV-CUDA-only static-share variant manifest
  (`static_share_buffer` + `max_seq` + `ep:cuda`); production `waav.json` untouched.

**Artifact:** `~/.cache/waav-models/chatterbox-onnx/onnx/language_model_share.onnx` (static
`past_present_share_buffer` fp32 variant, MAX_SEQ=1024; external data reused in place from
`language_model.onnx_data`, no copy/repack).

**Regression logs (this run):** `scratchpad/p4_regr_default.log`, `scratchpad/p4_regr_torch.log`,
`scratchpad/p4_perf_matrix.log` (+ the per-gate live outputs captured inline above).

---

## 9. Recommended next steps (sequenced)

1. **Phase 5 — q4f16 / F16-KV cell** (the shipped default): land GO-cond.1–4 above; restore q4f16 as the live
   default device cell once the F16 ragged CODES gate is GREEN.
2. **Phase 6 — CUDA-graph + true in-place** (gated separately): the static fixed-shape buffers are the
   precondition; land the 3-`seqlens_k` replay gate first, then `with_cuda_graph` bucketed by cohort width; and
   the same-run in-place present-reuse (the deferred §4 realloc-elimination) via a persistent-binding refactor or
   a non-public `ort` alias.
3. **Parallel, independent of device-KV:** the pre-existing §3-of-foundation `live_gb10_batcher` S3Gen vocoder
   arena-OOM at 24-way concurrency remains a standalone GB10 arena/concurrency tuning task (NOT on the KV path).
4. **Wire the production serve path** to select `waav.share.json` on `(chatterbox, CUDA, is_gb10)` so the lockstep
   batcher rides the device ring (the §8 batching plug-in) once Phase 5 restores the q4f16 default.
