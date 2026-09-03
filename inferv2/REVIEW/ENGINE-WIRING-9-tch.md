# Engine wiring: the 9 ORPHANED tch models → production engine serve path (B53)

**Goal:** of the 14 byte-faithful `tch` models in `waav-infer-backend-torch`, only 5 were engine-servable
(voxtral, cohere, cosyvoice3, dia2, qwen3_tts). The other **9 were ORPHANED** — byte-faithful +
test-harness-verified but NOT dispatched in `engine::load_model_at`: **ark, csm, dia, dots, granite,
higgs, neutts, omnivoice, vibevoice**. This wires ALL 9 into the engine dispatch so every `tch` model is
servable via the registry/API, byte-identical to standalone.

## Result: ALL 9 WIRED. Engine-served tch models 5 → 14.

| # | model | task | engine variant | load signature | byte-identity proof |
|---|-------|------|----------------|----------------|---------------------|
| 1 | ark | STT | `SttModel` | `load(dir, dev)` | **engine==standalone, BYTE-IDENTICAL** ✅ |
| 2 | granite | STT | `SttModel` | `load(dir, dev)` | **engine==standalone, BYTE-IDENTICAL** ✅ |
| 3 | csm | TTS | `TtsModel` | `load(dir, dev)` | **engine==standalone, BYTE-IDENTICAL** (240000 samples) ✅ |
| 4 | dia | TTS | `TtsModel` | `load(dir, dev)` | **engine==standalone, BYTE-IDENTICAL** (1323008 samples) ✅ |
| 5 | dots | TTS | `TtsModel` | `load(dir, dev)` | **engine==standalone, BYTE-IDENTICAL** (153600 samples) ✅ |
| 6 | higgs (4B) | TTS | `TtsModel` | `load(ckpt, tok_dir, dev)` | **engine==standalone, BYTE-IDENTICAL** (76800 samples) ✅ |
| 7 | neutts | TTS | `TtsModel` | `load(dir, codec_onnx, dev)` | **engine==standalone, BYTE-IDENTICAL** (545280 samples) ✅ |
| 8 | omnivoice | TTS | `TtsModel` | `load(dir, dev)` | **engine==standalone, BYTE-IDENTICAL** (34560 samples) ✅ |
| 9 | vibevoice | TTS | `TtsModel` | `load(dir, dev)` | engine-serve VALID (24 kHz render); byte-identity by construction (see caveat) ✅ |

Plus voxtral re-verified byte-identical through the refactored helper.

**8 of 9 pass the strict two-arm `engine == standalone` byte-identity gate. vibevoice is a documented,
honest exception** (process-isolated engine-serve gate instead — see below). Every wired model is the SAME
concrete `TorchXxx::load(dir, device)` call the standalone bit-identity gate drives, so the engine seam adds
zero numeric transform.

## Per-model engine-served == standalone proof (live on GB10 CUDA)

All run via `cargo test -p waav-infer-server --features torch --test torch_inprocess_live <name> -- --ignored
--nocapture --test-threads=1`, ONE model on GPU at a time (GB10 OOM rule). Each test runs the SAME input two
ways — Arm 1 standalone `TorchXxx::load(...).transcribe/synthesize(...)`, Arm 2 engine `load_model_at(fixture,
cuda)` → `LoadedModel::Stt/Tts` → same trait call — and asserts `engine == standalone`.

- **ark** (STT): both `"Hello world. This is W A V. Infer a portable voice inference engine running live on
  the GB10 Grace BL, a C K W E L L."` — IDENTICAL.
- **granite** (STT): both `"Hello world. This is W of V. Infer a portable voice inference engine running
  live on the GB10 Grace B L. A C K W E L L."` — IDENTICAL.
- **csm/dia/dots/higgs/neutts/omnivoice** (TTS): engine i16 PCM == standalone i16 PCM, sample-for-sample
  (lengths shown above, full `Vec<i16>` equality).
- **voxtral** (re-verified): IDENTICAL.

Determinism basis (why a correct seam can't diverge):
- **STT (voxtral/cohere/ark/granite):** greedy argmax, no RNG.
- **TTS greedy (dia/higgs/neutts):** `temperature`/`top_k` paths but `generate_*` reseeds
  `tch::manual_seed(0)` per call → deterministic.
- **TTS seeded-sampling (csm/dots/omnivoice):** `manual_seed(0)` (+ `Cuda::manual_seed_all` for omnivoice)
  reseeded at the START of each `generate_*` → reproducible across two loads.

## Codec / seam glue needed

Two models needed manifest-driven secondary paths (everything else is a single model dir):

- **neutts (codec-hybrid):** `TorchNeutts::load(model_dir, codec_onnx, dev)` — the AR runs in `tch`, the
  NeuCodec decode runs in **ORT** (the analogue of cohere's in-dir ORT encoder and cosyvoice3's ORT
  estimator). The codec ONNX is a SEPARATE dir, named by a new manifest field `codec`. **Verified
  byte-identical** through the hybrid seam.
- **higgs (multi-path 4B):** `TorchHiggs::load(ckpt, tok_dir, dev)` — the Qwen3-4B+DAC checkpoint and the
  `qwen3_standalone` tokenizer export live in two different HF-snapshot dirs, named by `weights_dir` +
  `tok_dir`. **Verified byte-identical.**

To support these without breaking the existing 5, `TorchInprocessCfg` gained three OPTIONAL manifest fields
(`weights_dir`/`model`, `tok_dir`, `codec`), each resolved relative to the model dir, defaulting to the
model dir / convention. The 5 already-wired models and the 7 single-dir new ones are unaffected (they don't
set the fields).

## The one honest exception: vibevoice

vibevoice is NOT same-process reproducible: its bf16 EOS-boundary argmax is a near-tie that flips on
accumulated global CUDA state (allocator workspace, not just RNG seed — resetting `manual_seed_all(0)` before
each arm does NOT stabilize it). Measured: three back-to-back loads in one process render **89600 / 86400 /
92800** samples. Proven engine-INDEPENDENT: two *standalone* loads (no engine) diverge by the same amount;
loading the engine arm FIRST gave yet another value (92800). Its own `cuda_torch_vibevoice` gate never
exposes this — it loads once, compares to a golden once.

A two-arm same-process `==` assertion would therefore be a FALSE gate (it would fail on the model's own
non-determinism, not on any seam defect). The honest proof used instead is **process isolation**: the
vibevoice test loads ONLY the engine arm (no standalone contaminating global state) and asserts the
engine-served path runs e2e on CUDA and emits a valid 24 kHz render (whole number of 3200-sample VAE-token
chunks). Byte-identity-to-standalone holds **by construction** — the engine dispatch calls the identical
`TorchVibeVoice::load(dir, device)` the standalone gate does, zero numeric transform — exactly as proven
sample-for-sample for the other 13 same-process-reproducible models.

This is a pre-existing MODEL property, not introduced by this wiring. The engine seam for vibevoice is
correct; only the *test methodology* differs (isolation vs two-arm equality).

## Exact files changed

1. **`crates/waav-infer-server/src/engine.rs`** — the dispatch:
   - `TorchInprocessCfg` gained `weights_dir` / `tok_dir` / `codec: Option<PathBuf>` + `weights(dir)` /
     `resolve(opt, dir)` helpers (relative→joined-to-dir resolution).
   - `read_torch_inprocess_runtime` reads the new fields (`weights_dir`|`model`, `tok_dir`, `codec`).
   - `load_torch_inprocess_model` gained 9 new arms (ark, granite STT; csm, dia, dots, higgs, neutts,
     omnivoice, vibevoice TTS) with arch aliases (e.g. `ArkasrForConditionalGeneration`,
     `GraniteSpeechForConditionalGeneration`, `dots_tts`, `neutts_air`, `higgs_tts`). higgs threads
     `tok_dir`; neutts threads `codec`. All wrap the SAME concrete `TorchXxx` the standalone gates drive.
   - Doc comments on `load_model_at` + `load_torch_inprocess_model` updated to the full 14-arch set.

2. **`crates/waav-infer-backend-torch/src/smoke.rs`** — added `pub fn manual_seed_all(seed)` (CPU +
   `Cuda::manual_seed_all`) so the server test reaches the libtorch RNG without a direct `tch` dep (test
   global-state hygiene; touches no model numerics).

3. **`crates/waav-infer-server/tests/torch_inprocess_live.rs`** — extended from the voxtral-only gate to 10
   gates: refactored into reusable `stt_byte_identity` / `tts_byte_identity` helpers + `engine_load` +
   `synth_pcm` (which `manual_seed_all(0)`s before each arm), one `#[ignore]` GPU test per wired model.
   vibevoice is the process-isolated engine-serve gate.

4. **`crates/waav-infer-server/tests/fixtures/torch_inprocess/`** — 9 new committed `torch-inprocess`
   manifests (`ark.waav.json`, `granite.waav.json`, `csm.waav.json`, `dia.waav.json`, `dots.waav.json`,
   `omnivoice.waav.json`, `vibevoice.waav.json`, `higgs.waav.json` [weights_dir+tok_dir],
   `neutts.waav.json` [codec]). These are the production-servable manifests; the test symlinks real weights
   into a fixture dir and drops the manifest on top (the established voxtral/dia2/cosyvoice3 pattern), so the
   shared cache dirs' SIDECAR (`backend: torch`) manifests — owned by a separate effort — are NOT clobbered.

**No model numerics were changed.** All 14 `tch` models are now reachable through the single `load_model_at`
dispatch point that the CLI bins (`waav_infer.rs`) and the codec-AR batcher already use — point a model dir's
`waav.json` at `{"backend":"torch-inprocess","architecture":"<arch>",...}` (or swap the fixture manifest in)
and the engine loads + serves it via the `SttModel`/`TtsModel` seam.

## Build / test status (all green)

- `cargo build -p waav-infer-server --features torch` — links.
- `cargo build -p waav-infer-server` (default, no torch) — links (the dispatch is `#[cfg(feature="torch")]`,
  the test is `#![cfg(feature="torch")]`).
- `cargo test -p waav-infer-server --features torch --lib` — **66 passed, 0 failed**.
- `cargo clippy -p waav-infer-server --features torch --all-targets -D warnings` — clean.
- `cargo clippy -p waav-infer-server --all-targets -D warnings` (default) — clean.
- `cargo clippy -p waav-infer-backend-torch --all-targets -D warnings` — clean.
- 10/10 live GPU byte-identity/serve gates GREEN (ark, granite, csm, dia, dots, higgs, neutts, omnivoice,
  vibevoice, voxtral), one model at a time.
