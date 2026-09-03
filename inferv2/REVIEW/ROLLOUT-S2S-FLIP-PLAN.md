# ROLLOUT — S2S `as_duplex` verb + ring-flip readiness audit

Branch `waav-infer-v2-build`, HEAD `3571b71` (post torch-2.12 + TRT-Throughput merge).
Scope: CODE/compile only (no GPU; another agent holds the GPU). Two deliverables.

---

## DELIVERABLE A — S2S `LoadedModel::as_duplex` verb (NET-NEW, compile-green)

### What shipped (additive, mirrors `as_stepped`)

| Piece | File:line | Role |
|---|---|---|
| `S2sModel::as_duplex(&mut self) -> Option<&mut dyn DuplexStepModel>` | `crates/waav-infer-core/src/model.rs:566` (trait hook, default `None`) | the verb hook — duplex analog of `TtsModel::as_stepped`/`SttModel::as_stepped` |
| `LoadedModel::as_duplex(&mut self) -> Option<&mut dyn DuplexStepModel>` | `crates/waav-infer-core/src/model.rs:615` (dispatcher) | the verb — delegates to the inner `S2sModel` hook; `Tts`/`Stt` → `None` |
| `DuplexS2s` adapter + `DuplexS2sCodec` trait | `crates/waav-infer-core/src/s2s/duplex_s2s.rs` (new module) | the canonical bridge that makes ANY `DuplexStepModel` reachable through the verb |
| re-exports | `crates/waav-infer-core/src/s2s/mod.rs` | `pub use duplex_s2s::{DuplexS2s, DuplexS2sCodec}` |
| import | `model.rs:22` | `use waav_infer_runtime::{ArStepModel, DuplexStepModel}` |

### Design (why this shape)

Today the only `DuplexStepModel` impl is `CodecArDuplexModel` (`s2s/duplex_codec_ar.rs`) — a *bench backbone*
(real chatterbox codec-AR graphs driving the batched `step(&SlotBatch)` seam) with **no `LoadedModel`
surface**. The registry returns only `Stt`/`Tts`/`S2s`, and the two `S2s` arms (hibiki, `lfm2_audio_s2s`)
are **turn-based** (whole-utterance `s2s_turn` round-trip; not step-batchable). So the batched full-duplex
*step* seam — the read-while-emit `step(&SlotBatch)` the multiplexed duplex serve loop drives — had **no
path from a loaded model to a verb**.

The verb closes that, mirroring the existing `as_stepped` precedent exactly:
- `S2sModel` gains a default `as_duplex() -> None` hook (additive: hibiki + `Lfm2AudioS2s` inherit it
  unchanged — they stay turn-based, served via `s2s_turn`). Only `S2s` can be full-duplex, so `LoadedModel`
  dispatches `S2s(m) => m.as_duplex()` and `Tts/Stt => None`. The 23-arm config-arch registry is untouched
  (count assertion `== 23` still holds).
- `DuplexS2s` is the bridge a **real** native-duplex model plugs into: it wraps any
  `Box<dyn DuplexStepModel>` + a model-supplied `DuplexS2sCodec` (the only model-specific pieces: a
  user-side encoder `pcm → user-in codec frames`, and an output decoder `model-out deltas → reply PCM16`).
  - `as_duplex()` → `Some(&mut *self.duplex)` (the throughput path the serve loop drives directly).
  - `s2s_turn(channel, pcm)` drives the **same** step seam frame-by-frame over the turn's audio: encode →
    one `[B=1]` `SlotBatch` per frame → `step` (READ user-in while EMIT model-out) → collect deltas until a
    confident end-of-turn → decode to PCM16. Per-channel `MultiStreamSlot` + frame clock = F3 isolation.
  - `DuplexS2s::over_codec_ar(CodecArDuplexModel, codec, sr)` **statically proves the real
    `CodecArDuplexModel` plugs in** (no test double) — compile-checked that `CodecArDuplexModel:
    DuplexStepModel + Send`.

### What a real S2S model plugs into (the registry plug point — documented, not faked)

A future native-duplex engine arm in `engine.rs::load_torch_inprocess_model` becomes:
```rust
let duplex: CodecArDuplexModel = /* over the model's real codec-AR backbone */;
let codec:  Box<dyn DuplexS2sCodec> = /* the model's Mimi/DAC encode + decode */;
Ok(LoadedModel::S2s(Box::new(DuplexS2s::over_codec_ar(duplex, codec, 24_000))))
```
No fake registry arm was added (registry count is asserted == 23, and no full S2S model is loadable on this
box). The verb + dispatch + the compile-verified `DuplexS2s`/`over_codec_ar` path are wired against the
existing `DuplexStepModel` seam; the only remaining model-specific work for a live model is its two codec
hooks.

### Verification (GPU-free)
- `cargo build -p waav-infer-core` — green.
- `cargo build -p waav-infer-server --features torch` — green (engine.rs `S2sModel` consumers unchanged;
  hibiki/lfm2 inherit the default hook).
- `cargo clippy -p waav-infer-core` — clean; `-p waav-infer-server --features torch` — clean (the only
  warnings are pre-existing in `backend-torch/vibevoice.rs`, unrelated to this change).
- New lib tests (pure-logic, no GPU): `s2s::duplex_s2s::tests::duplex_s2s_surfaces_step_seam_and_drives_turn`
  and `..._eot_ends_turn_and_channels_isolated` — PASS. Extended
  `model::tests::s2s_seam_registered_and_object_safe` asserts a **turn-based** S2S model returns `None` from
  `as_duplex` (the additive default) — PASS.

---

## DELIVERABLE B — ring-flip readiness audit

### ⚠ CRITICAL FINDING — the on-disk state is FAR AHEAD of the task's stated baseline

The task baseline ("13 models opt-in; **only qwen3 default-on**") reflects the `b888edf` Wave-1 state. **It
is STALE.** On HEAD `3571b71` (after the torch-2.12 merge), reading `engine.rs::live_serve_green` shows
**10 of the 13 rollout models are ALREADY default-green** (`live_serve_green("WAAV_<M>_BATCHED", true)`),
not opt-in. Only **csm, misotts, s2_pro** remain opt-in among the 13 (plus **higgs** v3 and **pocket_tts**
outside the 13). So this is less a "flip plan" and more a **state reconciliation + a re-verification gate +
a plan for the remaining opt-in tail.**

`live_serve_green(env, default_green)`: `WAAV_<M>_BATCHED` set ⇒ that value (`0/off/false/no`→solo, else
ring); UNSET ⇒ `default_green`. Every model also ANDs `device.is_cuda()` (the device-resident-KV win is
CUDA-only; CPU/unsupported → unwrapped B=1 one-shot = byte-identical solo). qwen3 additionally rides on CPU
when its env is explicitly set.

### Current state table (the SOURCE OF TRUTH for this audit)

| # | Model | env var | engine.rs line | `default_green` NOW | sampling | RNG-isolation (D2) | codes-identical evidence |
|---|---|---|---|---|---|---|---|
| 1 | **qwen3_tts** | `WAAV_QWEN3_BATCHED` | 454 | **TRUE (default-on)** | — | n/a (pilot) | the proven pilot, 2.61×@B12, B16 clean shed |
| 2 | **dia2** | `WAAV_DIA2_BATCHED` | 381 | **TRUE** | sampled (CFG-axis) | `rng_base` ✓ | oracle GREEN both cells: CPU-f32 4 rows/6176 codes/max\|Δ\|=0; CUDA-bf16 4/5472/max\|Δ\|=0 |
| 3 | csm | `WAAV_CSM_BATCHED` | 580 | **FALSE (opt-in)** | dual-AR | (depth per-slot) | oracle GREEN: 4 rows/9568 codes/max\|Δ\|=0 |
| 4 | misotts | `WAAV_MISOTTS_BATCHED` | 624 | **FALSE (opt-in)** | dual-AR (8B) | (depth per-slot) | oracle GREEN: 4 rows/4128 codes/max\|Δ\|=0 (488s) |
| 5 | **dia** | `WAAV_DIA_BATCHED` | 667 | **TRUE** | GREEDY | D2-free by construction | codes-identical-to-solo VERIFIED (enc-dec CFG-axis, dia B36 golden) |
| 6 | **neutts** | `WAAV_NEUTTS_BATCHED` | 907 | **TRUE** | sampled (top-k/p) | `rng_base` ✓ + per-slot rep-pen seen-set | codes-identical-to-solo VERIFIED |
| 7 | s2_pro | `WAAV_S2PRO_BATCHED` | 497 | **FALSE (opt-in)** | GREEDY (RNG-free) | D2-free by construction | Fork-A1 partial; oracle cell green (slow Dual-AR) |
| 8 | **higgs_v2** | `WAAV_HIGGS_V2_BATCHED` | 866 | **TRUE** | sampled (delay-pattern) | `rng_base` ✓ | codes-identical-to-solo VERIFIED |
| 9 | **cosyvoice3** | `WAAV_COSYVOICE3_BATCHED` | 419 | **TRUE** | hybrid AR+flow | (no AR CFG; greedy AR) | codes-identical-to-solo VERIFIED |
| 10 | **voxtral_tts** | `WAAV_VOXTRAL_TTS_BATCHED` | 537 | **TRUE** | hybrid AR+flow | `rng_base` ✓ (D2-flow-only) | codes-identical-to-solo VERIFIED |
| 11 | **dots** | `WAAV_DOTS_BATCHED` | 796 | **TRUE** | hybrid (sampled-via-noise) | `rng_base` ✓ | codes-identical-to-solo VERIFIED; ⚠ 48 kHz BigVGAN AudioVAE P4-OOM transient (per-slot) |
| 12 | **indextts2** | `WAAV_INDEXTTS2_BATCHED` | 761 | **TRUE** | GREEDY mel-LM | D2-free by construction | codes-identical-to-solo VERIFIED; ⚠ BigVGAN CUDA-only, no CPU twin, P4-HIGH (per-slot) |
| 13 | **vibevoice** | `WAAV_VIBEVOICE_BATCHED` | 959 | **TRUE** | GREEDY AR backbone | per-slot DDPM head `rng_base` | tokens-identical-to-solo VERIFIED (dual-ring) |
| — | higgs (v3, 4B) | `WAAV_HIGGS_BATCHED` | 830 | **FALSE (opt-in)** | sampled | `rng_base` ✓ | **UNVERIFIED here — weights not on this box** |
| — | pocket_tts | `WAAV_POCKET_TTS_BATCHED` | 720 | **FALSE (opt-in)** | GREEDY (hybrid AR+flow) | D2-free | latent maxΔ=0 (per-slot flow head) |

(zonos2/irodori/omnivoice/viitorvoice/vibevoice_realtime have **no ring wrapper** — always B=1 one-shot,
not part of the batched-serve fleet.)

D1 (the no-wedge serve-shed) is backend-agnostic and already covers ALL ring models:
`codec_ar_admission.rs:455 serve_deadline()` → used by the mux loop at `codec_ar_batcher.rs:300`
(`StallTimeout`, slot+ticket freed). D2 (`rng_base`, content-keyed FNV-1a, slot-independent) is present in
`backend-torch/{dia2,neutts,higgs_v2,voxtral_tts,dots,higgs,vibevoice,...}.rs` for every sampled model;
greedy models are D2-free by construction.

---

### FLIP VERDICTS (do NOT apply now — the flips need live-GPU re-verification on the merged baseline)

#### GROUP 1 — GO / already default-on (10 models): NO engine.rs change; CONFIRM via re-verification
**Models:** qwen3_tts, dia2, dia, neutts, higgs_v2, cosyvoice3, voxtral_tts, dots, indextts2, vibevoice.

These satisfy the GO bar — a direct codes/tokens-identical-to-solo oracle (max|Δ|=0), the D1 serve-shed
(backend-agnostic), and D2 content-keyed RNG wherever sampled. **They are already flipped (`true`) — there
is NO code change to make.** The audit's only action here is the **re-verification gate** below: their
default-green flips were attested on the **pre-merge (torch-2.11)** baseline. The torch-2.12 + TRT-Throughput
merge changed the numeric baseline; the merge note attests the *solo goldens* are byte-identical, but the
*batched-ring* codes-identical-to-solo oracle is a **separate** test and must be RE-RUN per model on
`3571b71` before these ship (the deferred GPU phase). Two carry a standing per-slot risk to monitor (NOT a
codes-identity risk): **dots** (48 kHz BigVGAN AudioVAE, P4-OOM transient) and **indextts2** (BigVGAN
CUDA-only, no CPU bit-twin) — the per-slot vocoder VRAM transient must be inside the admission budget at the
served cohort width.

Per-model engine.rs change: **none** (line already reads `..., true)`). Escape hatch `WAAV_<M>_BATCHED=0`
retained.

#### GROUP 2 — STAY OPT-IN: deadline-safety (3 models) — `csm`, `misotts`, `s2_pro`
**Verdict: KEEP `false`. The blocker is NOT a verification gap** — all three are codes-identical-to-solo
GREEN (csm 9568/max|Δ|=0; misotts 4128/max|Δ|=0; s2_pro Fork-A1 partial-green, GREEDY/RNG-free). The blocker
is **deadline safety**: flipping `as_stepped()->Some` routes them onto the deadlined mux serve loop, whose
per-slot shed is a HARD total-wall-clock deadline (`now - admitted_at > serve_deadline`, default 30s — NOT
contention-aware, NOT per-frame-stall; `codec_ar_admission.rs:455` → `codec_ar_batcher.rs:300`). These are
the **slowest** fleet members (csm dual-AR; misotts 32L-8B + 31-step depformer, 488s oracle; s2_pro 36L
slow-AR + 9-step fast-AR + firefly codec, 50min-class) — a *legitimate* long synthesis past 30s would be
`StallTimeout` 503'd, a regression vs the B=1 one-shot.

- **Prerequisite to flip:** make the mux serve deadline **contention-aware / per-model-generous** (e.g. a
  per-frame-stall watchdog instead of a fixed total-wall-clock cap, or a per-model `serve_deadline`
  override). This is a `codec_ar_admission.rs`/`codec_ar_batcher.rs` change — once it lands, all three flip.
- **Exact engine.rs change once unblocked** (per model, the `false` → `true` flip):
  - csm L580: `live_serve_green("WAAV_CSM_BATCHED", false)` → `... , true)`
  - misotts L624: `live_serve_green("WAAV_MISOTTS_BATCHED", false)` → `... , true)`
  - s2_pro L497: `live_serve_green("WAAV_S2PRO_BATCHED", false)` → `... , true)`
- **Flip priority once unblocked (lowest risk × upside first):**
  1. **csm** — verified, smallest of the trio, but **depth-bound small upside** (only the 16L backbone
     batches; the 4L depth decoder + Mimi stay per-slot) → low net win; flip first because it's the safest.
  2. **s2_pro** — GREEDY (cleanest, D2-free), but the **slowest** model → highest deadline exposure; flip
     only after the deadline is comfortably contention-aware.
  3. **misotts** — 8B, 488s oracle → real batching upside at concurrency but the largest deadline exposure;
     flip last.

#### GROUP 3 — STAY OPT-IN: unverified-here (1 model) — `higgs` (v3, Qwen3-4B)
**Verdict: KEEP `false`. CANNOT flip — verification gap, not a deadline decision.** higgs-v3 weights are
**not on this box**, so its batched-ring codes-identical-to-solo oracle has never run. The wrapper is wired
(sampled, D2 `rng_base` present), but a flip requires the oracle to pass on real weights first.
- **Prerequisite:** stage higgs-v3 weights on a GPU box and run the codes-identical-to-solo oracle (CPU-f32
  + CUDA cells, max|Δ|=0).
- **Exact engine.rs change once verified:** L830 `live_serve_green("WAAV_HIGGS_BATCHED", false)` → `... , true)`.

#### GROUP 4 — STAY OPT-IN: hybrid per-slot flow, small AR lever (1 model) — `pocket_tts`
**Verdict: KEEP `false` (low priority).** Hybrid AR+flow with a per-slot `SimpleMLPAdaLN` flow head +
continuous-Mimi decode; only the small 6L Moshi backbone batches (the AR lever is small), and pocket_tts is
**CPU-first** (the unwrapped CPU path is the common case). The latent-trajectory maxΔ=0 bar is met, but the
upside is marginal. Flip is optional and last.
- **Exact engine.rs change if ever flipped:** L720 `live_serve_green("WAAV_POCKET_TTS_BATCHED", false)` → `... , true)`.

---

### THE RE-VERIFICATION GATE (the actual deferred-GPU work item)

Because HEAD is a **post-merge** baseline (torch-2.12 + TRT Throughput tier became the new default), the
single highest-value GPU action is **NOT new flips** — it is **re-attesting the 10 already-default-green
models' batched-ring codes-identical-to-solo oracle on `3571b71`**. The merge attests solo goldens are
byte-identical, but the ring oracle (batched == solo, max|Δ|=0) is a distinct gate and is what guards every
default-green flip. Run, per Group-1 model, the `*_batched` codes-identical oracle (CPU-f32 + CUDA cells)
under the Accuracy `PerfMode` (byte-identical default — never the lossy TRT Throughput tier for the identity
gate). Any model whose ring oracle no longer reads max|Δ|=0 on the merged baseline must be reverted to
`false` until reconciled. Only after that pass should the Group-2 deadline work + flips proceed.

### Summary

- **Already default-on (10):** qwen3, dia2, dia, neutts, higgs_v2, cosyvoice3, voxtral_tts, dots, indextts2,
  vibevoice — evidence-backed (codes-identical oracle + D1 + D2-where-sampled); **action = re-verify on the
  merged baseline, no code change.**
- **Stay opt-in (5):** csm / misotts / s2_pro (deadline-safety, blocked on a contention-aware serve
  deadline), higgs-v3 (unverified — weights absent), pocket_tts (marginal AR lever). Exact `false→true`
  per-model lines listed above; do not apply until the respective prerequisite lands + live-GPU re-verify.
- **Net:** the rollout is much further along than the task baseline implied; the gating, D1/D2 plumbing, and
  byte-identity oracles are all present and code-grounded.
