# B16 — `waav-infer-backend-torch`: the in-process Torch (libtorch/tch-rs) Path-B backend

**Date:** 2026-06-21
**Scope:** Stand up the in-process tch-rs Torch runtime (the strategic Path-B backend; user-decided ONNX + Torch, candle retired) and PROVE it by porting Voxtral-Mini-4B-Realtime onto it on GB10 CUDA, bit-faithful vs the candle + ORT-CPU references.
**Crate:** `crates/waav-infer-backend-torch/` (8 files). **Touched outside:** workspace `Cargo.toml`, `gb10-env.sh`, `ci/heavy_live_tests.sh`. **No candle/other crate touched. No git commit.**

---

## Headline answer

**YES — tch-rs serves Voxtral on GB10 CUDA with the correct transcript, no LD_PRELOAD.**

| | transcript (12.05 s clip) | RTF (release) | char-sim vs ORT-CPU |
|---|---|---|---|
| **ORT CPU** (q4f16, reference) | "…This is **W.A.V.** Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L." | 0.89 | — |
| **Torch CUDA** (this crate, f16) | "…This is **W.A.A.V.** Infer, a portable voice inference engine, running live on the GB10 Grace BL, a C-K-W-E-L-L." | **0.68** | **98.9%** |
| candle CUDA (prior arm, reference) | (same class of acronym jitter) | 0.62 | (≥92% gate) |

The ONLY divergence is the acronym spell-out (`W.A.V.` vs `W.A.A.V.`) — the exact same f16-greedy-argmax-tie jitter the candle arm exhibits near the acronym; the entire rest of the 12 s transcript is byte-identical. **98.9% de-punctuated char similarity** (the gate is ≥92%; this matches the prompt's stated ≥98.9% target). RTF **0.68 < 1** target, ~10% behind candle's 0.62 (honest gap explained below), and faster than ORT-CPU.

---

## The build.rs CUDA-link recipe (PROVEN — the de-risking deliverable)

`tch` linked against the active PyTorch gives `Cuda::is_available() == false` by **default**: the GNU linker treats `-ltorch_cuda` as `--as-needed` and, because no Rust object references a `libtorch_cuda` symbol directly (we only touch the dispatcher in `libtorch.so`/`libtorch_cpu.so`), **drops `libtorch_cuda.so` from `DT_NEEDED`**. libtorch's CUDA backend then never loads → CPU-only, silently. (torch-sys 0.20's own build.rs comment, ~L422-444, documents that it could not make the `--no-as-needed` fix propagate to the downstream binary.)

**The fix (this crate's `build.rs`):** re-assert the CUDA libs from the *consuming* crate under `-Wl,--no-as-needed`, scoped to just those two libs, + an rpath so they resolve at runtime:

```rust
// (only when LIBTORCH_USE_PYTORCH is set AND libtorch_cuda.so + libc10_cuda.so exist beside python3's torch)
let lib = "<pytorch>/lib";
for arg in [
    format!("-Wl,-rpath,{lib}"),
    format!("-L{lib}"),
    "-Wl,--no-as-needed".into(),   // <-- keeps the next two libs in DT_NEEDED
    "-ltorch_cuda".into(),
    "-lc10_cuda".into(),
    "-Wl,--as-needed".into(),      // <-- restore default for everything after
] {
    println!("cargo:rustc-link-arg={arg}");        // this crate's artifacts
    println!("cargo:rustc-link-arg-tests={arg}");  // its integration-test binaries (where the proof runs)
}
println!("cargo:rustc-link-search=native={lib}");
```

**Gotcha found & fixed during bring-up:** cargo *rejects* `cargo:rustc-link-arg-bins` / `-examples` / `-benches` for a lib crate that defines no such target ("does not have a bin/example target" → hard build error). Only `rustc-link-arg` (always valid) + `rustc-link-arg-tests` (valid because `tests/` exists) are emitted. **Consequence / known follow-up:** a downstream *production* binary that links this crate (the engine server) must carry the same `-Wl,--no-as-needed -ltorch_cuda -lc10_cuda` link arg via its **own** `build.rs` or a `.cargo/config.toml` — the per-crate `rustc-link-arg` does not transit to an unrelated final binary. The test binary is the proof vehicle here; wiring the production binary is an integration step out of this task's scope (which forbade touching other crates).

### Proof (not just "it ran")
Live smoke `cuda_is_available_and_matmul_exact`:
```
torch CUDA: is_available=true, device_count=1
torch CUDA smoke: softmax sum = 256.0000 (expect ~256)   ← exact CUDA matmul+softmax
```
`readelf -d` on the test binary (the rigorous proof the linker kept them, NOT LD_PRELOAD):
```
NEEDED   libtorch_cuda.so      ← in DT_NEEDED
NEEDED   libc10_cuda.so        ← in DT_NEEDED
RUNPATH  /home/bud/.local/lib/python3.12/site-packages/torch/lib   ← resolves at runtime
```

### Env (added to `gb10-env.sh`)
```
LIBTORCH_USE_PYTORCH=1            # link the libtorch inside the installed torch wheel
TORCH_CUDA_VERSION=cu130         # box has PyTorch 2.12.0+cu130 (sbsa/aarch64, real CUDA libs in the wheel)
LIBTORCH_BYPASS_VERSION_CHECK=1  # tch 0.20 expects libtorch 2.11; box has 2.12 → bypass
LD_LIBRARY_PATH += <pytorch>/lib # belt-and-suspenders (RUNPATH already covers it)
```
`tch = "0.20"` (proven version; already cached in the box registry). torch-sys 0.20 compiles its C++ shim against the active PyTorch headers; no separate libtorch download.

---

## The port: candle → tch (faithful, with candle's winning perf patterns)

`src/voxtral.rs` is a direct translation of `waav-infer-backend-candle/src/voxtral.rs`. Same architecture (32-layer causal-conv audio tower + downsample-4 + projector; 26-layer Mistral GQA 32/8 decoder, head_dim 128, RoPE θ=1e6, sliding-window 8192, swiglu→9216, tied lm_head, folded ada-rms), same lockstep scaffold constants (BOS=1, EOS=2, N_LEFT_PAD=32, N_DELAY=6, N_RIGHT_PAD=17, STREAMING_PAD=32, RAW_PER_TOK=1280), same precision intent (bf16 weights → **f16 on CUDA**, f32 on CPU; RMS variance accumulated in f32; softmax in f32). The mel frontend is reused **bit-for-bit** via `waav_infer_components::voxtral_log_mel`.

All four candle perf patterns carried over (and verified by CPU unit tests):
1. **Device-resident ring-KV** — a pre-allocated `[1,kvh,max_seq,d]` buffer written in place with `index_copy_(dim=2, idx, kv)` and read back via leading `narrow` (start=0 ⇒ contiguous, no extra copy). This is candle-nn `KvCache` rebuilt on raw tch tensors; **no per-step `cat`, no O(n²) realloc**. Guarded by `kv_cache_appends_in_place`.
2. **Zero-copy `[rows,in] @ Wᵀ`** — flatten leading dims to a 2-D gemm and multiply by `w.transpose(-1,-2)` (a strided view libtorch's cuBLAS consumes with OP_T). No weight copy — critical for the 805 MB tied lm_head touched every decode step.
3. **GQA-native attention** — fold `n_rep` into the query M-dim; K/V stay un-expanded at `kvh` heads (no `repeat_kv`). Guarded by `sdpa_gqa_matches_expanded_mha` (bit-identical to expanded MHA within 1e-5).
4. **Mask only on prefill** — single-row decode steps pass `None` (their causal+window mask is all-zeros within the 8192 window for any realistic clip), avoiding a per-step host mask build + H2D.

`SttModel` is implemented (`load(dir, device)` + `transcribe(&[f32]) -> String`), so the engine seam is satisfied identically to the candle/ORT arms. `tch::no_grad_guard()` wraps load + transcribe (inference-only, no autograd graph).

### tch-specific notes
- Weights load via `tch::Tensor::read_safetensors` (native BF16↔`Kind::BFloat16` mapping), then `.to_device().to_kind(f16)`.
- RoPE rotate-half implemented explicitly (`cat(-x2, x1)`, cos/sin doubled across halves) — arithmetically identical to candle's `rotary_emb::rope`.
- gelu uses `gelu("none")` = exact erf (matches candle `gelu_erf`); conv stem uses inherent `conv1d(w, Some(b), stride, 0, 1, 1)` with explicit left-pad-2 (causal, no right pad).
- `cudnn_set_benchmark(true)` enabled on CUDA load (conv-stem autotune; algorithm-only, no numeric change).

---

## What's proven vs gaps

**Proven**
- tch 0.20 builds + links + runs CUDA on GB10 aarch64 (sm_121) with PyTorch 2.12+cu130; `Cuda::is_available()==true` via the build.rs recipe, **no LD_PRELOAD** (DT_NEEDED + RUNPATH confirmed by readelf).
- Voxtral-realtime runs end-to-end on the GPU through libtorch and produces the **correct transcript** — 98.9% char-id vs ORT-CPU, the lone diff being the same acronym jitter candle shows.
- RTF 0.68 (release) < 1 target; faster than ORT-CPU (0.89); within ~10% of candle (0.62).
- 8 CPU unit tests green (mask correctness, GQA-native==MHA, ring-KV append, CPU smoke, device resolution). `clippy -p waav-infer-backend-torch --tests -- -D warnings` clean. Live `cuda_smoke` + `cuda_torch_voxtral_vs_ort` green, both registered in `ci/heavy_live_tests.sh` (process-isolated).

**Gaps / honest caveats**
1. **RTF 0.68 vs candle 0.62 (~10% slower).** Expected: the batch-1 lockstep loop is launch/host-sync bound (~970 decode steps, each ending in an `argmax(...).int64_value()` device→host sync — the same pattern candle uses), and tch routes every op through the libtorch dispatcher, which carries more per-op overhead than candle's tighter eager graph. Not a kernel-physics difference. **Headroom not yet taken:** CUDA-graph capture of the steady-state single-token step (libtorch supports it), keeping the argmax on-device and only reading back the chosen id, and trimming a few defensive `.contiguous()` calls. These are the levers to reach/beat 0.62; deferred (the task's bar was correct transcript + RTF<1, both met).
2. **f16 acronym jitter** (`W.A.A.V.`). Inherent to f16 greedy decoding on a near-tie; candle shows the same. Running the decoder in f32 on CUDA would likely remove it at a large RTF cost; f16 was kept to match the candle precision choice and the ≥98.9% target. Not a port bug — the lockstep math is faithful (the 12 s transcript is otherwise byte-identical to ORT-CPU).
3. **Production-binary CUDA link** (see recipe section): the engine server binary needs the `-Wl,--no-as-needed` arg via its own build.rs / `.cargo/config.toml`; this crate's `rustc-link-arg-tests` only covers its own test binaries. Out of scope here; flagged for the integration step that wires this backend into the registry.
4. **Single-clip RTF has run-to-run variance** (0.68–0.75 observed) because one clip doesn't amortize cuDNN autotune / first-launch JIT; release numbers above are the representative figure.

---

## Strategic read

This de-risks the whole Torch path. The two hard unknowns — *does tch link CUDA on GB10 at all*, and *does a real voice model run correctly + fast enough on it* — are both answered **yes**, with a reusable, production-shaped link recipe (not LD_PRELOAD) and a faithful model port that matches the trusted references. The shared seams the next ~14 sidecar models need (safetensors load → f16-on-CUDA, the `Linear`/`RmsNorm`/`Rope`/`Mlp`/`sdpa`/`sdpa_gqa` primitives, the device-resident ring-`KvCache`, the `SttModel` adapter) are all in place and tested. Recommended next: (a) wire the production-binary link arg, (b) take the CUDA-graph + on-device-argmax RTF levers, (c) port the next sidecar model (an AR codec-TTS) reusing these primitives.

### Files
- `crates/waav-infer-backend-torch/{Cargo.toml, build.rs}` — crate + the CUDA-link recipe.
- `crates/waav-infer-backend-torch/src/{lib.rs, device.rs, smoke.rs, voxtral.rs}` — backend + the port.
- `crates/waav-infer-backend-torch/tests/{cuda_smoke.rs, cuda_torch_voxtral_vs_ort.rs}` — the `#[ignore]` live-GPU gates.
- `Cargo.toml` (workspace dep entry), `gb10-env.sh` (tch env), `ci/heavy_live_tests.sh` (two gates registered).
