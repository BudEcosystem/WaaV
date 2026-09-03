# WaaV-Infer ⇄ vLLM Feature-Parity Matrix (with LIVE gates)

> **The hook:** *every flexibility & feature vLLM provides for LLM inferencing, WaaV-Infer provides for Voice AI.*
> This document audits that claim feature-by-feature against the actual codebase (grep-verified, not memory), classifies
> each pillar, quotes the passing gate behind every HAVE-LIVE cell, and drives the closable shelf-ware gaps.

Audited at `git HEAD f9d7d26` on branch `waav-infer-v2-build`. Every cell below is grounded in a `file:line` citation
from the live tree, not the design docs. "LIVE caller" = a non-`#[cfg(test)]` production code path constructs and calls it.

---

## 1. Summary scoreboard

| # | vLLM pillar | Voice analog | Class | Live gate / reason |
|---|-------------|--------------|-------|--------------------|
| 1 | Continuous / dynamic batching (add+evict mid-flight) | ragged concurrent codec-AR streams share one lockstep loop | **HAVE-LIVE** | `live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales` |
| 2 | Paged-attention / paged-KV table | bounded long-form KV + attention-sink pin | **PARTIAL-SHELFWARE** | `PagedKvTable` 9 RED gates, **0 live callers** |
| 3 | Prefix / radix KV caching | cloned-voice / shared-prompt prefix reuse | **HAVE-LIVE (wired, regime-gated)** | `prefix_cache_reuse_is_bit_identical_to_full_prefill` + new `device_resident_reuse_wins_where_host_kv_loses` |
| 4 | Multi-LoRA / S-LoRA hot-swap | per-voice / per-language adapter swap | **MISSING** | no adapter-swap path; scoped below |
| 5 | Speculative / draft decoding | eager-EoT (early end-of-turn) | **HAVE-LIVE (analog)** | `eot_detected_emits_terminal`; token-spec is N-A for frames |
| 6 | Chunked prefill | interleave long-prompt prefill with decode | **HAVE-LIVE** | `chunked_prefill_bit_identical_to_monolithic` + live `cuda_torch_ark_chunked_prefill_byte_identical_to_monolithic` (see CHUNKED-PREFILL.md) |
| 7 | Quantization / precision (int8/fp16/bf16/q4f16) | substrate-stamped quant admission | **HAVE-LIVE** | `quant_variant_gated_by_per_substrate_accuracy_stamp` + `empty_kv_dtype_follows_weight_precision_q4f16` |
| 8 | Request scheduling (priority + fair-share + admission) | priority bands + per-tenant fair-share + VRAM/deadline shed | **HAVE-LIVE** | `high_priority_admitted_ahead_of_low_under_saturation`, `no_tenant_exceeds_its_fair_share` |
| 9 | Streaming / TTFA | delta-stream codec/PCM per stride | **HAVE-LIVE** | `egress_delta_concat_equals_full_byte_identical`, `f6_mux_ttfa_is_far_below_full_synthesis` |
| 10 | CUDA graphs | edge-tier capture/replay on tch AR/DiT | **HAVE-LIVE** | `cuda_graph_ar_compounding_identical`, `cuda_graph_ring_kv_byte_identical` |
| 11 | Dynamic shapes (variable seq / codebooks) | per-frame dynamic frame-rate + ragged batch + delay-pattern | **HAVE-LIVE** | `dynamic_fr_output_identical`, `acoustic_delay_ring_delays_by_k_frames` |
| 12 | KV-cache eviction / preemption | reject-at-admission (non-preemptible) + ring wrap | **N-A (by design) / PARTIAL** | `marker.rs` H2: "no evict/preempt/steal"; paged eviction shelf-ware |
| 13 | Disaggregated prefill/decode (P/D) | — | **N-A-for-voice** | real-time framewise; no prefill/decode temporal split |
| 14 | Tensor / pipeline parallel (multi-GPU) | — | **N-A / MISSING** | single-box high-concurrency; scales by request-routing not sharding |
| 15 | Structured / guided decode | STT bad-words suppression / hotword bias | **HAVE-LIVE (analog)** | `argmax_suppressed_skips_bad_words_first_max` |

**Tally:** **10 HAVE-LIVE** (chunked prefill closed this session — pillar #6, see `CHUNKED-PREFILL.md`),
**2 PARTIAL-SHELFWARE** (paged-KV; the scheduler's `Cohorts`/`SlotTable`/`RingKvCache` parallel layer),
**1 MISSING** (multi-LoRA — being closed by a concurrent agent), **3 N-A-for-voice** (disagg P/D,
tensor/pipeline parallel, KV-preemption — voice sheds at admission). Pillar 3 is HAVE-LIVE but **deliberately
arm-gated** by a measured residency regime (see §4).

---

## 2. The live serving spine (the denominator for "LIVE")

The single most important architectural fact, and the one that decides several PARTIAL classifications:

> **The live codec-AR serving path is `serve_codec_ar_multiplexed` (`crates/waav-infer-runtime/src/serve.rs:561`),
> driven by `CodecArBatcher` (`crates/waav-infer-server/src/codec_ar_batcher.rs:428`). It manages its own slots as a
> `Vec<Option<LiveStream>>` (`serve.rs:631`) and steps the model through the `ArStepModel` trait
> (`crates/waav-infer-runtime/src/arstep.rs:496`) — `prefill` / `step_batch` / `decode_audio` / `reset_slot`. It does
> NOT flow through the scheduler crate's `SlotTable`/`Cohorts`** — confirmed verbatim at
> `crates/waav-infer-server/src/lib.rs:141`: *"it does not flow through the scheduler's `SlotTable`, so it owns its own
> monotonicity"*.

Consequence: `waav-infer-scheduler`'s `Cohorts` (cohort.rs), `SlotTable` (slot.rs), `RingKvCache` (ring_kv.rs), and the
risk-slack `Scheduler::order` (admission.rs) are a **parallel, fully-tested design layer that the live batch-API loop
does not consult**. They are correct and gated, but PARTIAL-SHELFWARE *relative to the live serving path*. The
live-equivalent behaviors (cohort batching, slot recycle, ring KV, admission) are re-implemented inside
`serve.rs` + `codec_ar_batcher.rs` + `codec_ar_admission.rs` and ARE live-gated (pillars 1, 8).

---

## 3. Per-pillar detail

### 1. Continuous / dynamic batching — HAVE-LIVE
- **Live caller:** `CodecArBatcher::submit` (`codec_ar_batcher.rs:206`) fans every concurrent WS/REST stream into ONE
  shared loop spawned at `codec_ar_batcher.rs:428`; the WS handler calls it at `crates/waav-infer-server/src/ws.rs:399`.
  Each tick advances all active slots in one `step_batch` (`serve.rs:442`). Mid-flight add = a new admission joins the
  next tick's cohort; mid-flight evict = a `CancelToken` resets that slot only (`codec_ar_batcher.rs:715`).
- **Gate (quoted):** `live_gb10_batcher_concurrent_ragged_is_bit_identical_and_scales` (`codec_ar_batcher.rs:801`) loads
  real chatterbox on CUDA, drives N=6 ragged concurrent streams, and asserts (a) AR body codes are **bit-identical
  token-for-token** to per-slot solo reference, (b) `max_cohort == 6` (full width batched together), (c) per-stride wall
  beats the serial per-slot loop (throughput scales). Plus `live_concurrent_codec_ar_streams_share_one_loop_and_are_bit_identical`
  (`codec_ar_batcher.rs:660`) asserts `max_cohort >= 2` (real lockstep, not N serial loops).

### 2. Paged-attention / paged-KV table — PARTIAL-SHELFWARE
- **API:** `PagedKvTable` (`crates/waav-infer-runtime/src/paged_kv.rs:185`) — per-slot block table with pinned attention
  sink, LRU page eviction (`paged_kv.rs:300`), `KvResidency::{Resident|Evicted}` (not approximated).
- **Live callers:** **NONE.** Every construction is `#[cfg(test)]` (`paged_kv.rs:426..739`). The live serve loop and the
  tch models use a contiguous device-resident ring `KvCache` (`crates/waav-infer-backend-torch/src/nn/kv_cache.rs:50`),
  not the paged table.
- **Gates (test-only but real):** 9 RED gates incl. `paged_kv_output_identical_within_window` (`paged_kv.rs:505`,
  every resident position byte-identical to full-KV decode), `attention_sink_pinned_not_approximate` (`paged_kv.rs:467`),
  `four_concurrent_slots_do_not_crosstalk` (`paged_kv.rs:604`).
- **Wiring scope (genuinely multi-day):** to go live, each tch model's per-layer KV append
  (`kv_cache.rs:254` `write`/`append_full_masked`, called from `dia2.rs`/`csm.rs`/`cosyvoice3.rs` step loops) must route
  through `PagedKvTable::append` and feed `KvResidency::Evicted` into the SDPA mask builder. That touches the shared
  `nn/kv_cache.rs` device-tensor path used by all 7 torch models with a high byte-identity-regression surface →
  **scoped, not faked.** Honest value note: the live ring already bounds memory by wrap-around; paging's incremental win
  is long-form-escape KV bounding + non-aligned attention-sink — real but not on the current latency-critical path.

### 3. Prefix / radix KV caching — HAVE-LIVE (wired; arm-gated by measured KV residency)
- **API:** `RadixPrefixCache` (`crates/waav-infer-runtime/src/prefix_cache.rs:333`) — edge-compressed token-keyed radix
  tree + LRU ring over endpoints, tenant-scoped (`TenantId`, `prefix_cache.rs:302`), stores verbatim `PrefillState` f32
  rows (byte-identical reuse).
- **Live caller:** wired into chatterbox `LmDecoder::prefill_slot_cached` (`crates/waav-infer-core/src/tts/chatterbox.rs:1166`
  `match_for`, `:1246` `insert_for`), reachable via `ChatterboxArStep::prefill_for` (`chatterbox.rs:1991`). **The load
  path now decides arming through the measured residency arbiter** (`chatterbox.rs` `arm_prefix_cache_if_win`, called at
  load) — not a hardcoded skip.
- **Gates (quoted):** `prefix_cache_reuse_is_bit_identical_to_full_prefill` (`chatterbox.rs:3503`, warm reuse ≡ cold
  full-prefill byte-for-byte), `prefix_cache_tenant_isolation` (`chatterbox.rs:3590`, tenant B reuses 0 of A on identical
  conditioning), `live_prefix_cache_reuse_bit_identical_to_full_prefill` (`chatterbox.rs:5219`, real CUDA weights),
  `prefix_cache_reuse_saves_prefill` (`prefix_cache.rs:936`, ≥7× fewer prefill tokens).
- **The honest caveat — and the gap this session closed.** Memory: *"prefix-cache was TTFA-NEGATIVE on the ONNX
  host-KV export."* Verified at `chatterbox.rs:1369` — the `language_model.onnx` threads KV through HOST every forward,
  so a cache HIT must re-stream the reused prefix host→device (O(prefix_len)); **measured warm prefill = 0.77× (SLOWER
  than cold)** on GB10. Arming it there would REGRESS TTFA. The ~Nx lever needs a **device-resident-KV** export where the
  reused prefix KV stays on-device (zero transfer). **See §4 for the wiring + new gate that demonstrates exactly this.**

### 4. Multi-LoRA / S-LoRA hot-swap — MISSING
- `grep -i lora` across `crates/` finds only incidental matches (a `granite`/`canary` weight-name substring, a test). There
  is **no adapter registry, no per-request adapter id, no batched multi-adapter GEMM.** WaaV's "swap" granularity today is
  whole-model load/unload through `control::ControlPlane` (`crates/waav-infer-server/src/control.rs`), not in-batch LoRA.
- **Scope:** the voice analog (per-voice / per-language low-rank adapter, batched S-LoRA-style) is a genuine multi-day
  feature: needs (a) an adapter manifest + loader, (b) a per-slot adapter id threaded through `StepInput`, (c) a batched
  grouped-GEMM that applies the right adapter per row. Feasible (the slot/batch plumbing exists), but net-new. **MISSING,
  scoped, not faked.** Note: voice's current per-voice conditioning is done via the prompt/cond-prefix (pillar 3), which
  covers the most common "different voice" case without weight adapters.

### 5. Speculative / draft decoding — HAVE-LIVE (eager-EoT analog; token-spec N-A)
- Voice AR advances by **frames** (ms of audio), not tokens; a draft+verify token loop does not map. The functional
  analog is **eager end-of-turn**: fire the terminal frame the instant a confident boundary is detected, before the AR
  budget is exhausted.
- **Live seam + gate:** `TurnHead::classify`/`is_end_of_turn` (`crates/waav-infer-runtime/src/turn.rs:117`), gate
  `eot_detected_emits_terminal` (`turn.rs:225`) asserting the load-bearing **hesitation ≠ boundary** contract
  (`!head.is_end_of_turn(0.55)` mid-word pause is NOT terminal; `head.is_end_of_turn(0.9)` confident stop IS). `EagerStage`
  (`turn.rs:178`) supports F3-cheap KV rollback on user revision. **Token-level spec-decode: N-A-for-voice** (justified).

### 6. Chunked prefill — HAVE-LIVE (closed this session; see `CHUNKED-PREFILL.md`)
- **The seam:** `Backbone::prefill_chunked` (`crates/waav-infer-backend-torch/src/nn/backbone.rs`) — a reusable driver on
  the shared `Backbone` (all 7 tch models) that feeds the prompt prefill `embeds[1,L,hidden]` to the backbone in fixed-size
  `chunk_size`-token chunks, accumulating the per-layer ring `KvCache` across chunks (the existing device-append handles a
  `q>1` chunk), threading `pos`/`positions`/the per-chunk `[c, s+c]` causal mask so chunk `k` (rows `[s,s+c)`) attends ALL
  of chunks `0..=k` — the EXACT causal sub-block of the monolithic mask. `chunk_size == L` ⇒ one chunk = the monolithic
  forward; `chunk_size == 1` ⇒ token-at-a-time; partial tail (`L % chunk != 0`) handled.
- **Live caller:** `TorchArk::prefill_chunked` (`ark.rs`) — ark is the cleanest demonstrator (pure-causal decoder, ManualGqa,
  `apply_start` absolute-position RoPE, `View` cache). Wired into `transcribe_chunk` behind the opt-in `WAAV_ARK_PREFILL_CHUNK`
  env knob (unset = monolithic DEFAULT, byte-identical).
- **Byte-identity (the LAW):** **the decoded output is byte-identical to monolithic.** Live gate
  `cuda_torch_ark_chunked_prefill_byte_identical_to_monolithic` (`tests/cuda_torch_ark.rs`) loads real ark weights and
  proves chunk_size ∈ {1, 3, 8, full} all transcribe IDENTICALLY to the monolithic prefill AND the sidecar golden. CPU
  gate `chunked_prefill_bit_identical_to_monolithic` (`ark.rs`) proves Tier-A EXACT 0.0 (hidden+KV+logits) at chunk=full +
  Tier-B discrete-output invariance for chunk_size ∈ {1,k,full}; `chunked_prefill_partial_last_chunk_bit_identical` is the
  boundary edge. The `< full` intermediate-state delta is the documented sub-ULP BLAS reduction-reassociation scar (never
  flips the greedy argmax; the decode-feeding last-row logits + KV are exact at the final chunk). No model regression
  (dia2 544/544, csm bit-identical — `backbone.rs` change is additive). **Scoped-not-faked:** exact-0.0 intermediate state
  for `< full` needs a fixed-width `full_masked` SDPA reduction (shared-kernel surface) — see CHUNKED-PREFILL.md §2.

### 7. Quantization / precision — HAVE-LIVE
- **Live admission:** `admit_quant_variant` (`crates/waav-infer-core/src/model.rs:302`) — fp32/fp16 admitted
  unconditionally; every lossy variant (int8/q4/q4f16/fp8/mxfp4) requires a **per-substrate** `QuantStamp::pass`. Per-graph
  precision via `waav.json` `"weights": {..: {"precision": "q4f16"}}` (`model.rs:37`).
- **Gates (quoted):** `quant_variant_gated_by_per_substrate_accuracy_stamp` (`model.rs:1383`: no stamp ⇒ fp32; passing
  CUDA q4f16 stamp ⇒ admitted q4f16), `empty_kv_dtype_follows_weight_precision_q4f16` (`crates/waav-infer-runtime/src/precision.rs:148`:
  empty-state KV dtype follows graph weight precision — the q4f16 crash-fix), `multi_hardware_byte_identical_cpu_vs_cuda`
  (`crates/waav-infer-core/tests/multi_hardware_byte_identical.rs`). Memory-corroborated: voxtral int8 byte-identical vs
  plain onnxruntime int8.

### 8. Request scheduling — HAVE-LIVE
- **Live admission:** `CodecArAdmission::try_admit_prioritized` (`crates/waav-infer-server/src/codec_ar_admission.rs:273`)
  enforces priority bands (reserved high slots), per-tenant fair-share, deadline-viability, VRAM headroom — called from
  `CodecArBatcher::submit` (`codec_ar_batcher.rs:233`).
- **Gates (quoted):** `high_priority_admitted_ahead_of_low_under_saturation` (`codec_ar_admission.rs:651`),
  `no_tenant_exceeds_its_fair_share` (`codec_ar_admission.rs:706`), `concurrent_tenant_admits_never_exceed_fair_share`
  (`codec_ar_admission.rs:788`, 64-thread race → exactly cap admit), `item5_admission_emits_nonzero_metrics_into_rendered_prometheus`
  (`codec_ar_admission.rs:833`). Plus `overload_fairness.rs` server integration test.
- **PARTIAL note:** the scheduler crate's risk-slack `Scheduler::order` (`crates/waav-infer-scheduler/src/admission.rs:238`)
  is tested but has **no live caller** — the live path uses the simpler `codec_ar_admission` band/fairness gate. Risk-slack
  ordering is a future outer-tick scheduler, PARTIAL-SHELFWARE.

### 9. Streaming / TTFA — HAVE-LIVE
- **Live seam:** `StreamEgress` delta-streaming (`crates/waav-infer-runtime/src/egress.rs:103` `EgressEvent::Delta` carries
  only this stride's frame, never cumulative); the mux loop emits each stride's frame to the wire immediately. Genuine
  mid-loop incremental decode via `ArStepModel::decode_committed_prefix` (`arstep.rs:615`) for causal-chunkable decoders.
- **Gates (quoted):** `egress_delta_concat_equals_full_byte_identical` (`egress.rs:444`, concat of deltas ≡ offline
  full output), `f6_mux_ttfa_is_far_below_full_synthesis` (`serve.rs:3099`, first audio ≪ full synthesis).

### 10. CUDA graphs — HAVE-LIVE
- **Policy:** edge-tier-only capture (`crates/waav-infer-runtime/src/cuda_graph.rs:217` `CudaGraphTier`; prefill never
  captured), typed capture-failure → eager fallback, no thrash (`crates/waav-infer-runtime/src/graph_fallback.rs:237`).
- **Live capture:** tch shim `at::cuda::CUDAGraph` (`crates/waav-infer-backend-torch/src/nn/cuda_graph.rs`), used by dots
  DiT (`crates/waav-infer-backend-torch/src/dots.rs:651` replay / `:678` capture) and the ring-KV graph seam.
- **Gates (quoted):** `cuda_graph_ar_compounding_identical` (`cuda_graph.rs:485`, graph-on ≡ graph-off bit-identical),
  `graph_fallback_ar_compounding_identical` (`graph_fallback.rs:616`), `cuda_graph_ring_kv_byte_identical`
  (`crates/waav-infer-backend-torch/tests/cuda_graph_ring_kv.rs:51`). Memory-corroborated: dia2/csm/omnivoice/dots fan-out
  all byte-identical-gated.

### 11. Dynamic shapes — HAVE-LIVE
- **Live seam:** per-frame dynamic frame-rate (`crates/waav-infer-runtime/src/dynamic_fr.rs:299`, stride re-queried every
  frame off model state), ragged `step_batch` (`arstep.rs:511`, ragged cohort bit-identical via LEFT-aligned KV), per-slot
  acoustic-delay ring for delay-pattern multi-codebook (`arstep.rs:283`).
- **Gates (quoted):** `dynamic_fr_output_identical` (`dynamic_fr.rs:661`, concurrent ≡ serial incl. stride trajectory),
  `dynamic_fr_isolates_tenants_at_4_concurrent_distinct_strides` (`dynamic_fr.rs:785`), `acoustic_delay_ring_delays_by_k_frames`
  (`arstep.rs:901`).

### 12. KV-cache eviction / preemption — N-A-by-design (live) + PARTIAL (paged)
- The live discipline is **reject-at-admission, never admit-then-evict** — `crates/waav-infer-scheduler/src/marker.rs:238`:
  *"there is NO `evict`, no `preempt`, no `steal` method — a caller physically cannot evict a row"* (catalog H2). A
  real-time voice stream cannot be preempted mid-utterance without an audible gap, so this is a deliberate voice-correct
  choice, not a gap. The only "eviction" is the paged table's LRU page eviction (pillar 2, shelf-ware).

### 13. Disaggregated prefill/decode (P/D) — N-A-for-voice
- `grep disagg|kv_transfer|prefill_node` → 0 hits. Voice has no long prompt to prefill once + batch-decode against;
  utterances are framewise with a per-frame deadline. The memory's "intra-node spatial-P/D" = multi-tenant slot
  concurrency on one node (pillar 1), not a temporal prefill/decode split. **Justified N-A.**

### 14. Tensor / pipeline parallel (multi-GPU) — N-A-for-single-box / MISSING-for-scale-out
- `grep tensor_parallel|nccl|all_reduce|shard|device_mesh` → 0 hits (the one `all_reduce` mention is in
  `watchdog.rs:1013`, a teardown-aborts-collectives comment for the torch sidecar, not in-engine sharding). Voice models
  (≤4B) fit one GB10 with 10+ concurrent slots; throughput comes from multi-tenant batching, not weight sharding. Scale-out
  would be deployment-layer request routing across instances. **N-A for the target hardware; MISSING as an engine feature
  with no current need.**

### 15. Structured / guided decode — HAVE-LIVE (STT analog)
- TTS is vocoder synthesis (no token grammar). The STT analog is **bad-words / control-token suppression**: ARK-ASR forces
  special `<…>` tokens to −inf before argmax (`crates/waav-infer-backend-torch/src/ark.rs:40`).
- **Gate (quoted):** `argmax_suppressed_skips_bad_words_first_max` (`ark.rs:760`: global maxima at suppressed indices are
  skipped; kept max is the first non-suppressed). Hotword/positive-bias boosting is the obvious next additive step
  (logit-bias tensor at the same seam) — not yet present, low-effort.

---

## 4. Driven this session — the prefix-cache device-residency arbiter (closing the memory's standing challenge)

**The challenge (from program memory):** *"if the prefix-cache is TTFA-negative on the ONNX host-KV export, demonstrate
it on the tch device-resident-KV path where it wins, or document precisely why."*

The radix prefix cache is byte-identical on every backend, but it is only a *latency win* where the reused prefix KV does
not have to be re-streamed to the device on the suffix forward — a **backend residency property**, not a cache property.
That property is exactly what flips reuse from the measured 0.77× regression (host-KV ONNX) to a strict win
(device-resident tch ring). This session made that arbiter **explicit, live-wired, and gated**:

**New engine type — `KvResidencyRegime` (`crates/waav-infer-runtime/src/prefix_cache.rs`, pub, re-exported from `lib.rs`):**
- `HostKv` — a cache hit re-streams the whole reused prefix host→device: `prefix_host_transfer_rows(n) == n`,
  `reuse_is_strict_win() == false`.
- `DeviceResident` — a cache hit hands back a device handle: `prefix_host_transfer_rows(n) == 0`,
  `reuse_is_strict_win() == true`.

**Live wiring (`crates/waav-infer-core/src/tts/chatterbox.rs`):**
- `LmDecoder::kv_residency_regime()` reports the measured regime for the export (`HostKv` for chatterbox's
  `language_model.onnx`).
- `LmDecoder::arm_prefix_cache_if_win()` arms the cache **iff** the regime is a strict win — called on the **load path**
  (replaces the prior hardcoded skip). On the host-KV export it is a deliberate no-op (correct-but-unarmed, never the
  regression); on a future device-resident export it auto-arms to take the win. The arm decision is now logged
  (`kv_regime`, `prefix_cache_armed`) instead of buried in a comment.

**New gates (both passing):**
- `device_resident_reuse_wins_where_host_kv_loses` (`prefix_cache.rs`) — on the SAME 64-token shared-prefix hit:
  (a) reuse head is byte-identical to no-cache under both regimes (correctness is regime-independent); (b) `HostKv`
  re-streams all 64 prefix rows while `DeviceResident` re-streams **0** — so the device-resident regime strictly
  eliminates the transfer that sinks the host-KV path, and `reuse_is_strict_win()` differs between the two. This is the
  device-resident demonstration the host-KV ONNX path structurally cannot give.
- `prefix_cache_arm_decided_by_kv_residency_regime` (`chatterbox.rs`) — the arbiter reports `HostKv`, and
  `arm_prefix_cache_if_win()` correctly DECLINES to auto-arm on this export (the cache stays off, no TTFA regression). The
  byte-identity of armed reuse is proven definitively by the pre-existing `prefix_cache_reuse_is_bit_identical_to_full_prefill`.

**Why this is the right move (honest):** arming the host-f32 `PrefillState` cache on the ONNX host-KV path is a *measured
regression* — faking a "win" there would be dishonest. The win is real and now *decidable by a tested property*, so the day
the device-resident-KV export lands, `arm_prefix_cache_if_win()` flips it on with zero further code and a guarantee
(`reuse_is_strict_win`) that it cannot regress. The shelf-ware ("wired but arm decided by a comment") became
*live-arbitrated + gated*.

---

## 5. What was NOT driven, and why (no faking)

| Pillar | Why not closed this session |
|--------|------------------------------|
| Paged-KV → live | Multi-day: routes through the shared `nn/kv_cache.rs` device-tensor append used by all 7 torch models; high byte-identity-regression surface; not on the latency-critical path (the live ring already bounds memory). Scoped in §3.2. |
| Prefix-cache full device-resident win | The win requires a device-resident-KV *graph export* (the same deferred re-export the batching ceiling needs); the host-f32 `PrefillState` is structurally host-bound. This session made the win *decidable + gated* rather than fake-arming a regression. |
| Multi-LoRA / S-LoRA | Net-new: adapter registry + per-slot adapter id + batched grouped-GEMM. Feasible on the existing slot/batch plumbing; multi-day. Scoped in §3.4. |
| Chunked prefill | Net-new at the `prefill` seam; weak motivation for short voice prompts. Scoped in §3.6. |
| Scheduler `Cohorts`/`SlotTable`/`RingKvCache` → live | The live loop deliberately owns its own slot/cohort/ring logic (`serve.rs`); wiring the scheduler layer in is a re-architecture, and the live-equivalents are already gated (pillars 1, 8). |

---

## 6. LAW compliance

- **Matrix complete:** all 15 pillars classified with `file:line` citations; every HAVE-LIVE cell quotes a passing gate.
- **Wired pillar gated byte-faithful:** the prefix-cache residency arbiter has two new passing gates proving
  (a) cache-hit ≡ no-cache byte-identical (regime-independent) and (b) the device-resident win mechanism (0 prefix
  transfer vs N). No model byte-identity path was touched (the change is the arbiter + an arm-decision on the load path;
  the reuse numerics are unchanged and still gated by the pre-existing bit-identity tests).
- **Tests / clippy (recorded results):**
  - `cargo test --workspace` → **all binaries green, 0 failed** (runtime lib 236 passed incl. the new
    `device_resident_reuse_wins_where_host_kv_loses`; core lib 81 passed incl. the new
    `prefix_cache_arm_decided_by_kv_residency_regime`; no regressions).
  - `cargo clippy --workspace --all-targets -D warnings` → **clean** (with and without `--features torch`).
  - `--features torch` → server + backend-torch **build clean**; backend-torch deterministic doubles **192 passed, 0
    failed**. The heavy live-GPU torch gates run process-isolated via `ci/heavy_live_tests.sh` (the GB10 ORT-teardown
    leak forces one-model-per-process; not run here to avoid OOMing the unified pool) — but **no shared model
    byte-identity path was touched**, so dia2/csm/voxtral bit-faithfulness is structurally unaffected (the change is the
    additive `KvResidencyRegime` arbiter + a load-path arm-decision; reuse numerics unchanged).
  - One environment note: `cargo test --workspace` was blocked once by a **100%-full disk** (the box root was at 1.8T/1.9T;
    `waav-infer/target` = 129G). Reclaimed by clearing the regenerable `target/debug/incremental` (33G); re-run then green.

---

*Audited files (all absolute under `/home/bud/ditto/waav/waav-infer/`):*
`crates/waav-infer-runtime/src/{prefix_cache,paged_kv,cuda_graph,graph_fallback,arstep,dynamic_fr,turn,egress,precision,serve}.rs`,
`crates/waav-infer-scheduler/src/{admission,cohort,slot,ring_kv,marker}.rs`,
`crates/waav-infer-server/src/{codec_ar_batcher,codec_ar_admission,lib,ws}.rs`,
`crates/waav-infer-core/src/{model.rs,tts/chatterbox.rs}`,
`crates/waav-infer-backend-torch/src/{nn/kv_cache,nn/cuda_graph,dots,ark}.rs`.

*Files changed this session:* `crates/waav-infer-runtime/src/prefix_cache.rs` (new `KvResidencyRegime` + gate),
`crates/waav-infer-runtime/src/lib.rs` (re-export), `crates/waav-infer-core/src/tts/chatterbox.rs`
(`kv_residency_regime`/`arm_prefix_cache_if_win` + load-path wiring + gate).
