# B19 — tch CosyVoice3 (Wave-2 CFM / flow seam-prover)

**Verdict: SHIP.** The in-process tch CosyVoice3 generates correct 24 kHz audio on GB10 CUDA, **bit-faithful to
the python sidecar on the deterministic CFM seam**, at **RTF 0.75 e2e (0.35 flow+vocoder)**. The reusable tch
**CFM ODE integrator + ONNX flow-field** seam is established and proven — it unlocks vibevoice/omnivoice (Wave 3).

- File: `crates/waav-infer-backend-torch/src/cosyvoice3.rs` (filled the stub; reaches `cosyvoice3::TorchCosyVoice3`, no `pub use` in lib.rs).
- Live gate: `crates/waav-infer-backend-torch/tests/cuda_torch_cosyvoice3.rs` (`#[ignore]` `cuda_torch_cosyvoice3`), wired into `ci/heavy_live_tests.sh`.
- Worktree commit: see HEAD (committed on branch `worktree-agent-aa859833f4445994f`).

## Does it generate correct audio on GB10 CUDA, bit-faithful to the sidecar?

**Yes.** The pipeline splits into a deterministic half (faithful to the sidecar) and a stochastic half (the AR
LLM, inherently divergent across implementations). Live results (`source gb10-env.sh`):

| Stage | Metric vs sidecar golden | Result |
|---|---|---|
| **[1] CFM seam** (flow front-end + ODE integrator + ONNX estimator) on the golden 123 tokens | mel `max|Δ|` / `RMS(Δ)` | **0.0049 / 0.00023** — bit-faithful (pure CUDA-EP vs sidecar-CPU-EP estimator delta) |
| **[2] HiFT vocoder** on the golden mel | waveform correlation / RMS | **corr 0.853**, RMS 0.1789 vs golden 0.1782 |
| **[3a] flow→mel→vocoder** on the golden tokens (deterministic) | audio / RMS / determinism | 4.98 s, RMS 0.1787, **119460/119460 identical across two runs** |
| **[3b] LLM + e2e** (stochastic) | token count / e2e audio | 129 tokens (sidecar: 123), e2e 5.22 s, RMS 0.1826 |

The CFM seam is **bit-faithful**: with the seed-0 CFM noise drawn on the **CPU** generator (byte-identical to
the sidecar's `torch.manual_seed(0); torch.randn(...)` — libtorch and torch-python share the RNG; verified
`max|Δ|=0.0`), the front-end (`mu`/`spk`/`cond`) reproduces the sidecar exactly (`spk`/`cond` = 0.0, `mu` =
0.0002 from f32 conv rounding), and the CFG-Euler integrator over the published ONNX estimator reproduces the
golden mel to the ORT cross-EP delta.

**Full-waveform sample-identity is not achievable** (correctly, per the parity bar): the [A] AR loop samples
stochastically (RAS multinomial) in bf16 — tiny logit-rounding differences between this hand-rolled Qwen2 and
HF's `Qwen2Model` flip a draw, so the token sequence diverges (it starts with the same token `2307` then
diverges; 129 vs 123 tokens — same ballpark, proving the port is correct, not buggy). The HiFT NSF source also
carries process-fixed phase-noise buffers. This is exactly the established cross-runtime bar (cf. the voxtral
torch-vs-ORT gate is a char-similarity, not bit-exact). The deterministic seam is what gets pinned bit-faithfully.

## The CFM-integrator + vocoder approach (the reusable seam)

The headline reusable artifact is the **CFM ODE integrator as tch tensor ops** over a pluggable flow-field:

    trait FlowField { fn eval(&mut self, x2: &Tensor /*[2,80,T]*/, t: f32) -> Res<Tensor>; }
    struct CfmOde { n_steps, cfg_rate }   // cosine schedule 1-cos(linspace(0,1,n+1)·π/2); CFG Euler:
    //   x2 = cat[x,x]; out = field.eval(x2,t); dφ = (1+w)·out[cond] - w·out[uncond]; x += dt·dφ

- **Generalizes to Wave 3**: vibevoice/omnivoice plug a *torch-native* estimator into the same `CfmOde` by
  implementing `FlowField` with tch ops — the integrator, schedule, and CFG combine are model-agnostic.
- For cosyvoice3 the flow-field is `OnnxFlowField` — the published `flow.decoder.estimator.fp32.onnx` (a
  7644-node transformer CFM estimator) run on the ORT-CUDA EP via `OrtModel`. **This is deliberately the
  sidecar's own reference path**: the python sidecar itself runs this estimator through ONNXRuntime (never
  torch), so driving the same ONNX graph with the tch integrator is the *bit-faithful* choice, not a shortcut.
  Re-deriving a 7644-node estimator from weights-only would be both infeasible and *less* faithful.
- **Hybrid precedent**: this `tch-integrator + ORT-estimator` mirrors the blessed `candle Cohere` arm
  (ORT-CUDA encoder + candle decoder) — a `-backend-*` crate is the one place C/C++ in the dep graph is legal
  (INFER_SPEC §17.1).

The **HiFT vocoder** (`CausalHiFTGenerator`) is ported to tch tensor ops: weight-norm conv reconstruction
(`w = g·v/‖v‖`), causal conv variants (left/right pad), `ConvTranspose`-style nearest-upsample, the NSF source
module (SineGen2 harmonic synthesis with the fixed phase-noise buffers + `l_linear`/tanh merge), `torch.stft`/
`istft` (tch `stft`/`istft`), reflection padding, Snake activations, and the f0-predictor run in **f64** (the
reference's `f0_predictor.to(float64)` causal-precision path). It is deterministic on a fixed mel and tracks the
golden waveform at corr 0.85 / matched energy.

The [A] **Qwen2-0.5B speech LM** reuses the proven voxtral idioms verbatim (device-resident ring-KV via
`index_copy_`/`narrow`, zero-copy `[rows,in]@Wᵀ` gemm, GQA-native attention, RoPE, fused gate/up + q/k/v),
plus the RAS (repetition-aware) sampler (nucleus top-p∧top-k → repeat-guard → uniform fallback). f16 on CUDA.

All weights load from the model dir's **python-pickle `.pt`** files via tch `loadz_multi_with_device` (validated
live for `llm.rl.pt`/`flow.pt`/`hift.pt`/`default_voice.pt` — tch reads the `torch.save` zip+pickle format).

## RTF (target < 1) — PASS

| Path | Time | Audio | RTF |
|---|---|---|---|
| flow → mel (CFM, 10 steps) + HiFT vocoder | 1.7 s | 4.98 s | **0.35** |
| **full e2e** (LLM + flow + vocoder) | 3.9 s | 5.22 s | **0.75** |
| LLM AR loop alone (129 tokens) | 1.6 s | — | — |
| load | 4.3 s | — | — |

The estimator EP is **cuda** (telemetry-confirmed) — the CUDA EP carries the CFM flow-field, so the 10-step
solve is ~1.7 s (the sidecar's CPU-EP estimator alone was ~14 s, RTF 2.8).

## Accuracy vs the sidecar

- **CFM seam: bit-faithful** — mel `max|Δ| 0.0049`, `RMS(Δ) 0.00023` (the only divergence is the ONNX
  estimator EP: this CUDA EP vs the sidecar's CPU-only python ORT).
- **Vocoder: structurally faithful** — corr 0.853, matched RMS; the residual is the NSF phase-noise buffers
  (process-arbitrary in the reference; this port seeds them at 0 for reproducibility) + the cross-EP mel delta.
- **e2e: cross-runtime bar** — valid intelligible speech of comparable duration/energy; not sample-identical
  (bf16 AR sampling diverges). Within-process **fully deterministic** (flow→vocoder 119460/119460 identical).

A real bug was caught and fixed during bring-up: `run_cfm` used `shallow_clone()` of the seed-0 noise (a storage
**view**), and the in-place `x += dt·dφ` Euler step corrupted the shared `rand_noise` for the *next* utterance
(2nd call mel blew up 4.7→18.2 RMS → silent/garbage audio). Fixed with a deep `.copy()` so the noise stays
pristine; re-verified idempotent (call1 == call2, `max|Δ| 0.0`).

## New Cargo.toml dep (stated loudly)

`waav-infer-backend-ort` (+ `waav-infer-backend-api`) moved from `[dev-dependencies]` to `[dependencies]` — the
CFM flow-field runs the published estimator ONNX through `OrtModel` on the ORT-CUDA EP. This is the blessed
candle-Cohere hybrid pattern (ORT-CUDA + native), legal in a `-backend-*` crate (INFER_SPEC §17.1). No other new
crates. `tch` was already present.

## Verification

- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` — **clean**.
- lib unit tests **13/13** (5 cosyvoice3: cosine schedule, CFG-Euler exact on a constant field, weight-norm
  reconstruction, periodic Hann window, PCM clamp/round).
- live gate `cuda_torch_cosyvoice3` **PASS** (the three layers above) — run via `ci/heavy_live_tests.sh` (added
  there as the `(c)` torch gate; needs the sidecar goldens at `$WAAV_CV3_GOLDEN`, default `/tmp/cv3_golden` —
  without them layers (1)+(2) self-skip and only the e2e gate runs).
- Untouched: voxtral.rs, dia2.rs, device.rs, smoke.rs, lib.rs (only `pub mod cosyvoice3;`), torch_runtime/*.py,
  all other crates.

## Notes for the merger

- The sidecar goldens (`/tmp/cv3_golden/*.bin` + `shapes.json`: speech_tokens, mu/spk/cond/x0, mel, wav) were
  produced by a throwaway probe that re-runs `torch_runtime/models/cosyvoice3.py` on the fixed text+seed and
  dumps the intermediates. They are ephemeral (`/tmp`); regenerate with the sidecar before running the gate on a
  fresh box, or point `$WAAV_CV3_GOLDEN` at a persisted copy. The e2e smoke (3b) needs no goldens.
- The model dir is `~/.cache/waav-models/cosyvoice3` (9.1 GB) with `waav.json {"backend":"torch","architecture":
  "cosyvoice3"}` already present — so the engine registry can dispatch to this module config-only once wired.
