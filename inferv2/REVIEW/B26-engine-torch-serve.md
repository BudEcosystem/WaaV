# B26 — Wire the in-process Torch backend into the engine (it now SERVES its models)

**Status: DONE + live-proven on GB10 CUDA, byte-identical.**
Worktree: `worktree-agent-a38b9c2072362c602` (fast-forwarded onto `waav-infer-v2-build` `d6b80b5`).
**Commit SHA: `aa73ac635a29e615029fbf9fa499f38542a64e98`.**

## TL;DR

Does the engine now serve a tch model on CUDA, byte-identical to standalone? **YES.**

`engine::load_model_at`, on a `torch-inprocess` `waav.json` manifest, constructs the in-process
`waav_infer_backend_torch::TorchVoxtral` and serves it as `LoadedModel::Stt`. The engine-served transcript
is **byte-for-byte equal** to the standalone `TorchVoxtral`:

    standalone: "Hello world! This is W.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L."
    engine    : "Hello world! This is W.A.V. Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L."
    PASS: engine-served transcript is BYTE-IDENTICAL to standalone TorchVoxtral.
    test result: ok. 1 passed; ... finished in 48.23s

The engine seam is pure plumbing — no numeric transform (no device/dtype/load-order change), so the LAW
holds: **served output == standalone, byte-identical.** The same test also asserts
`tch::Cuda::is_available()==true` *from inside the server test binary* (via
`waav_infer_backend_torch::smoke::cuda_is_available()`), proving the B16 downstream-binary link-arg gap is
closed at runtime.

## Worktree note (why a fast-forward was needed)

The worktree was branched from a stale commit (`152caab`, the M1 server) that PREDATES the entire
`waav-infer-backend-torch` crate — it had only 6 crates and no Torch backend to wire in. I fast-forwarded
the worktree branch to `waav-infer-v2-build` (`d6b80b5`, a strict descendant, no divergent local commits) so
the backend-torch crate + its tested models (voxtral/cohere/dia2/cosyvoice3) were present. The parallel
worktree `agent-acaf5056` (the dia2.rs owner) is already at `d6b80b5`; I did not touch any backend-torch
`.rs`, the scheduler, or the runtime.

## What was delivered

### 1. Feature-gate (DEFAULT-OFF) — `crates/waav-infer-server/Cargo.toml`

    [features]
    default = []
    torch = ["dep:waav-infer-backend-torch"]

    [dependencies]
    waav-infer-backend-torch = { workspace = true, optional = true }

An ONNX-only deployment (the default) pulls **no libtorch** into the dep graph. The Torch backend +
dispatch only exist under `--features torch`, behind `#[cfg(feature = "torch")]`.

### 2. The bin CUDA-link arg (the B16 gap) — `crates/waav-infer-server/build.rs` (NEW, 96 lines)

`backend-torch`'s own `build.rs` emits `-Wl,--no-as-needed -ltorch_cuda -lc10_cuda`, but
`cargo:rustc-link-arg` is **scoped to the emitting crate** — it does NOT propagate to a downstream binary.
So the `waav-infer` server bin (which links the Torch backend under `--features torch`) re-asserts the SAME
fragment from its OWN `build.rs`. It is **conditional**: emits nothing unless `CARGO_FEATURE_TORCH` is set
(so the ONNX-only build links zero extra), and only when the active PyTorch ships `libtorch_cuda.so` +
`libc10_cuda.so`.

**Proof (link metadata).** Feature-built bin `DT_NEEDED`:

    NEEDED  libtorch_cpu.so   NEEDED  libc10.so
    NEEDED  libtorch_cuda.so  NEEDED  libc10_cuda.so   <- forced by the new build.rs
    RUNPATH /home/bud/.local/lib/python3.12/site-packages/torch/lib

Without the build.rs, `libtorch_cuda.so`/`libc10_cuda.so` would be dropped (`--as-needed`) ->
`Cuda::is_available()==false`. **Proof (runtime).** The live test's first assertion
(`cuda_is_available()==true`) passed from inside the server test binary.

### 3. The dispatch — `crates/waav-infer-server/src/engine.rs` (+106 lines, all `#[cfg(feature="torch")]`)

`load_model_at` gained a new arm BEFORE the sidecar/ORT arms:

    #[cfg(feature = "torch")]
    if let Some(rt) = read_torch_inprocess_runtime(dir) {
        return load_torch_inprocess_model(dir, &rt, ep);
    }

- `read_torch_inprocess_runtime(dir)` reads `waav.json {"runtime":{"backend":"torch-inprocess",
  "architecture":"voxtral_realtime|cohere|dia2|cosyvoice3","device":"cuda"}}`.
- `ep_to_torch_device(ep)` maps `EpRequest -> DeviceRequest` (`Cpu->Cpu`, explicit-CUDA->`Cuda(0)`,
  `Auto->Auto`); no `tch` type crosses the seam.
- `load_torch_inprocess_model` constructs the matching backend type via its `load(dir, TorchDevice)` ctor
  and wraps it: `voxtral_realtime`/`cohere` -> `LoadedModel::Stt(Box::new(..))` (impl `SttModel`),
  `dia2`/`cosyvoice3` -> `LoadedModel::Tts(Box::new(..))` (impl `TtsModel`). Unknown arch -> typed error.

Additive: a new Wave-3 tch model is auto-served by adding a manifest (and, for a brand-new type, one arm).

**The 4 `waav.json` manifests** are committed as fixtures:
`crates/waav-infer-server/tests/fixtures/torch_inprocess/{voxtral_realtime,cohere,dia2,cosyvoice3}.waav.json`.
Kept as fixtures rather than written into the shared `~/.cache/waav-models/{dia2-2b,cosyvoice3}` dirs, which
already carry the SIDECAR `{"backend":"torch"}` manifests owned by the parallel effort — not clobbered.

### 4. Live test — `crates/waav-infer-server/tests/torch_inprocess_live.rs` (NEW, `#[ignore]`, GB10-CUDA)

`engine_serves_inprocess_torch_voxtral_byte_identical_to_standalone`: (1) builds a fixture model dir
(symlinks the real voxtral-realtime-hf weights + drops the `torch-inprocess` manifest — no cache clobber),
(2) `load_model_at(fixture, Cuda)` -> `LoadedModel::Stt` -> `transcribe(kokoro_m1_sample.wav)`, (3) asserts
`engine_txt == standalone TorchVoxtral.transcribe(..)` **byte-identical**, (4) asserts CUDA is live from the
server bin. Added to `ci/heavy_live_tests.sh` as gate (f) (process-isolated; `--features torch` rides the
unquoted-expanded target field).

## Build + verification matrix (all green, GB10, `gb10-env.sh`)

| Check | Result |
|---|---|
| `cargo build -p waav-infer-server` (NO feature) | OK, 21.82s |
| no-feature dep graph has `tch`/`torch-sys`/`backend-torch` | NONE (libtorch not forced) |
| no-feature bin `DT_NEEDED` has libtorch | NONE |
| `cargo build -p waav-infer-server --features torch` | OK, 27.79s (tch+torch-sys+backend-torch compiled) |
| feature bin `DT_NEEDED` has `libtorch_cuda.so`+`libc10_cuda.so` | YES (B16 link-arg) |
| `clippy -p waav-infer-server --features torch --all-targets -- -D warnings` | clean |
| `clippy -p waav-infer-server -- -D warnings` (no feature, incl. `--all-targets`) | clean |
| `cargo test -p waav-infer-server --lib` (no feature) | 61 passed |
| `cargo test -p waav-infer-server --features torch --lib` | 61 passed |
| **live: engine serves in-process voxtral byte-identical** | **PASS (48.23s, byte-identical)** |

## No-feature ONNX-only build proof (the critical one)

    $ cargo tree -p waav-infer-server -e normal | grep -wiE 'tch|torch-sys|waav-infer-backend-torch|libtorch'
    NONE - neither tch, torch-sys, nor waav-infer-backend-torch in the ONNX-only dep graph (PROOF)
    $ objdump -p target/debug/waav-infer | grep -iE 'NEEDED.*(torch|c10)'
    (no output) - no libtorch in the ONNX-only bin's DT_NEEDED

An ONNX-only deployment builds and links with zero libtorch dependency. libtorch is NOT forced on every
build.

## Scope honored

Touched ONLY: `crates/waav-infer-server/{Cargo.toml, build.rs (new), src/engine.rs,
tests/torch_inprocess_live.rs (new), tests/fixtures/torch_inprocess/*.waav.json}` + `ci/heavy_live_tests.sh`
+ `Cargo.lock` (mechanical, from the new optional dep). Did NOT touch any backend-torch `.rs` model file,
the scheduler, the runtime, or the parallel effort's sidecar manifests.
