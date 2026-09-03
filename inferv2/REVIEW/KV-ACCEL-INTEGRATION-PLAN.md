# Device-Resident KV + ORT-Accel Integration — FINAL Production Plan

**2026-06-25. GB10 (Grace-Blackwell sm_121, unified 121 GB) + aarch64 CPU floor. `ort = "=2.0.0-rc.12"` (ORT ≥1.24, `api-24`, `load-dynamic`, `half`).**

Goal: make the codec-AR LM KV **device-resident** so the chatterbox lockstep batcher recovers the device-resident scaling curve the host-KV re-stream caps at (~1.8×@B16 → ~1.06×@B64, `BATCHING-ANALYSIS-SYNTHESIS.md` G1), by porting the ORT device-resident-KV technique (IoBinding device ping-pong + buffer-sharing GQA `past_present_share_buffer`) + the accuracy-neutral ORT accel catalog into WaaV — **byte-identical (decoded CODES), no-regression, hardware-portable**.

This plan is RISK-FIRST: every step is gated bit-identical against a **provably-host reference** before it is defaulted on; CPU/non-CUDA and every unproven `(export, EP, precision)` combo degrade cleanly to the existing host-KV path. Every claim below is grounded in a verified `file:line`.

---

## 0. Ground-truth corrections (verified, vs the input design)

The input design + critiques contained several factual inversions. Verified against the live tree:

| Claim in inputs | VERIFIED TRUTH (file:line) |
|---|---|
| "resolved ort is rc.10; citations wrong" | **FALSE.** `Cargo.toml:43` + `Cargo.lock:1277-1280` = `ort 2.0.0-rc.12`. The vendored tree at `/home/bud/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ort-2.0.0-rc.12/` is the resolved one. Use rc.12 line numbers. |
| `Session` is non-Send under cuda-graph ⇒ compile error | **FALSE.** `ort-2.0.0-rc.12/src/session/mod.rs:675-676` `unsafe impl Send/Sync for Session {}` UNCONDITIONALLY. No trait-bound break. (The "not Send/Sync" doc note is a runtime-thread caveat, satisfied by the single `codec-ar-mux` thread.) |
| `do_copy_in_default_stream=0` is a usable knob | **FALSE.** `ort-2.0.0-rc.12/src/ep/cuda.rs:211` is a tombstone: "Here once lied `do_copy_in_default_stream` … the setter here was removed." **DELETE it from the plan.** |
| `kv_residency_regime()` is per-decoder, per-cell | **FALSE.** `chatterbox.rs:519` `pub(crate) fn kv_residency_regime() -> KvResidencyRegime` — **no `&self`, no args**, hard-returns `HostKv`. Cannot branch on EP/precision/export. **Must be made instance-aware FIRST (Phase 0).** |
| `DeviceResident` = the target ONNX device-KV regime | **FALSE.** `prefix_cache.rs:699-701` documents `DeviceResident` as "the tch `KvCache`: an in-place `index_copy_` device buffer" whose rows are host-serializable. The ONNX IoBinding path needs a **new `DeviceKvOrt` variant** (opaque `ort::Value`, prefix-cache NOT armable). |
| serve LM default is fp32 | **FALSE.** `~/.cache/waav-models/chatterbox-onnx/waav.json` → `"language_model": {"precision": "q4f16"}`. The live default is **q4f16 (F16 KV)**. fp32-first phasing optimizes a graph production never serves unless we flip waav.json. |
| Phase-1 IoBinding-only is a "shippable partial win" | **FALSE on GB10.** `chatterbox.rs:589-603` already measured IoBinding-keep-on-device on the real LM = **0.77× (slower)**. On a coherent unified pool the host "transfer" is a coherent alias, so removing it alone is ~flat; and the growing `[B,H,past+1,D]` output forces per-stride device re-alloc. **Phase-1 is a seam-proving step on a TOY graph, NOT a production landing.** |

These corrections drive the phasing and the gating below.

---

## 1. Scope, blast radius, and the P-8 invariant

### 1.1 What is in scope (verified disjoint)

- **ONNX codec-AR LM device-KV** applies to **exactly ONE model today: chatterbox `language_model.onnx`** (the sole host-KV ONNX codec-AR LM through `OrtModel`). Grep confirms `impl ArStepModel` in core exists only at `chatterbox.rs` (`ChatterboxArStep`); the torch fleet (dia2/csm/qwen3-tts/voxtral/cohere/ark) implements `TtsModel::synthesize` and inherits `as_stepped()->None` — they ride one-shot `synthesize`, NOT `step_batch`, and they are **already device-resident** via the tch `KvCache` ring (`backend-torch nn/kv_cache.rs`). They get ZERO from this ORT work and are OUT OF SCOPE.
- **Diffusion (masked, omnivoice) + flow (CFM, supertonic/cosyvoice3)** return `as_stepped()=None` (`model.rs:as_stepped` dispatch) and ride the step-bucket micro-batch cohort. They have **no AR-step KV** — out of scope for ring-KV. Their bit-identity is gated by their own harnesses (`cfg_batch_ar_compounding_identical` with the per-step RNG-draw-order law; the CFM maxΔ=0.0 gate) — NOT the AR codes gate.
- **Codecs** (mimi/dac/dacvae tch + chatterbox S3Gen `conditional_decoder` + NeuCodec ONNX) have **NO KV cache** — ring-KV is inert there. Their accel is a separate additive track (§7), not part of device-KV.

### 1.2 The P-8 layering invariant (must not break)

`driver.rs:8-15` documents the runtime crate is **backend-free**: "names no `ort`/`candle`/`ggml` AND no `SlotTable`." The orchestration the runtime hands down is identical whether KV is host- or device-resident:

- `driver.rs:229` `pub fn tick(&self, model: &mut dyn ArStepModel, active: &ActiveSet)` — calls `model.step_batch(&step_inputs)` ONCE/tick (`driver.rs:256`), asserts `stepped.len()==step_inputs.len()` (`driver.rs:257`). Pure `&mut dyn ArStepModel` seam, no backend.
- `serve.rs:778-781` builds `active_rows: Vec<SlotInput>` by **compacting**: `live.iter().filter_map(|c| c.as_ref()).filter(|s| !s.accum.done)`. `live: Vec<Option<LiveStream>>` is a fixed Vec indexed `0..max_slots` (`serve.rs:650`).
- `codec_ar_batcher.rs` spawns ONE `codec-ar-mux` thread that locks the model once and runs `serve_codec_ar_multiplexed_bounded` for the server lifetime — capture/replay/allocator all on this single thread.

**An `ort::Value` MUST NOT leak into runtime/server.** The device handle is a backend-api abstraction; only `backend-ort` holds the concrete `ort::Value`; only the chatterbox decoder (core) binds through it. The orchestration stays **byte-for-byte unchanged**.

### 1.3 The B-index instability (the load-bearing design constraint)

The host batched path tolerates membership churn ONLY because it never persists a B-layout: `lm_forward_batched` (`chatterbox.rs:683-839`) recomputes `max_past = rows.iter().map(|r| r.past_seq).max()` (`:693`) and re-builds a fresh `[B,H,max_past,D]` buffer LEFT-aligned at compacted index `bi` **every stride** (`:738,759-761`). When a slot finishes mid-cohort (`serve.rs:766-773`) the compacted `bi` of every later slot **shifts**.

**Therefore a persistent device ring MUST be keyed by `SlotId` (stable, `0..max_slots`), NOT by compacted cohort position.** This is the single most important architectural decision in this plan and is detailed in §3.2.

---

## 2. The integration points (verified file:line) and what changes at each

| # | Site | Current state (verified) | Change |
|---|---|---|---|
| I1 | `serve.rs:778-781` active_rows | compacted `filter_map+filter`, `SlotId`+`frame_idx` only | **NO CHANGE** (backend-free). The device ring is keyed by `SlotId` which is stable; compaction only affects which rows bind this tick. |
| I2 | `driver.rs:229-257` `tick` → `step_batch` | one `step_batch`/tick, backend-free | **NO CHANGE** (the static-cohort precondition CUDA-graph needs is exactly this). |
| I3 | `model.rs` `as_stepped` dispatch | AR overrides `step_batch`; diffusion/flow return `None` | **NO CHANGE** (bounds blast radius to chatterbox). |
| I4 | `chatterbox.rs` `ChatterboxArStep::step_batch` (the `ArStepModel` override) | routes `len>1` → `step_slots_batched`, else per-slot `step` | **per-arch entry**: select device-KV path behind `self.device_kv_step_enabled()` (new, §3.1). |
| I5 | `chatterbox.rs:947-1012` `step_slots_batched` | `remove(&slot)` from `HashMap<SlotId,SlotDecode>`, `mem::take(&mut st.past)` (`:981`), scatter `st.past=grown` (`:992`), Err-arm re-homes (`:1006-1008`) **leaving `past` empty** | REUSE the per-slot remove/insert + Err re-home structure, but the device handle stays SlotId-owned (NOT `mem::take`-out — §4.6). |
| I6 | `chatterbox.rs:683-839` `lm_forward_batched` | THE host-KV wall: rebuild `[B,H,max_past,D]` host KV/layer/stride (`:738,757-762`), plain `run` (`:777`), read 60 `present.*` via `to_f32_vec` + un-pad (`:805-835`) | a **device-KV variant** that binds the SlotId-keyed device ring, feeds only `inputs_embeds`/`attention_mask`/`total_sequence_length`, extracts ONLY logits. |
| I7 | `chatterbox.rs:136-139` `SlotDecode.past: Vec<NamedTensor>` | host KV per slot | for the device path, the slot's KV is the **fixed device ring row** (held by the decoder's `DeviceKvRing`, NOT in the per-slot host Vec); `SlotDecode` keeps only logical anchors (`attn_len`, `generated`, `next_pos`). |
| I8 | `chatterbox.rs:519-521` `kv_residency_regime()` | static fn, hard `HostKv` | **Phase 0**: make `&self`, branch on `active_ep()` + export + KV dtype (§3.1). |
| I9 | `chatterbox.rs:528-535` `arm_prefix_cache_if_win` | arms iff `kv_residency_regime().reuse_is_strict_win()` | decouple: arming reads the **prefix-residency axis** (stays host until a device-resident `PrefillState` exists — §3.3), NOT the step-residency axis. |
| I10 | `chatterbox.rs:1098-1127` `prefill_slot` | produces host KV via `lm_forward` → `SlotDecode.past` | add the **prefill→device-ring scatter** at first batched step (one-time bounded H2D, ledgered — §4.7). |
| I11 | `chatterbox.rs:1531-1533` `drop_slot` (+ `reset_slot:1627`) | bare `self.lm.slots.remove(&slot)` | extend: **zero** the recycled SlotId's ring row + reset its seqlens/len to 0 (the recycle-is-clean invariant — §4.5). |
| I12 | `backend-api/src/lib.rs:240-348` `IoBinding` | pure host data (`constants`/`inputs`/`device_outputs` name-hints + `epoch`); `run_bound` default `:372-374` = `run(all_inputs())` | extend with a **pure-data device-KV spec** (shared in/out name pairs + `ElemType` + shape + flag); host/CPU/non-CUDA backends ignore it (clean fallback). NO `ort` type, `forbid(unsafe)` preserved. |
| I13 | `backend-ort/src/lib.rs:489-566` `run_bound` | binds outputs to `CUDA_PINNED`/`CPUOutput` (`:515-527,547`) — the 0.77× pinned-host trap; extracts every output to host (`:555-563`) | add a SIBLING `run_device_kv` (do NOT mutate `run_bound` — its 8 CFM gates stay green); CUDA `Allocator` device Values, ping-pong, extract ONLY logits (§4.1). |
| I14 | `backend-ort/src/ep.rs:67-117` CUDA EP builder | `with_tf32` + `with_memory_limit(cuda_arena_limit_bytes)` + `SameAsRequested` (GB10), arena cap = 48 GiB | wire device-KV allocator under the SAME arena cap; add §7 conv/SDPA knobs env-gated. |
| I15 | `precision.rs:26,59,82-101` dtype seam | `empty_kv_dtype_for` (F16 vs F32), `feed_float`, `F32_FEATURE_INPUTS`; **NO BF16** (`:78` "bf16 surfaces as Other" → F32) | the device ring dtype is read PER-INPUT from `input_types()`; bf16/Other ⇒ stay HostKv (§6). |

**UNCHANGED, byte-for-byte:** `driver.rs`, `serve.rs`, `codec_ar_batcher.rs`, `model.rs` `as_stepped`.

---

## 3. The three structural design decisions (resolve the P0 critiques)

### 3.1 Make the regime arbiter instance-aware (Phase 0, hard prerequisite)

`kv_residency_regime() -> KvResidencyRegime` (`chatterbox.rs:519`) is a parameterless constant. Flipping it flips EVERY chatterbox instance (CPU, q4f16, fp32) at once — the exact combos the design says must stay HostKv. **Split into two orthogonal, instance-aware axes:**

```
// Axis 1 — STEP routing (host step loop vs device-IoBinding step loop). Gates lm_forward_batched ONLY.
fn device_kv_step_enabled(&self) -> bool {
    matches!(self.language_model.active_ep(), ActiveEp::Ep(EpKind::Cuda))   // CUDA only (HIP future, §5)
      && self.export_is_static_share_buffer()                               // graph-derived: static cache axis
      && Self::kv_dtype_is_device_supported(self.language_model)            // F16 or F32 (not Other/bf16)
}

// Axis 2 — PREFIX-CACHE residency (where a cached PREFIX lives). Gates arm_prefix_cache_if_win ONLY.
fn kv_residency_regime(&self) -> KvResidencyRegime {                         // now &self
    // ONNX device-KV step path: KV is an opaque ort::Value with NO host f32 to serialize into
    // PrefillState(Vec<f32>). So the prefix cache STAYS host-serializable & stays UNARMED.
    KvResidencyRegime::HostKv          // until a device-resident PrefillState store exists (§3.3)
}
```

Add a **new enum variant** `KvResidencyRegime::DeviceKvOrt` to `prefix_cache.rs:695` whose `prefix_host_transfer_rows()` returns `matched_tokens` (truthful — the opaque-handle cache would still re-stream) and `reuse_is_strict_win()` returns **false**. `DeviceResident` keeps its tch meaning. This makes the device-KV STEP flip independent of the prefix-cache ARM, and keeps the cost model honest. `arm_prefix_cache_if_win` (`:528`) becomes `self.kv_residency_regime()`.

**Why first:** without this, no per-cell gate (CPU-stays-host, q4f16-stays-host) is *expressible*, so no failing-first test can exist (§9, EXTREME-TDD).

### 3.2 The device ring is SlotId-keyed and statically allocated ONCE (not per-cohort, not per-epoch)

Driven by the B-index instability (§1.3) + the per-cohort-epoch OOM hazard.

- Pre-allocate ONE device ring at model load (or first cohort): **60 KV `Value`s** (`n_layers=30` × 2, `chatterbox.rs:440`) each shaped `[MAX_SLOTS, kv_heads(16), MAX_SEQ, head_dim(64)]` (`:454-455`), via a CUDA `Allocator` (`memory.rs:153`) + `Tensor::<T>::new(&cuda_alloc, shape)` (`create.rs:108`), dtype per `input_types()`.
- Each `SlotId` permanently owns row `[slot, …, …, …]` for its stream's lifetime. A slot writes its new K/V at its own `seqlens_k` (its TRUE history length) — **never re-left-aligned to a cohort `max_past`** (that artifact exists only because the host buffer is batch-shared and rebuilt each stride). The GQA op "appends the new K at buffer index `seqlens_k`" (`chatterbox.rs:706`) and "rotates each cached key at its ABSOLUTE buffer index" (`:705`), so a private LEFT-aligned buffer is self-aligning: the new K lands at the same absolute index it already occupies, prior keys never move, **no host gather, no re-pack**.
- The batched `run_binding` operates on the ACTIVE subset (compacted active_rows → their fixed ring rows) with `attention_mask`/`seqlens_k` built per active row exactly as today (`:711-717`).
- **NEVER re-alloc on cohort/epoch change.** Membership churn = which rows are bound this tick, NOT a `Tensor::new`. This satisfies BOTH the GB10 OOM guardrail (per-stride/per-epoch growing alloc is the twice-observed box-kill, `ep.rs:97-109`) AND the CUDA-graph fixed-address precondition (§7, lever 4).

**Concrete byte budget (gate at admission):** `MAX_SLOTS × 30 × 2 × 16 × MAX_SEQ × 64 × sizeof(dtype)`. With `MAX_NEW_TOKENS=1000` (`chatterbox.rs:35`) bounding the AR length, `MAX_SEQ` must = `max_prefill_len + MAX_NEW_TOKENS`. Measured: f32 @ `MAX_SEQ=2048`, `MAX_SLOTS=64` = **30 GiB** (62.5% of the 48 GiB cap); F16 @ `MAX_SEQ=1536` = **11.25 GiB** (23.4%). Wire `kv_bytes_per_slot` into the existing admission `slot_cap` (`backend-api/src/lib.rs` `slot_cap`/`vram_cap = (free-weights)/kv_bytes_per_slot`) bounded by `cuda_arena_limit_bytes` (`ep.rs:37-53`) so an over-large `MAX_SEQ×MAX_SLOTS` SHRINKS `MAX_SLOTS` (clean refusal), never a runaway alloc.

### 3.3 Prefix cache is OUT OF SCOPE for the device-KV landing (enforced in code)

`PrefillState(Vec<f32>)` (`prefix_cache.rs:224`) stores HOST f32 rows; `prefill_state_from_kv` does `t.data.as_f32().ok_or("prefix-cache needs f32 KV")` (`chatterbox.rs:~1729`). An opaque `ort::Value` has no `as_f32()`. The device-KV step path lands with the cache **provably unarmed** (`kv_residency_regime()` returns `HostKv`/`DeviceKvOrt`, both `reuse_is_strict_win()==false`). This is already the production state (the cache is shelfware — `arm_prefix_cache_if_win` is a deliberate no-op on the host export). A device-resident `PrefillState` (device-handle store) is a **separate future project**, NOT this plan.

---

## 4. Feature implementation + dependency order

### Dependency DAG

```
Phase 0  (regime arbiter instance-aware + DeviceKvOrt variant)        [no behavior change]
   └─> Phase 1  (backend-api IoBinding device-KV spec, pure data)     [no behavior change]
          └─> Phase 2  (backend-ort run_device_kv sibling + Phase-0 ping-pong TOY gate)
                 └─> Phase 3  (re-export static past_present_share_buffer GQA graph + waav.json variant)   ← PREREQUISITE, not optional
                        └─> Phase 4  (chatterbox device-KV decoder: SlotId ring, prefill→device, recycle-clean; flip for ONE cell)
                               ├─> Phase 5  (q4f16 device-KV — the SHIPPED default cell)
                               └─> Phase 6  (CUDA-graph capture, bucketed by cohort width)
Phase 7  (ORT accel catalog — fuse+serialize, SDPA-pin, conv knobs)   [PARALLEL, independent of device-KV]
Phase 8  (codec cohort decode — separate additive seam)               [PARALLEL, lowest value]
```

Note the critical reordering vs the input design: **the static `past_present_share_buffer` re-export (Phase 3) is a PREREQUISITE, not a later phase.** IoBinding-only on the growing export reproduces the measured 0.77× regression (`chatterbox.rs:589-603`) and on the coherent GB10 pool removes a near-zero host alias. There is no device-resident KV without the static export. The IoBinding ping-pong is only meaningful ON that export.

### 4.1 `run_device_kv` (backend-ort, sibling — Phase 2)

`backend-ort/src/lib.rs`, a NEW method (do NOT touch `run` `:438` or `run_bound` `:489`):

- Build a CUDA `Allocator`: `Allocator::new(&session, MemoryInfo::new(AllocationDevice::CUDA, 0, AllocatorType::Device, MemoryType::Default))` (`memory.rs:153,400`) — **NOT** `CUDA_PINNED`/`CPUOutput` (the `:515-527` trap).
- Hold a per-session `DeviceKvRing` (separate field from `bound_state` — they never share an epoch slot; chatterbox's `language_model` and `conditional_decoder` are different `OrtModel`/`Session`s, so no interleave): the 60 KV device `Value`s allocated ONCE (§3.2) at `input_types()` dtype (per-name, `precision.rs`).
- **Two-buffer ping-pong, NOT same-Value-to-both-names.** `bind_output` CONSUMES the Value by move (`io_binding.rs:144`); `run_binding` returns FRESH handles in `SessionOutputs<'b>` (`mod.rs:340`), not the bound buffer. So: pre-allocate ping `A` + pong `B` per (slot,layer,kv); stride N binds `A` as `past.*` input (`bind_input(name,&A)`, `io_binding.rs:123`) + `B` as `present.*` output (`bind_output(name,B)`); stride N+1 swaps. With the **static share-buffer export** the in-place write means a single ring buffer per (slot,layer,kv) suffices and ORT's GQA kernel writes present over past at `seqlens_k` — the Rust API never aliases one Value across two names. The Phase-0 TOY gate pins exactly which (single static buffer vs A/B swap) the re-exported graph requires.
- Feed only `inputs_embeds` (F32), `attention_mask`/`total_sequence_length`/`position_ids` (I64) as varying HOST inputs; extract ONLY `logits` (the one bounded D2H the ledger allows, `chatterbox.rs:778-781`). NEVER per-stride KV D2H.
- **Non-CUDA EP**: return a typed `not-supported` (defense-in-depth; routing in §3.1 ensures it is never reached on non-CUDA).
- **Allocator is Send-not-Sync** (`memory.rs:82-83` "the CUDA allocator can sometimes crash when used on multiple threads") — keep the ring + allocator on the single `codec-ar-mux` thread. Add a `static_assertions::assert_not_impl_any!(DeviceKvRing: Sync)` so a future cross-thread refactor fails to compile.

### 4.2 `IoBinding` device-KV spec (backend-api, pure data — Phase 1)

Extend `IoBinding` (`backend-api/src/lib.rs:240`) with a pure-data carrier: `Vec<DeviceKvBuffer { in_name, out_name, shape: Vec<i64>, dtype: ElemType, slot_keyed: bool }>` + `device_kv: bool`. Host/CPU/non-CUDA backends ignore it and fall back to `run_bound` default (`:372` `run(all_inputs())`) bit-identically. NO `ort` type crosses; `forbid(unsafe)` stays. The concrete `ort::Value` lives only in `backend-ort`'s `DeviceKvRing`; the core crate names only this backend-api spec.

### 4.3 The static `past_present_share_buffer` GQA re-export (Phase 3 — THE prerequisite)

Re-export `language_model.onnx` to static BNSH `[B,16,MAX_SEQ,64]` GQA buffer-sharing (`present.*` aliasing `past.*` in place). The GQA `seqlens_k = ReduceSum(attention_mask)-1` math is UNCHANGED (`chatterbox.rs:659-667`) — only WHERE the buffer lives and that it stops growing changes; bit-identity holds by construction. Ship as a NEW `waav.json` weights variant (`language_model_share.onnx`) so registry selection is zero-code. Produce via the chatterbox export source (the GQA contrib-op `past_present_share_buffer=1` recipe; `builder.py` is the reference producer).

**This is a DISTINCT model variant**, NOT a transparent weights swap: it adds `total_sequence_length`/`seqlens_k` graph inputs and a static KV shape that the legacy host `lm_forward_batched` cannot feed. So: keep the growing-buffer graph (`language_model.onnx`) as the canonical host-fallback; the static graph is a **device-KV-CUDA-only variant**; a CPU/non-CUDA load of that registry entry hard-falls-back to the growing graph (host path). The "host fallback is bit-identical for every variant" invariant holds for the growing graph, NOT the share-buffer graph.

**Known seam risk to confirm in Phase 3:** WaaV's GQA exports carry an `attention_bias` the ORT-CUDA GQA kernel rejects (the existing `onnx_drop_gqa_bias.py` surgery). The static-share-buffer re-export must ALSO drop/relocate the bias AND emit the static buffer — confirm these compose on the real graph before trusting the path.

### 4.4 The device-KV `lm_forward_batched` variant (Phase 4)

Replaces the host wall (`:683-839`) when `device_kv_step_enabled()`:
- Build the active-SlotId → ring-row index map (O(B), the only residual of the old host gather).
- Bind each active slot's ring row as `past.*`/`present.*`; feed `attention_mask` (per-row LEFT-justified, `:711-717`) + `total_sequence_length`; drop the host rebuild (`:738-771`) and readback (`:805-835`).
- `run_device_kv`; extract only logits; per-row argmax with penalty (`argmax_row_with_penalty`, unchanged, `:799`).
- The per-slot remove/insert + Err re-home structure (`:953-1011`) is REUSED — but the device handle is **SlotId-owned in the ring** (never `mem::take`-out, §4.6). `SlotDecode.past` for the device path holds NO host Vec; logical anchors (`attn_len`/`generated`/`next_pos`) advance ONLY on Ok (`:993-998`).

### 4.5 Recycle-is-clean (privacy-critical, §1.2 — Phase 4)

On `reset_slot`/`drop_slot` (`:1627`/`:1531`) of a SlotId, **zero the recycled ring row** (device memset) + reset its `seqlens_k`/`attn_len`/`generated`/`next_pos` to prefill-zero. A fixed ring row is never freed (that is the point), so "clean" = explicit zero, NOT drop. Two-layer defense: (1) the mask argument — GQA reads only `[0..=seqlens_k)`, and a recycled tenant B's mask starts at its own prefill, so A's residual bytes at `>= seqlens_k_B` are masked exactly as the existing RIGHT-pad cells; (2) explicit zero so the invariant survives any future non-causal read. Zero-on-recycle uses the ring's CUDA allocator/stream and completes before the prefill write (gate on real CUDA for stream ordering).

### 4.6 Concurrent mid-batch reject (§4.4 — Phase 4)

Host path: `step_slots_batched` `mem::take(&mut st.past)` (`:981`) then Err re-inserts `st` with `past` **empty** (`:1006-1008`) — safe for host because the next step regenerates. For the in-place device ring this is a desync hazard. **Fix:** the device ring row is SlotId-owned, never moved out; advance the logical position (`attn_len`/`generated`) ONLY on Ok (`:993-998`), and on ANY `run_binding` error leave `attn_len` unchanged so the next stride's `seqlens_k` is unchanged — a partial device write lands ENTIRELY at cell `== seqlens_k` (the next write target) and beyond, i.e. re-derivable scratch, invisible to the committed-history read. Wire the advance through the scheduler's `RingKvCache::append_where(active=run_binding_succeeded)` pattern (the won't-compile `MaskedCell::set_where` gate) so an ungated advance cannot compile.

### 4.7 Prefill → device-ring handoff (Phase 4)

Prefill produces host KV via `lm_forward` (`:1098-1127`) — geometry differs (prefill_len frames). On a slot's FIRST batched step, scatter its host prefill KV LEFT-aligned into its device ring row at `[0..prefill_len]` — a **bounded one-time H2D per stream** (the single tolerated host→device copy, the analog of `kv_from_prefill_state` `:1753`). Add it to the D2H/H2D ledger as a distinct once-per-stream kind. Staggered cohorts cross at different wall steps, so the scatter is a per-row partial upload into the LIVE ring (one row, prefill_len frames), NOT a whole-buffer rebind. Guard `prefill_len ≤ MAX_SEQ` (hard-reject or fall back to HostKv on overflow). Bit-gate that codes after the host-prefill→device-step handoff equal the all-host reference.

---

## 5. Hardware portability — CUDA device-KV + graceful host fallback

- **CPU + every non-CUDA EP**: `device_kv_step_enabled()` returns false (it gates on `ActiveEp::Ep(EpKind::Cuda)`, §3.1). `step_batch` takes the host `lm_forward_batched` from the start; `run_device_kv` is never reached. The GQA LEFT-align/`seqlens_k` identity is EP-agnostic (`chatterbox.rs:659-667`), so the host path is bit-identical on CPU and CUDA. **This is the price of portability: a slower CPU path, never a regression.**
- **ROCm / MiGraphX** (`is_accelerated()`, `ep.rs:141`) have device memory; `run_bound` already branches `Rocm|MiGraphX → HIP_PINNED` (`lib.rs:520-522`). Parameterize the device-KV allocator by `ActiveEp` (`device_kv_allocator(active) -> Option<AllocationDevice>`: `Cuda→CUDA`, `Rocm|MiGraphX→HIP`, else `None`) so ROCm is a FUTURE per-EP cell (not foreclosed), but stays HostKv until separately gated. Do NOT hardcode CUDA-only into the seam.
- **Unified/coherent (GB10, CoreML/Apple)**: `buf_type==Coherent` means the host "transfer" is a coherent alias, so the GB10 win is removing the per-stride **re-extract + re-alloc** (Phase 3 static buffer), NOT the H2D copy. Re-baseline Phase-1's "partial win" explicitly: on a coherent pool IoBinding-only may measure flat — which is why Phase 1 is a TOY-graph seam-proving step, and the production lever is the static buffer (Phase 3+). On coherent memory, recycle-zero (§4.5) is doubly required (freed device bytes are host-coherent-readable).
- **Discrete CUDA vs unified CUDA**: both are `EpKind::Cuda` but have opposite arena strategies (`ep.rs:114-116` `SameAsRequested` only on `is_gb10`). Add `is_gb10()` to the cell key: flip the regime for `(model, share-export, is_gb10, precision)`; a discrete CUDA dGPU stays HostKv until separately gated (matches the existing `is_gb10`-scoped TF32/arena discipline).

---

## 6. Multi-quant support (per the actual KV tensor dtype, not the weight quant)

Device-KV residency is a function of the **KV TENSOR declared dtype** (`empty_kv_dtype_for`, `precision.rs:59`), NOT the weight quant. There are only TWO bindable KV dtypes (`ElemType` has F32/F16/I64/Bool, NO BF16, `backend-api/src/lib.rs:13-19`):

| Precision (weight quant) | KV tensor dtype | Device-KV cell | Plan |
|---|---|---|---|
| **fp32** (`language_model.onnx`) | F32 | **F32-KV cell** | Phase 4 — prove FIRST (lossless host `run` reference). |
| **fp16** (`language_model_fp16.onnx`) | F16 | **F16-KV cell** | shares the F16 cell. |
| **q4f16** (`language_model_q4f16.onnx`, MatMulNBits + F16 KV) — **the SHIPPED DEFAULT** | F16 | **F16-KV cell** | Phase 5 — the live-default cell. Mixed-dtype bind: `inputs_embeds` F32, `attention_mask` I64, KV F16, `logits` F32 — resolve PER-INPUT-NAME via `input_types()`/`F32_FEATURE_INPUTS` (`precision.rs:21`), NEVER per-graph. |
| **q4** (`language_model_q4.onnx`) | F16 (KV) | F16-KV cell | rides F16 cell. |
| **bf16** | Other → F32 fallback (`precision.rs:78`) | **UNSUPPORTED at seam** | `ElemType` has no BF16; `to_ort_value`/`extract` reject it. Stay HostKv; hard-error a bf16-KV graph rather than silently mis-bind. A future bf16 cell needs `ElemType::BF16` + `TensorData::BF16` + `to_f32_vec` widening first (separate prerequisite). |
| **int8** | int8 is a WEIGHT quant; KV stays F16/F32 | rides whichever KV cell | "int8 KV device-residency" is a category error. Forbidden on CUDA EP anyway (`guard_precision_ep`). |
| **NVFP4** | no `ElemType`/`TensorData` mapping → Other | UNSUPPORTED at seam | stay HostKv-or-unloadable. |

**Two device-KV cells to prove: F32-KV (fp32 export) and F16-KV (fp16/q4f16/q4 exports — identical binding).** Allocate the device ring at `empty_kv_dtype_for` dtype; for F16 there is no lossless widening safety net (`f16::from_f32` is bit-exact only for f16-origin values, `precision.rs:97`), so the F16 cell needs its OWN bit-gate on a ragged-mid-finish cohort. The bf16-batched GEMM reduction-order code-flip floor (B23 scar, `BATCHING-ANALYSIS-SYNTHESIS.md:29`) is a **tch-only** concern (different backend, GEMM-order); it does NOT apply to the chatterbox-ONNX GQA device-KV move (same kernel, no reduction-order change) — do not let it block this work.

**The shipped-default ordering correction:** because q4f16 is the live default, EITHER (a) make the **F16-KV cell the first device-KV target** (its F16 KV is the more valuable cell), OR (b) flip `waav.json` `language_model` to fp32 as a documented prerequisite of the fp32-first rollout (the G5 recommendation, which the host-KV bottleneck already motivates — `BATCHING-ANALYSIS-SYNTHESIS.md` G5). This plan recommends (b) for Phases 3-4 (fp32 has a lossless host reference, de-risks the seam), then Phase 5 proves F16/q4f16 and restores the shipped default. State per phase WHICH graph file each gate loads (the CUDA test gates already load `language_model.onnx`/fp32; production loads `language_model_q4f16.onnx`). Phase 4.5: produce + bit-verify `language_model_q4f16_share.onnx` (static buffer + MatMulNBits) against the growing q4f16 graph BEFORE wiring device-KV to it — Phases 3 (fp32 static) and 5 (q4f16 quant) do NOT compose for free.

---

## 7. ORT accel catalog (Phase 7, PARALLEL track — independent of device-KV)

Ranked, each accuracy-neutral + bit-gated, GB10-scoped behind env flags like the existing `WAAV_ORT_TF32`:

1. **Offline fuse + serialize** (`optimize_model` O2, **fp16 OFF**) for whisper/chatterbox encoder+decoder graphs + `with_optimized_model_path` to kill per-cold-start re-prepacking. EP-portable (helps CPU too). The highest accuracy-preserving compute win that does NOT depend on the KV fix; helps STT and codec-AR. Bit-verify each fused export. (`ORT-PERF-FEATURES.md §2`.)
2. **SDPA-pin** (`with_attention_backend`, `ep/cuda.rs:343`) once (1) lands fused `MultiHeadAttention`/`GroupQueryAttention` nodes. NEVER FlashInfer on `sm_12x`; ORT's cuDNN/efficient flash via `sdpa_kernel` (`INFER_PERF` 40-135× on the attn op). Bit-gate (reduction order). (`§8`.)
3. **CUDA-graph capture on the AR step** (`with_cuda_graph`, `ep/cuda.rs:255`) — **gated on Phase 3 static shapes** (the growing `[B,H,past+1,D]` breaks the fixed-address precondition). The verified language_model.onnx has **NO control-flow ops** (flat 30× GQA+MatMul; the AR loop is Rust-side one-run-per-stride) so capture is op-legal. `Session` is unconditionally Send (`mod.rs:675`) so no trait break; the single `codec-ar-mux` thread satisfies the same-thread-capture-and-replay rule. Bucket by cohort width (`gpu_graph_id` per B; `serve.rs:552` `COHORT_WIDTH_METRIC` is the live distribution). **Precondition beyond static shapes:** the GQA write index `seqlens_k`/`total_sequence_length` must be a graph INPUT read at replay, not a capture-time constant — gate a "graph replays across 3 different seqlens_k, codes byte-identical" unit test BEFORE wiring (the tch backend needed a device-scalar `GraphState` for exactly this; confirm ORT GQA reads it from the runtime input). tch proved 1.04-1.20× on the analog.
4. **Conv knobs** (`with_prefer_nhwc:313`, `with_fuse_conv_bias:349`, `with_conv_algorithm_search=HEURISTIC:180`) for the chatterbox S3Gen `conditional_decoder` + conv-front-end STT, in `ep.rs` inside the `EpKind::Cuda` arm (no-op on non-CUDA). **`do_copy_in_default_stream=0` is DELETED** (removed from ort, `cuda.rs:211`); if copy/compute overlap is later wanted, use `Session::run_async` (`mod.rs:402`) as a scheduler change, separately benchmarked — do NOT promise the removed flag.

**Process-wide-knob caveat:** `ep.rs` EP config is applied per-session-build but a global env flag flips it for EVERY ONNX graph (supertonic flow maxΔ=0.0, whisper, omnivoice CFG). Either thread per-graph knob selection through `load_ep`, or the bit-identity gate must re-run ALL ONNX archs under the flag (supertonic flow + whisper transcript + omnivoice CFG-RNG gates), not just the chatterbox codes gate.

---

## 8. Batching orchestration: how device-KV plugs into step_batch/cohorts

- The device-KV path lives ENTIRELY below `ArStepModel::step_batch` (`chatterbox.rs` override). The driver's one-`step_batch`-per-tick contract (`driver.rs:256`) and `serve.rs` active_rows compaction are UNCHANGED.
- Per arch: only chatterbox-ONNX (the one codec-AR ONNX LM) reaches the seam and overrides. tch codec-AR (dia2/csm/qwen3) are already device-resident via tch `KvCache` and ride one-shot `synthesize` (`as_stepped()=None`) — a SEPARATE Path-B workstream (G2), not this. Diffusion/flow ride step-bucket cohorts, never reach the seam.
- The device ring (§3.2) keyed by stable `SlotId` is the bridge between the compacted cohort (which rows bind this tick) and persistent residency. A finished slot's row is zeroed + recycled (§4.5); a new slot scatters its prefill into its row (§4.7).

---

## 9. EXTREME-TDD test gates (failing-first, one per failure-case / arch / quant / HW)

Every gate below is RED-first (authored to fail before the code exists). CUDA gates run isolated via `ci/heavy_live_tests.sh` (wired as a merge gate on GB10, NOT a manual script — the existing live gates are `#[ignore]`d, so device-KV MUST add its CUDA gate to the heavy-list AND keep a deterministic non-CUDA twin on every `cargo test`).

**Phase 0 (always-run, deterministic, no GPU):**
- `kv_residency_regime_is_hostkv_off_cuda()` — CPU `OrtModel` double ⇒ `HostKv`. (forces the `&self` signature)
- `kv_residency_regime_is_hostkv_for_f16_until_proven()` — q4f16 graph ⇒ `HostKv`/`DeviceKvOrt`, not armed.
- `device_kv_step_disabled_on_cpu_and_rocm()` — `device_kv_step_enabled()==false` for CPU/ROCm/CoreML.
- `arm_prefix_cache_declines_under_device_kv_ort()` — `DeviceKvOrt` ⇒ `arm_prefix_cache_if_win()==false` (the decoupling).
- `device_kv_value_dtype_equals_graph_input_type()` — pure dtype-selection fn: `input_types()=F16` ⇒ ring allocates F16 (assert the `ElemType` passed to the allocator, no CUDA).
- `each_codec_ar_arch_default_regime_is_hostkv()` — enumerate registered codec-AR models, assert HostKv until per-arch proven.

**Phase 2 (Phase-0 device-Value lifetime pin — CUDA-gated, isolated; deterministic twin skips clean on CPU):**
- `device_ping_pong_two_buffer_bit_identical_to_host_run()` — bind one output Value as next input across N strides on the REAL `language_model.onnx`, assert byte-identical to the host `run` loop. Pins rc.12 lifetime + the A/B-vs-single-buffer mechanic.
- `compile_fail` doc-test — holding `SessionOutputs` borrow across the next `run_binding` does not compile (codifies the alias safety the borrow checker gives, `mod.rs:340` `'s:'b`).
- `assert_not_impl_any!(DeviceKvRing: Sync)` static-assert (the Send-not-Sync allocator, `memory.rs:82`).
- `run_bound_8_cfm_gates_unchanged()` — the existing CFM/Supertonic `run_bound` gates (`lib.rs` `run_bound_matches_run_bit_identical:3166`, `run_bound_keeps_io_on_device:3191`, …) stay GREEN (sibling method untouched them).

**Phase 4 (the no-regression core — CUDA-gated live + deterministic twins):**
- `host_vs_device_kv_codes_identical_ragged()` — **THE oracle.** Construct the reference model with an EXPLICIT `force_host_kv`/`with_kv_regime(HostKv)` override and the SUT with device-KV; assert codes byte-identical PER RAGGED STRIDE on a staggered mid-finish cohort, depth ≥ the ring-wrap horizon. (Without the override, both sides consult the same arbiter ⇒ device-vs-device, no host reference survives — the P0 the input design missed.)
- `device_ring_recycle_is_clean_bit_identical()` — prefill A long, run N, drop A, recycle SAME SlotId for B short (A's tail outlives B's window), assert B's codes == a never-recycled fresh-row run. Run WITH zero-on-recycle on AND off (research check on the mask argument). CUDA-only.
- `device_kv_mid_batch_backend_error_re_homes_each_slot()` — fake backend errors on slot 3 of 4; assert all 4 retain their ring rows, the committed-prefix cells `[0..seqlens_k)` unchanged, and the re-tried codes == a clean run.
- `prefill_to_device_handoff_codes_identical_to_all_host()` — codes after the host-prefill→device-step handoff == all-host reference (staggered arrivals).
- `device_kv_b16_not_slower_than_host_kv_b16()` — fails if device path is slower than the CURRENT host baseline at the same B (catches the 0.77× trap). Separate from the doc-curve gate (which must NEVER ratchet a constant down).
- Re-keep GREEN: `batched_forward_codes_identical_to_per_slot`, `live_ragged_batched_forward_bit_identical_and_scales`, the AR-compounding identity test — now rewired so the reference is `with_kv_regime(HostKv)`.

**Phase 5 (q4f16 / F16-KV cell):**
- `f16_device_kv_codes_identical_to_host_kv_ragged()` — the F16 cell's own ragged-mid-finish CODES gate (no lossless widening safety net).
- `q4f16_share_export_decodes_identical_to_growing_q4f16()` — Phase-4.5 artifact gate: `language_model_q4f16_share.onnx` == growing q4f16 on a fixed prompt.
- `bf16_kv_graph_stays_hostkv_or_hard_errors()` — a bf16-KV input ⇒ HostKv (or typed not-supported), never a silent f32 mis-bind.

**Phase 6 (CUDA-graph):**
- `cuda_graph_replays_across_3_seqlens_k_codes_identical()` — capture, replay at 3 different `seqlens_k`, codes byte-identical (pins the per-position write-index-from-runtime-input precondition) — BEFORE wiring the decoder.
- `cuda_graph_invalid_on_growing_export_is_rejected()` — capture on the growing graph is refused (fixed-address precondition).

**Phase 7 (accel catalog):**
- `fused_export_decodes_identical()` (codes/transcripts), `supertonic_flow_maxdelta_zero_under_flag()`, `omnivoice_cfg_rng_order_identical_under_flag()`, `sdpa_pin_codes_identical()` — each ONNX arch the (possibly process-wide) knob touches.

**Phase 8 (codecs):**
- `decode_audio_batch_row_b_identical_to_decode_audio_b()` per regime (f32/dia2, bf16/csm) on a RAGGED cohort (right-pad + per-row crop), NOT equal-length only. `rvq_decode_batch_bf16_accumulation_order_identical()`. Demote "bit-identical by construction" → "bit-identical under a RED Δ==0 gate"; the default `decode_audio_batch`→per-slot delegate IS by-construction, but any real batching override is NOT.

---

## 10. Failure cases + mitigations (consolidated)

| Failure | Mitigation (verified hook) |
|---|---|
| Pinned-host output trap (0.77×) | bind KV outputs to `AllocationDevice::CUDA`/`Device` (`memory.rs:400`), NOT `CUDA_PINNED`/`CPUOutput` (`lib.rs:515-527`); extract ONLY logits. Phase-2 ping-pong gate asserts no per-stride KV D2H. |
| Growing-shape re-alloc churn + GB10 OOM | Phase 3 static buffer is the PREREQUISITE (Phase 1 = toy only); ring allocated ONCE at MAX_SLOTS (§3.2); arena cap on (`ep.rs:110-116`). |
| B-index instability / ragged drift | SlotId-keyed ring (§3.2), per-slot true `seqlens_k`, NO re-left-align; `host_vs_device_kv_codes_identical_ragged` + mid-finish gate. |
| Quant dtype mismatch | per-input-name dtype from `input_types()` (`precision.rs`); bf16/Other ⇒ HostKv/hard-error (§6); `device_kv_value_dtype_equals_graph_input_type` (no-CUDA) gate. |
| Slot-recycle contamination (privacy) | zero-on-recycle (§4.5) + `device_ring_recycle_is_clean` gate; doubly required on coherent GB10. |
| Mid-batch reject desync | SlotId-owned handle (never `mem::take`-out), advance-on-Ok-only, partial write = scratch ≥ seqlens_k (§4.6); fault-injection gate. |
| Prefill→device handoff | one-time bounded H2D scatter per stream, ledgered, `prefill_len ≤ MAX_SEQ` guard, handoff codes gate (§4.7). |
| rc.12 device-Value lifetime / alias | two-buffer swap (NOT same-Value-both-names); `bind_output` consumes (`io_binding.rs:144`), `run_binding` returns fresh handles (`mod.rs:340`); Phase-2 ping-pong + compile-fail gates. `Tensor::clone` is an Identity-session run (NOT memcpy) — never on the hot path. |
| CUDA-graph premature capture | gated on Phase 3 static shapes + the 3-seqlens_k replay gate; bucket by cohort width. |
| CPU / non-CUDA | `device_kv_step_enabled()==false` off CUDA (§5); host path bit-identical (EP-agnostic GQA identity). |
| Regime-flip compounds two changes | step-routing axis ≠ prefix-arm axis (§3.1); `DeviceKvOrt` keeps cache unarmed + cost model truthful. |

---

## 11. Executable roadmap (developer step-by-step)

1. **Phase 0 — regime arbiter (no behavior change).** `kv_residency_regime(&self)`; add `device_kv_step_enabled(&self)`; add `KvResidencyRegime::DeviceKvOrt` (`prefix_cache.rs:695`, `reuse_is_strict_win=false`, `prefix_host_transfer_rows=matched`); rewire `arm_prefix_cache_if_win` (`:528`) + the `prefix_cache_arm_decided_by_kv_residency_regime` test. Land the 6 always-run Phase-0 gates RED→GREEN.
2. **Phase 1 — backend-api IoBinding device-KV spec (pure data).** Extend `IoBinding` (`:240`); host fallback ignores it. `forbid(unsafe)` stays.
3. **Phase 2 — backend-ort `run_device_kv` sibling + Phase-0 device-Value pin.** CUDA `Allocator`, `DeviceKvRing` (separate field), two-buffer ping-pong, extract-only-logits, typed-not-supported off CUDA. Land the Phase-2 gates; keep the 8 CFM `run_bound` gates green.
4. **Phase 3 — static `past_present_share_buffer` re-export (PREREQUISITE).** Re-export fp32 `language_model.onnx` → `language_model_share.onnx` (drop GQA bias + static buffer); ship as a waav.json device-KV-CUDA-only variant; CPU loads of it fall back to the growing graph.
5. **Phase 4 — chatterbox device-KV decoder (fp32 cell).** SlotId ring (§3.2), device `lm_forward_batched` (§4.4), recycle-zero (§4.5), mid-batch-reject (§4.6), prefill→device (§4.7). Flip `device_kv_step_enabled` for `(chatterbox, share-export, is_gb10, fp32)` ONLY. Land all Phase-4 gates; re-measure B1..B64. (Optionally flip waav.json LM to fp32 here per G5.)
6. **Phase 5 — F16/q4f16 cell.** Phase-4.5 produce+verify `language_model_q4f16_share.onnx`; ring allocates F16 per-input; land the F16 gates; restore q4f16 as the shipped default cell.
7. **Phase 6 — CUDA-graph.** 3-seqlens_k replay gate first, then `with_cuda_graph` + `gpu_graph_id` per cohort width.
8. **Phase 7 — accel catalog (parallel).** fuse+serialize → SDPA-pin → conv knobs; per-arch bit gates; `do_copy_in_default_stream` deleted.
9. **Phase 8 — codec cohort decode (parallel, lowest value).** `decode_audio_batch` default→per-slot; thread B through tch RVQ (DAC first); right-pad+crop ragged; Δ==0 B>1 gates.
10. **Promote doc constants** (`CHATTERBOX_HEADLINE_PEAK_BATCH_SPEEDUP/_BATCH`) ONLY after the live curve gate re-measures — NEVER as a target, only post-measurement; never ratchet a constant down.

**Honest baseline:** the only in-repo measurement is ~1.8×@B16 (chatterbox.rs); ~30×@B64 is the tch device-resident probe (`BATCHING-ANALYSIS-SYNTHESIS.md`), a hypothesis to verify on the ORT static-buffer export, not a promised number. Pull the live `COHORT_WIDTH_METRIC` histogram before committing Phases 4-6 to confirm the cohort right-tail is fat enough to be worth it.
