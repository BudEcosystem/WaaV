# L3 + L6 — Duplex/Multistream seam + R6 Reasoning cascade, in full

**Status:** deep design (closes the named-but-unbuilt L3 §6.3 + L6 §6.6 of `INFER_ENGINE_V2.md`) · **Date:** 2026-06-17 · **Device of record:** GB10 (sm_121, sm_12x Blackwell family)

> Scope: the two layers the v2.1 audit-closure (`INFER_ENGINE_V2.md §6.3` LAYER 3 + `§6.6` LAYER 6, gated in `INFER_ENGINE_IMPL.md §7` M2/M5) named at gate-NAME granularity but did **not** specify as typed Rust seams. This document takes them "in full": actual traits, structs, algorithms, and representative RED test bodies, in the codebase's own extreme-TDD idiom (`NopLoader`/`tmp_with_config` test-doubles, typed errors not panics, `MaskedCell` won't-compile discipline). Everything here is **additive** — the single-stream `ArStepModel` (IMPL §1) is a special case (K=1, Q=0, `delay_sign=TTS`) of the new `DuplexStepModel`; the 16-arm registry, `StaticGraph`, and Path-A one-shot models are untouched.

The audits this closes: `03_s2s_coverage.md` ROOT GAPs **A** (no full-duplex always-modeled seam, 7 GAP scenarios), **B** (no semantic-EoT head, 4 GAP), **C** (multistream/delay-sign/inner-monologue engine prose-only, 7 GAP), **D** (two-tier reasoning+filler not in docs, 7 PARTIAL→GAP), **E** (cascade LLM/sentence-agg/tool-call no seam, 1 GAP + 5 PARTIAL), **PARTIAL F** (F8 acoustic-delay ring never milestoned); `08_slo_coverage.md` GAPs **SLO-18/59/60/102** (reasoning realtime cascade) and PARTIAL **SLO-16** (barge-in ≤1-tick reclaim); `04_arch_coverage.md` ARCH-69/70/13/49/65 (full-duplex contract + state tokens + delay-ring gate-gap).

---

## (a) Convergence table

For each previously PARTIAL/GAP S2S/SLO/ARCH scenario, whether **v2.1 §6.3/§6.6 + this design** closes it, and the **adversarial** check on whether the new type actually composes with the proven lockstep core.

| Prev verdict | Scenario(s) | Closed by (mechanism) | Adversarial check — does it actually compose? | Now |
|---|---|---|---|---|
| GAP A | S2S-3 backchannel-while-user-speaks | `DuplexStepModel`: `user_in` is a **resident input ring per slot**, ingested every tick even while `model_out` emits; `TurnState::Backchannel` emits non-silence with **no** turn transition | Backchannel is just an output-stream value while the user input-ring keeps advancing — **same exec-mask**: the user-input write and the model-output read are both `MaskedCell::set_where(exec_mask,…)` mutations on the *same* slot row. No new batching axis. ✓ | **CLOSED** |
| GAP A | S2S-43 double-talk | `DoubleTalkPolicy` enum consulted per-tick from `(user_vad, model_speaking, eot_confidence)` | Policy is a pure function of per-slot state read inside the masked step; masked rows return `Yield` (frozen) → never spuriously grabs the turn. ✓ | **CLOSED** |
| GAP A | S2S-48/95 multi-party N-input | `MultiStreamSlot{ streams: SmallVec<[StreamLane; K]> }`, K=2Q+1; each participant lane has `(role, delay_sign, ring)` | K is **fixed at cohort-capture** (like B, T=1) → the CUDA-graph stays static (F7); per-lane exec gating is the *same* masked-select replicated over K lanes (a `[K,B]` mask, not `[B]`). Masked≠absent holds per-lane. ✓ | **CLOSED** (role-tracking) |
| GAP A | S2S-50/88/96 compound duplex+barge-in | duplex seam + `barge_in_cancels_llm` (§6.6) + G2 cancelled≠completed | Compound = product of two now-closed primitives; the cancel fans to all K lanes' rings via one `reset_lane(slot,lane)` (F3 transactional). ✓ | **CLOSED** |
| GAP B | S2S-4/24/38/62 EoT / hesitation / false-trigger | **`TurnHead`** (per-step linear head, §6.3) emitting `eot_confidence: f32` + `EotClass{Speaking,Hesitation,Boundary}`; eager-EoT staging gated on confidence ≥ θ | The head is one extra linear projection in the *same* forward (ARCH-75 §9.7 "extra heads are cheap in-step fan-out"); it reads the already-computed hidden state, adds ~0 to step time. The marker heap (F5, exists) consumes its boundary. ✓ | **CLOSED** |
| GAP B | S2S-53/70/72 variable-block / eager-start / history-staging | `PAD/EPAD/SILENCE` state tokens drive `TurnState`; `eager_eot` stages history into a cheaply-unwindable `EagerStage{committed_at, rollback}` | State tokens are ordinary emitted tokens; the turn FSM transition is a `MaskedCell` mutation (F2). Eager rollback drops staged history without touching KV (append-only, truncate offset). ✓ | **CLOSED** |
| GAP C | S2S-23/39/49/63/69 multistream/delay-sign/inner-monologue/role-swap | `delay_sign: DelaySign` per lane selects task mode (STT/TTS/S2S/Translate); write `(offset+delay)%CT`, read `(offset−max_delay+gen_delay)%CT`; inner-monologue = a text lane with its own delay | **The delay engine is the acoustic-delay ring generalized from per-codebook to per-lane** — same `(offset±delay)%CT` index math as F8, same `max_delay+2` depth, same PAD teacher-force. Role-swap = flip `delay_sign` on a live lane (KV preserved; only the read/write offsets change). ✓ | **CLOSED** |
| PARTIAL F | S2S-20/79/89, ARCH-13/49/65/103 acoustic-delay ring | `acoustic_delay.rs` (`AcousticDelayRing`, depth `max_delay+2`, pad-force warm-up) **promoted to M2** + `StepOutput` per-codebook depth | F8 was fully specified in the catalog but had **no IMPL gate** and `StepOutput{frame}` had no codebook structure to hang it on. Now `StepOutput.codebooks: SmallVec<[Frame; D]>` threads each codebook through; the ring writes per-(slot,codebook) under the same exec-mask. The 3-way collision (delay × wraparound × recycle) gets an interaction gate. ✓ | **CLOSED** |
| GAP D | S2S-25/52/80/90/96/100, SLO-18/60/102 reasoning filler / two-tier | **R6 §6.6**: `LatencyFiller` state machine fires on `ttft_predicted > TTFA_budget`; two-tier fast(non-committal)∥reasoning parallel-fire; `barge_in_cancels_llm` reclaims leftover compute | Reasoning LLM is an **off-AR-clock `LlmStreamNode`** (DAG stage, not a lockstep slot) → it never competes for a frame slot; the filler is a pre-rendered clip enqueued on the *audio* egress ring. Barge-in cancel is the reliable G9 acked abort fanned to the LLM node. **Determinism of reclaim** (the adversarial ask): the LLM node holds a **`ComputeLease`** (a duty-ledger reservation, §6.4); `cancel()` returns the lease **synchronously at the tick boundary** the cancel is observed → next-tick admission sees the freed duty. No async race: the lease is a slot in the ledger, freed under the same lock that admits. ✓ | **CLOSED** |
| GAP E | S2S-2/19/26/27/52/70, SLO-59 cascade LLM / sentence-agg / tool-call | `LlmStreamNode` (token egress + cancel) + `SentenceAggregator` (commit on clause boundary, partial-fire) + `ToolCallNode` (off-audio, partial-fire, context-merge-on-resume) | All three are **DAG stages off the AR clock** (LAYER 2 §6.2 machinery, already typed) — they compose with R6 by feeding the audio DAG sentence-by-sentence; `FINAL`/`cancelled` propagate per-edge (the existing DAG-FINAL mechanism). Tool node keeps the user-input lane modeling while it runs (duplex seam). ✓ | **CLOSED** |
| PARTIAL | SLO-16/95 barge-in ≤1-tick reclaim, simultaneous | `barge_in_aborts_output_within_one_tick`: the cancel is checked at **every** frame boundary (J15 cooperative token); the lane's rings + lease free in the *same* tick | The ≤1-tick bound is structural: the duplex loop wakes every frame and checks the per-slot cancel token before the step; a set token → `reset_lane` + lease-return *before* the next step() → output stops within one frame period. Storm: cancels are drained as a batch at the tick boundary (one pass over the cancel queue), not per-message. ✓ | **CLOSED** (bound now testable) |
| SATISFIED→reinforced | ARCH-69 always-modeled user stream | duplex seam makes the v1.0 §9.6 contract a **typed** `user_in` lane | Was SATISFIED-in-prose; now type-enforced. ✓ | **CLOSED** |
| PARTIAL | ARCH-70 PAD/EPAD/SILENCE → slot lifetime | `state_token_drives_slot_lifetime`: `EPAD`/`SILENCE` → transactional `reset_lane`; `PAD` → keep-but-masked | State-token → lifetime mapping is a small match in the turn FSM, gated. ✓ | **CLOSED** |

**Adversarial verdict on the three load-bearing composition questions:**

1. **Does `DuplexStepModel` compose with the lockstep `SlotBatch`/exec-mask?** **Yes, and it must, because it is the single→multi-stream *generalization* of `ArStepModel`, not a parallel engine.** The proof obligation is "the K-stream interleave is still masked≠absent-correct." The interleave is **K lanes per slot**, each lane a `(role, delay_sign, ring)`. The exec-mask generalizes from `BitVec[B]` to a `[B]` slot-mask × a per-lane `active` bit — i.e. a row is computed iff its **slot** is active; *within* an active slot, every lane advances under the *same* slot-active gate, and per-lane writes go through `MaskedCell::set_where(slot_active, …)`. Crucially **the model still runs ONE forward over `[B, K, …]`** (Moshi's 17-stream = K interleaved codebooks/text streams summed into one embedding, ARCH-47 §9.4 RQ-Transformer) — K is a *feature/embedding* dimension folded at input, **not** a new batch dimension that could break F7's static-shape graph. So: B fixed at capture, K fixed at capture, T=1 → one graph for server lifetime, idle slots masked-not-removed (F7). A masked slot's K lanes are all frozen by the single slot-gate; F1's substituted init-token applies per-lane (`is_init |= ~slot_active` broadcast over K). **Masked≠absent survives the K-stream interleave because the gate is per-slot and the lanes are an inner dimension, exactly like the per-codebook depth already is.**

2. **Does per-codebook `StepOutput` thread through to the codec stage?** **Yes.** `StepOutput.codebooks: SmallVec<[Frame; D]>` (D = codebook depth) replaces the flat `Frame`. The acoustic-delay ring reorders the D codebooks by their per-codebook delays *before* the codec stage reads them (read at `(offset−max_delay+gen_delay[k])%CT`), so the codec stage receives **time-aligned, delay-reversed** codebooks — which is exactly what F8 requires and what every multi-codebook TTS (Moshi/Mimi/Orpheus) needs. The codec stage's `batch_policy` stays `1` (C6/RFC#2568); it micro-batches the D codebooks of all active slots. The DAG edge carries `SmallVec<[Frame; D]>` not `Frame` — a typed widening, additive.

3. **Does barge-in-cancels-LLM reclaim leftover compute deterministically?** **Yes, by construction, because the reasoning LLM holds a `ComputeLease` in the duty ledger and `cancel()` returns it synchronously at the tick boundary under the admission lock.** The non-determinism trap (and the live-system bug the RR memory flags: `handle_barge_in` early-returns in a protected window → cancels nothing) is avoided three ways: (i) the cancel token is checked **every frame boundary** (J15), so there is no protected window where a barge-in is swallowed; (ii) the LLM node runs **off the AR clock** as a DAG stage, so cancelling it cannot glitch any audio frame — it only frees a ledger lease; (iii) the lease return is **the same critical section** as admission (`ComputeLedger::return_lease` and `try_admit` share one lock), so the reclaimed duty is visible to the *next* tick's admission with no read-modify-write race. Invariant tested: `barge_in_cancels_llm_reclaims_leftover` (the freed duty is admissible next tick) **and** `barge_in_does_not_poison_other_sessions` (cancel touches only the barging slot's lease + lane rings, by `channel_id`).

---

## (b) Deep design

Two new modules in the (greenfield) `waav-infer-runtime` crate plus one in `waav-infer-scheduler`, all additive:

```
waav-infer-runtime/src/
  duplex.rs           # DuplexStepModel trait, MultiStreamSlot, StreamLane, StepOutput{codebooks}
  turn.rs             # TurnState, TurnHead, EotClass, DoubleTalkPolicy, EagerStage
  acoustic_delay.rs   # AcousticDelayRing (depth max_delay+2, pad-force), per-(slot,codebook|lane)
  reasoning.rs        # LatencyFiller FSM, TwoTier, LlmStreamNode, SentenceAggregator, ToolCallNode
waav-infer-scheduler/src/
  lease.rs            # ComputeLease + ComputeLedger.return_lease (R6 deterministic reclaim)
```

### 1. The `DuplexStepModel` seam (vs `ArStepModel`)

`ArStepModel` (IMPL §1) is recovered as `DuplexStepModel` with K=1 lane, `delay_sign=Tts`, D=1 codebook. The driver calls `step` once per tick over the whole slot batch.

```rust
// waav-infer-runtime/src/duplex.rs
use smallvec::SmallVec;
use waav_infer_protocol::InferError;

pub type SlotId = u32;
pub type LaneId = u8;

/// What a lane carries and which direction time flows through its delay ring.
/// `delay_sign` SELECTS THE TASK MODE (ROOT GAP C / S2S-39): the SAME engine is STT, TTS,
/// S2S, or translate depending on which lanes are inputs (read-from-user) vs outputs
/// (written-by-model) and the sign of their integer delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRole { UserIn, ModelOut, InnerMonologue }   // inner-monologue = the text lane (Hibiki)

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelaySign { Stt, Tts, S2s, Translate }            // task mode, not a numeric sign alone

/// One interleaved stream in the K=2Q+1 set. Its ring is a per-lane acoustic/text delay ring.
pub struct StreamLane {
    pub id: LaneId,
    pub role: StreamRole,
    pub delay_sign: DelaySign,
    pub delay: i32,             // per-lane integer delay (frames); inner-monologue uses the alignment delay
    pub ring: AcousticDelayRing, // depth max_delay+2 (see acoustic_delay.rs)
    pub active: bool,           // per-lane gate; ANDed with the slot's exec bit (masked≠absent)
}

/// A full-duplex slot: K=2Q+1 interleaved streams, a turn FSM, and a double-talk policy.
/// The single-stream ArStepModel is exactly K=1 (one ModelOut lane, delay_sign=Tts).
pub struct MultiStreamSlot {
    pub slot: SlotId,
    pub channel_id: u64,        // monotonic; drops a stale prev-occupant's output (F3)
    pub streams: SmallVec<[StreamLane; 3]>, // K lanes (typically user_in + model_out + inner_monologue)
    pub turn: TurnState,        // turn.rs — a MaskedCell-gated FSM
    pub policy: DoubleTalkPolicy,
}

/// One codec frame's worth of output, NOW PER-CODEBOOK (threads to the codec stage).
/// D codebooks; the codec stage receives them delay-REVERSED + time-aligned.
pub struct StepOutput {
    pub codebooks: SmallVec<[Frame; 8]>, // D codebooks (RQ-Transformer / Mimi depth); D=1 for single-codebook
    pub eos: bool,
    pub turn: TurnState,                  // Listening | Speaking | Backchannel | Yielding
    pub eot_confidence: f32,              // for the marker heap / eager-EoT staging (ROOT GAP B)
}

/// A batch of slots advancing ONE stride this tick. The driver passes the exec-mask; the model
/// MUST treat masked slots as no-ops (every lane frozen) and accept substituted init tokens (F1).
pub struct SlotBatch<'a> {
    pub slots: &'a [MultiStreamSlot],
    pub exec_mask: &'a [bool],   // [B]; a SLOT-level gate (each slot's K lanes inherit it)
    pub frame_idx: u64,
}

pub trait DuplexStepModel: Send {
    fn prefill(&mut self, slot: SlotId, cond: &Conditioning) -> Result<PrefixKey, InferError>;

    /// Advance ALL active slots ONE stride. ONE forward over [B, K, …]; K is a folded
    /// embedding dimension (Moshi 17-stream), NOT a batch dimension — so the graph stays static.
    /// user_in lanes are READ (always modeled while speaking); model_out lanes are WRITTEN.
    fn step(&mut self, batch: &SlotBatch) -> Result<Vec<StepOutput>, InferError>;

    /// Transactional fan-out (F3): KV + EVERY lane's ring + sampler + word buffers + turn FSM.
    fn reset_slot(&mut self, slot: SlotId);
    /// Reset a single lane (role-swap / per-lane barge-in) WITHOUT dropping the slot.
    fn reset_lane(&mut self, slot: SlotId, lane: LaneId);

    fn kv_footprint_per_slot(&self) -> KvFootprint;
    fn stride_class(&self) -> StrideClass;
    fn n_codebooks(&self) -> u8;   // D, for StepOutput sizing + codec-stage contract
    fn n_lanes(&self) -> u8;       // K=2Q+1, fixed at capture (F7 static shape)
}

// LoadedModel gains: fn as_duplex(&mut self) -> Option<&mut dyn DuplexStepModel> { None }
// AR/duplex arms override; one-shot arms (kokoro/whisper) return None → micro-batch stage (Path-A untouched).
```

**The K=2Q+1 interleave algorithm** (Moshi: Q codebooks of user audio + Q of model audio + 1 text/inner-monologue = 2Q+1 streams summed into one embedding):

```rust
// Inside step(): build the [B, K, H] input by summing per-lane embeddings, gated per slot.
for b in active_slots(exec_mask) {                  // skip masked slots' KERNEL? No — see F1
    // ... but we DON'T skip: masked rows get the substituted init token (F1) so the dense
    //     [B,K] kernel never reads a sentinel. The loop here is the LOGICAL view; physically
    //     it is one where(is_init, initial, gathered) over the whole [B,K] tile.
}
// Physical (vectorized, F1+F2):
let is_init = broadcast(!exec_mask, /*over*/ K);     // [B,K]: masked OR still-warming → init token
let tokens  = where_(is_init, initial_kk, gathered); // [B,K] substituted BEFORE embedding
let embeds  = sum_over_k(embed(tokens));             // [B,H]  — K folded here (not a batch axis)
// one forward → temporal transformer → D-deep Depformer (per-codebook, ARCH-47) → StepOutput.codebooks
```

This is the single→multi generalization: **K and D are inner (feature/depth) dimensions folded inside one `[B,…]` forward**; only B is the batch/slot axis. F7's "B fixed → one graph forever" is therefore untouched.

### 2. `TurnState` + the EoT head + `DoubleTalkPolicy` (ROOT GAP B)

```rust
// waav-infer-runtime/src/turn.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnState { Listening, Speaking, Backchannel, Yielding }

/// What the per-step EoT head says about the user's current frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EotClass { Speaking, Hesitation, Boundary } // hesitation ≠ boundary (S2S-24)

/// A cheap per-step linear head (ARCH-75 §9.7): reads the already-computed hidden state,
/// projects to eot_confidence + class. ~0 added step-time (one matmul over [B,H]→[B,3]).
pub struct TurnHead { /* weights via StaticGraph; or a fused output of step() */ }
impl TurnHead {
    pub fn classify(&self, hidden: &Tensor, eot_threshold: f32) -> (EotClass, f32) { /* … */ }
}

/// BayLing-Duplex variable-block turn-taking encoding (L7). The model EMITS these as ordinary
/// tokens; the engine maps them to turn transitions + slot lifetime (ARCH-70).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateToken { Pad, Epad, Silence } // PAD=keep-masked; EPAD/SILENCE=turn boundary→reset_lane

/// Configurable policy for "both speak at once" (ROOT GAP A / S2S-43).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoubleTalkPolicy {
    Yield,        // model stops talking when the user sustains speech (default for assistants)
    HoldFloor,    // model keeps the turn (e.g. reading a long disclaimer)
    Backchannel,  // emit a short acknowledgement without grabbing the turn
}

impl DoubleTalkPolicy {
    /// Pure function of per-slot state, evaluated INSIDE the masked step (masked → Yield/frozen).
    pub fn decide(&self, user_vad: bool, model_speaking: bool, eot_conf: f32) -> TurnState {
        match (self, user_vad, model_speaking) {
            (DoubleTalkPolicy::Yield, true, true)        => TurnState::Yielding,
            (DoubleTalkPolicy::Backchannel, true, true)  => TurnState::Backchannel, // no turn grab
            (DoubleTalkPolicy::HoldFloor, _, true)       => TurnState::Speaking,
            (_, true, false)                             => TurnState::Listening,
            _                                            => TurnState::Speaking,
        }
    }
}

/// Eager-EoT history staging: speak/commit on a CONFIDENT boundary, cheaply unwindable on revision.
pub struct EagerStage {
    pub committed_offset: u64, // KV offset at stage time; rollback = truncate to here (no KV wipe, F3-cheap)
    pub fired: bool,
}
impl EagerStage {
    pub fn fire_if_confident(&mut self, eot_conf: f32, theta: f32, offset: u64) -> bool {
        if !self.fired && eot_conf >= theta { self.committed_offset = offset; self.fired = true; true }
        else { false }
    }
    pub fn rollback(&mut self) -> u64 { self.fired = false; self.committed_offset } // truncate, append-only
}
```

### 3. The acoustic-delay ring (F8, promoted to M2) — per-codebook AND per-lane

This is **one type** serving two needs (F8 per-codebook delays, and the multistream per-lane delays of ROOT GAP C) because the index math is identical.

```rust
// waav-infer-runtime/src/acoustic_delay.rs
/// Per-codebook (or per-lane) delay ring. Depth = max_delay+2 (the +2 is the off-by-one guard so
/// the max-delay write and the oldest read never collide — F8). Before step < acoustic_delay,
/// codebooks≥1 are teacher-forced to PAD (pre_delay_tokens) since no real acoustic token exists yet.
pub struct AcousticDelayRing {
    ct: usize,                 // ring depth = max_delay + 2
    max_delay: i32,
    delays: SmallVec<[i32; 8]>,    // per-codebook/per-lane integer delay
    gen_delays: SmallVec<[i32; 8]>,
    cells: Vec<Frame>,         // ct * D, flat
    offset: u64,               // logical write head (a MaskedCell in the slot — gated by exec_mask)
    pad: Frame,                // PAD token, teacher-forced in the warm-up window
}

impl AcousticDelayRing {
    pub fn new(max_delay: i32, delays: &[i32], gen_delays: &[i32]) -> Self {
        let ct = (max_delay + 2) as usize;             // F8: max_delay+2
        Self { ct, max_delay, delays: delays.into(), gen_delays: gen_delays.into(),
               cells: vec![Frame::pad(); ct * delays.len()], offset: 0,
               pad: Frame::pad() }
    }

    /// Write codebook k's token for this frame at the delayed position.
    #[inline] fn widx(&self, k: usize) -> usize {
        let pos = (self.offset as i32 + self.delays[k]).rem_euclid(self.ct as i32) as usize;
        pos * self.delays.len() + k
    }
    /// Read codebook k delay-reversed + time-aligned (what the codec stage consumes).
    #[inline] fn ridx(&self, k: usize) -> usize {
        let pos = (self.offset as i32 - self.max_delay + self.gen_delays[k]).rem_euclid(self.ct as i32) as usize;
        pos * self.delays.len() + k
    }

    /// Advance one frame. `step_idx < acoustic_delay` ⇒ pad-force codebooks≥1 (warm-up).
    pub fn write_frame(&mut self, out: &StepOutput, step_idx: u64, acoustic_delay: i32) {
        for k in 0..self.delays.len() {
            let tok = if (step_idx as i32) < acoustic_delay && k >= 1 { self.pad.clone() }
                      else { out.codebooks[k].clone() };
            let i = self.widx(k); self.cells[i] = tok;
        }
        self.offset += 1; // (the slot gates this via MaskedCell::set_where; masked ⇒ offset frozen)
    }
    /// The time-aligned, delay-reversed codebooks for the codec stage.
    pub fn read_aligned(&self) -> SmallVec<[Frame; 8]> {
        (0..self.delays.len()).map(|k| self.cells[self.ridx(k)].clone()).collect()
    }
}
```

### 4. R6 — the reasoning cascade (§6.6 / ROOT GAP D + E + SLO-18/60/102)

**The whole reasoning path runs OFF the AR clock** (DAG stages), so it never competes for a frame slot and cancelling it cannot glitch audio. It composes with the duplex seam (user-input lane keeps modeling) and the §6.2 DAG machinery (FINAL/cancelled propagation).

```rust
// waav-infer-runtime/src/reasoning.rs
/// Fires a pre-rendered, non-committal filler clip when the slow tier's predicted TTFT exceeds
/// the voice TTFA budget. Invariant (RR memory): fire the slow op IN PARALLEL with the filler;
/// the fast/filler tier NEVER asserts committed facts; barge-in cancels the slow LLM.
pub enum FillerState { Idle, Armed { eot_at: u64 }, Firing { clip: ClipId, interruptible: bool }, Done }

pub struct LatencyFiller {
    state: FillerState,
    ttfa_budget_ms: u32,
}
impl LatencyFiller {
    /// Arm ONLY on a CONFIRMED EoT (not eager/superseded — the RR double-fire critique).
    pub fn arm(&mut self, eot_at: u64) { self.state = FillerState::Armed { eot_at }; }
    /// On the tick where predicted TTFT > budget, enqueue a non-committal clip (interruptible).
    pub fn maybe_fire(&mut self, predicted_ttft_ms: u32, now: u64) -> Option<ClipId> {
        if let FillerState::Armed { .. } = self.state {
            if predicted_ttft_ms > self.ttfa_budget_ms {
                let clip = ClipId::non_committal();
                self.state = FillerState::Firing { clip, interruptible: true };
                return Some(clip); // enqueue_prerendered_clip(audio, interruptible=true)
            }
        }
        None
    }
    pub fn on_first_reasoning_sentence(&mut self) { self.state = FillerState::Done; } // crossfade
    pub fn cancel(&mut self) { self.state = FillerState::Idle; } // barge-in
}

/// Two-tier: fast (non-committal) ∥ reasoning, parallel-fire. The fast tier's output is a holding
/// reply; the reasoning tier's first sentence crossfades over it. Both cancellable by barge-in.
pub struct TwoTier { pub fast: LlmStreamNode, pub reasoning: LlmStreamNode }

/// An LLM as a DAG stage: streams tokens off the AR clock, holds a ComputeLease, is cancellable.
pub struct LlmStreamNode {
    lease: Option<ComputeLease>,   // duty-ledger reservation — the deterministic-reclaim key
    cancel: CancelToken,           // checked every tick boundary (J15)
    agg: SentenceAggregator,
}
impl LlmStreamNode {
    /// Cancel + return the lease SYNCHRONOUSLY at the tick boundary (deterministic reclaim).
    pub fn cancel(&mut self, ledger: &mut ComputeLedger) {
        self.cancel.set();
        if let Some(lease) = self.lease.take() { ledger.return_lease(lease); } // same lock as admit
    }
    /// Feed a token; emit a clause to TTS the instant a sentence boundary is reached (partial-fire).
    pub fn push_token(&mut self, tok: &str) -> Option<String> { self.agg.push(tok) }
}

/// Commit to TTS only on a sentence/stable-span boundary (no O(N²) MT churn; feeds the cascade).
pub struct SentenceAggregator { buf: String }
impl SentenceAggregator {
    pub fn push(&mut self, tok: &str) -> Option<String> {
        self.buf.push_str(tok);
        if ends_clause(&self.buf) { Some(std::mem::take(&mut self.buf)) } else { None }
    }
}

/// A tool call running OFF the audio path; the user-input lane keeps modeling (duplex seam) while it
/// runs; the result is merged into context on resume, or DROPPED if the turn is abandoned (barge-in).
pub struct ToolCallNode { call: ToolCall, cancel: CancelToken }
```

**Deterministic-reclaim lease** (the adversarial #3):

```rust
// waav-infer-scheduler/src/lease.rs
pub struct ComputeLease { pub duty: u32, pub slot: SlotId, lease_id: u64 }

pub struct ComputeLedger { /* per-substrate Σduty under ONE Mutex */ }
impl ComputeLedger {
    /// Admit iff Σduty + want ≤ bound. Returns a lease on success. (SAME LOCK as return_lease.)
    pub fn try_admit(&mut self, want: u32, slot: SlotId) -> Option<ComputeLease> { /* … */ }
    /// Return a cancelled stage's duty SYNCHRONOUSLY — visible to the NEXT tick's try_admit with
    /// no read-modify-write race (this is what makes barge-in reclaim DETERMINISTIC).
    pub fn return_lease(&mut self, lease: ComputeLease) { /* Σduty -= lease.duty, under the lock */ }
}
```

### 5. The driver tick (how it all composes, KISS)

```
per tick (every frame period):
  1. drain control: admissions / resets / BARGE-IN CANCELS (J15, checked here — no protected window)
        for each cancel: slot.reset_lane(barging lane) + llm_node.cancel(&mut ledger)  // ≤1-tick, deterministic
  2. compute exec_mask (slot-level); if !any → short-sleep 1-2ms (F6), DONE
  3. substitute init tokens for masked/warming rows: where(is_init, initial, gathered) over [B,K] (F1)
  4. ONE forward over [B,K,H] → temporal → D-deep Depformer → StepOutput{codebooks[D], turn, eot_conf}
  5. per slot (gated by MaskedCell::set_where(exec_mask,…)):    // F2 — every mutation masked
        - TurnHead.classify → EotClass; DoubleTalkPolicy.decide → TurnState
        - state-token → lifetime (EPAD/SILENCE ⇒ reset_lane; PAD ⇒ keep-masked)   // ARCH-70
        - EagerStage.fire_if_confident(eot_conf)                                   // S2S-72
        - for each lane: ring.write_frame(out, step_idx, acoustic_delay)           // F8
  6. codec stage (batch_policy=1): for each active slot, ring.read_aligned() → D delay-reversed
        codebooks → codec micro-batch → vocoder stream-window → egress delta (I1)
  7. reasoning DAG (OFF this clock, in parallel): LlmStreamNode streams tokens → SentenceAggregator
        → partial-fire clause into the TTS DAG; LatencyFiller.maybe_fire masks TTFT
  8. meter step wall-time vs frame budget (F10)
```

### Representative RED test bodies (extreme-TDD, the codebase idiom)

```rust
// duplex.rs — masked≠absent survives the K-stream interleave (the #1 composition proof)
#[test]
fn duplex_user_stream_modeled_every_tick_even_while_speaking() {
    let mut m = FakeDuplex::new(/*K=*/3, /*D=*/2);     // user_in + model_out + inner_monologue
    let mut slots = vec![slot_speaking_with_user_audio(0), slot_idle(1)];
    // slot 1 masked: its 3 lanes must ALL stay frozen; slot 0 must STILL ingest user_in while emitting.
    let before = slots[1].clone_state();
    let out = m.step(&SlotBatch { slots: &slots, exec_mask: &[true, false], frame_idx: 7 }).unwrap();
    assert_eq!(slots[1].clone_state(), before, "masked slot's K lanes must be byte-identical (F1/F2)");
    assert!(m.recorded_user_in_read(0), "active slot ingests user_in WHILE model_out is Speaking");
    assert!(matches!(out[0].turn, TurnState::Speaking | TurnState::Listening));
}

#[test]
fn step_output_per_codebook_shape_threads_to_codec() {
    let m = FakeDuplex::new(1, /*D=*/4);
    let out = m.step(&one_active()).unwrap();
    assert_eq!(out[0].codebooks.len(), 4, "StepOutput carries D codebooks for the codec stage");
}

// turn.rs
#[test]
fn eot_head_distinguishes_hesitation_from_boundary() {
    let head = TurnHead::fake();
    assert_eq!(head.classify(&hidden_for_trailing_off(), 0.6).0, EotClass::Hesitation); // mid-word pause
    assert_eq!(head.classify(&hidden_for_clean_stop(),  0.6).0, EotClass::Boundary);
}
#[test]
fn double_talk_policy_yields_on_sustained_user_speech() {
    assert_eq!(DoubleTalkPolicy::Yield.decide(true, true, 0.9), TurnState::Yielding);
    assert_eq!(DoubleTalkPolicy::Backchannel.decide(true, true, 0.1), TurnState::Backchannel); // no grab
}
#[test]
fn eager_eot_rollback_truncates_without_kv_wipe() {
    let mut s = EagerStage::default();
    assert!(s.fire_if_confident(0.95, 0.9, /*offset*/100));
    assert_eq!(s.rollback(), 100); assert!(!s.fired);   // cheaply unwindable
}

// acoustic_delay.rs — port Moshi lm.rs vectors
#[test]
fn acoustic_delay_ring_depth_is_max_delay_plus_2() {
    let r = AcousticDelayRing::new(/*max_delay*/4, &[0,1,2,2], &[0,1,2,2]);
    assert_eq!(r.ct, 6, "F8: depth = max_delay + 2 (off-by-one guard)");
}
#[test]
fn pre_delay_codebooks_pad_forced() {
    let mut r = AcousticDelayRing::new(2, &[0,1], &[0,1]);
    r.write_frame(&out_with_real_tokens(), /*step_idx*/0, /*acoustic_delay*/2);
    assert!(r.read_aligned()[1].is_pad(), "codebook≥1 teacher-forced to PAD in the warm-up window");
}
#[test]
fn delay_write_read_alignment_vectors() { /* assert (offset±delay)%ct against Moshi-published vectors */ }
#[test]
fn delay_sign_selects_task_mode_no_code_fork() {
    // same engine; flipping a lane's delay_sign STT→TTS reuses the SAME step() path.
    for sign in [DelaySign::Stt, DelaySign::Tts, DelaySign::S2s, DelaySign::Translate] {
        assert!(run_one_tick_with_sign(sign).is_ok());
    }
}
#[test]
fn role_swap_flips_delay_sign_preserves_kv_no_readmit() { /* flip live lane; KV offset unchanged */ }

// reasoning.rs + lease.rs — the deterministic-reclaim proof
#[test]
fn filler_fires_when_ttft_exceeds_budget() {
    let mut f = LatencyFiller { state: FillerState::Idle, ttfa_budget_ms: 300 };
    f.arm(10);
    assert!(f.maybe_fire(8900, 11).is_some(), "reasoning TTFT 8.9s ≫ 300ms budget → non-committal filler");
}
#[test]
fn barge_in_cancels_llm_reclaims_leftover_deterministically() {
    let mut ledger = ComputeLedger::with_bound(100);
    let lease = ledger.try_admit(40, 0).unwrap();
    let mut node = LlmStreamNode::with_lease(lease);
    node.cancel(&mut ledger);                                  // synchronous, same lock
    assert!(ledger.try_admit(40, 1).is_some(), "freed duty admissible the NEXT tick (no race)");
}
#[test]
fn barge_in_does_not_poison_other_sessions() {
    // cancelling slot 0's LLM must not touch slot 1's lease/lane rings (by channel_id).
    let (mut l0, mut l1) = two_llm_nodes();
    let mut ledger = ledger_with_two_leases();
    l0.cancel(&mut ledger);
    assert!(l1.lease.is_some(), "other session's compute untouched");
}
#[test]
fn sentence_aggregation_streams_first_clause() {
    let mut a = SentenceAggregator::default();
    assert!(a.push("Hello").is_none());
    assert_eq!(a.push(" there.").as_deref(), Some("Hello there."), "partial-fire on clause boundary");
}
#[test]
fn barge_in_aborts_output_within_one_tick_listen_continues() {
    // set cancel token mid-stream; after ONE driver tick, model_out is silent but user_in still read.
    let mut d = driver_with_speaking_slot();
    d.signal_barge_in(0);
    d.tick();
    assert!(d.model_out_silent(0) && d.user_in_still_modeled(0));
}
```

---

## Residual gaps

What this L3/L6 design does **not** fully close, ranked, with where each belongs:

1. **[L4, not L3/L6] Variable-NFE `T_step` admission for a duplex+nested model** — when a duplex model also has a variable-NFE inner head (FlashTTS-class), the lockstep tick is paced by `T_step = T_ar + max_over_active(inner_steps_i × T_inner)` (the third-execution-class math, `04_arch` ARCH-44/83/90). This design composes with it (the duplex step is the AR-outer; the inner head is unchanged) but the **admission math for the combined case** lives in LAYER 4 (`§6.4` / IMPL M4.3 `nested_variable_nfe_T_step_sums_max_subbucket`), not here. Cross-reference, not re-spec.

2. **[L4/L5] Cross-worker reasoning placement + the prefix-affinity router** — R6 runs the reasoning LLM as a local DAG stage; placing the reasoning *prefill* on an intra-node spatial partition (SLO-90/100) or routing a returning voice to its ref-KV holder (SLO-30/57/93) is the LAYER 4 router (`waav-infer-router`) + LAYER 5 control plane. The lease mechanism here is the hook they bind to, but the router component itself is out of L3/L6 scope.

3. **[L2 reinforcement] DAG-wide barge-in ACK across cloud stages** — `barge_in_cancels_llm` here cancels a *local* `LlmStreamNode` deterministically. A *remote* `CloudStage{paradigm=remote}` reasoning LLM (§6.2) needs the reliable barge-in cancel-to-remote-session with per-stage ACK (G9). The cancel-token plumbing generalizes, but the network round-trip ACK + fail-fast-on-disconnect is LAYER 2's `cloud_stage_remote_cancel_and_failfast` gate.

4. **[fidelity] The `TurnHead` weights / EoT calibration** — this design types the head and its `eot_confidence`/`EotClass` output and gates its *behavior* (hesitation≠boundary, threshold-gated eager fire), but the actual **head weights** (a Smart-Turn-v3-class semantic-VAD model) and its accuracy/calibration bar (false-barge-in rate under noise, S2S-38) are a model-onboarding concern (the onboarding skill + accuracy harness), not an architecture seam. The seam is here; the trained head is acquired.

5. **[L1 reinforcement] Inner-monologue alignment-drift over long passages** — the inner-monologue lane and its contextual-alignment delay are typed here (`StreamRole::InnerMonologue`, per-lane `delay`), closing S2S-23/63/69 at the seam level. But the **long-passage alignment-pin + re-sync-at-marker** stability property (S2S-63/98, drift over 15 min) needs the pinned-attention-sink + paged-escape (R5d / `ring_overflow_spills_to_paged`) from LAYER 7 to bound the ring; this design assumes ctx ≤ ring and defers the long-form escape to L7.

6. **[interaction-gate fidelity] The 3-way collision test is named, not exhaustively vectorized** — `delay_ring_plus_wraparound_plus_nested_inner_no_corruption` (ARCH-103) is specified as an interaction gate here, but generating the *reference* (a single-rule oracle to byte-compare against under the combined load) is a test-harness build-out, not a design decision. The gate exists; its golden vectors are an impl task.

---

**Mechanism→gate completeness (META, §6.8 rule):** every type/algorithm above maps to ≥1 named RED gate (`duplex_user_stream_modeled_every_tick…`, `step_output_per_codebook_shape…`, `eot_head_distinguishes_hesitation…`, `double_talk_policy_yields…`, `acoustic_delay_ring_depth_is_max_delay_plus_2`, `pre_delay_codebooks_pad_forced`, `delay_write_read_alignment_vectors`, `delay_sign_selects_task_mode_no_code_fork`, `role_swap_flips_delay_sign_preserves_kv…`, `eager_eot_rollback_truncates…`, `filler_fires_when_ttft_exceeds_budget`, `barge_in_cancels_llm_reclaims_leftover_deterministically`, `barge_in_does_not_poison_other_sessions`, `sentence_aggregation_streams_first_clause`, `barge_in_aborts_output_within_one_tick_listen_continues`). No un-gated mechanism ships (`requires-gate-before-production`).
```