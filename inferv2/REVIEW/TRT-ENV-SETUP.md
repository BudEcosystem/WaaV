# TRT-ENV-SETUP — offline TensorRT engine-compile env + the cross-version load verdict

> **Goal:** stand up a TensorRT engine-compile env to unblock the int8/fp8/nvfp4 **Throughput** tier, and
> answer the one open question the prior TRT agent left: *does a `.ts`/TRT engine compiled against torch 2.12
> LOAD + RUN in the build's torch-2.11 runtime?*
>
> **Headline verdict (measured, not assumed) — the no-Python TensorRT `.ts` path is torch-2.12+ ONLY:**
> 1. The **torch-2.12 compile + torch-2.11 runtime SPLIT does NOT work** — it fails at **link time** with a
>    hard C++ ABI mismatch (`torch::headeronly::Tag`, new in 2.12). The 2.12 `libtorchtrt_runtime.so` is
>    version-locked to torch 2.12.
> 2. The **matching-version 2.11 path ALSO does not yield a usable artifact**: `torch_tensorrt==2.11.0` (its
>    aarch64 wheel DOES exist) *compiles + runs the engine in Python*, but **cannot serialize it to a `.ts`** —
>    every TorchScript path (`torch_tensorrt.save(torchscript)`, `torch.jit.trace`, `torch.jit.script`) fails on
>    torch 2.11 with `c10::SymInt in the JIT tracer`. No `.ts` ⇒ the no-Python `tch::CModule` runtime
>    (`src/trt.rs`, the whole Throughput design) has nothing to load.
> 3. **Net:** the no-Python `.ts` TensorRT runtime requires **torch 2.12 + torch_tensorrt 2.12.1 for BOTH
>    compile AND runtime** (matched). Pinning the build to torch 2.11 to protect the goldens **disables the TRT
>    Throughput tier entirely**. To enable it, the build/runtime must move to torch 2.12 and the byte-identical
>    goldens be re-validated there (the box's torch was historically 2.12 — this is likely a revert to the
>    known-good TRT config, not new ground). See §5.

Date: 2026-06-27. Box: NVIDIA GB10 (sm_121, cap 12.1), aarch64, 121 GB unified pool.

---

## 1. The envs that were built (the build venv was NOT touched)

| venv | torch | torch_tensorrt | tensorrt | purpose |
|---|---|---|---|---|
| `/home/bud/torch_venv` (**PROTECTED — untouched**) | 2.11.0+cu130 | — | — | the Rust build/runtime libtorch (byte-identical goldens) |
| `/home/bud/torch212_trt_venv` (NEW) | 2.12.0+cu130 | 2.12.1 | 10.16.1.11 | the prior agent's intended "2.12 offline compile" env |
| `/home/bud/torch211_trt_venv` (NEW) | 2.11.0+cu130 | **2.11.0** | 10.15.x | **the matching-version compile env — the actual unblock** |

Both new venvs were created with `python3 -m venv` and `pip install` from `download.pytorch.org/whl/cu130`
(torch) + PyPI (torch_tensorrt/tensorrt). `/home/bud/torch_venv` (2.11) was never modified — verified
before/after (`torch 2.11.0+cu130`, `libtorch_cuda.so` present).

modelopt is NOT installed in either (only needed for int8/fp8/nvfp4 PTQ; the fp16 compile that answers the
ABI question does not need it). Add `nvidia-modelopt` to the chosen compile venv for the low-precision tiers.

---

## 2. The offline compile WORKS (torch 2.12 venv) — `trt_compile_neutts.py`

Recipe (the existing `torch_runtime/trt_compile_<model>.py` scripts, unchanged):

```bash
VENV=/home/bud/torch212_trt_venv
export LD_LIBRARY_PATH="/usr/local/cuda-13.1/compat:\
$VENV/lib/python3.12/site-packages/tensorrt_libs:\
$VENV/lib/python3.12/site-packages/torch_tensorrt/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH"
$VENV/bin/python torch_runtime/trt_compile_neutts.py \
  --model-dir ~/.cache/waav-models/neutts-air \
  --out ~/.cache/waav-models/neutts-air/trt/backbone_fp16_t212.ts --precision fp16
```

Result (neutts-air Qwen2-0.5B per-step backbone, fp16, dynamic KV profile min1/opt128/max512):
`stage=done, ok=true`, **hidden_correlation 0.99919** (accuracy-preserving, corr>0.999), per-step
**1.84× speedup** (eager 7.92 ms → TRT 4.30 ms). Engine `.ts` = 914 MB. CUDA from the 2.12 venv works on
GB10 (cap 12.1). So **compiling engines is not the problem** — loading them in a mismatched runtime is.

---

## 3. THE KEY TEST — does a torch-2.12-compiled `.ts` load in the torch-2.11 runtime? **NO.**

The faithful runtime call (`tch::CModule::load_on_device` + `forward_is`, exactly what
`src/trt.rs TrtStepBackbone::{load,run}` do) was exercised by a standalone tch binary that force-links the
**torch-2.12** `libtorchtrt_runtime.so` while linking the build's **torch-2.11** libtorch (the in-crate
`tests/cuda_torch_trt_abi.rs` is the durable home for this gate, but the crate currently can't build — see §6).

It FAILS at **link time** (`cc`/`ld`), before any `.ts` is even loaded:

```
/usr/bin/ld: .../torch212_trt_venv/.../libtorchtrt_runtime.so: undefined reference to
  `torch::Library::_def(c10::FunctionSchema&&, c10::OperatorName*,
     std::vector<torch::headeronly::Tag, ...> const&, torch::_RegisterOrVerify) &'
/usr/bin/ld: .../libtorchtrt_runtime.so: undefined reference to
  `torch::Library::_def(std::variant<c10::OperatorName, c10::FunctionSchema>&&,
     torch::CppFunction&&, std::vector<torch::headeronly::Tag, ...> const&) &'
collect2: error: ld returned 1 exit status
```

### Root cause (symbol-level proof, not a guess)

PyTorch 2.12 moved the op-registration `Tag` type into a new `torch::headeronly` namespace, changing the
exported C++ symbol for `torch::Library::_def`. The 2.12 `libtorchtrt_runtime.so` imports the
`torch::headeronly::Tag` overload; torch 2.11 only exports the `at::Tag` overload:

| symbol (`nm -DC libtorch_cpu.so \| grep Library::_def`) | torch 2.11 | torch 2.12 |
|---|---|---|
| `_def(..., std::vector<torch::headeronly::Tag>...)` | **0 (absent)** | **4 (defined, T)** |
| `_def(..., std::vector<at::Tag>...)` | defined (T) | (renamed away) |

So the 2.12 runtime lib's undefined `torch::headeronly::Tag` references cannot be resolved by torch 2.11 —
the split is impossible at the linker, and would equally fail as an `undefined symbol` at `dlopen`/`jit::load`
time. **This is a genuine 2.11↔2.12 libtorch ABI break, not a missing-path / LD_LIBRARY_PATH issue.** Note
the build.rs force-link of the runtime lib itself worked (the `B49 TensorRT runtime force-linked` warning
fired) — the breakage is purely the cross-version symbol set.

---

## 4. The matching-version 2.11 path — links, COMPILES, but cannot SERIALIZE a `.ts`

The prior agent's premise ("the only aarch64 torch_tensorrt wheel is 2.12.1, which forces torch 2.12") is
**false**. PyPI serves `torch_tensorrt-2.11.0-cp312-cp312-manylinux_2_28_aarch64.whl`, with:

- `Requires-Dist: torch <2.12.0,>=2.11.0`  → matches the build's torch **2.11.0** exactly.
- `Requires-Dist: tensorrt <10.16.0,>=10.15.1`.
- its `libtorchtrt_runtime.so` imports the **`at::Tag`** `_def` overload — the one torch 2.11 **exports** (T),
  so the 2.11 runtime lib *would* link cleanly into the torch-2.11 build (no ABI break — the §3 problem is
  gone for the matched stack).

So the ABI is solved on the matched 2.11 stack — **but a second, independent wall appears**: torch 2.11 cannot
produce the `.ts` the no-Python runtime needs (next).

### 4a. Measured: `torch_tensorrt 2.11.0` compiles + runs, but NO `.ts` can be serialized on torch 2.11

`torch_tensorrt 2.11.0` (tensorrt **10.15.1.29**) on GB10 **compiles + runs the neutts backbone in Python** —
the forward is correct (`hidden (1,1,896)`, `new_k (24,1,2,129,64)`), for both dynamic and static export. But
**every** TorchScript serialization path — the artifact `tch::CModule::load` requires — FAILS on torch 2.11:

| serializer | result on torch 2.11 |
|---|---|
| `torch.jit.trace(compiled, example)` (the path the 2.12 script uses) | `RuntimeError: Found an unsupported argument type c10::SymInt in the JIT tracer` |
| `torch_tensorrt.save(compiled, out, output_format="torchscript")` (canonical API, with `inputs=` and `arg_inputs=`) | same `c10::SymInt in the JIT tracer` (it traces internally) |
| `torch.jit.script(compiled)` | `NotSupportedError: Compiled functions can't take variable number of arguments` (the dynamo module's `*args`) |

→ `FINAL: NO_TS_PRODUCED`. The torch_tensorrt-dynamo backend emits **`SymInt`-bearing shape nodes even for a
STATIC export**, and torch 2.11's TorchScript tracer cannot handle `SymInt` at all (torch 2.12's can — which
is why the 2.12 `jit.trace` succeeded and produced the 914 MB engine). This is a **core torch-2.11
limitation**, not a torch_tensorrt-version bug — 2.11.x is the newest torch_tensorrt for torch 2.11, so there
is no 2.11 escape.

**Consequence:** the no-Python `tch::CModule` runtime (`src/trt.rs`, the entire Throughput design) has no
artifact to load on torch 2.11. The matched-version path unblocks the ABI but is dead-ended by serialization.
(Two further 2.11-stack rough edges seen before the serialize wall — both moot given NO_TS: the dynamic engine
also hit `setInputShape ... got false` at the profile min/max under tensorrt 10.15; the interior opt shape ran
fine.) **The capability is real but torch-2.12-only** — the prior agent's working `neutts/trt/*.ts` (Jun 22)
were necessarily produced on torch 2.12.

---

## 5. What actually unblocks the Throughput tier — move the build to torch 2.12 (matched)

The no-Python `.ts` TRT runtime needs **the same torch for compile AND runtime, and that torch must be 2.12**
(only 2.12 can serialize the dynamo-compiled engine to TorchScript). So the only working configuration is:

```bash
# ── compile + runtime BOTH on torch 2.12 + torch_tensorrt 2.12.1 (matched) ──
#    the byte-identical goldens must be re-validated on torch 2.12 (the box's historical torch).
# compile (offline), exactly as proven this session (corr 0.9991, 1.84x):
VENV=/home/bud/torch212_trt_venv     # torch 2.12.0 + torch_tensorrt 2.12.1 + tensorrt 10.16.1
export LD_LIBRARY_PATH="/usr/local/cuda-13.1/compat:$VENV/.../tensorrt_libs:$VENV/.../torch_tensorrt/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH"
$VENV/bin/python torch_runtime/trt_compile_neutts.py --model-dir <model> --out <model>/trt/backbone_fp16.ts --precision fp16
# Rust runtime ALSO on torch 2.12: point the build venv / gb10-env's python3 at a torch-2.12 venv,
# WAAV_TORCHTRT_LIB/WAAV_TENSORRT_LIB → the same 2.12 venv, RUSTFLAGS="--cfg accel_tensorrt".
```

The `engine_path_with_precision` (`src/trt.rs`) staging layout `<model>/trt/backbone_<precision>.ts` is
unchanged. The hard rule: **compile torch == runtime torch, and == 2.12** (the `torch::headeronly::Tag` ABI
locks compile/runtime together; the `c10::SymInt` JIT-tracer support locks both to ≥2.12).

### Alternatives considered (and why they don't unblock it on torch 2.11)

- **Matching-version torch_tensorrt 2.11.0 on the torch-2.11 build** — links cleanly (ABI ok) but **cannot
  serialize a `.ts`** (§4a, `c10::SymInt`). Dead end for the no-Python runtime.
- **torch.export + AOTInductor `.so`** — embeds/links torch C++ symbols, so it carries the same compile==runtime
  version lock; and `tch::CModule` loads TorchScript, not an AOTInductor `.so` / ExportedProgram, so it would
  need a new Rust runtime seam anyway. Does not rescue torch 2.11.
- **torch_tensorrt Python runtime (`use_python_runtime=True`) as a sidecar** — would run a 2.12-compiled engine
  from Python, but reintroduces a Python serve path (against the WaaV "no venv/pip serving" hard rule). Only a
  fallback if the build truly cannot move off 2.11.
- **Keep torch 2.11, drop the TRT tier** — rely on the byte-identical CUDA-graph default (Accuracy mode) and
  forgo int8/fp8/nvfp4 Throughput. The honest status-quo if the goldens cannot be re-validated on 2.12.

---

## 6. Caveat encountered — an unrelated, in-progress crate break (NOT mine)

Building the in-crate gate `crates/waav-infer-backend-torch/tests/cuda_torch_trt_abi.rs` is currently blocked
by an **unrelated**, actively-edited WIP in `src/s2_pro.rs` (mid-refactor adding a `proj_native: bool` arg to
`S2Layer::load`/`TextModel::load`; call sites not yet updated — file mtime was seconds before the build).
Another agent owns that file, so it was left untouched. The ABI verdict above was obtained with a **standalone
tch binary** (`scratchpad/trt_abi_standalone/`) that does NOT depend on the broken crate but calls the
identical `tch::CModule::load_on_device` + `forward_is` — so the result is faithful. Once `s2_pro.rs`
compiles again, `cuda_torch_trt_abi.rs` is the durable in-tree home for this gate.

## 7. Artifacts

- Compile venvs: `/home/bud/torch212_trt_venv` (torch 2.12 + torch_tensorrt 2.12.1, the WORKING compile env),
  `/home/bud/torch211_trt_venv` (torch 2.11 + torch_tensorrt 2.11.0, the matched env that compiles but cannot
  serialize a `.ts`). Both throwaway; neither is `/home/bud/torch_venv`.
- Engine produced: `~/.cache/waav-models/neutts-air/trt/backbone_fp16_t212.ts` (914 MB, torch 2.12). No
  `…_t211.ts` exists — every torch-2.11 serialize attempt failed (`NO_TS_PRODUCED`).
- New durable in-tree test: `crates/waav-infer-backend-torch/tests/cuda_torch_trt_abi.rs` (the cross-version
  load gate; blocked from building today only by the unrelated `s2_pro.rs` WIP — see §6).
- Standalone ABI probe + scripts (scratchpad, throwaway): `scratchpad/trt_abi_standalone/` (the faithful
  `tch::CModule` load/run harness), `scratchpad/compile_211_*.py`, `scratchpad/run_*.sh`.
- The protected build venv `/home/bud/torch_venv` (torch 2.11.0+cu130) was verified untouched (no
  torch_tensorrt added, `libtorch_cuda.so` intact).
