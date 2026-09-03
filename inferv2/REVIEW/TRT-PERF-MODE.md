# TRT-PERF-MODE — TensorRT as a first-class PERF-MODE (not per-model env knobs)

> **The change:** TensorRT acceleration is now selected by an engine-wide **`PerfMode`** that the
> `AccelMapper` consults — `PerfMode::Accuracy` (the **default**) keeps the eager **byte-identical** path,
> `PerfMode::Throughput` auto-selects the staged TensorRT engine for the NOT-graphable AR models. The old
> per-model `WAAV_NEUTTS_TRT=1` / `WAAV_DIA_TRT=1` / `WAAV_HIGGS_TRT=1` knobs are **kept as per-model
> overrides**, but they are no longer the primary lever.

The hard invariant: **DEFAULT = Accuracy = byte-identical** — zero behavior change, every existing
byte-identity gate (neutts 0/96, dia2 544/544, csm) stays green. No accuracy degradation by default.

---

## 1. What landed

### 1a. A first-class perf-mode selector — `PerfMode`
`crates/waav-infer-backend-api/src/lib.rs` — new public `PerfMode { Accuracy (default), Throughput }`:
- `PerfMode::default()` == `Accuracy` (the byte-identical eager path).
- `PerfMode::from_env()` reads the **`WAAV_PERF_MODE`** env mirror (`throughput`/`fast`/`perf` ⇒ Throughput;
  unset / empty / `accuracy` / unknown ⇒ `Accuracy`). Case-insensitive, trimmed.
- `is_throughput()`, `label()` helpers for the gates + honest logs.

Mirrored as a first-class config field on the server: `EngineConfig.perf_mode`
(`crates/waav-infer-server/src/engine.rs`), defaulted from `PerfMode::from_env()` so a server reads the
**same** selector the torch backend reads at model-load time (one source of truth).

### 1b. `AccelMapper` auto-select — `select_perf(perf_mode, model, dev, staged)`
`crates/waav-infer-backend-api/src/lib.rs` — the perf-mode-aware selector on `AccelMapper` (replaces the
per-model env-knob decision). The routing law:

| perf_mode | model | worker | staged engine | → selected |
|---|---|---|---|---|
| **Accuracy** (default) | any | any | any | **Eager** (byte-identical) |
| Throughput | **graphable** (`dia2`/`csm`/`omnivoice`/`dots`) | any | any | **Eager** (keeps its byte-identical CUDA graph) |
| Throughput | NOT-graphable (`neutts`/`dia`/`higgs`) | NVIDIA in TRT band | **yes** | **TorchTensorRt** (~1.65× AR decode) |
| Throughput | NOT-graphable | NVIDIA | **no** | Eager (honest fallback — no engine to load) |
| Throughput | NOT-graphable | non-NVIDIA | any | Eager (TensorRT is NVIDIA-only) |

It consults the SAME priority-ordered `is_compatible` registry as `select` (device-vendor routing is one
source of truth — NVIDIA→TRT, AMD→MIGraphX, …); `select_perf` only adds the perf-mode + graphable + staged
gates **on top**. A new `ModelSpec.graphable` field (`with_graphable(true)` for dia2/csm/omnivoice/dots)
carries the "I own a byte-identical CUDA-graph lever — never route me to lossy TRT" fact, so the throughput
selector deliberately leaves graphable models on the accuracy-preserving path.

**Why graphable models are excluded (correctness, not omission):** the graphable models
(`dia2`/`csm`/`omnivoice`/`dots`) have **no** `maybe_load_trt` path at all (grep-verified) — their throughput
lever IS the byte-identical CUDA graph. `select_perf` encodes that: even with a staged engine on NVIDIA in
Throughput, a graphable model resolves to Eager.

### 1c. Per-model wiring — the env knob became an override
`crates/waav-infer-backend-torch/src/{neutts,dia,higgs}.rs` `maybe_load_trt` (under `cfg(accel_tensorrt)`)
now:
1. resolves `perf_mode = WAAV_<MODEL>_TRT==1 ? Throughput : PerfMode::from_env()` — the per-model knob is a
   per-model **override** that forces Throughput for that one model; otherwise the engine-wide
   `WAAV_PERF_MODE` decides;
2. resolves the staged `.ts` path and checks `staged = ts.is_file()`;
3. routes through `mapper.select_perf(perf_mode, &spec, &dev_caps, staged)` (the spec carries
   `with_graphable(false)` — these are the NOT-graphable AR models);
4. loads the engine iff the selector returns `torch-tensorrt`; else eager.

### 1d. Honest labeling
- `TorchNeutts::trt_precision()` (new) surfaces the selected TRT precision (`fp16` accuracy-preserving |
  `int8`/`fp8`/`nvfp4` lossy throughput levers), `None` on the eager path — for the served model's
  metadata/logs.
- The load log line now reads `neutts TRT [perf-mode=throughput]: loaded Fp16 engine … — AR decode
  accelerated`, so a throughput deployment surfaces both `trt_active` and the precision (the accuracy trade).
- `trt_active()` (pre-existing on all three models) reports whether the lossy path is live.

**The accuracy trade, stated honestly (grounded in B49):** `PerfMode::Throughput` is the FP16/int8
throughput-vs-accuracy trade — the per-step backbone is accuracy-preserving (**hidden corr = 0.999964**,
rel max|Δ| = 0.55% vs eager fp16), but the **full AR greedy sequence forks** (a ~15-code agreeing prefix,
then a different valid utterance). `PerfMode::Accuracy` (the default) is **byte-identical**.

---

## 2. The routing gate (unit, no silicon) — `AccelMapper::select_perf`

`crates/waav-infer-backend-api/src/lib.rs` (tests), all GPU-free on synthetic `DeviceCaps`:
- `perf_mode_parses_and_defaults_to_accuracy` — `from_str_value`/`from_env` default = Accuracy; throughput
  parsing.
- `select_perf_routes_throughput_nvidia_staged_to_tensorrt` — the headline:
  - Accuracy (default) + staged + NVIDIA ⇒ **Eager** (the invariant: no degradation by default);
  - Throughput + NVIDIA (GB10 + A100) + staged ⇒ **torch-tensorrt**;
  - Throughput + NVIDIA but **no** staged engine ⇒ Eager;
  - Throughput on AMD/Intel/Apple/CPU ⇒ Eager (TensorRT NVIDIA-only).
- `select_perf_never_routes_graphable_models_to_tensorrt` — dia2/csm/omnivoice/dots stay **Eager** in BOTH
  modes even with a staged engine on NVIDIA (byte-identical CUDA-graph lever kept).

**Result:** `cargo test -p waav-infer-backend-api --lib` → **79 passed, 0 failed** (was 76; +3 new gates).
Pre-existing AccelMapper routing gates (`mapper_picks_tensorrt_on_nvidia_gb10`,
`mapper_routes_each_vendor_to_its_accelerator`, …) all still green.

---

## 3. Live gate (GB10, accel_tensorrt) — `WAAV_PERF_MODE` Accuracy vs Throughput

New live test `crates/waav-infer-backend-torch/tests/cuda_torch_neutts_trt.rs`
`neutts_perf_mode_accuracy_is_eager_byteid_throughput_is_trt` — drives the FIRST-CLASS selector (NOT the
per-model knob), same model dir + same prompt ids:
- **(a) perf_mode=Accuracy (default, `WAAV_PERF_MODE` unset):** neutts loads EAGER, `trt_active() == false`,
  and the greedy codes are asserted **byte-identical** to `greedy_codes_eager` (no degradation by default).
- **(b) perf_mode=Throughput (`WAAV_PERF_MODE=throughput`):** the SAME model loads the TRT engine,
  `trt_active() == true`, `trt_precision()` is `Some(...)` (honest labeling), and the codes share a real
  agreeing greedy prefix with eager (TRT fp16 forks after — expected).

The pre-existing override gates (`neutts_trt_e2e_accuracy_and_rtf` via `WAAV_NEUTTS_TRT=1`,
`cuda_torch_dia_trt`, `cuda_torch_higgs_trt`, `cuda_torch_neutts_trt_lowp`) are unchanged and continue to
work through the override branch (the per-model knob now maps to `PerfMode::Throughput`).

> **LIVE-GATE STATUS — RAN GREEN on GB10 (sm_121), 2026-06-24** (`RUSTFLAGS=--cfg accel_tensorrt`, the §5
> venv recipe, `--test-threads=1`). Verbatim:
> ```
> test neutts_perf_mode_accuracy_is_eager_byteid_throughput_is_trt ...
>   perf_mode=accuracy (default): trt_active = false
>   neutts TRT [perf-mode=throughput]: loaded Fp16 engine …/trt/backbone_fp16.ts (no-Python CModule::load)
>   perf_mode=throughput: trt_active = true, trt_precision = Some(Fp16)
>   perf_mode comparison: Accuracy codes=221 (==eager), Throughput codes=256, agreeing prefix=15
>   ok
> test neutts_trt_e2e_accuracy_and_rtf ... loaded neutts (CUDA); trt_active = true
>   B49 BACKBONE accuracy (1 step): hidden corr = 0.999964, max|Δ| = 0.0781, rel = 0.5482%
>   B49 RTF: eager 0.636 | TRT 0.344 | per-step speedup ~1.85x
>   ok
> test result: ok. 2 passed; 0 failed
> ```
> - **(a) Accuracy (default):** `trt_active=false`, eager, and `Accuracy codes == eager` **byte-identical**.
> - **(b) Throughput:** the SAME model `trt_active=true`, `trt_precision=Some(Fp16)`, ~1.85× per-step,
>   agreeing prefix 15 (TRT fp16 forks after — expected). The label `[perf-mode=throughput]` is surfaced.
> - The legacy `WAAV_NEUTTS_TRT=1` override gate (`neutts_trt_e2e_accuracy_and_rtf`) still green (corr
>   0.999964, RTF 0.636→0.344).

---

## 4. Files changed (the flag is shared)

- `crates/waav-infer-backend-api/src/lib.rs` — `PerfMode` enum + `from_env`/`from_str_value`/`is_throughput`/
  `label`; `ModelSpec.graphable` + `with_graphable`; `AccelMapper::select_perf`; 3 new unit gates.
  (THE SHARED FLAG lives here — reachable by both the server `EngineConfig` and the torch backend.)
- `crates/waav-infer-server/src/engine.rs` — `EngineConfig.perf_mode` (mirrors `WAAV_PERF_MODE`).
- `crates/waav-infer-backend-torch/src/neutts.rs` — `maybe_load_trt` perf-mode routing; stored
  `trt_precision` + accessor (honest labeling).
- `crates/waav-infer-backend-torch/src/dia.rs` — `maybe_load_trt` perf-mode routing.
- `crates/waav-infer-backend-torch/src/higgs.rs` — `maybe_load_trt` perf-mode routing.
- `crates/waav-infer-backend-torch/tests/cuda_torch_neutts_trt.rs` — the new perf-mode live gate.
- `crates/waav-infer-server/src/bin/waav_infer.rs`, `.../tests/perf_bench.rs`, `engine.rs` (test) —
  `..EngineConfig::default()` spread on the fully-enumerated literals (the new field).

---

## 5. The live-gate recipe (the venv path)

```
source gb10-env.sh
export WAAV_TORCHTRT_PYTHON=/tmp/trt_e2e_venv/bin/python
export WAAV_TORCHTRT_LIB=/tmp/trt_e2e_venv/lib/python3.12/site-packages/torch_tensorrt/lib
export WAAV_TENSORRT_LIB=/tmp/trt_e2e_venv/lib/python3.12/site-packages/tensorrt_libs
export LD_LIBRARY_PATH="$WAAV_TORCHTRT_LIB:$WAAV_TENSORRT_LIB:$LD_LIBRARY_PATH"
RUSTFLAGS="--cfg accel_tensorrt" cargo test -p waav-infer-backend-torch --features cuda \
  --test cuda_torch_neutts_trt -- --ignored --nocapture --test-threads=1
```

---

## 6. LAW compliance

- **Perf-mode wired:** `EngineConfig.perf_mode` + `WAAV_PERF_MODE` env mirror; default = Accuracy = byte-id.
- **select() routing:** `AccelMapper::select_perf` — Throughput+NVIDIA+staged+not-graphable → TorchTensorRt;
  Accuracy → Eager; graphable → Eager (CUDA-graph). Unit-gated (§2), 79/0 in backend-api.
- **No default accuracy degradation:** Accuracy is the default; the eager byte-identity path is unchanged
  (no shared numeric path touched — the change is additive selection + a load-path gate).
- **Honest labeling:** Throughput is documented FP16/int8 (per-step corr 0.999964, full-gen forks);
  `trt_active` + `trt_precision` surfaced.
- **Tests/clippy (recorded):**
  - `cargo test --workspace` → **exit 0, 0 failed** (backend-api lib 79/0 incl. the 3 new perf-mode gates).
  - `cargo clippy --workspace --all-targets -- -D warnings` → **clean** (the torch crate is a workspace
    member with `default=["cuda"]`, so this covers it).
  - `RUSTFLAGS="--cfg accel_tensorrt" cargo clippy -p waav-infer-backend-torch --features cuda --all-targets
    -- -D warnings` → **clean** (covers the `accel_tensorrt`-gated `maybe_load_trt` + `trt_precision` edits).
  - `cargo clippy -p waav-infer-server --features torch --all-targets -- -D warnings` → **clean** (the
    `EngineConfig.perf_mode` field + the `..EngineConfig::default()` spreads).
  - `cargo test -p waav-infer-backend-torch --features cuda --lib` → **206 passed, 0 failed** (the eager
    byte-identity doubles for the touched models — no regression).
  - **Live (GB10, accel_tensorrt):** the perf-mode gate + the override gate → **2 passed, 0 failed** (§3).
