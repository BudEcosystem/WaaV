# KV-ACCEL Device-Resident-KV — Foundation (Phase 0–2) Regression Status + Phase-4 GO/NO-GO

**Date:** 2026-06-25. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified pool), aarch64.
**Tree:** `waav-infer` @ branch `waav-infer-v2-build` (HEAD `52068a9`) + the on-disk uncommitted
Phase 0–2 foundation (6 files, +1169/−17; left on disk, NOT committed, per the coordinator hand-off).
**Plan followed:** `WaaV/inferv2/REVIEW/KV-ACCEL-INTEGRATION-PLAN.md` (§9 gates).
**Env:** `source gb10-env.sh` (ORT 1.27 CUDA EP, rc.12, `CARGO_BUILD_JOBS=6`, 48 GiB arena cap). Live gates
run process-isolated `--test-threads=1`. `free -g` checked before each live gate (≥101 GiB free throughout).

---

## 1. Headline verdict

**Workspace is GREEN. No regression attributable to the Phase 0–2 KV-ACCEL foundation.**
**The critical de-risk flipped POSITIVE: the Phase-2 device-KV IoBinding ping-pong is byte-identical to
the host `run()` loop on the REAL chatterbox `language_model.onnx` on GB10 ORT-CUDA — in the Rust
`backend-ort` seam — exactly the path the Phase-3 reference-only investigation predicted would resolve the
Python-pybind IoBinding-GQA blocker.**

**Phase 4 (the device decoder): GO — conditional**, scoped to the chatterbox device-KV decoder behind the
existing instance-aware gate, with the residual blockers carried into Phase 4 as RED-first gates (below).

---

## 2. What is GREEN (verified this run)

### 2.1 Build / lint (both feature configs)

| Check | Result |
|---|---|
| `cargo test --workspace --no-run` (default features) | compiles clean |
| `cargo clippy --workspace --all-targets -D warnings` (default) | **GREEN** |
| `cargo clippy --workspace --all-targets --features torch -D warnings` | **GREEN** (torch backend incl.) |
| `forbid(unsafe)` + P-8 backend-free layering | held (no `ort` type crosses backend-api; spec is pure data) |

### 2.2 Deterministic workspace suites (the no-GPU twins + everything else)

| Suite | passed | failed | ignored |
|---|---|---|---|
| `cargo test --workspace -- --test-threads=1` (default features) | **1169** | **0** | 152 |
| `cargo test --workspace --features torch -- --test-threads=1` | **1169** | **0** | 171 |

(The +19 extra `ignored` under `--features torch` are the torch-only live-GPU gates that don't exist in the
default build. The `ignored` set in both configs is the `#[ignore]`'d live-GPU gates run separately below.)

**All Phase-0/1/2 deterministic gates GREEN** (the §9 "always-run" set), individually confirmed:
- Phase 0 (chatterbox, no GPU): `kv_residency_regime_is_hostkv_off_cuda`,
  `kv_residency_regime_is_hostkv_for_f16_until_proven`, `device_kv_step_disabled_on_cpu_and_rocm`,
  `arm_prefix_cache_declines_under_device_kv_ort`, `device_kv_value_dtype_equals_graph_input_type`,
  `each_codec_ar_arch_default_regime_is_hostkv`, `force_host_kv_override`,
  `prefix_cache_arm_decided_by_kv_residency_regime` (rewired to the `&self` arbiter).
- Phase 0 (runtime): `prefix_cache::device_kv_ort_regime_is_cost_truthful_and_not_a_strict_win`.
- Phase 1 (backend-api): `device_kv_spec_constructs_and_round_trips`,
  `device_kv_spec_ignored_by_host_backend_bit_identical`.
- Phase 2 (backend-ort): `device_kv::device_kv_ring_is_send_not_sync` (the Send-not-Sync static assert).

### 2.3 Live byte-identity gates (CUDA, process-isolated)

| Gate | Result |
|---|---|
| **Phase-2** `device_ping_pong_two_buffer_bit_identical_to_host_run` (real chatterbox LM, CUDA) | **GREEN** — 8 strides byte-identical, ring retains all 60 device KV tensors |
| `cuda_torch_dia2` (codec-decode + step-0 logits + synthesis) | **GREEN** |
| **dia2 544/544** `cpu_fp32_codes_byte_identical` (32 cb × all frames, CPU fp32) | **GREEN** |
| **dia2 544/544** `cuda_bf16_codes_byte_identical` (the B25 LAW, CUDA bf16 vs CUDA sidecar) | **GREEN** |
| `cuda_csm_codes_byte_identical_to_sidecar` (dual-AR greedy, all frames × codebooks) | **GREEN** |
| chatterbox host-KV `live_ragged_batched_forward_bit_identical_and_scales` (the direct lockstep proof the arbiter wraps) | **GREEN** (310 s) |

---

## 3. The one non-green gate — RCA'd, NOT a regression

### `codec_ar_batcher::tests::live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales` — **FAIL (pre-existing)**

**Failure mode (identical across all 3 runs):**
```
backend run error: Non-zero status code returned while running Conv node. Name:'/conv_pre/Conv'
bfc_arena.cc:358 AllocateRawInternal: Available memory of 18319576576 is smaller than
requested bytes of 21686026240
```
This is the chatterbox **S3Gen vocoder `conv_pre` Conv** requesting ~21.7 GiB inside the ORT-CUDA BFC
arena (capped at 48 GiB on GB10, `ep.rs:24` `GB10_ARENA_LIMIT_BYTES`), with only ~18.3 GiB free in-arena
after the 24-slot (`MAX_SLOTS=24`) concurrent ragged cohort's LM sessions+KV have claimed the rest. It is
an **arena-cap-vs-concurrency / fragmentation** failure, NOT a bit-identity failure.

**Attribution — DEFINITIVELY pre-existing (3 controlled runs):**
1. With the Phase 0–2 changes on disk → FAIL (conv_pre OOM, byte counts 21686026240 / 18319576576).
2. With `WAAV_FORCE_HOST_KV=1` (pins the exact pre-change host path) → FAIL, **byte-for-byte identical**.
3. **Stashed the Phase 0–2 changes → ran on CLEAN HEAD `52068a9` → FAIL, byte-for-byte identical** (same
   conv_pre, same 21686026240 / 18319576576, EXIT 101). The working changes were then **fully restored to
   disk** (re-verified: 6 files, +1169/−17; `DeviceKvOrt`/`run_device_kv`/`DeviceKvBuffer` all present;
   `backend-ort`+`core` recompile clean). The temporary RCA stash was dropped; the 2 unrelated pre-existing
   M4.4 stashes were preserved.

**Why the KV changes cannot be the cause structurally:** the serve path here loads the production GROWING
`language_model.onnx` (no `total_sequence_length`/`seqlens_k` graph inputs), so
`device_kv_step_enabled()==false` and the path is the unchanged host `lm_forward_batched`. The KV-ACCEL
device path is dormant unless the static-share export is loaded.

**Disposition:** OUT OF SCOPE for this foundation review (pre-existing GB10 arena-tuning issue on the
24-stream concurrent vocoder-decode cohort). Tracked as an environmental/tuning item — the S3Gen vocoder
decode peaks past the 48 GiB arena under 24-way concurrency; fix is a separate arena/concurrency tuning
task (e.g. shrink `MAX_SLOTS` for this gate, raise/relax the arena cap, or serialize vocoder decode),
independent of device-resident KV.

---

## 4. Foundation state (Phase 0–2), as reviewed on disk

- **Phase 0 (regime arbiter, no behavior change) — COMPLETE.** `kv_residency_regime` is now `&self`
  (instance-aware); split into the two orthogonal axes `device_kv_step_enabled()` (STEP routing, gates
  `lm_forward_batched`) and the prefix-residency arbiter (gates `arm_prefix_cache_if_win`). New
  `KvResidencyRegime::DeviceKvOrt` variant added (cost-truthful `prefix_host_transfer_rows==matched`,
  `reuse_is_strict_win()==false`). Gated conjunctively on CUDA EP + static-share export
  (`total_sequence_length`+`seqlens_k` graph inputs) + device-supported KV dtype (F16/F32; `Other`/bf16
  stays host). `WAAV_FORCE_HOST_KV` operator kill-switch wired (env-free `_with(force_host)` core +
  env-reading public wrapper, no cross-test env race). Production (growing export, any precision, CPU or
  CUDA) stays `HostKv` — no behavior change shipped.
- **Phase 1 (backend-api IoBinding device-KV spec, pure data) — COMPLETE.** `DeviceKvBuffer { in_name,
  out_name, shape, dtype: ElemType, slot_keyed }` + `IoBinding::with_device_kv`/`device_kv()`/
  `device_kv_buffers()`. Pure data; no `ort` type crosses; host/CPU/non-CUDA backends ignore it and the
  default `run_bound` is bit-identical to a no-spec binding (proven); the spec never enters `all_inputs()`.
- **Phase 2 (backend-ort `run_device_kv` sibling) — COMPLETE + de-risked GREEN.** `run_device_kv` is a
  NEW method (the 8 CFM/Supertonic `run_bound` gates untouched and still green). CUDA `Allocator` device
  `DeviceKvRing` (separate field, Send-not-Sync, `PhantomData<Cell<()>>` marker + static assert), two-buffer
  ping-pong, extracts ONLY `logits`, ring retained across strides. The live ping-pong gate proves
  byte-identity to the host run loop on the real LM — **the rc.12 present→past device-alias lifetime
  survives in the Rust seam.**

---

## 5. Phase-4 GO/NO-GO

### Decision: **GO (conditional)** for Phase 4 — the chatterbox device-KV decoder.

The two gating inputs the coordinator asked to weigh:

### (a) Phase-2 device-binding result (does the rc.12 device ping-pong work byte-identical?) — **YES.**
`device_ping_pong_two_buffer_bit_identical_to_host_run` is **GREEN** on the real
`language_model.onnx` on GB10 ORT-CUDA: 8 strides of `run_device_kv` (device KV bound through IoBinding,
present.* device-resident in the ring, only `logits` extracted) are byte-identical to the host
present→past `run()` loop. This is the load-bearing de-risk for the whole plan, and it passed **in the
Rust backend-ort seam** — the finer-control path (`Allocator`+`MemoryInfo(CUDA,Device)`, explicit
`bind_input`/`bind_output`, the device ring) the Phase-3 reference investigation named as the resolution.

### (b) Phase-3 re-export feasibility — **FEASIBLE-WITH-CAVEAT (as reported), and the caveat is now retired by (a).**
The Phase-3 reference-only investigation (`scratchpad/PHASE3_FINDINGS.md`, no repo src touched) established:
the static `past_present_share_buffer` GQA variant is PRODUCIBLE (dim-rewrite only; the chatterbox bias-drop
is a no-op — 0 GQA nodes carry `attention_bias`) and **byte-identical via plain ORT `run()`** (453==453,
CPU+CUDA, both mask modes). The reported blocker was that **every Python-pybind `run_with_iobinding()`**
device-resident binding decoded WRONG (212 from prefill) — single-buffer alias, A/B ping-pong,
ORT-alloc-present, pre-alloc-present alike. The fallback verdict was explicit: the static export is SOUND;
the viable path is to **resolve the IoBinding-GQA blocker in the Rust backend-ort seam, which has finer
control than Python pybind**, and that this MUST be proven green BEFORE wiring the decoder.

**That Rust-seam proof is exactly the Phase-2 ping-pong gate, and it is now GREEN (item (a)).** So the
Python-pybind 212-divergence does NOT block Phase 4: the Rust `run_device_kv` reaches byte-identity on the
real graph. **One important nuance the de-risk noted, carried as a Phase-4 gate:** the Phase-2 gate proves
byte-identity on the **GROWING/symbolic** export (KV shape `[1,16,-1,64]`, F32), threading present→past
across strides with a fresh handle each stride — it proves the device-residency MECHANIC + rc.12 lifetime,
but it does NOT by itself prove the **fixed-buffer static-share in-place geometry** that Phase 3 produces
and that the SlotId ring (§3.2) and CUDA-graph (Phase 6) require. The static `language_model_share.onnx`
re-export + its own `run_device_kv` byte-identity gate is therefore the **first RED-first task of Phase 4**
(NOT assumed green from Phase 2's growing-export result).

### GO conditions (RED-first gates Phase 4 must land before flipping `device_kv_step_enabled` for any cell):
1. **Produce + verify the static-share export.** `language_model_share.onnx` (dim-rewrite, bias-drop no-op
   for chatterbox); gate `static_share_run_byte_identical_to_growing` (plain `run()`, already proven in the
   Python reference at 453==453 — re-prove in-repo on load) AND the new
   `run_device_kv` byte-identity gate on the STATIC export (single fixed buffer per (layer,kv), in-place
   `present` aliasing `past`) — the geometry the Phase-2 growing-export gate did NOT cover.
2. **THE oracle:** `host_vs_device_kv_codes_identical_ragged` with an explicit `with_kv_regime(HostKv)`
   reference vs the device-KV SUT, per-ragged-stride on a staggered mid-finish cohort (the SlotId ring,
   §3.2; B-index instability, §1.3).
3. `device_ring_recycle_is_clean_bit_identical` (zero-on-recycle, privacy, §4.5 — doubly required on the
   coherent GB10 pool), `device_kv_mid_batch_backend_error_re_homes_each_slot` (§4.6 advance-on-Ok-only),
   `prefill_to_device_handoff_codes_identical_to_all_host` (§4.7).
4. `device_kv_b16_not_slower_than_host_kv_b16` (catch the 0.77× trap; perf is the WHOLE point — Phase 1
   IoBinding-only on the growing export was measured 0.77× on this LM, `chatterbox.rs:589-603`).
5. **Wire the device-KV ring under the SAME 48 GiB arena cap + the §3.2 admission byte-budget** so the ring
   pre-alloc cannot starve the vocoder decode (the §3 failure above is a live reminder that the unified
   pool + 48 GiB arena is already tight at 24-way concurrency; the SlotId ring at `MAX_SEQ×MAX_SLOTS` must
   SHRINK `MAX_SLOTS` via `slot_cap`, never runaway-alloc).

### NO-GO triggers (fall back to host-KV, keep the static export as a Phase-6 CUDA-graph substrate only):
- If the **static-export** `run_device_kv` byte-identity gate (cond. 1) fails (i.e. the Rust seam reaches
  byte-identity on the growing export but NOT on the fixed in-place static-share geometry), DO NOT wire the
  decoder; keep the host-KV path. The static export still serves Phase 6 (CUDA-graph fixed-shape substrate)
  independent of device-resident KV, per the de-risk fallback.

---

## 6. Recommended next steps (sequenced)

1. **Phase 4.0 — static-share re-export, IN-REPO.** Port `scratchpad/make_static.py` into a checked-in
   producer; ship `language_model_share.onnx` as a `waav.json` device-KV-CUDA-only weights variant
   (registry-zero-code). Land cond.-1 gates (plain-`run` identity + **static-export `run_device_kv`
   identity** — the geometry Phase 2 did not cover). **This is the gate that converts the Phase-3
   "feasible-with-caveat" into "proven" or trips the NO-GO.**
2. **Phase 4.1 — the chatterbox device-KV decoder (fp32 cell first).** SlotId ring (§3.2), device
   `lm_forward_batched` variant (§4.4), recycle-zero (§4.5), mid-batch-reject advance-on-Ok (§4.6),
   prefill→device scatter (§4.7). Flip `device_kv_step_enabled` for `(chatterbox, share-export, is_gb10,
   fp32)` ONLY. Land cond. 2–4 gates. (fp32 has a lossless host reference — de-risks the seam before F16.)
3. **Phase 5 — F16/q4f16 cell** (the shipped default): produce+verify `language_model_q4f16_share.onnx`,
   ring allocates F16 per-input-name, own ragged F16 gate (no lossless widening safety net).
4. **Parallel, independent of device-KV:** resolve the §3 `live_gb10_batcher` arena OOM (a standalone
   GB10 arena/concurrency tuning task — NOT on the KV path) so the heavy-list goes fully green again.
5. Phase 6 (CUDA-graph, 3-seqlens_k replay gate first) and Phase 7 (accel catalog) per the plan, after
   Phase 4–5 land.

---

## 7. Files

- On-disk foundation (NOT committed, left for the coordinator): `crates/waav-infer-backend-api/src/lib.rs`,
  `crates/waav-infer-backend-ort/src/lib.rs`, `crates/waav-infer-core/src/tts/chatterbox.rs`,
  `crates/waav-infer-runtime/src/prefix_cache.rs`, `ci/heavy_live_tests.sh`, `Cargo.lock`.
- Phase-2 live gate: `crates/waav-infer-backend-ort/src/lib.rs` `mod device_kv` →
  `device_ping_pong_two_buffer_bit_identical_to_host_run`.
- Phase-3 reference findings: `scratchpad/PHASE3_FINDINGS.md` (+ `make_static.py`, the `ph3_*.py` ladder).
- Regression logs (this run): `scratchpad/regr_default.log`, `scratchpad/regr_torch.log`,
  `scratchpad/live_pingpong.log`, `scratchpad/live_dia2*.log`, `scratchpad/live_csm.log`,
  `scratchpad/live_chatterbox_ragged.log`, `scratchpad/live_gb10_batcher{,2,_cleanHEAD}.log`.
