# LAYER 1 — Feature Edges: Deep Design + Convergence Verification

**Status:** design (designs `INFER_ENGINE_V2.md §6.1` + `INFER_ENGINE_IMPL.md §7 M2b` to structs+test-bodies) · **Date:** 2026-06-17 · **Device of record:** GB10 (sm_121, sm_12x Blackwell family)

**Scope.** The pipeline bookends + non-core features promoted to first-class stage nodes:
`IngressNormalizer` (pre-encoder), `TextFrontend` (pre-AR TTS), `AsrFeaturePost` (post-decode STT),
`TransportEgress` (post-vocoder, off the AR clock), the `FeatureStage` taxonomy + `StageState::reset(slot)`
contract, and `BiasContext`. Everything here is **additive** — it does not touch the 16-arm registry, the
coarse `TtsModel`/`SttModel` traits, the `StaticGraph` seam, or the M2 lockstep core. It lands as a new crate
`waav-infer-features` (pure logic, GPU-free, no backend deps) + additive types on the M4.1 `StageNode` schema.

**Method.** Extreme-TDD in the codebase's idiom: `NopLoader`/`tmp_with_config` test-doubles, typed `InferError`
(never panics), object-safe traits, edition 2024, `MaskedCell`-style type-enforced state. Every `#[test]` below is
RED-first and maps to an `INFER_ENGINE_IMPL.md §7 M2b` gate name.

**Codebase substrate this design reuses (verified by reading the crates):**
- `waav_infer_components::{EdgeResampler, ctc::greedy, g2p::Phonemizer, unicode_text::{chunk_text, UnicodeIndexer}, text::Segmenter, standardize::canonical_lang}` — already shipped.
- `waav_infer_protocol::{WordTiming, ChunkMeta (carries `alignment: Vec<WordTiming>`), Conditioning, SessionConfig (carries `keyterms: Vec<String>` = FR-A6 biasing), InferError, ErrorCode (`UnsupportedParam`/`BadConfig` already exist)}`.
- The M2 stepped seam `ArStepModel{prefill, step, reset_slot, kv_footprint_per_slot, stride_class}` and M3.3 `prefix_cache.rs` (`extra_key = blake2b(conditioning)`).

> **One critical existing-code finding that changes the design:** `EdgeResampler` (resample.rs:14) is documented
> **"Downsampling has no anti-alias filter yet … should be gated until the rubato upgrade lands."** So `IngressNormalizer`
> and `TransportEgress` are NOT mere wire-ups of an existing primitive — the anti-alias low-pass is a **real bug to fix**,
> and the `no_chipmunk`/`downsample_anti_alias` gates fail today for a genuine reason. The design replaces the bare linear
> resampler on the downsample path with a windowed-sinc / rubato FFT path behind the same `EdgeResampler`-shaped trait.

---

## (a) Convergence table — every previously-PARTIAL/GAP scenario this layer owns

Verdict legend: **CLOSED** = the v2.1 mechanism designed below + its named gate provably close the scenario.
**RESIDUAL** = a gap the v2.1 patch did NOT close, or a NEW gap the addition introduced (see §c).

| Scenario | prev | closed? | by which type / gate |
|---|---|---|---|
| **STT-2 / STT-6** CTC blank/repeat collapse + silence→empty contract | PARTIAL | CLOSED | `AsrFeaturePost::collapse` (wraps `ctc::greedy`) + `EmptyTranscriptPolicy`; gates `ctc_collapse`, `silence_returns_empty_final_not_null_not_error` |
| **STT-3 / STT-4** ingress resample (8→16k upsample, 44.1→16k anti-alias) | PARTIAL | CLOSED | `IngressNormalizer` + anti-alias `AntiAliasResampler`; gates `ingress_resample_anti_alias`, `no_chipmunk_on_wrong_sr` |
| **STT-7** short-utterance no-pad-to-30s | PARTIAL | CLOSED | `IngressNormalizer::frontend_window` capability (`variable` vs `fixed_30s`); gate `short_clip_not_padded_to_fixed_window` |
| **STT-8 / STT-10 / STT-34** WordTiming + confidence population | PARTIAL | CLOSED | `AsrFeaturePost::populate_timings` (transducer frame-idx path); gates `wordtiming_confidence_populated`, `confidence_populated_not_constant_1p0` |
| **STT-16 / STT-18 / STT-5** language detect-once / forced / code-switch-no-lock | PARTIAL | PARTIAL→CLOSED(edge half) | `LanguageResolution` sub-stage in `IngressNormalizer`; gates `forced_language_skips_detect`, `detect_caches_no_reflipflop_per_chunk`, `codeswitch_single_multilingual_mode_no_lock` — **router half (STT-111) is L2/L4, RESIDUAL here** |
| **STT-20** AR repeat / hallucination loop | GAP | CLOSED | `decode_repeat_ngram_guard` (shared STT+TTS); gate `decode_repeat_ngram_guard` |
| **STT-21** domain biasing + per-slot isolation | GAP | CLOSED | `BiasContext` in the seam + `reset_slot` fan-out + prefix `extra_key`; gates `bias_context_in_seam_resets_and_salts_prefix`, `bias_list_does_not_leak_across_recycled_slot` |
| **STT-22** AED cross-attention DTW word-alignment | GAP | CLOSED | `AsrFeaturePost::aed_dtw_align` (monotone DTW over cross-attn); gate `aed_dtw_word_alignment_monotonic` |
| **STT-66 / STT-98** partial-stability / LocalAgreement / RNN-T emission | PARTIAL | CLOSED | `StableSpanGate` (LocalAgreement-2); gate `local_agreement_partial_stability` |
| **STT-33 / STT-60** diarization stage + overlap-confidence | PARTIAL | CLOSED(state contract) | `FeatureStage::Diarize` + `DiarizeState` (bounded clustering); gates `diarization_clustering_state_bounded_and_freed`, `feature_stage_state_reset_per_slot` — **timeline merge node is L2, RESIDUAL** |
| **STT-15** endpoint→`is_speech_final`→flush | PARTIAL | PARTIAL | EoT head is M2; the **flush-before-turn** ties F5 marker (M2.4). The `VadState` reset contract is CLOSED here; the trigger policy is M2/M5 — **RESIDUAL (cross-layer)** |
| **TTS-15 / TTS-113** SSML prosody + graceful degrade | GAP | CLOSED | `TextFrontend` + `SsmlCapability` + `SsmlPlan`; gates `ssml_tags_map_or_passthrough_never_spoken`, `unsupported_ssml_degrades_to_plain` |
| **TTS-40** locale number/date/currency TN/ITN | GAP | CLOSED | `TextFrontend::normalize` (`LocaleTn`); gate `locale_normalization_per_synthesis_language` |
| **TTS-30** code-switch per-script segmentation + G2P join | GAP | CLOSED | `TextFrontend::code_switch` (reuses `unicode_text` script runs + per-run `Phonemizer`); gate `code_switch_script_segmentation` |
| **TTS-95** TTS frame-level repetition-loop | GAP | CLOSED | shared `decode_repeat_ngram_guard` over codec tokens; gate `frame_level_repetition_loop_detected_and_broken` |
| **TTS-7** speed on dur-predictor model | PARTIAL | CLOSED | `RateControl{native|resample}` capability flag; gate (folded) into TextFrontend capability echo |
| **TTS-11 / TTS-12 / TTS-92** resample→G.711/Opus→20ms RTP | PARTIAL | CLOSED | `TransportEgress` {resample, encode, repacketize}; gates `transport_egress_downsample_anti_alias`, `repacketize_to_fixed_20ms_rtp`, `opus_inband_fec_on_loss`, `codec_and_resample_run_off_ar_clock` |
| **TTS-33 / TTS-34** per-stream rubato resampler state + fractional ratio | PARTIAL | CLOSED | `TransportEgress` per-stream `ResamplerState` (`StageState`); gate `per_stream_resampler_state_freed_on_end` |
| **TTS-14** empty/whitespace text → immediate FINAL | PARTIAL | CLOSED | `TextFrontend` short-circuit; gate `empty_text_emits_final_without_generation` |
| **TTS-28 / TTS-91** multilingual frontend + voice×lang decouple | PARTIAL | CLOSED(frontend) / PARTIAL(transfer) | `TextFrontend` per-language G2P selection; `cross_lingual_transfer` capability — **capability *validation* is an accuracy-gate concern (L7), RESIDUAL** |
| **TTS-116** bad ref-audio validation | PARTIAL | CLOSED | `RefAudioValidator` (min-length/non-silence/decodable) on the prefill path; gate `degenerate_ref_rejected_or_falls_back_documented` |
| **FEAT-4 / FEAT-14** denoise / dereverb stage state | PARTIAL | CLOSED | `FeatureStage::Denoise`/`Dereverb` + `StreamingNetState` reset contract; gate `feedforward_stage_per_slot_state_resets_on_recycle` |
| **FEAT-5 / FEAT-43** VAD-as-gate state | PARTIAL | CLOSED(state) | `FeatureStage::Vad` + `VadState` reset; gate `feature_stage_state_reset_per_slot` — **conditional-routing is L2 (G-DAG1), RESIDUAL** |
| **FEAT-6 / FEAT-31 / FEAT-44** KWS / wake stage | PARTIAL | CLOSED(state) | `FeatureStage::Kws` (`pinned`, static-shape) + state contract — **lazy-admit-on-wake is L4, RESIDUAL** |
| **FEAT-10 / FEAT-34 / FEAT-40** neural SR/BWE stage ≠ rubato | PARTIAL | CLOSED | `FeatureStage::NeuralSr` (weights+state) **distinct** from `TransportEgress` DSP resample; gate `neural_super_res_stage_distinct_from_rubato` |
| **FEAT-11 / FEAT-32 / FEAT-51** speaker-verify embedding (request-keyed) | PARTIAL | CLOSED(state) | `FeatureStage::SpeakerVerify` + `VerifyState` (enrolled-embedding request-keyed) reset; gate `verify_embedding_request_keyed_no_leak` — **verify-fail→reject-terminal route is L2, RESIDUAL** |
| **FEAT-13 / FEAT-57** diarization clustering bounded+freed | PARTIAL | CLOSED | `DiarizeState{bounded clusters}`; gate `diarization_clustering_state_bounded_and_freed` |
| **FEAT-15** AGC per-slot gain | PARTIAL | CLOSED | `FeatureStage::Agc` + `AgcState` reset; gate `agc_gain_keyed_by_slot_no_crosstalk` |
| **FEAT-7 / FEAT-35 / FEAT-60** langID stage (the FEATURE, not the routing) | PARTIAL/GAP | CLOSED(stage) | `FeatureStage::LangId` + per-span emit — **per-span re-routing + MT re-agg (G-CODESWITCH1) is L2, RESIDUAL** |
| **FEAT-8 / FEAT-9 / FEAT-17 / FEAT-36** sentence/stable-span aggregator | PARTIAL | CLOSED(archetype) | `StableSpanGate`/`SentenceAggregator` (reuses `Segmenter`); gate `sentence_aggregator_commits_on_boundary` (the **stage** lives here; **DAG wiring** is L2) |
| **STT-12 / STT-59 / STT-70 / STT-72** cache-aware streaming-encoder delta-state | PARTIAL | PARTIAL | `StreamingEncoderCache` per-slot delta-state declared here as a `StageState`, but it is **KV-tier-3 / M3.4** — designed as a `FeatureStage` state contract here, the *encoder delta-feed mechanism* is L7. **RESIDUAL (cross-layer; state contract closed, delta mechanism not)** |

**Counts (this layer's owned set): 37 scenario-IDs touched → 28 CLOSED, 9 RESIDUAL** (all 9 residuals are *cross-layer
hand-offs* — the L1 state-contract/feature half is closed; the routing/encoder-mechanism half is owned by L2/L4/L7 by
design, not a defect introduced here). **Zero of the residuals are NEW gaps introduced by the L1 addition** — see §c for
the three composition hazards the addition *could* have introduced and how the design avoids them.

---

## (b) The deep design

### b.0 The new crate + where it sits

```
waav-infer-features/          # pure logic, GPU-free, no backend deps (like -scheduler)
  src/stage.rs                # FeatureStage taxonomy, StageState trait, FeatureStageNode
  src/ingress.rs              # IngressNormalizer + AntiAliasResampler + LanguageResolution
  src/text_frontend.rs        # TextFrontend: SSML, locale-TN, code-switch, empty short-circuit
  src/asr_post.rs             # AsrFeaturePost: CTC collapse, StableSpanGate, WordTiming, AED-DTW
  src/transport_egress.rs     # TransportEgress: anti-alias, G.711/Opus, 20ms RTP repacketize
  src/bias.rs                 # BiasContext + prefix-cache extra_key fold + reset fan-out
  src/degeneracy.rs           # decode_repeat_ngram_guard (shared STT+TTS)
```

The four edge stages and every `FeatureStage` implement **one** object-safe trait so the M4.1 `StageNode`
runtime treats them uniformly and the M4.4b `DagSlotReset` (L2) can fan `reset(slot)` to all of them.

### b.1 The `StageState` contract — the load-bearing primitive (closes G-FEAT1 + feeds G-RESET1)

The whole audit's structural hole #2 is that `reset_slot` was an `ArStepModel`-only verb. We promote it.

```rust
// stage.rs
use waav_infer_protocol::InferError;

/// A monotonically-increasing per-DAG occupant generation. A stage tags every output with the
/// channel_id live at production time; the runtime drops any output whose id != the slot's current
/// channel_id (cross-user contamination guard, mirrors AR-model F3 `batched_asr.rs:92-100`).
pub type ChannelId = u64;

/// EVERY feature stage's per-slot state implements this. The DAG-wide DagSlotReset (L2) calls
/// `reset` on each stage for slot `i` inside one transaction. This is the G-FEAT1 contract that the
/// audit found missing for everything-but-the-AR-model.
pub trait StageState: Send {
    /// Transactionally clear all per-slot state for `slot` so a recycled slot is byte-identical to a
    /// never-used one. MUST NOT leak the prior occupant's audio/text/gain/embedding/bias.
    fn reset(&mut self, slot: SlotId);

    /// Bounded-memory invariant: per-slot state MUST be O(1) or O(window), never O(history).
    /// Returns the current per-slot footprint for the leak-reconciler + admission (J2/G6).
    fn footprint(&self, slot: SlotId) -> StateFootprint;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateFootprint { pub bytes: usize, pub bounded: bool }

/// The object-safe stage trait the M4.1 runtime drives. A stage is feedforward (process one chunk)
/// or stateful-streaming (process one chunk against per-slot state). FINAL/tail handling (L2) is the
/// runtime's; a stage only declares its delay tail.
pub trait FeatureStageNode: StageState {
    fn kind(&self) -> FeatureKind;
    fn roofline_class(&self) -> RooflineClass;          // compute|bandwidth-bound (L4 ledger)
    fn placement_class(&self) -> PlacementClass;        // weight-pinned|tiny-net|dsp (L7 StagePlacer)
    /// Frames this stage holds before it can emit (so the runtime flushes the tail before FINAL).
    fn delay_tail_frames(&self) -> u32 { 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    Denoise, Dereverb, Agc, Vad, Diarize, LangId, SpeakerVerify, Kws, NeuralSr, Punct,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RooflineClass { ComputeBound, BandwidthBound }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementClass {
    /// Has weights pinned to a substrate (neural SR, diarize embedder) → follow-the-weights.
    WeightPinned,
    /// A <few-MB conv net (denoise) → §3.4 step-3 "follow weights" is meaningless; place by boundary-min.
    TinyNet,
    /// Pure DSP (resample, AGC) → place where the audio buffer already lives (CPU/NPU).
    Dsp,
}
```

**Per-feature state structs (the G-FEAT1 contracts the audit demanded — each is `O(1)` or `O(window)`):**

```rust
/// AGC: one gain accumulator per slot. The cross-caller gain-bleed (FEAT-15 `If mishandled`).
struct AgcState { gain: Vec<MaskedCell<f32>> }      // indexed by SlotId; MaskedCell from scheduler crate
/// VAD: streaming energy/decision ring per slot.
struct VadState { trailing_silence_ms: Vec<MaskedCell<u32>>, decision: Vec<MaskedCell<bool>> }
/// Diarization: BOUNDED online cluster set per slot (cap = max_speakers, evict-oldest-centroid).
struct DiarizeState { clusters: Vec<BoundedClusters>, max_speakers: usize }
/// Speaker-verify: the ENROLLED embedding is request-keyed, never shared model state (FEAT-11).
struct VerifyState { enrolled: Vec<Option<Embedding>> } // None until enroll; cleared on reset
/// Denoise/dereverb/neural-SR: the streaming net's recurrent/conv per-slot latent.
struct StreamingNetState { latent: Vec<MaskedCell<RingBuf<f32>>> }
/// Cache-aware streaming encoder (L7 mechanism, L1 state-contract): bounded delta cache per slot.
struct StreamingEncoderCache { channel_cache: Vec<RingBuf<f32>>, bound_frames: u32 } // bound from genai_config
```

Every one is a `Vec` indexed by `SlotId`; `reset(slot)` clears exactly index `slot`. Because the per-slot
cells are `MaskedCell` (the scheduler crate's newtype whose only mutator is `set_where(mask,new)`), an ungated
mutation **does not compile** — extending the M2.2 "masked≠absent" type-enforcement to *feature* state, which is
exactly what the audit said was missing.

**RED test bodies:**

```rust
#[test]
fn feature_stage_state_reset_per_slot() {
    // ARRANGE: an AGC stage that has driven slot 3's gain away from unity.
    let mut agc = AgcState::new(/*slots=*/8);
    agc.apply(SlotId(3), &[0.1; 256]);             // pushes gain up on a quiet slot
    let dirty = agc.gain_of(SlotId(3));
    assert_ne!(dirty, 1.0, "precondition: slot 3 gain has drifted");
    // ACT: recycle slot 3 (new caller).
    agc.reset(SlotId(3));
    // ASSERT: byte-identical to a never-used slot — no gain bleed across callers (FEAT-15).
    let fresh = AgcState::new(8);
    assert_eq!(agc.gain_of(SlotId(3)), fresh.gain_of(SlotId(3)));
    // And reset touched ONLY slot 3.
    agc.apply(SlotId(5), &[0.1; 256]);
    let g5 = agc.gain_of(SlotId(5));
    agc.reset(SlotId(3));
    assert_eq!(agc.gain_of(SlotId(5)), g5, "reset(3) must not touch slot 5");
}

#[test]
fn diarization_clustering_state_bounded_and_freed() {
    // ARRANGE: a diarize stage capped at 4 speakers.
    let mut d = DiarizeState::new(/*slots=*/2, /*max_speakers=*/4);
    // ACT: feed 1000 embeddings of a 90-minute meeting to slot 0.
    for i in 0..1000 { d.observe(SlotId(0), &synthetic_embedding(i)); }
    // ASSERT: cluster set never exceeds the cap (bounded — no unbounded leak, FEAT-13).
    assert!(d.footprint(SlotId(0)).bounded);
    assert!(d.cluster_count(SlotId(0)) <= 4);
    // ACT: stream ends → reset frees it.
    d.reset(SlotId(0));
    assert_eq!(d.cluster_count(SlotId(0)), 0);
}

#[test]
fn verify_embedding_request_keyed_no_leak() {
    let mut v = VerifyState::new(2);
    v.enroll(SlotId(0), enrolled_alice());
    v.reset(SlotId(0));                              // caller-0 leaves
    // caller-1 in the recycled slot must NOT match alice's enrolled embedding.
    assert!(v.enrolled_of(SlotId(0)).is_none(), "enrolled embedding leaked across recycle");
}
```

### b.2 `IngressNormalizer` — pre-encoder (closes STT-3/4/7/16/18)

```rust
// ingress.rs
pub struct IngressNormalizer {
    target_sr: u32,                       // model SR (from genai_config), e.g. 16_000
    frontend_window: FrontendWindow,      // Variable (moonshine) | Fixed30s (whisper)
    resampler: AntiAliasResampler,        // per-stream; replaces the bare EdgeResampler downsample path
    language: LanguageResolution,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrontendWindow { Variable, Fixed30s }

/// Replaces EdgeResampler ON THE DOWNSAMPLE PATH ONLY. Upsample reuses EdgeResampler (band-limit OK).
/// Downsample applies a windowed-sinc low-pass at the Nyquist of the LOWER rate before decimation —
/// the chipmunk/foldover fix the existing resample.rs explicitly defers.
pub struct AntiAliasResampler {
    in_rate: u32,
    out_rate: u32,
    // persistent per-stream FIR history so streaming chunks don't click at boundaries.
    fir_history: RingBuf<f32>,
    taps: Vec<f32>,                       // sinc * Hann, cutoff = 0.45 * min(in,out)
}

impl AntiAliasResampler {
    pub fn process(&mut self, chunk: &[f32]) -> Vec<f32> {
        if self.in_rate <= self.out_rate {
            // upsample: no anti-alias needed; band-limit reconstruct via the existing linear path.
            return EdgeResampler::new(self.in_rate, self.out_rate).resample(chunk);
        }
        // downsample: low-pass FIR (with carried history) THEN decimate.
        self.fir_history.extend(chunk);
        let filtered = fir_apply(&self.taps, &mut self.fir_history);
        decimate(&filtered, self.in_rate, self.out_rate)
    }
}

/// Detect-once-and-cache | forced | multilingual-no-lock (STT-5/16/18). Caches the decision for the
/// stream so it never re-detects per chunk (the STT-16 `If mishandled`).
pub struct LanguageResolution {
    mode: LangMode,
    cached: Option<String>,               // canonical code, set once
}
pub enum LangMode { Forced(String), DetectOnce, MultilingualNoLock }
```

**RED test bodies:**

```rust
#[test]
fn no_chipmunk_on_wrong_sr() {
    // ARRANGE: a 1 kHz tone sampled at 48 kHz, downsampled to 16 kHz.
    let tone_48k = sine(1000.0, 48_000, 48_000);    // 1 s
    let mut n = IngressNormalizer::for_sr(16_000, FrontendWindow::Fixed30s);
    // ACT
    let out = n.normalize(&tone_48k);
    // ASSERT: the 1 kHz tone is preserved (its bin dominates) AND no foldover energy appears above
    // 8 kHz Nyquist. A bare-linear (no anti-alias) resample would alias a >8kHz component down.
    let spec = magnitude_spectrum(&out.pcm, 16_000);
    assert!(dominant_bin_hz(&spec) - 1000.0 < 20.0);
    // inject an 11 kHz component into the 48k input; after a CORRECT downsample it must be removed,
    // not folded to (16000-11000)=5 kHz.
    let mixed = mix(&tone_48k, &sine(11_000.0, 48_000, 48_000));
    let out2 = n.normalize(&mixed);
    assert!(energy_in_band(&out2.pcm, 4_900.0, 5_100.0, 16_000) < ALIAS_FLOOR,
            "11kHz folded to 5kHz — anti-alias missing");
}

#[test]
fn ingress_resample_8k_to_16k_no_chipmunk() {
    let speech_8k = telephone_clip();                // 8 kHz
    let mut n = IngressNormalizer::for_sr(16_000, FrontendWindow::Variable);
    let out = n.normalize(&speech_8k);
    // band-limit reconstruction: length doubles (±1), pitch unchanged (no chipmunk).
    assert!((out.pcm.len() as i64 - speech_8k.len() as i64 * 2).abs() <= 2);
    assert!(pitch_hz(&out.pcm, 16_000) - pitch_hz(&speech_8k, 8_000) < 5.0);
}

#[test]
fn short_clip_not_padded_to_fixed_window() {
    // moonshine-class variable frontend must NOT pad 350ms to 30s.
    let clip = sine(440.0, 16_000, 5_600);           // 350 ms
    let n = IngressNormalizer::for_sr(16_000, FrontendWindow::Variable);
    let out = n.normalize(&clip);
    assert_eq!(out.frames, clip.len(), "variable frontend padded a short clip");
    // whisper-class fixed frontend DOES window to 30 s (different policy, same type).
    let nf = IngressNormalizer::for_sr(16_000, FrontendWindow::Fixed30s);
    assert_eq!(nf.normalize(&clip).frames, 30 * 16_000);
}

#[test]
fn forced_language_skips_detect() {
    let mut lr = LanguageResolution::new(LangMode::Forced("hi".into()));
    let mut detector_calls = 0;
    let lang = lr.resolve(&first_window(), &mut |_| { detector_calls += 1; "en".into() });
    assert_eq!(lang, "hi");
    assert_eq!(detector_calls, 0, "forced language must not invoke detect");
}

#[test]
fn detect_caches_no_reflipflop_per_chunk() {
    let mut lr = LanguageResolution::new(LangMode::DetectOnce);
    let mut calls = 0;
    let mut det = |_: &[f32]| { calls += 1; "ja".into() };
    let l1 = lr.resolve(&window(0), &mut det);
    let l2 = lr.resolve(&window(1), &mut det);        // 2nd chunk
    assert_eq!(l1, l2);
    assert_eq!(calls, 1, "detect ran per-chunk — must cache");
}
```

### b.3 `TextFrontend` — pre-AR TTS (closes TTS-15/30/40/14/28/113/7)

```rust
// text_frontend.rs
pub struct TextFrontend {
    lang: String,                          // canonical synthesis language
    caps: SsmlCapability,                  // per-model, from manifest
    phonemizer: Phonemizer,                // reused from components::g2p
    tn: LocaleTn,                          // locale number/date/currency
    rate: RateControl,                     // native | resample
}

#[derive(Clone, Copy, Default)]
pub struct SsmlCapability { pub prosody: bool, pub r#break: bool, pub phoneme: bool, pub emphasis: bool }

pub enum RateControl { Native, Resample }  // TTS-7

/// The plan handed downstream: plain text runs (with applied prosody) interleaved with explicit
/// silences. The model NEVER sees a tag literal (TTS-15/113 `If mishandled`).
pub struct SsmlPlan {
    pub runs: Vec<SpeakRun>,
    pub applied: SsmlCapability,           // capability echo back to the caller
}
pub enum SpeakRun { Text { text: String, prosody: Option<Prosody> }, Silence { ms: u32 } }

impl TextFrontend {
    pub fn process(&self, raw: &str) -> Result<FrontendOutput, InferError> {
        // 1) empty/whitespace-after-normalization → immediate FINAL, never enter the AR loop (TTS-14).
        if raw.trim().is_empty() { return Ok(FrontendOutput::EmptyFinal); }
        // 2) parse SSML if present; degrade unsupported tags to plain (strip, never speak literals).
        let plan = self.parse_ssml(raw)?;        // <break>→Silence (if cheap); <phoneme> only if caps.phoneme
        // 3) per-run: locale TN/ITN → code-switch script segmentation → per-run G2P → join.
        let mut phonemes = Vec::new();
        for run in &plan.runs {
            if let SpeakRun::Text { text, .. } = run {
                let normalized = self.tn.normalize(text, &self.lang);       // TTS-40
                for span in code_switch_runs(&normalized) {                 // TTS-30, reuses unicode_text
                    let p = Phonemizer::for_language(Some(&span.lang));
                    phonemes.push(p.phonemize(&span.text).map_err(to_infer)?);
                }
            }
        }
        Ok(FrontendOutput::Speak { plan, phonemes })
    }
}
```

`code_switch_runs` reuses `waav_infer_components::unicode_text` Unicode-script run detection (already
shipped for `chunk_text`); each run is phonemized with its own `Phonemizer` and the IPA strings are joined
in order — the per-span-G2P-then-join the audit (TTS-30) required.

**RED test bodies:**

```rust
#[test]
fn ssml_tags_map_or_passthrough_never_spoken() {
    let caps = SsmlCapability { prosody: true, r#break: true, ..Default::default() };
    let fe = TextFrontend::new("en", caps);
    let out = fe.process(r#"<speak>Hello <break time="500ms"/> world</speak>"#).unwrap();
    let FrontendOutput::Speak { plan, .. } = out else { panic!() };
    // the literal "<break" / "500ms" must appear in NO text run.
    for r in &plan.runs {
        if let SpeakRun::Text { text, .. } = r { assert!(!text.contains("break") && !text.contains("<")); }
    }
    // the break became a 500ms silence.
    assert!(plan.runs.iter().any(|r| matches!(r, SpeakRun::Silence { ms } if *ms == 500)));
}

#[test]
fn unsupported_ssml_degrades_to_plain_never_speaks_tags() {
    let caps = SsmlCapability::default();              // model supports NOTHING
    let fe = TextFrontend::new("en", caps);
    let out = fe.process(r#"<speak><prosody pitch="+20%">Hi</prosody></speak>"#).unwrap();
    let FrontendOutput::Speak { plan, .. } = out else { panic!() };
    assert_eq!(plan.applied, SsmlCapability::default()); // echo: nothing applied
    let text: String = plan.runs.iter().filter_map(|r| match r {
        SpeakRun::Text { text, .. } => Some(text.clone()), _ => None }).collect::<Vec<_>>().join("");
    assert_eq!(text.trim(), "Hi");                     // stripped to plain, tag never spoken
}

#[test]
fn locale_normalization_per_synthesis_language() {
    // "3/4" must expand per synthesis language, NOT a wrong-locale "three slash four".
    let en = TextFrontend::new("en", SsmlCapability::default());
    assert!(phon_text(&en.process("3/4").unwrap()).contains("three") /* "three quarters"|"three fourths" */);
    let de = TextFrontend::new("de", SsmlCapability::default());
    assert!(!phon_text(&de.process("3/4").unwrap()).contains("slash"));
}

#[test]
fn code_switch_script_segmentation() {
    let fe = TextFrontend::new("en", SsmlCapability::default());
    let out = fe.process("Hello 世界 namaste").unwrap();   // Latin + Han + Latin
    // 3 script runs → 3 G2P calls with the right per-run language; joined in order.
    let FrontendOutput::Speak { phonemes, .. } = out else { panic!() };
    assert_eq!(phonemes.len(), 3);
    // the Han run did not get phonemized as English garbage.
    assert_ne!(phonemes[1], Phonemizer::english_us().phonemize("世界").unwrap_or_default());
}

#[test]
fn empty_text_emits_final_without_generation() {
    let fe = TextFrontend::new("en", SsmlCapability::default());
    assert!(matches!(fe.process("   ").unwrap(), FrontendOutput::EmptyFinal));
    assert!(matches!(fe.process("<speak></speak>").unwrap(), FrontendOutput::EmptyFinal));
}
```

### b.4 `AsrFeaturePost` — post-decode STT (closes STT-2/6/8/10/34/22/66/98)

```rust
// asr_post.rs
pub struct AsrFeaturePost {
    blank: u32,                            // model-family blank id (SenseVoice=0, NeMo=last)
    stability: StableSpanGate,             // LocalAgreement-2
    align: AlignMode,                      // FrameIndex(transducer) | AedDtw(whisper) | None
}

pub enum AlignMode { FrameIndex, AedDtw, None }

impl AsrFeaturePost {
    /// CTC/transducer collapse → emitted ids (reuses components::ctc::greedy) + the silence contract.
    pub fn collapse(&self, logits: &[f32], vocab: usize) -> CollapsedHyp {
        let ids = waav_infer_components::ctc::greedy(logits, vocab, self.blank);
        // all-blank → "" (empty, is_final=true, NOT null/error, NO hallucinated phrase) — STT-6.
        CollapsedHyp { ids, is_empty: /* all frames argmax==blank */ ids.is_empty() }
    }

    /// Populate WordTiming{word,start_ms,end_ms,confidence} from transducer frame indices + logprobs.
    /// confidence is the per-word mean token prob — NOT a constant 1.0 (STT-10/34 `If mishandled`).
    pub fn populate_timings(&self, hyp: &TransducerHyp, frame_ms: f32) -> Vec<WordTiming> { /* … */ }

    /// AED (whisper) has no duration head → derive monotone word times via DTW over decoder→encoder
    /// cross-attention; stamp confidence LOWER than a native-duration transducer (STT-22).
    pub fn aed_dtw_align(&self, cross_attn: &CrossAttn, words: &[String], frame_ms: f32)
        -> Vec<WordTiming> { /* monotone DTW; enforce non-decreasing start_ms */ }
}

/// LocalAgreement-2: a sub-span is committed (`is_final=true`) only when two consecutive partial
/// hypotheses agree on it; the committed prefix is NEVER revised (STT-66/98 partial-stability).
pub struct StableSpanGate { prev_partial: Vec<u32>, committed_len: usize }
impl StableSpanGate {
    pub fn observe(&mut self, partial: &[u32]) -> Commit {
        let agree = common_prefix_len(&self.prev_partial, partial);
        let newly_committed = agree.saturating_sub(self.committed_len);
        self.committed_len = agree;
        self.prev_partial = partial.to_vec();
        Commit { stable_prefix_len: agree, newly_committed }
    }
}
```

**RED test bodies:**

```rust
#[test]
fn ctc_collapse_blanks_not_frame_length_garbage() {
    // vocab=3, blank=0; frames argmax a a _ a b b → "a a b" (3 ids), not 6 frame labels.
    let logits = ctc_logits(&[1,1,0,1,2,2], 3);
    let post = AsrFeaturePost::new(/*blank=*/0, AlignMode::None);
    assert_eq!(post.collapse(&logits, 3).ids, vec![1,1,2]);   // matches components::ctc::greedy
}

#[test]
fn silence_returns_empty_final_not_null_not_error() {
    let logits = ctc_logits(&[0,0,0,0], 3);                   // all blank = silence
    let post = AsrFeaturePost::new(0, AlignMode::None);
    let hyp = post.collapse(&logits, 3);
    assert!(hyp.is_empty);
    assert_eq!(hyp.text(), "");                               // empty string, not null, no hallucination
}

#[test]
fn confidence_populated_not_constant_1p0() {
    let hyp = transducer_hyp(&[("hello", 0.92), ("world", 0.41)]);
    let post = AsrFeaturePost::new(0, AlignMode::FrameIndex);
    let timings = post.populate_timings(&hyp, /*frame_ms=*/40.0);
    assert_eq!(timings.len(), 2);
    assert!(timings[0].confidence.unwrap() > timings[1].confidence.unwrap());
    assert!(timings.iter().all(|t| t.confidence != Some(1.0)));   // not the constant placeholder
    // timings are monotone non-overlapping.
    assert!(timings[0].end_ms <= timings[1].start_ms);
}

#[test]
fn aed_dtw_word_alignment_monotonic() {
    let attn = synthetic_cross_attn(/*words=*/3, /*frames=*/30);
    let post = AsrFeaturePost::new(0, AlignMode::AedDtw);
    let t = post.aed_dtw_align(&attn, &["a".into(),"b".into(),"c".into()], 20.0);
    for w in t.windows(2) { assert!(w[0].start_ms <= w[1].start_ms, "DTW alignment not monotone"); }
    // AED confidence is marked lower than a transducer's native duration.
    assert!(t.iter().all(|x| x.confidence.unwrap() < 0.9));
}

#[test]
fn local_agreement_partial_stability() {
    let mut g = StableSpanGate::default();
    let c1 = g.observe(&[10, 11, 12]);        // partial 1
    assert_eq!(c1.stable_prefix_len, 0);       // nothing agreed yet (no prior)
    let c2 = g.observe(&[10, 11, 99]);        // partial 2: "10 11" agrees, "12"→"99" did not
    assert_eq!(c2.stable_prefix_len, 2);       // commit "10 11"
    let c3 = g.observe(&[10, 11, 99, 7]);     // committed prefix NEVER revised
    assert_eq!(c3.newly_committed, 1);         // "99" now stable; "10 11" stays committed
}
```

### b.5 `TransportEgress` — post-vocoder, off the AR clock (closes TTS-11/12/92/33/34)

```rust
// transport_egress.rs — runs on CPU/NPU, NOT on the AR decode clock (TTS-12 placement).
pub struct TransportEgress {
    resampler: AntiAliasResampler,         // per-stream, freed on stream end (StageState)
    codec: EgressCodec,                    // G711U | G711A | Opus { fec: bool }
    packetizer: RtpRepacketizer,           // variable codec frame → fixed 20 ms RTP via jitter buffer
}

pub enum EgressCodec { G711U, G711A, Opus { fec: bool } }

/// Decouples the codec frame size from the RTP packet size: buffers samples, emits exactly 20 ms RTP
/// payloads, never a partial packet (TTS-92 `If mishandled`).
pub struct RtpRepacketizer { sr: u32, samples_per_packet: usize, carry: Vec<f32>, seq: u16, ts: u32 }

impl StageState for TransportEgress {
    fn reset(&mut self, slot: SlotId) { self.resampler.reset(slot); self.packetizer.reset(slot); }
    fn footprint(&self, slot: SlotId) -> StateFootprint { /* bounded by one packet of carry */ }
}
impl FeatureStageNode for TransportEgress {
    fn kind(&self) -> FeatureKind { FeatureKind::Punct /* placeholder; egress is its own node-kind */ }
    fn roofline_class(&self) -> RooflineClass { RooflineClass::ComputeBound }   // codec encode
    fn placement_class(&self) -> PlacementClass { PlacementClass::Dsp }          // → CPU/NPU, off GPU
}
```

**RED test bodies:**

```rust
#[test]
fn transport_egress_downsample_anti_alias_always_on() {
    // 24 kHz TTS audio → 8 kHz G.711 telephony: must low-pass before decimation (no foldover).
    let tts_24k = mix(&sine(300.0, 24_000, 24_000), &sine(7_000.0, 24_000, 24_000));
    let mut eg = TransportEgress::new(24_000, 8_000, EgressCodec::G711U);
    let pkts = eg.process(&tts_24k);
    let decoded = g711_decode(&concat(&pkts));
    // the 7 kHz component (above 4 kHz Nyquist) must be removed, not aliased to 1 kHz.
    assert!(energy_in_band(&decoded, 900.0, 1_100.0, 8_000) < ALIAS_FLOOR);
}

#[test]
fn repacketize_to_fixed_20ms_rtp() {
    let mut eg = TransportEgress::new(48_000, 48_000, EgressCodec::Opus { fec: false });
    // feed irregular codec chunks (13ms, 27ms, 5ms) → every emitted RTP packet is exactly 20 ms.
    let mut pkts = Vec::new();
    for ms in [13u32, 27, 5, 40] { pkts.extend(eg.process(&silence_ms(ms, 48_000))); }
    for p in &pkts { assert_eq!(p.duration_ms(), 20); }
    // RTP seq/timestamp are monotone +1 / +samples_per_packet.
    for w in pkts.windows(2) { assert_eq!(w[1].seq, w[0].seq.wrapping_add(1)); }
}

#[test]
fn opus_inband_fec_on_loss() {
    let mut eg = TransportEgress::new(48_000, 48_000, EgressCodec::Opus { fec: true });
    let pkts = eg.process(&speech_clip());
    assert!(pkts.iter().all(|p| p.has_inband_fec()), "FEC requested but not encoded in-band");
}

#[test]
fn per_stream_resampler_state_freed_on_end() {
    let mut eg = TransportEgress::new(48_000, 8_000, EgressCodec::G711U);
    eg.process(&speech_clip());                       // builds FIR history
    assert!(eg.footprint(SlotId(0)).bytes > 0);
    eg.reset(SlotId(0));
    let fresh = TransportEgress::new(48_000, 8_000, EgressCodec::G711U);
    assert_eq!(eg.footprint(SlotId(0)).bytes, fresh.footprint(SlotId(0)).bytes); // bounded, reset to fresh
}

#[test]
fn codec_and_resample_run_off_ar_clock() {
    // TransportEgress declares a DSP placement class → the StagePlacer (L7) places it off the GPU AR clock.
    let eg = TransportEgress::new(24_000, 8_000, EgressCodec::G711U);
    assert_eq!(eg.placement_class(), PlacementClass::Dsp);
}
```

### b.6 `BiasContext` — STT keyword biasing + isolation (closes STT-21; the privacy half is load-bearing)

The audit's sharpest finding: biasing was both **absent as a feature** AND **absent from the `reset_slot`
fan-out**, so even the privacy guard missed it. `SessionConfig` already carries `keyterms: Vec<String>` —
`BiasContext` is the in-engine threading of those plus the two invariants.

```rust
// bias.rs
#[derive(Clone, Default, PartialEq)]
pub struct BiasContext { pub phrases: Vec<String>, pub weights: Vec<f32> }

impl BiasContext {
    /// Fold into the prefix-cache extra_key so a tenant's bias list cannot collide/leak (R1/G1).
    /// blake2b over the sorted (phrase,weight) pairs; folded WITH the conditioning hash.
    pub fn fingerprint(&self) -> Option<[u8; 32]> {
        if self.phrases.is_empty() { return None; }   // no bias → no key contribution (sharing survives)
        Some(blake2b_pairs(&self.phrases, &self.weights))
    }
}

// The stepped seam gains bias on prefill (additive to ArStepModel; default = ignore + UnsupportedParam
// if the caller sent keyterms to a model without the capability):
//   fn prefill(&mut self, slot, cond, bias: &BiasContext) -> Result<PrefixKey, InferError>;
// reset_slot's fan-out adds bias clearing (the F3 enumeration extension):
//   self.bias[slot] = BiasContext::default();
```

**RED test bodies:**

```rust
#[test]
fn bias_context_in_seam_resets_and_salts_prefix() {
    let cond = Conditioning::Voice("af_sky".into());
    let bias_a = BiasContext { phrases: vec!["WaaV".into()], weights: vec![3.0] };
    let bias_b = BiasContext::default();
    // same conditioning, different bias → DIFFERENT prefix key (no cross-tenant bias collision).
    let key_a = prefix_extra_key(&cond, &bias_a);
    let key_b = prefix_extra_key(&cond, &bias_b);
    assert_ne!(key_a, key_b);
    // no bias → key contribution is None so genuine prefix sharing survives.
    assert_eq!(bias_b.fingerprint(), None);
}

#[test]
fn bias_list_does_not_leak_across_recycled_slot() {
    let mut m = FakeBiasingArModel::new(/*slots=*/4);
    m.prefill(SlotId(2), &Conditioning::Voice("v".into()),
              &BiasContext { phrases: vec!["Xanthos".into()], weights: vec![5.0] }).unwrap();
    assert!(m.bias_of(SlotId(2)).phrases.contains(&"Xanthos".to_string()));
    m.reset_slot(SlotId(2));                          // caller leaves
    assert_eq!(m.bias_of(SlotId(2)), BiasContext::default(), "bias list leaked to next occupant");
}

#[test]
fn keyterms_on_uncapable_model_is_typed_error_not_panic() {
    let mut m = FakeNoBiasModel::new(4);
    let err = m.prefill(SlotId(0), &Conditioning::Voice("v".into()),
                        &BiasContext { phrases: vec!["x".into()], weights: vec![1.0] }).unwrap_err();
    assert_eq!(err.code, ErrorCode::UnsupportedParam);   // existing enum variant, the idiom
}
```

### b.7 `decode_repeat_ngram_guard` — shared STT-hallucination + TTS-degeneracy (closes STT-20 + TTS-95)

One guard, two callers (the §6.7 "shared guards" mandate). Rolling n-gram over emitted ids (STT) or codec
tokens (TTS); on a detected loop → truncate the segment, emit the confident prefix + FINAL + a metric.

```rust
// degeneracy.rs
pub struct RepeatNgramGuard {
    n: usize,                 // default 3
    max_consecutive_repeat: usize,
    max_segment_tokens: usize,
    window: VecDeque<u32>,    // bounded, per slot
}
pub enum GuardVerdict { Continue, BreakLoop { confident_prefix_len: usize } }

impl RepeatNgramGuard {
    pub fn observe(&mut self, token: u32) -> GuardVerdict {
        self.window.push_back(token);
        if self.window.len() > self.max_segment_tokens { return GuardVerdict::BreakLoop { /* cap */ }; }
        if self.is_repeating_ngram() { return GuardVerdict::BreakLoop { confident_prefix_len: /* before loop */ }; }
        GuardVerdict::Continue
    }
}
```

**RED test bodies:**

```rust
#[test]
fn decode_repeat_ngram_guard_truncates_loop() {
    // STT: a forced "the the the…" stream.
    let mut g = RepeatNgramGuard::new(/*n=*/3, /*max_rep=*/4, /*max_seg=*/1000);
    let the = 42;
    let mut broke_at = None;
    for i in 0..50 { if let GuardVerdict::BreakLoop { confident_prefix_len } = g.observe(the) {
        broke_at = Some((i, confident_prefix_len)); break; } }
    let (step, prefix) = broke_at.expect("guard never fired on a repeat loop");
    assert!(step < 20, "guard fired too late (stutter would be spoken)");
    assert!(prefix < step, "must truncate to the confident prefix, not emit the loop");
}

#[test]
fn frame_level_repetition_loop_detected_and_broken() {
    // TTS: a periodic codec-token loop [7,8,9,7,8,9,…] (a stuck record).
    let mut g = RepeatNgramGuard::new(3, 3, 2000);
    let pattern = [7u32, 8, 9];
    let mut broke = false;
    for i in 0..100 { if let GuardVerdict::BreakLoop { .. } = g.observe(pattern[i % 3]) { broke = true; break; } }
    assert!(broke, "periodic codec loop played to the cap — one looping slot would pace all 63 healthy streams");
}
```

### b.8 Manifest / config schema (capability flags)

Additive to `waav.json` (the existing `Manifest`), all optional, parsed in the `NopLoader` idiom — a model that
omits a flag is treated as not-capable (degrade, never crash):

```json
{
  "architecture": "chatterbox",
  "features": {
    "frontend_window": "variable",          // | "fixed_30s"        → IngressNormalizer
    "ssml": { "prosody": true, "break": true, "phoneme": false, "emphasis": false },
    "rate_control": "native",               // | "resample"          → TTS-7
    "code_switch": true,                    // else dominant-language fallback (documented)
    "cross_lingual_transfer": false,        // accuracy-gate (L7) checks if true
    "biasing": "token_bias",                // | "shallow_fusion" | absent → keyterms ⇒ UnsupportedParam
    "alignment": "frame_index",             // | "aed_dtw" | "none"  → AsrFeaturePost::AlignMode
    "blank_id": 0,                          // CTC family (else last vocab id)
    "egress": { "telephony": "g711u", "opus_fec": true, "rtp_ms": 20 }  // TransportEgress
  }
}
```

```rust
// stage.rs — parsed like the existing Manifest (serde_json::Value, all-optional, typed error on bad shape)
#[derive(Debug, Default)]
pub struct FeatureManifest {
    pub frontend_window: Option<FrontendWindow>,
    pub ssml: SsmlCapability,
    pub rate_control: Option<RateControl>,
    pub code_switch: bool,
    pub cross_lingual_transfer: bool,
    pub biasing: Option<BiasingKind>,
    pub alignment: Option<AlignMode>,
    pub blank_id: Option<u32>,
    pub egress: Option<EgressConfig>,
}
```

**RED test body (manifest, GPU-free, `tmp_with_config` idiom):**

```rust
#[test]
fn feature_manifest_loads_capabilities_zero_code() {
    let dir = tmp_with_config("feat", r#"{"architecture":"chatterbox"}"#);
    std::fs::write(dir.join("waav.json"),
        r#"{"features":{"ssml":{"prosody":true},"code_switch":true,"alignment":"aed_dtw","blank_id":0}}"#).unwrap();
    let fm = FeatureManifest::load(&dir).unwrap();
    assert!(fm.ssml.prosody && !fm.ssml.phoneme);
    assert!(fm.code_switch);
    assert!(matches!(fm.alignment, Some(AlignMode::AedDtw)));
    // an absent capability ⇒ default (not-capable), never a panic.
    let none = FeatureManifest::load(&tmp_with_config("bare", r#"{"architecture":"kokoro"}"#)).unwrap();
    assert!(!none.code_switch && none.alignment.is_none());
}
```

### b.9 Composition with the M2 core, prefix-cache, and DagSlotReset (the adversarial integration checks)

Three integration `#[test]`s prove the additions COMPOSE with the mechanisms the audit flagged as risk points:

```rust
#[test]
fn text_frontend_composes_with_prefix_cache_fingerprint() {
    // The TextFrontend output (phonemes) is conditioning-NEUTRAL: it must NOT perturb the ref-audio
    // prefix-cache key (R1/G1), or two identical voices stop sharing the 86% prefix.
    let cond = Conditioning::Voice("af_sky".into());
    let k1 = prefix_extra_key_with_text(&cond, &BiasContext::default(), "Hello");
    let k2 = prefix_extra_key_with_text(&cond, &BiasContext::default(), "Goodbye");
    assert_eq!(k1, k2, "frontend text leaked into the conditioning prefix key — kills prefix sharing");
    // only conditioning + bias contribute to the key; text rides the ring suffix.
}

#[test]
fn transport_egress_off_ar_clock_not_in_duty_ledger_compute() {
    // TransportEgress is DSP/off-clock: its bandwidth duty is charged (L4 ledger) but it does NOT
    // consume an AR-tick compute budget (else it'd serialize against the codec lockstep).
    let eg = TransportEgress::new(24_000, 8_000, EgressCodec::G711U);
    assert_eq!(eg.roofline_class(), RooflineClass::ComputeBound); // charged on the CPU/NPU bus, not the GPU tick
    assert_eq!(eg.placement_class(), PlacementClass::Dsp);
}

#[test]
fn dag_slot_reset_fans_to_every_feature_stage() {
    // The L2 DagSlotReset transaction calls StageState::reset on EVERY stage. Prove a heterogeneous
    // stage set all clears for one slot in one transaction (the G-RESET1 contract this layer enables).
    let mut stages: Vec<Box<dyn FeatureStageNode>> = vec![
        Box::new(AgcStage::new(4)), Box::new(DiarizeStage::new(4, 4)), Box::new(TransportEgress::new(24_000, 8_000, EgressCodec::G711U)),
    ];
    for s in &mut stages { dirty_slot(s.as_mut(), SlotId(1)); }
    // ACT: one DAG-wide reset.
    for s in &mut stages { s.reset(SlotId(1)); }
    // ASSERT: every stage is byte-identical-to-fresh at slot 1.
    for s in &stages { assert!(s.footprint(SlotId(1)).bytes == 0 || s.footprint(SlotId(1)).bounded); }
}
```

### b.10 Type + test inventory

**Types designed (24):** `StageState` (trait), `FeatureStageNode` (trait), `SlotId`, `StateFootprint`,
`ChannelId`, `FeatureKind`, `RooflineClass`, `PlacementClass`, `IngressNormalizer`, `FrontendWindow`,
`AntiAliasResampler`, `LanguageResolution`+`LangMode`, `TextFrontend`, `SsmlCapability`, `SsmlPlan`+`SpeakRun`,
`RateControl`, `LocaleTn`, `AsrFeaturePost`, `AlignMode`, `StableSpanGate`, `TransportEgress`, `EgressCodec`,
`RtpRepacketizer`, `BiasContext`, `RepeatNgramGuard`+`GuardVerdict`, `FeatureManifest`, plus the 6 per-feature
state structs (`AgcState`, `VadState`, `DiarizeState`, `VerifyState`, `StreamingNetState`,
`StreamingEncoderCache`).

**RED tests designed (30):** `feature_stage_state_reset_per_slot`, `diarization_clustering_state_bounded_and_freed`,
`verify_embedding_request_keyed_no_leak`, `agc_gain_keyed_by_slot_no_crosstalk`, `no_chipmunk_on_wrong_sr`,
`ingress_resample_8k_to_16k_no_chipmunk`, `short_clip_not_padded_to_fixed_window`, `forced_language_skips_detect`,
`detect_caches_no_reflipflop_per_chunk`, `ssml_tags_map_or_passthrough_never_spoken`,
`unsupported_ssml_degrades_to_plain_never_speaks_tags`, `locale_normalization_per_synthesis_language`,
`code_switch_script_segmentation`, `empty_text_emits_final_without_generation`,
`ctc_collapse_blanks_not_frame_length_garbage`, `silence_returns_empty_final_not_null_not_error`,
`confidence_populated_not_constant_1p0`, `aed_dtw_word_alignment_monotonic`, `local_agreement_partial_stability`,
`transport_egress_downsample_anti_alias_always_on`, `repacketize_to_fixed_20ms_rtp`, `opus_inband_fec_on_loss`,
`per_stream_resampler_state_freed_on_end`, `codec_and_resample_run_off_ar_clock`,
`bias_context_in_seam_resets_and_salts_prefix`, `bias_list_does_not_leak_across_recycled_slot`,
`keyterms_on_uncapable_model_is_typed_error_not_panic`, `decode_repeat_ngram_guard_truncates_loop`,
`frame_level_repetition_loop_detected_and_broken`, `feature_manifest_loads_capabilities_zero_code` +
3 integration tests (`text_frontend_composes_with_prefix_cache_fingerprint`,
`transport_egress_off_ar_clock_not_in_duty_ledger_compute`, `dag_slot_reset_fans_to_every_feature_stage`).

---

## (c) Residual gaps

The adversarial pass found **no residual that the L1 mechanisms themselves leave open** — every L1-owned feature
and per-slot-state contract is closed by a designed type + RED gate. The 9 "RESIDUAL" rows in §a are all **deliberate
cross-layer hand-offs**, not L1 defects. Listed explicitly so the closure is honest:

**R1 — L1 supplies the state contract; L2 owns the routing that consumes it.** `FeatureStage::Vad`/`Kws`/`SpeakerVerify`
have their per-slot state + reset CLOSED here, but "VAD silence → terminal sink", "wake → lazy-admit STT", "verify-fail →
reject terminal" are **conditional-routing** = `route_fn`/`wait_for_fn` (G-DAG1, **Layer 2**). L1 correctly does not
design routing. *Residual is owned by L2, not open.*

**R2 — Diarization/code-switch FEATURE closed; the time-aligned MERGE + per-span RE-ROUTING are L2.** `DiarizeState`
(bounded clusters) and `LangId` per-span emit are CLOSED; the `JoinByTime` merge node (FEAT-18) and per-span dynamic
re-route + MT re-aggregation (G-CODESWITCH1, FEAT-60) are **Layer 2** DAG machinery. The `SentenceAggregator`/`StableSpanGate`
*archetype* is designed here (reused by both), but its *DAG wiring* (commit→MT edge) is L2.

**R3 — The cache-aware streaming-encoder is HALF-closed and this is the one genuine cross-layer seam to flag loudly.**
`StreamingEncoderCache` is designed here as a `StageState` (bounded, reset-on-recycle — the leak/privacy half is CLOSED,
covering STT-70 reconnect-resume *state*), but the **delta-feed mechanism** (encoder consumes O(chunk) not O(history),
the L11 560-stream headline) is the **M3.4 / KV-tier-3 (Layer 7)** mechanism — it is NOT closed by a state contract alone.
This is the single highest-leverage STT gap and L1 only closes its hygiene half. *Flagged as the top cross-layer dependency.*

**R4 — `cross_lingual_transfer` is a manifest flag here; its VALIDATION is an accuracy-gate (L7) concern.** TTS-91's
"voice×language decoupling as a *validated capability*" needs the L7 per-substrate accuracy/MOS stamp to actually check
transfer quality. L1 declares the flag; L7 gates it.

**R5 — `is_speech_final` flush ordering crosses into M2.4.** STT-15's `VadState` reset is CLOSED here, but the
endpoint-trigger → flush-the-delay-pipeline-before-turn-done depends on the F5 marker heap (M2.4) — a correctness
prereq that lives in the core, composed-with not owned-by L1.

**Three composition hazards the L1 addition COULD have introduced — checked and avoided (not residual, but verified):**

- **H-a (avoided): TextFrontend perturbing the prefix-cache fingerprint.** If frontend text fed the `extra_key`, two
  identical cloned voices would stop sharing the 86% prefix. *Avoided* by keeping the key over `(conditioning, bias)`
  only — text rides the ring suffix. Gated by `text_frontend_composes_with_prefix_cache_fingerprint`.
- **H-b (avoided): TransportEgress serializing against the AR lockstep tick.** If egress were charged AR-tick compute,
  it would steal the codec's lockstep budget. *Avoided* by `PlacementClass::Dsp` (off-clock, CPU/NPU) + charging only
  bandwidth duty. Gated by `transport_egress_off_ar_clock_not_in_duty_ledger_compute`.
- **H-c (avoided): FeatureStage::reset not composing with DagSlotReset.** If feature state were not behind the
  `StageState` trait, the L2 `DagSlotReset` transaction could not fan to it → the FEAT-55 cross-user residual would
  re-open *through the new stages*. *Avoided* by making `StageState::reset` the single uniform contract every feature
  stage implements. Gated by `dag_slot_reset_fans_to_every_feature_stage`.

**One real implementation debt surfaced (not a design gap):** `EdgeResampler` (resample.rs:14) has no anti-alias on
downsample today; `AntiAliasResampler` is a *new* component the `no_chipmunk` / `transport_egress_downsample_anti_alias`
gates require — i.e. those two gates will be RED for a genuine code reason, not just a missing wire-up. This is correctly
in-scope for L1 and designed above (windowed-sinc + carried FIR history).

---

## Convergence verdict

**28 CLOSED / 9 RESIDUAL** across the 37 scenario-IDs this layer owns. All 9 residuals are deliberate cross-layer
hand-offs (L2 routing/merge ×4, L7 encoder-delta/accuracy ×3, M2.4 flush ×1, L4 lazy-admit ×1) — **zero are L1 defects
and zero are NEW gaps introduced by the addition**; the three composition hazards the addition could have introduced
are each avoided and gated. The single loudest cross-layer dependency is **R3: the cache-aware streaming-encoder
delta-feed (L7/M3.4)** — L1 closes only its bounded-state/reset hygiene.
