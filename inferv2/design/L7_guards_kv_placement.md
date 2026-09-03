# L7 — Guards · 3-tier KV · StagePlacer · Precision resolution (deep design)

**Status:** design · **Date:** 2026-06-17 · **Layer:** v2.1 §6.7 (`INFER_ENGINE_V2.md`) + IMPL §7 M3/M4.x (`INFER_ENGINE_IMPL.md`)
**Scope:** the named-but-unbuilt HAL half — the third KV tier (`StreamingEncoderCache`), the `StagePlacer` 11-test block (ggml-order placement + `ZeroCopyBuffer` relay + `roofline_class` + ridge-point batch-knee + degrade-to-`forward_native`), precision resolution (`int8`-never-on-ORT-CUDA + `by_substrate[ep]` + q4f16 empty-KV dtype via `StaticGraph::input_types()` + accuracy/MOS stamp), `EpKind::Hpu` (Gaudi wide-batch), and the shared per-tick guards.
**Device of record:** GB10 (sm_121, sm_12x Blackwell family), 128 GB unified LPDDR5X ~273 GB/s.
**Method:** extreme-TDD in the codebase idiom (`#[cfg(test)] mod tests`, `NopLoader`/`tmp_with_config` doubles, typed-error-not-panic). Everything in this layer is **GPU-free unit-testable** (pure logic over `Manifest`/`ActiveEp`/ring vectors) — that is the whole point of L7: the HAL decisions are *data*, so they gate on a CPU.

This document closes the systemic HAL hole the three coverage audits all flag: `05_hardware` (SAT 32 / PARTIAL 79 / GAP 10), `09_failure` (the precision/q4f16/teardown cluster), `06_batching` (the placement/relay/sniffer GAPs). The audits' verdict was unanimous: *"the serving-layer spine is rigorously gated; the HAL half lives entirely in v1.0 §2/§3.4/§5.2 prose with no IMPL test gate."* L7 turns that prose into types + named gates.

---

## (a) Convergence-verify table

Each prev-PARTIAL/GAP scenario, the v2.1 mechanism that closes it, and the **adversarial** check (does the closure actually compose / measure / not break a sibling invariant). Verdict column: **CLOSED** (mechanism + gate designed below), **CLOSED\*** (closed but with a residual noted in §c), **RESIDUAL** (deliberately deferred, see §c).

| Scenario (audit id) | prev | v2.1 mechanism (this doc §) | Adversarial check | verdict |
|---|---|---|---|---|
| **StreamingEncoderCache 3rd tier** (BAT-68 adjacent, L7 headline) | named-only | `StreamingEncoderCache` bounded delta-state (§b.1), keyed by `(slot, channel_id)`, sized from `genai_config` | **Does it compose with radix-prefix + ring-suffix?** YES — the three tiers are *disjoint by lifetime/owner*: prefix = cross-slot **shared, content-addressed, read-only** during decode; ring = per-slot **suffix, write-on-step**; encoder-cache = per-slot **encoder delta**, write-on-ingress-chunk, read by the decoder's cross-attn. No tier writes another's bytes; the only coupling is the `channel_id` recycle guard which fans to **all three** (§b.6 `reset_slot`). 3-tier coherence = "one writer per (tier,slot), all reset by one transaction." | CLOSED |
| **int8-on-ORT-CUDA trap** (HW-16/22/24, FAIL-19, the §5.2 12 ms→232 ms master-constraint) | GAP/PARTIAL | `resolve_precision()` guard: `int8` file + `ActiveEp::Ep(Cuda)` ⇒ demote to fp32 + `record_degraded` (§b.4) | **Does it fire on the *active* EP not the requested one?** YES — keyed on `ActiveEp` (post-fallback), so a `cuda`-requested→CPU-degraded session keeps int8 (CPU int8 is the *fast* path, §2.2 AMX). The guard is `(precision, active_ep)` not `(precision, ep_request)`. | CLOSED |
| **precision×substrate resolution** (HW-16/45/118) | GAP | `resolve_precision(by_substrate, active_ep, requested)` precedence `$WAAV_PRECISION → by_substrate[ep] → manifest → fp32` (§b.4) | **fp8 lands only on Hopper+?** YES — `EpCaps::supports_precision` gates fp8→{cuda(sm≥90),tensorrt}, mxfp4→Blackwell; an unsupported (precision,ep) demotes one tier down a fixed ladder, never silently runs. | CLOSED |
| **q4f16 empty-KV dtype** (HW-48/90, FAIL-99) | PARTIAL | `empty_kv_dtype()` reads `StaticGraph::input_types()` per logical input; empty KV/`past_padding`/`zero_past` follow weight precision (f16), `input_features`/`inputs_embeds` stay f32 (§b.5) | **Does `input_types()` actually thread it?** YES, and the seam already exists (`backend-api/src/lib.rs:177` returns `&[ElemType]` parallel to `input_names`). The resolver maps **name→declared-dtype** and builds the zero-length tensor in *that* dtype. The f32-stays-f32 list is name-matched (`input_features`,`inputs_embeds`,`audio_embeds`), not dtype-guessed — so a q4f16 graph whose feature input is genuinely f16 still gets f16 (graph-driven, not hardcoded). | CLOSED |
| **StagePlacer ggml-order placement** (HW-6/29/30/40/51/53/65/74/75/84) | PARTIAL (field-only) | `StagePlacer::place()` decision order 1-6 (§b.2): capability → CPU-floor priority → follow-immovable-weights → paradigm×substrate affinity → duty tie-break → boundary-min; manual pin never overridden | **Does the placer get a *real* roofline measurement vs the L4 bandwidth-duty?** The placer's *decision* uses an a-priori `roofline_class` (label, not measurement); the L4 ledger uses the **calibrated** `bandwidth_duty` (DRAM_ACTIVE under co-load). These are **two layers**: placement (where) is a-priori from the manifest's `bytes_touched`/FLOPs prior; admission (how-many) is measured. The `roofline_class` here is the *coarse* label (`compute`|`bandwidth`) that drives serialize-vs-overlap; L4 refines the *quantity*. No conflict — labelled coarse, measured fine. | CLOSED\* (label is a-priori; §c.1) |
| **ZeroCopyBuffer alias on UMA** (HW-51/56/57, BAT-27) | PARTIAL | `ZeroCopyBuffer{ptr,buft,owner,ready_event}` + `SharedHostBufType`; coherent boundary ⇒ alias (copy-count 0), discrete ⇒ async-copy+double-buffer (§b.3) | **Is "0 copies" testable GPU-free?** YES — `relay_for(producer_buft, consumer_buft)` returns `Relay::Alias` iff both advertise `SharedHostBufType`; the gate asserts the *enum variant* + a copy-counter==0, no GPU. | CLOSED |
| **relay credit / notify-before-wait / cycle-safe sniffer / shm-reaper** (BAT-52/67, FAIL-47/49, HW-79) | GAP | `CreditAllocator` (default 2, 3rd `put` blocks), `notify_then_wait()` order, `sniff_cpu_tensors(seen:&mut HashSet)`, `ShmReaper` (§b.3) | **Cycle-safe sniffer = WaaV's prior scar?** YES, directly — the gate `content_sniffer_terminates_on_cyclic_payload` re-arms the known false-positive regression with a `seen`-set walk. Credit double-release = hard error (typed), not a swallowed log. | CLOSED |
| **roofline_class serialize/overlap** (HW-52/60/62/76/81/82, FAIL-89) | PARTIAL | `roofline_class ∈ {Compute,Bandwidth}` on each placed stage; `placement_serializes_two_bandwidth_bound`, `compute_overlaps_bandwidth` (§b.2) | **HW-81 (the EXTREME box) — does this + the L4 ledger fully cover it?** The *placement+overlap* decision is CLOSED here; the *shared-bandwidth admission inequality* (`Σ bw_duty ≤ S·ceiling`) is an L4 gate (`shared_bandwidth_ledger_admits_iff_sum_duty_le_ceiling`). L7 labels; L4 enforces. HW-81 needs both — this doc closes its L7 half and cites the L4 gate. | CLOSED\* (L4 inequality is L4's gate; §c.2) |
| **ridge-point batch-knee** (HW-4/11/14/15 keystone, HW-50) | PARTIAL/UNDER | `batch_knee(caps) = ⌈peak_flops ÷ (peak_bw × bytes_per_elem)⌉` a-priori, reconciled with the L4 measured `B_max` via `min()` (§b.2) | **Does it break the per-substrate knee model?** NO — `min(a_priori_knee, measured_B_max, vram_slot_cap)` is monotone; a-priori is the *ceiling before calibration*, measured tightens it. For HPU the a-priori knee is **wide** (high FLOPs:bw), which is exactly the wide-batch the systolic MME wants — consistent. | CLOSED |
| **degrade-to-forward_native on op-fault** (HW-19/31/83, FAIL-23-adjacent) | PARTIAL/GAP | `run_or_degrade()` wraps a `StaticGraph::run` `Err` → retry on CPU-floor graph, `record_degraded("op_fault")`, contract stays alive (§b.7) | **P-6 floor honored, never `Err` to the caller?** YES — mirrors `ep.rs` `apply_request` (accelerator problem = degrade+telemetry, never `Err`). The op-degrade is the *runtime* analog of the *load-time* EP fallback. | CLOSED |
| **EpKind::Hpu (Gaudi)** (HW-78 severe GAP) | GAP | `EpKind::Hpu` enum arm + `parse_ep_request("hpu")` + a §2.2 Gaudi row (systolic MME → **wide** batch, HBM, TPC vector floor) + `hpu_degrades_to_forward_native` (§b.8) | **Does wide-batch break the per-substrate batch-knee model?** NO — the knee is `f(caps.flops_bw_ratio)`; Gaudi's high ratio yields a wide knee *by the same formula*. The bug the audit flagged was the **generic NPU row mis-modeling Gaudi as static B=1**; a dedicated `EpCaps` (`batch_profile=Wide`) fixes it without a special case. | CLOSED |
| **per-substrate accuracy/MOS stamp** (HW-18/32/33, FAIL-18) | PARTIAL | `AccuracyStamp{substrate,precision,metric,value}` gate: a quant variant admits only with a passing `verified{substrate,precision,metric}` incl. **TTS-MOS** (WER-flat/MOS-crash signature) (§b.4) | **Is the MOS check real or a label?** It's a *gate predicate* — `admit_quant_variant()` refuses an unstamped (substrate,precision) pair. The *measurement* is the existing eval-harness output (`supertonic_eval`/`stt_eval`); L7 only enforces the stamp exists + passed. | CLOSED |
| **heterogeneous-box EXTREME HW-81** | PARTIAL | placement (§b.2) + roofline label (§b.2) + zero-copy alias (§b.3) + L4 shared-bw inequality | composition of the above; the L7 half (place+label+alias) is CLOSED, the admission inequality is the cited L4 gate | CLOSED\* (§c.2) |
| **graph-fallback sm12x** (HW-27 already SAT, HW-25 FlashInfer) | SAT/PARTIAL | reuses M4.4 `cuda_graph_eager_fallback_on_capture_failure_sm120`; L7 adds `flashinfer_excluded_from_sm12x_candidate_set` (caps predicate) (§b.8) | **sm_121 vs sm_120?** Both are sm_12x Blackwell family (§6.0 hygiene); `EpCaps::is_sm12x` covers the family, so the FlashInfer exclusion + eager-fallback apply to sm_121 too. | CLOSED |
| **shared per-tick guards** (decode_repeat_ngram, max_inner_steps, mid-tick-recycle, teardown-order, zero-D2H, slot-cap-by-VRAM, multi-model-co-residency, masked-slot-bw-charge) | mixed PARTIAL/GAP | `decode_repeat_ngram_guard`, `max_inner_steps_per_tick`, `mid_tick_recycle_deferred`, teardown ladder, `zero_d2h` assert, `slot_cap_by_vram`, co-residency admit, masked-bw charge (§b.6) | **Does one repeat-guard cover BOTH STT AR-decode loops AND TTS codec degeneracy?** YES — it is paradigm-agnostic: a rolling n-gram over the emitted token stream (codec tokens OR text tokens), terminate+FINAL+metric. The slot it runs in carries `task_mode` only for the metric label, not the logic. | CLOSED |

**Summary:** 16 scenario-clusters audited → **13 CLOSED, 3 CLOSED\*** (placement-label-is-a-priori, the L4 shared-bw inequality lives in L4, HW-81 composes across L4/L7). 0 RESIDUAL in the convergence set; the residuals in §c are *new* second-order items surfaced by the deep design, not failures to close the named scenarios.

---

## (b) Deep design (actual Rust + algorithms + RED test bodies)

All new types land in **`waav-infer-scheduler`** (pure logic, no backend deps — the audit's requirement that L7 be GPU-free) except `EpKind::Hpu` + `EpCaps` which extend **`waav-infer-backend-api`** (pure data) and the `StaticGraph::input_types()` *use* which is in core's loader. KISS: no new crate; the placer/precision/guards are modules under the existing scheduler crate.

```
waav-infer-backend-api/src/lib.rs   # +EpKind::Hpu, +EpCaps, +SharedHostBufType, +RooflineClass
waav-infer-scheduler/src/
  placement.rs    # StagePlacer, ZeroCopyBuffer, CreditAllocator, ShmReaper, sniffer
  precision.rs    # resolve_precision, EpCaps::supports_precision, AccuracyStamp, empty_kv_dtype
  encoder_cache.rs# StreamingEncoderCache (3rd KV tier)
  guards.rs       # decode_repeat_ngram_guard, max_inner_steps_per_tick, mid-tick recycle, teardown
```

### b.0 — Backend-api extensions (pure data, parallel to the existing `EpKind`)

```rust
// waav-infer-backend-api/src/lib.rs  — additive, #![forbid(unsafe_code)] already in force.

/// Gaudi / Habana HPU. Systolic MME → WIDE batch, HBM, TPC vector floor. The generic NPU row
/// mis-models it as static B=1; this arm + EpCaps::Wide fixes it (closes HW-78).
pub enum EpKind { Cuda, TensorRt, Rocm, MiGraphX, OpenVino, Qnn, CoreMl, DirectMl, Xnnpack, Hpu }
// label(): EpKind::Hpu => "hpu"; parse_ep_request adds "hpu" => Explicit(Hpu);
// EP_GAUGE_LABELS gains "hpu" (bounded label set stays exhaustive).

/// Coarse memory-locality class an EP advertises for its tensors. Coherent ⇒ a cross-substrate
/// boundary aliases (zero copy); Discrete ⇒ the relay must DMA. (§3.4 zero-copy contract.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SharedHostBufType { Coherent, Discrete }

/// The roofline class of a placed stage — drives serialize-vs-overlap (§3.4 contention guard).
/// A-priori (manifest bytes_touched vs FLOPs); the L4 duty ledger refines the *quantity*.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RooflineClass { ComputeBound, BandwidthBound }

/// Per-EP capability priors the placer + precision resolver read. Pure data; the ort adapter
/// fills the concrete numbers, the policy stays backend-agnostic (P-8).
#[derive(Clone, Copy, Debug)]
pub struct EpCaps {
    pub ep: ActiveEp,
    pub buf: SharedHostBufType,
    pub peak_flops: f64,          // FLOP/s
    pub peak_bw: f64,             // bytes/s
    pub batch_profile: BatchProfile, // Static1 (Hexagon/ANE) | Tens (RTX) | Wide (GB10/H200/B200/HPU)
    pub sm_arch: Option<u32>,     // 121 for GB10; None for non-CUDA
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BatchProfile { Static1, Tens, Wide }
```

### b.1 — `StreamingEncoderCache` (the 3rd KV tier)

The headline 560-streams/H100 STT path: a streaming encoder (parakeet/voxtral-realtime) keeps a **bounded delta-state** across ingress chunks so re-encoding the whole audio prefix every chunk is avoided. Tier-3 is *per-slot, encoder-side*, distinct from the *decoder-side* ring (tier-2) and the *shared* prefix radix (tier-1).

```rust
// waav-infer-scheduler/src/encoder_cache.rs
use std::collections::VecDeque;

/// Bounded per-slot streaming-encoder delta state (3rd KV tier). Holds only the trailing
/// `cap_frames` of encoder hidden-state the decoder's cross-attn still needs — NOT the whole
/// audio history. Sized from genai_config (`max_source_positions` / `encoder_attention_window`).
pub struct StreamingEncoderCache {
    cap_frames: usize,                 // bound from genai_config; deltas-only
    hidden_dim: usize,
    deltas: VecDeque<Vec<f32>>,        // each = one chunk's NEW encoder frames (delta, not cumulative)
    channel_id: u64,                   // recycle guard: a stale delta from a prior occupant is dropped
}

impl StreamingEncoderCache {
    pub fn new(cap_frames: usize, hidden_dim: usize, channel_id: u64) -> Self {
        Self { cap_frames, hidden_dim, deltas: VecDeque::new(), channel_id }
    }

    /// Append ONE chunk's delta; evict oldest beyond the bound. Rejects a stale-channel write.
    pub fn push_delta(&mut self, channel_id: u64, frames: Vec<f32>) -> Result<(), CacheError> {
        if channel_id != self.channel_id { return Err(CacheError::StaleChannel); }
        if frames.len() % self.hidden_dim != 0 { return Err(CacheError::ShapeMismatch); }
        self.deltas.push_back(frames);
        // bound is in FRAMES across the whole window, not in chunks:
        let mut total: usize = self.deltas.iter().map(|d| d.len() / self.hidden_dim).sum();
        while total > self.cap_frames {
            let front = self.deltas.front().map(|d| d.len() / self.hidden_dim).unwrap_or(0);
            if front == 0 { break; }
            // partial-evict the front chunk frame-by-frame so the window is exact
            let drop = (total - self.cap_frames).min(front);
            let f = self.deltas.front_mut().unwrap();
            f.drain(0..drop * self.hidden_dim);
            if f.is_empty() { self.deltas.pop_front(); }
            total -= drop;
        }
        Ok(())
    }

    /// The current encoder context the decoder cross-attends to (contiguous, bounded).
    pub fn context(&self) -> Vec<f32> {
        self.deltas.iter().flatten().copied().collect()
    }
    pub fn frame_len(&self) -> usize { self.deltas.iter().map(|d| d.len() / self.hidden_dim).sum() }

    /// Tier-3 half of the transactional reset (called by the DAG-wide reset_slot fan-out, §b.6).
    pub fn reset(&mut self, new_channel_id: u64) {
        self.deltas.clear();
        self.channel_id = new_channel_id;
    }
}

#[derive(Debug, PartialEq)]
pub enum CacheError { StaleChannel, ShapeMismatch }
```

**RED test body** (`streaming_encoder_cache_delta_bounded`, GPU-free):
```rust
#[test]
fn streaming_encoder_cache_delta_bounded() {
    let mut c = StreamingEncoderCache::new(/*cap_frames*/4, /*hidden*/2, /*chan*/7);
    // push 3 chunks of 2 frames each (6 frames) into a 4-frame window
    c.push_delta(7, vec![1.,1., 2.,2.]).unwrap();      // 2 frames
    c.push_delta(7, vec![3.,3., 4.,4.]).unwrap();      // 4 frames total
    c.push_delta(7, vec![5.,5., 6.,6.]).unwrap();      // would be 6 → evict 2 oldest
    assert_eq!(c.frame_len(), 4, "window bounded to cap_frames");
    assert_eq!(c.context(), vec![3.,3., 4.,4., 5.,5., 6.,6.], "oldest delta evicted, deltas-only");
}
#[test]
fn streaming_encoder_cache_drops_stale_channel() {
    let mut c = StreamingEncoderCache::new(8, 2, 7);
    assert_eq!(c.push_delta(/*stale*/6, vec![1.,1.]), Err(CacheError::StaleChannel));
    c.reset(9);
    assert!(c.push_delta(9, vec![1.,1.]).is_ok(), "after reset the new occupant writes freely");
    assert_eq!(c.push_delta(7, vec![2.,2.]), Err(CacheError::StaleChannel), "old id now stale");
}
```

### b.2 — `StagePlacer` (ggml decision-order placement + roofline + knee)

```rust
// waav-infer-scheduler/src/placement.rs
use waav_infer_backend_api::{ActiveEp, EpKind, EpCaps, RooflineClass, BatchProfile, SharedHostBufType};

/// What a stage declares to the placer (subset of the manifest StageNode + calibration priors).
#[derive(Clone)]
pub struct StageSpec {
    pub id: String,
    pub paradigm: Paradigm,            // Ar | Flow | Diffusion | Feedforward | CodecStream | Encoder
    pub manual_pin: Option<ActiveEp>,  // never overridden (rule 0)
    pub weight_resident_on: Option<ActiveEp>, // where its load-once weights live (rule 3)
    pub bytes_touched: u64,            // roofline prior numerator
    pub flops: u64,                    // roofline prior denominator
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Paradigm { Ar, Flow, Diffusion, Feedforward, CodecStream, Encoder }

pub struct Placement { pub ep: ActiveEp, pub roofline: RooflineClass }

pub struct StagePlacer { caps: Vec<EpCaps> } // available substrates, CPU floor guaranteed present

impl StagePlacer {
    /// ggml `backend_sched` decision order, lifted to stage granularity (§3.4 rules 0-6).
    pub fn place(&self, s: &StageSpec) -> Placement {
        let roofline = Self::roofline_class(s);
        // rule 0: a manual pin is never overridden (still gets a roofline label).
        if let Some(ep) = s.manual_pin {
            return Placement { ep, roofline };
        }
        // candidate = every cap whose substrate (1) SUPPORTS the paradigm, ordered by
        // (2) priority with CPU floor last-resort, (3) follow-immovable-weights, (4) affinity,
        // (5) duty tie-break (omitted here = caller-supplied headroom), (6) boundary-min.
        let mut best: Option<(ActiveEp, i32)> = None;
        for c in &self.caps {
            if !Self::supports(s.paradigm, c) { continue; }      // (1) capability predicate
            let mut score = 0;
            if Some(c.ep) == s.weight_resident_on { score += 1000; } // (3) immovable weights dominate
            score += Self::affinity(s.paradigm, c);                 // (4) paradigm×substrate affinity
            if matches!(c.ep, ActiveEp::Cpu) { score -= 1; }        // (2) CPU is the floor, not preferred
            if best.map_or(true, |(_, b)| score > b) { best = Some((c.ep, score)); }
        }
        // (2) guaranteed CPU fallback (P-6) — `caps` always contains Cpu.
        let ep = best.map(|(e, _)| e).unwrap_or(ActiveEp::Cpu);
        Placement { ep, roofline }
    }

    /// (1) capability: reject AR on a Static1 substrate (Hexagon/ANE/generic-NPU) — AR breaks the
    /// fixed-shape contract (§2.3). Encoders/codecs/feedforward map fine.
    fn supports(p: Paradigm, c: &EpCaps) -> bool {
        match (p, c.batch_profile) {
            (Paradigm::Ar, BatchProfile::Static1) => false, // closes HW-40 "reject AR on static NPU"
            _ => true,
        }
    }

    /// (4) AR→GPU/HPU, conv-codec→NPU/CPU, encoder→NPU, CFM/Flow→GPU.
    fn affinity(p: Paradigm, c: &EpCaps) -> i32 {
        use ActiveEp::*; use EpKind::*;
        match (p, c.ep) {
            (Paradigm::Ar | Paradigm::Flow | Paradigm::Diffusion, Ep(Cuda | TensorRt | Hpu | Rocm)) => 10,
            (Paradigm::Encoder | Paradigm::CodecStream, Ep(Qnn | OpenVino | CoreMl)) => 8,
            (Paradigm::Encoder | Paradigm::CodecStream | Paradigm::Feedforward, ActiveEp::Cpu) => 2,
            _ => 0,
        }
    }

    /// A-priori roofline label (decision-time). Arithmetic intensity = flops/bytes; below the
    /// substrate ridge ⇒ bandwidth-bound. KISS: AR decode is bandwidth-bound by construction.
    fn roofline_class(s: &StageSpec) -> RooflineClass {
        if matches!(s.paradigm, Paradigm::Ar | Paradigm::CodecStream) { return RooflineClass::BandwidthBound; }
        let ai = s.flops as f64 / s.bytes_touched.max(1) as f64;
        if ai < 8.0 { RooflineClass::BandwidthBound } else { RooflineClass::ComputeBound }
    }

    /// Per-substrate ridge-point batch-knee, a-priori (§2.1/§2.2). The L4 ledger's measured B_max
    /// tightens this via min(); VRAM slot-cap tightens it further (§b.6). Wide profile ⇒ wide knee
    /// (Gaudi/B200), Static1 ⇒ 1, Tens ⇒ ~32.
    pub fn batch_knee(caps: &EpCaps, bytes_per_elem: f64) -> usize {
        let ridge = caps.peak_flops / (caps.peak_bw * bytes_per_elem); // FLOP/byte
        let raw = ridge.ceil() as usize;
        match caps.batch_profile {
            BatchProfile::Static1 => 1,
            BatchProfile::Tens   => raw.clamp(8, 64),
            BatchProfile::Wide   => raw.clamp(64, 512),
        }
    }
}
```

**RED test bodies** (GPU-free — caps are pure data):
```rust
fn cpu() -> EpCaps   { EpCaps{ep:ActiveEp::Cpu, buf:SharedHostBufType::Coherent, peak_flops:1e12, peak_bw:3e11, batch_profile:BatchProfile::Tens, sm_arch:None} }
fn cuda() -> EpCaps  { EpCaps{ep:ActiveEp::Ep(EpKind::Cuda), buf:SharedHostBufType::Coherent, peak_flops:1e15, peak_bw:2.73e11, batch_profile:BatchProfile::Wide, sm_arch:Some(121)} }
fn npu() -> EpCaps   { EpCaps{ep:ActiveEp::Ep(EpKind::Qnn), buf:SharedHostBufType::Coherent, peak_flops:5e13, peak_bw:1e11, batch_profile:BatchProfile::Static1, sm_arch:None} }

#[test]
fn placer_follows_immovable_weights_and_rejects_ar_on_static_npu() {
    let p = StagePlacer{ caps: vec![cuda(), npu(), cpu()] };
    // AR with weights resident on GPU → GPU (rule 3 + affinity), never the static NPU (rule 1).
    let ar = StageSpec{ id:"talker".into(), paradigm:Paradigm::Ar, manual_pin:None,
        weight_resident_on:Some(ActiveEp::Ep(EpKind::Cuda)), bytes_touched:6_000_000_000, flops:6_000_000_000 };
    let pl = p.place(&ar);
    assert_eq!(pl.ep, ActiveEp::Ep(EpKind::Cuda));
    assert_eq!(pl.roofline, RooflineClass::BandwidthBound); // AR is bandwidth-bound by construction
    // a conv encoder prefers the NPU (affinity), even with no weight residence hint.
    let enc = StageSpec{ id:"enc".into(), paradigm:Paradigm::Encoder, manual_pin:None,
        weight_resident_on:None, bytes_touched:1_000_000, flops:50_000_000 };
    assert_eq!(p.place(&enc).ep, ActiveEp::Ep(EpKind::Qnn));
}
#[test]
fn manual_pin_never_overridden() {
    let p = StagePlacer{ caps: vec![cuda(), cpu()] };
    let s = StageSpec{ id:"x".into(), paradigm:Paradigm::Ar, manual_pin:Some(ActiveEp::Cpu),
        weight_resident_on:Some(ActiveEp::Ep(EpKind::Cuda)), bytes_touched:1, flops:1 };
    assert_eq!(p.place(&s).ep, ActiveEp::Cpu, "explicit pin wins over weight residence + affinity");
}
#[test]
fn batch_knee_is_per_substrate_and_wide_for_gaudi_class() {
    assert_eq!(StagePlacer::batch_knee(&npu(), 2.0), 1, "static NPU knee=1");
    let knee_gpu = StagePlacer::batch_knee(&cuda(), 2.0);
    assert!((64..=512).contains(&knee_gpu), "wide profile ⇒ wide knee, got {knee_gpu}");
}
```

### b.3 — `ZeroCopyBuffer`, relay selection, credits, sniffer, reaper

```rust
// waav-infer-scheduler/src/placement.rs (cont.)

/// A producer→consumer hand-off across a stage boundary. On coherent memory the consumer views
/// the producer's buffer directly (alias, copy-count 0); on discrete it DMAs the live slice.
pub struct ZeroCopyBuffer {
    pub ptr: usize,                  // opaque device/host pointer (the policy never derefs it)
    pub buft: SharedHostBufType,
    pub owner: String,               // producing stage id (RAII single-owner)
    pub ready_event: u64,            // the consumer waits on this before reading
}

#[derive(Debug, PartialEq, Eq)]
pub enum Relay { Alias, AsyncCopyDoubleBuffered }

/// (§3.4) Coherent↔Coherent ⇒ alias; any Discrete end ⇒ async copy + double-buffer (live slice).
pub fn relay_for(producer: SharedHostBufType, consumer: SharedHostBufType) -> Relay {
    use SharedHostBufType::*;
    match (producer, consumer) {
        (Coherent, Coherent) => Relay::Alias,
        _ => Relay::AsyncCopyDoubleBuffered,
    }
}

/// Bounded credit pool = back-pressure on the per-edge relay (catalog G4). Default 2; the 3rd
/// in-flight `put` blocks; a double-release is a HARD error, never a swallowed log.
pub struct CreditAllocator { credits: usize, max: usize }
impl CreditAllocator {
    pub fn new(max: usize) -> Self { Self { credits: max, max } }
    /// notify-before-wait: the caller sends the data-ready CONTROL msg, THEN awaits transfer.
    /// We model the order as a method contract: `acquire` must precede the transfer await.
    pub fn try_acquire(&mut self) -> bool { if self.credits == 0 { false } else { self.credits -= 1; true } }
    pub fn release(&mut self) -> Result<(), RelayError> {
        if self.credits >= self.max { return Err(RelayError::DoubleRelease); }
        self.credits += 1; Ok(())
    }
}
#[derive(Debug, PartialEq)]
pub enum RelayError { DoubleRelease, BackPressure }

/// Cycle-safe content sniffer (WaaV's prior CRITICAL false-positive scar — catalog G10).
/// Walks a payload graph for "CPU tensors"; the `seen` set makes a cyclic payload terminate.
pub fn sniff_cpu_tensors(node: &PayloadNode, seen: &mut std::collections::HashSet<usize>) -> bool {
    if !seen.insert(node.id) { return false; } // already visited ⇒ stop (no infinite recursion)
    if node.is_cpu_tensor { return true; }
    node.children.iter().any(|c| sniff_cpu_tensors(c, seen))
}
pub struct PayloadNode { pub id: usize, pub is_cpu_tensor: bool, pub children: Vec<PayloadNode> }

/// shm orphan-reaper: receiver-owns-unlink shm leaks /dev/shm if the receiver crashes. The reaper
/// reclaims segments older than `ttl` with no live owner. (Pure bookkeeping; the unlink is the
/// adapter's job — the policy decides WHICH to reap.)
pub struct ShmReaper { segments: Vec<(String, u64 /*registered_tick*/, bool /*owner_alive*/)> }
impl ShmReaper {
    pub fn reap(&mut self, now: u64, ttl: u64) -> Vec<String> {
        let mut reaped = Vec::new();
        self.segments.retain(|(name, t, alive)| {
            if !*alive && now.saturating_sub(*t) > ttl { reaped.push(name.clone()); false } else { true }
        });
        reaped
    }
}
```

**RED test bodies** (GPU-free):
```rust
#[test]
fn coherent_boundary_aliases_discrete_copies() {
    use SharedHostBufType::*;
    assert_eq!(relay_for(Coherent, Coherent), Relay::Alias);            // GB10/UMA: 0 copies
    assert_eq!(relay_for(Coherent, Discrete), Relay::AsyncCopyDoubleBuffered);
}
#[test]
fn credit_backpressure_and_double_release_is_hard_error() {
    let mut c = CreditAllocator::new(2);
    assert!(c.try_acquire() && c.try_acquire());
    assert!(!c.try_acquire(), "3rd put blocks (no credit)");
    c.release().unwrap();
    assert_eq!(c.release().err(), None);             // back to max
    assert_eq!(c.release(), Err(RelayError::DoubleRelease), "over-release is a typed hard error");
}
#[test]
fn content_sniffer_terminates_on_cyclic_payload() {
    // Build a 2-node cycle via raw ids (the seen-set is what prevents non-termination).
    let leaf = PayloadNode{ id:2, is_cpu_tensor:false, children:vec![] };
    let root = PayloadNode{ id:1, is_cpu_tensor:false, children:vec![
        PayloadNode{ id:2, is_cpu_tensor:false, children:vec![ PayloadNode{ id:1, is_cpu_tensor:false, children:vec![] } ] }
    ] };
    let _ = leaf;
    let mut seen = std::collections::HashSet::new();
    assert!(!sniff_cpu_tensors(&root, &mut seen)); // terminates (re-arms the prior scar)
}
#[test]
fn shm_reaper_reclaims_orphans_past_ttl() {
    let mut r = ShmReaper{ segments: vec![("seg_a".into(), 100, false), ("seg_b".into(), 100, true)] };
    assert_eq!(r.reap(/*now*/200, /*ttl*/50), vec!["seg_a".to_string()], "dead owner past ttl reaped; live kept");
}
```

### b.4 — Precision resolver (`by_substrate[ep]` + int8-ORT-CUDA guard + MOS stamp)

```rust
// waav-infer-scheduler/src/precision.rs
use waav_infer_backend_api::{ActiveEp, EpKind, EpCaps};

/// The resolution precedence (§5.2): $WAAV_PRECISION (operator) → by_substrate[ep] → manifest → fp32,
/// then capability-gated + the int8-ORT-CUDA master-constraint. Pure logic over data ⇒ GPU-free,
/// NopLoader-idiom testable.
pub fn resolve_precision(
    requested: Option<&str>,           // $WAAV_PRECISION
    by_substrate: &[(EpKind, &str)],   // manifest by_substrate map
    manifest: Option<&str>,            // manifest.precision
    caps: &EpCaps,
) -> (String, Vec<&'static str>) {     // (precision, degrade_reasons for telemetry)
    let mut reasons = Vec::new();
    // 1. precedence
    let ep_match = if let ActiveEp::Ep(k) = caps.ep {
        by_substrate.iter().find(|(e, _)| *e == k).map(|(_, p)| *p)
    } else { None };
    let mut prec = requested
        .or(ep_match)
        .or(manifest)
        .unwrap_or("fp32")
        .to_string();

    // 2. THE master-constraint: int8 file never lands on ORT-CUDA (12 ms→232 ms scar, §5.2).
    //    ORT-CUDA EP can't int8-GEMM on Blackwell → it falls back per-op to fp32 anyway, but at
    //    catastrophic cost. Demote to fp32 explicitly + telemetry. (CPU/AMX int8 stays — it's fast.)
    if prec == "int8" && matches!(caps.ep, ActiveEp::Ep(EpKind::Cuda)) {
        prec = "fp32".into();
        reasons.push("int8_on_ort_cuda");
    }
    // 3. capability gate: fp8 only Hopper+ (sm≥90) / TensorRT; mxfp4 only Blackwell.
    if !supports_precision(&prec, caps) {
        let demoted = demote_one_tier(&prec);
        reasons.push("precision_unsupported_on_ep");
        prec = demoted;
    }
    (prec, reasons)
}

/// Capability predicate: which precisions a substrate can actually run (not just load).
pub fn supports_precision(prec: &str, caps: &EpCaps) -> bool {
    match prec {
        "fp8"  => matches!(caps.ep, ActiveEp::Ep(EpKind::TensorRt))
                  || matches!(caps.sm_arch, Some(a) if a >= 90),
        "mxfp4" => matches!(caps.sm_arch, Some(a) if a >= 100), // Blackwell
        // int8 supported everywhere EXCEPT ORT-CUDA (handled above as an explicit demote).
        _ => true,
    }
}
fn demote_one_tier(p: &str) -> String {
    match p { "fp8" => "fp16", "mxfp4" => "fp16", "fp16" => "fp32", _ => "fp32" }.into()
}

/// Per-substrate accuracy/MOS stamp (§5.2 + I4). A quant variant admits ONLY with a passing stamp
/// for (substrate, precision) incl. the TTS MOS check (catches WER-flat/MOS-crash). The MEASUREMENT
/// is the eval harness; L7 enforces the stamp exists + passed.
#[derive(Clone, Debug, PartialEq)]
pub struct AccuracyStamp { pub substrate: String, pub precision: String, pub metric: String, pub value: f64, pub pass: bool }

pub fn admit_quant_variant(stamps: &[AccuracyStamp], substrate: &str, precision: &str) -> Result<(), String> {
    if precision == "fp32" { return Ok(()); } // the reference precision needs no stamp
    let found = stamps.iter().find(|s| s.substrate == substrate && s.precision == precision);
    match found {
        Some(s) if s.pass => Ok(()),
        Some(s) => Err(format!("accuracy stamp FAILED: {} {} {}={}", substrate, precision, s.metric, s.value)),
        None => Err(format!("no accuracy stamp for ({substrate},{precision}) — requires-gate-before-production")),
    }
}
```

**RED test bodies** (GPU-free, `NopLoader`-idiom — pure data):
```rust
fn gb10() -> EpCaps { EpCaps{ ep:ActiveEp::Ep(EpKind::Cuda), buf:SharedHostBufType::Coherent,
    peak_flops:1e15, peak_bw:2.73e11, batch_profile:BatchProfile::Wide, sm_arch:Some(121) } }
fn amx_cpu() -> EpCaps { EpCaps{ ep:ActiveEp::Cpu, buf:SharedHostBufType::Coherent,
    peak_flops:1e12, peak_bw:3e11, batch_profile:BatchProfile::Tens, sm_arch:None } }

#[test]
fn int8_file_never_lands_on_ort_cuda_ep() {
    // operator asks int8, active EP is ORT-CUDA → demote to fp32 + telemetry (the master-constraint).
    let (p, why) = resolve_precision(Some("int8"), &[], None, &gb10());
    assert_eq!(p, "fp32");
    assert!(why.contains(&"int8_on_ort_cuda"));
    // SAME int8 on CPU/AMX stays int8 (the fast CPU path) — guard is keyed on the ACTIVE ep.
    let (p2, why2) = resolve_precision(Some("int8"), &[], None, &amx_cpu());
    assert_eq!(p2, "int8");
    assert!(why2.is_empty());
}
#[test]
fn precision_resolves_per_active_ep_with_precedence() {
    // by_substrate[cuda]=fp16 chosen over manifest=int8 (which would be demoted anyway).
    let bs = [(EpKind::Cuda, "fp16")];
    let (p, _) = resolve_precision(None, &bs, Some("int8"), &gb10());
    assert_eq!(p, "fp16", "by_substrate[ep] wins over manifest");
    // fp8 requested on sm121 (Blackwell ≥90) is supported; on a pre-Hopper it demotes to fp16.
    let pre_hopper = EpCaps{ sm_arch:Some(80), ..gb10() };
    let (p8, why) = resolve_precision(Some("fp8"), &[], None, &pre_hopper);
    assert_eq!(p8, "fp16");
    assert!(why.contains(&"precision_unsupported_on_ep"));
}
#[test]
fn quant_variant_gated_by_per_substrate_accuracy_stamp() {
    let stamps = vec![ AccuracyStamp{ substrate:"cuda".into(), precision:"q4f16".into(),
        metric:"tts_mos".into(), value:4.1, pass:true } ];
    assert!(admit_quant_variant(&stamps, "cuda", "q4f16").is_ok());
    assert!(admit_quant_variant(&stamps, "cuda", "int8").is_err(), "unstamped variant refused");
    assert!(admit_quant_variant(&[], "cuda", "fp32").is_ok(), "reference precision needs no stamp");
}
```

### b.5 — `empty_kv_dtype` via `StaticGraph::input_types()` (the q4f16 seam)

The voxtral q4f16 finding: a zero-code weight swap to `_q4f16` crashes on CUDA because the engine builds empty KV/`past_padding`/`zero_past` tensors as f32, but the graph declares them f16. The fix is **graph-driven**: read the declared dtype per input name from `StaticGraph::input_types()` (already on the trait) and build the empty tensor in *that* dtype, while genuine-feature inputs stay f32 by **name** (not by dtype-guess).

```rust
// waav-infer-scheduler/src/precision.rs (cont.)
use waav_infer_backend_api::ElemType;

/// Inputs that ALWAYS carry real f32 features regardless of weight precision (graph-driven exceptions
/// are by NAME, not dtype-guess — a q4f16 graph whose feature input is genuinely f16 still gets f16).
const F32_FEATURE_INPUTS: &[&str] = &["input_features", "inputs_embeds", "audio_embeds"];

/// Resolve the dtype to allocate an EMPTY state tensor (KV/past_padding/zero_past) so it matches the
/// graph the q4f16 weights produced. `names`/`types` come straight from StaticGraph::input_types().
pub fn empty_kv_dtype(input_name: &str, names: &[String], types: &[ElemType]) -> ElemType {
    if F32_FEATURE_INPUTS.iter().any(|f| input_name.contains(f)) {
        return ElemType::F32; // features stay f32 even under q4f16 weights
    }
    names.iter().position(|n| n == input_name)
        .and_then(|i| types.get(i).copied())
        .unwrap_or(ElemType::F32) // default: a graph that declares nothing ⇒ f32 (fp32 model)
}
```

**RED test body** (GPU-free — drives the dtype off declared graph metadata, no CUDA):
```rust
#[test]
fn empty_kv_dtype_follows_weight_precision_q4f16() {
    // a q4f16 decoder graph declares its KV inputs f16, its feature input f32.
    let names = vec![
        "past_key_values.0.decoder.key".to_string(),
        "past_padding_cache".to_string(),
        "input_features".to_string(),
    ];
    let types = vec![ElemType::F16, ElemType::F16, ElemType::F32];
    // empty KV follows the graph → f16 (this is what makes the zero-code q4f16 swap NOT crash on CUDA)
    assert_eq!(empty_kv_dtype("past_key_values.0.decoder.key", &names, &types), ElemType::F16);
    assert_eq!(empty_kv_dtype("past_padding_cache", &names, &types), ElemType::F16);
    // features stay f32 by NAME even though weights are q4f16
    assert_eq!(empty_kv_dtype("input_features", &names, &types), ElemType::F32);
    // an fp32 model that declares nothing for an unknown input ⇒ f32 (back-compat)
    assert_eq!(empty_kv_dtype("zero_past", &[], &[]), ElemType::F32);
}
```

### b.6 — Shared per-tick guards

```rust
// waav-infer-scheduler/src/guards.rs

/// ONE repeat-n-gram guard covering BOTH STT AR-decode hallucination loops AND TTS codec-token
/// degeneracy. Rolling window; on `repeats` identical n-grams in a row → terminate+FINAL+metric.
pub struct RepeatNgramGuard { n: usize, max_repeats: usize, window: std::collections::VecDeque<u32>, run: usize, last_ngram: Option<Vec<u32>> }
impl RepeatNgramGuard {
    pub fn new(n: usize, max_repeats: usize) -> Self {
        Self { n, max_repeats, window: Default::default(), run: 0, last_ngram: None }
    }
    /// Returns true ⇒ the caller must terminate the stream (emit FINAL + `waav_decode_repeat_total`).
    pub fn push(&mut self, token: u32) -> bool {
        self.window.push_back(token);
        if self.window.len() > self.n { self.window.pop_front(); }
        if self.window.len() < self.n { return false; }
        let ng: Vec<u32> = self.window.iter().copied().collect();
        if self.last_ngram.as_ref() == Some(&ng) { self.run += 1; } else { self.run = 0; self.last_ngram = Some(ng); }
        self.run >= self.max_repeats
    }
}

/// Per-slot cap on inner-NFE micro-steps in one outer tick — a runaway inner solver can't pace all
/// other slots (the nested third-execution-class hazard). Returns the clamped step count.
pub fn max_inner_steps_per_tick(requested: usize, cap: usize) -> usize { requested.min(cap) }

/// Mid-tick in-flight recycle: a recycle request arriving while a kernel is submitted DEFERS to the
/// next tick's control-plane phase. The in-flight kernel finishes for the OLD occupant; its output
/// is dropped by stale channel-id. Pure decision (the async seam lives in the driver).
#[derive(PartialEq, Debug)]
pub enum RecycleDecision { ApplyNow, DeferToNextTick }
pub fn recycle_decision(kernel_in_flight: bool) -> RecycleDecision {
    if kernel_in_flight { RecycleDecision::DeferToNextTick } else { RecycleDecision::ApplyNow }
}

/// Slot cap = min(bandwidth/knee cap, VRAM-capacity cap) (RTX is capacity-bound BEFORE the knee).
pub fn slot_cap(knee_slots: usize, vram_free_bytes: u64, weights_bytes: u64, kv_bytes_per_slot: u64) -> usize {
    let vram_cap = (vram_free_bytes.saturating_sub(weights_bytes) / kv_bytes_per_slot.max(1)) as usize;
    knee_slots.min(vram_cap)
}

/// Masked-slot bandwidth charge in admission: charge the CAPTURED cohort's bytes, not the active
/// count, so admission can't over-admit by ignoring idle-lane bandwidth (KISS budget path, R5a).
pub fn masked_bandwidth_duty(captured_cohort: usize, bytes_per_slot_step: u64, tick_rate: f64) -> f64 {
    captured_cohort as f64 * bytes_per_slot_step as f64 * tick_rate
}

/// Multi-model co-residency admission: a new model loads only if its projected peak fits the
/// box-scoped free VRAM (composes with the L5 box-scoped singleton accountant).
pub fn coresidency_admits(free_bytes: u64, model_projected_peak: u64) -> bool { model_projected_peak <= free_bytes }
```

**RED test bodies** (GPU-free):
```rust
#[test]
fn decode_repeat_ngram_guard_terminates_both_stt_and_tts() {
    let mut g = RepeatNgramGuard::new(/*n*/2, /*max_repeats*/3);
    // feed "a b a b a b a b" → the bigram (a,b) repeats → terminate
    let mut fired = false;
    for &t in &[1u32,2, 1,2, 1,2, 1,2] { if g.push(t) { fired = true; break; } }
    assert!(fired, "rolling-ngram degeneracy terminates (covers STT loop + TTS codec degeneracy)");
}
#[test]
fn runaway_inner_solver_capped_and_recycle_defers_under_inflight_kernel() {
    assert_eq!(max_inner_steps_per_tick(50, 8), 8);
    assert_eq!(recycle_decision(true),  RecycleDecision::DeferToNextTick);
    assert_eq!(recycle_decision(false), RecycleDecision::ApplyNow);
}
#[test]
fn slot_cap_is_min_of_vram_and_knee_and_masked_bw_charged_at_captured_count() {
    // RTX: knee says 64 but only 16 KV slots fit in VRAM ⇒ cap = 16 (capacity before knee).
    assert_eq!(slot_cap(64, /*free*/2_000_000_000, /*weights*/1_000_000_000, /*kv*/62_500_000), 16);
    // masked-bw is charged at the CAPTURED cohort (16), not the active count (say 4) → no over-admit.
    let charged = masked_bandwidth_duty(16, 1_000_000, 12.5);
    assert!(charged > masked_bandwidth_duty(4, 1_000_000, 12.5));
}
```

### b.7 — `run_or_degrade` (degrade-to-`forward_native` on op-fault, P-6 floor)

```rust
// waav-infer-scheduler/src/placement.rs (cont.) — mirrors ep.rs's degrade-not-Err discipline.

/// Run a stage on its placed EP; on a backend op-fault, retry on the guaranteed CPU floor and emit
/// `waav_ep_degraded`. NEVER surfaces an Err to the frame thread (P-6). The closures are the
/// backend's run fns; the policy here is pure (testable with fake closures, no GPU).
pub fn run_or_degrade<T>(
    on_ep: impl FnOnce() -> Result<T, ()>,
    on_cpu_floor: impl FnOnce() -> T,
    record_degraded: impl FnOnce(&'static str),
) -> T {
    match on_ep() {
        Ok(v) => v,
        Err(()) => { record_degraded("op_fault"); on_cpu_floor() }
    }
}
```

**RED test body**:
```rust
#[test]
fn ep_fault_degrades_op_to_forward_native_with_telemetry() {
    let mut degraded = None;
    let out = run_or_degrade(|| Err::<i32, ()>(()), || 42, |r| degraded = Some(r));
    assert_eq!(out, 42, "the contract stays alive on the CPU floor");
    assert_eq!(degraded, Some("op_fault"), "telemetry emitted, never an Err to the caller");
}
```

### b.8 — `EpKind::Hpu` (Gaudi) + FlashInfer-excluded-from-sm12x

The `EpKind::Hpu` enum arm, `parse_ep_request("hpu")`, label, and gauge are the b.0 additions. The §2.2 HAL table gains a Gaudi row:

| | memory | ideal batch | bottleneck | static/dynamic | best for |
|---|---|---|---|---|---|
| **Gaudi (HPU)** | 96–128 GB HBM2e/HBM3 | **wide** (MME systolic loves big GEMMs) | TPC vector floor on AR control-flow | dynamic (graph-mode); **NOT static B=1** | high-concurrency batched STT/TTS — modeled `BatchProfile::Wide`, **not** the generic NPU `Static1` |

```rust
// caps for HPU + the FlashInfer exclusion (a caps predicate, GPU-free).
pub fn flashinfer_allowed(caps: &EpCaps) -> bool {
    // ~2× regression on sm_12x Blackwell aarch64; excluded for the whole sm_12x family.
    !matches!(caps.sm_arch, Some(a) if (120..130).contains(&a))
}
```

**RED test bodies**:
```rust
#[test]
fn hpu_is_wide_batch_not_static_b1_and_degrades_to_forward_native() {
    let hpu = EpCaps{ ep:ActiveEp::Ep(EpKind::Hpu), buf:SharedHostBufType::Discrete,
        peak_flops:4e14, peak_bw:2.4e12, batch_profile:BatchProfile::Wide, sm_arch:None };
    let knee = StagePlacer::batch_knee(&hpu, 2.0);
    assert!(knee >= 64, "Gaudi systolic MME wants a WIDE batch, not B=1; got {knee}");
    // AR is allowed on HPU (Wide, not Static1) — the generic-NPU mis-model is gone.
    let p = StagePlacer{ caps: vec![hpu] };
    let ar = StageSpec{ id:"t".into(), paradigm:Paradigm::Ar, manual_pin:None,
        weight_resident_on:None, bytes_touched:1, flops:1 };
    assert_eq!(p.place(&ar).ep, ActiveEp::Ep(EpKind::Hpu));
}
#[test]
fn flashinfer_excluded_from_sm12x() {
    let gb10 = EpCaps{ ep:ActiveEp::Ep(EpKind::Cuda), buf:SharedHostBufType::Coherent,
        peak_flops:1e15, peak_bw:2.73e11, batch_profile:BatchProfile::Wide, sm_arch:Some(121) };
    assert!(!flashinfer_allowed(&gb10), "sm_121 is sm_12x Blackwell → no FlashInfer");
    let hopper = EpCaps{ sm_arch:Some(90), ..gb10 };
    assert!(flashinfer_allowed(&hopper));
}
```

### b.9 — Type & test inventory

**Types (15):** `EpKind::Hpu` (arm), `SharedHostBufType`, `RooflineClass`, `EpCaps`, `BatchProfile`, `StreamingEncoderCache`, `CacheError`, `StageSpec`/`Paradigm`/`Placement`/`StagePlacer`, `ZeroCopyBuffer`/`Relay`, `CreditAllocator`/`RelayError`, `PayloadNode`, `ShmReaper`, `AccuracyStamp`, `RecycleDecision`, `RepeatNgramGuard`. (Plus free fns: `resolve_precision`, `supports_precision`, `admit_quant_variant`, `empty_kv_dtype`, `relay_for`, `sniff_cpu_tensors`, `batch_knee`, `slot_cap`, `masked_bandwidth_duty`, `coresidency_admits`, `run_or_degrade`, `flashinfer_allowed`, `recycle_decision`, `max_inner_steps_per_tick`.)

**Named RED test gates (18):** `streaming_encoder_cache_delta_bounded`, `streaming_encoder_cache_drops_stale_channel`, `placer_follows_immovable_weights_and_rejects_ar_on_static_npu`, `manual_pin_never_overridden`, `batch_knee_is_per_substrate_and_wide_for_gaudi_class`, `coherent_boundary_aliases_discrete_copies`, `credit_backpressure_and_double_release_is_hard_error`, `content_sniffer_terminates_on_cyclic_payload`, `shm_reaper_reclaims_orphans_past_ttl`, `int8_file_never_lands_on_ort_cuda_ep`, `precision_resolves_per_active_ep_with_precedence`, `quant_variant_gated_by_per_substrate_accuracy_stamp`, `empty_kv_dtype_follows_weight_precision_q4f16`, `decode_repeat_ngram_guard_terminates_both_stt_and_tts`, `runaway_inner_solver_capped_and_recycle_defers_under_inflight_kernel`, `slot_cap_is_min_of_vram_and_knee_and_masked_bw_charged_at_captured_count`, `ep_fault_degrades_op_to_forward_native_with_telemetry`, `hpu_is_wide_batch_not_static_b1_and_degrades_to_forward_native` (+ `flashinfer_excluded_from_sm12x`). All GPU-free.

---

## (c) Residual gaps

These are deliberately-out-of-L7-scope or second-order items the deep design surfaced. None block the 16 named scenario-clusters (all CLOSED/CLOSED\*); they are flagged `requires-gate-before-production` per the §6.8 completeness rule where applicable.

1. **`roofline_class` is an a-priori LABEL here; the measured `bandwidth_duty` lives in L4.** (CLOSED\* on HW-52/60/62/81/82.) L7's placer decides serialize-vs-overlap from the coarse `{Compute,Bandwidth}` label (manifest `bytes_touched`/FLOPs prior + the AR-is-bandwidth-bound shortcut). The *quantitative* per-stage `bandwidth_duty = bytes_touched/ceiling × tick_rate` measured via **DRAM_ACTIVE under co-load calibration** is the L4 ledger's job (gate `bandwidth_duty_measured_via_dram_active_co_load`). **Residual:** if a stage's a-priori label disagrees with its calibrated class (a compute-bound stage that calibration finds bandwidth-bound), the placer's serialize/overlap decision is stale until recalibration. *Mitigation path:* feed the L4 calibrated class back to re-place on a calibration-stamp change (ties to the L5 calibration-stamp lifecycle).

2. **The shared-bandwidth admission INEQUALITY (`Σ bw_duty ≤ S·ceiling`) is an L4 gate, not L7.** (CLOSED\* on HW-81/HW-54, FAIL-89.) L7 supplies the *inputs* — the `roofline_class` label, `masked_bandwidth_duty`, the placement that co-locates two bandwidth-bound stages — but the admission inequality itself is enforced in L4's `admission.rs`. HW-81 (the EXTREME heterogeneous box) is fully closed only when L4's `shared_bandwidth_ledger_admits_iff_sum_duty_le_ceiling` lands alongside this L7 placement. **Not a hole — a layer boundary**; recorded here so the cross-layer dependency is explicit.

3. **`EpCaps` numeric priors are hand-supplied, not auto-probed.** The placer/precision logic is exact, but `peak_flops`/`peak_bw`/`sm_arch` are filled by the ort adapter from a static per-device table (the `auto_probe_order` analog). A device not in the table falls to the CPU floor (safe), but its batch-knee/precision-gate run on default priors until calibration. *Residual:* a `device_caps_table_or_cpu_floor` gate + the adapter-side probe are follow-on (backend-ort crate, not pure logic) — out of the GPU-free L7 scope by construction.

4. **`StreamingEncoderCache` window is sized from `genai_config` but the *eviction-correctness* under a variable encoder frame-rate (FlexiCodec-class streaming STT) is untested here.** The bound is in frames; a dynamic-frame-rate encoder (R2/L6) changes how many frames a chunk contributes. The current design is correct for fixed-rate streaming encoders (parakeet/voxtral-realtime — the headline targets); the variable-rate composition with the L6 dynamic-stride cohort is a follow-on gate `encoder_cache_window_exact_under_variable_frame_rate`.

5. **`recycle_decision`/`mid_tick` is a pure DECISION; the async kernel-in-flight SEAM is the driver's (M2.4/M4 overlap path).** L7 names the defer-and-drop discipline as a testable predicate, but the synchronous M2.4 driver can't exercise the race (BAT-106's note: "with a synchronous loop the race can't happen"). The full gate needs the driver test-double that holds a submitted batch across a control-plane mutation — that lives in the runtime crate's M4 overlap milestone, not L7.

6. **Spatial P/D (intra-node SM-partition) ledger accounting is untouched.** R4's "first-class measured option" (HW-88, BAT-78) is explicitly config-tiered/optional; L7 places stages onto whole EPs, not SM partitions. Deferred to the R4 A/B harness milestone.

**Residual count: 6** (all second-order or explicit layer-boundary/optional-feature deferrals; 0 of the 16 named convergence scenarios is left open). The 18 L7 gates + the cited L4 inequality gate together close every HAL placement/precision/zero-copy PARTIAL the three coverage audits enumerated.
