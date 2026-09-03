# RESTORE — Torch-TensorRT runtime + 4 reboot-wiped goldens (2026-06-24)

A box reboot wiped the **env/infra** (NOT code) the fleet's perf-lever + accuracy gates depend on: the throwaway
TRT compile/link venv in `/tmp` (so `trt_active=false`, all 6 TRT gates failed to deserialize their staged `.ts`
engines), and the 4 honest-SKIP goldens that lived in `/tmp` (weights present, goldens gone). Both are now
restored and **reboot-durable**: the goldens persist in `~/.cache/waav-models/<model>-golden/`, and the TRT venv
has a one-command reinstall recipe (its `.ts` engines already survive in `~/.cache`).

LAW SATISFIED: **trt_active=true** with a TRT gate deserializing + running byte-faithful on CUDA at ~3.5× FP16,
eager byte-id intact; the **4 goldens regenerated, persisted durably, gates passing byte-faithful**;
`cargo test -p waav-infer-backend-torch --lib --features cuda` = **192 passed / 0 failed**; clippy clean.

---

## A. Torch-TensorRT runtime — RESTORED (trt_active=true)

**Diagnosis (confirmed):** `import torch_tensorrt` / `import tensorrt` → ModuleNotFoundError; `libtorchtrt_runtime.so`
nowhere on disk. System torch 2.12.0+cu130 (CUDA True) survived. The staged `.ts` engines ALL survived in the
durable model tree, so **no re-export was needed**:
- `~/.cache/waav-models/neutts-air/trt/{backbone_fp16,backbone_fp8,backbone_int8,backbone_nvfp4}.ts`
- `~/.cache/waav-models/dia-1.6b/trt/decoder_fp16.ts`
- `~/.cache/waav-models/higgs-tts/trt/backbone_fp16.ts`

**Reinstall (STOCK wheels, NO NGC — the [[waav-infer-vllm-voice-program]] pins; B48/B49 recipe; throwaway,
compile/link-time only, NOT a serving path):**
```bash
python3 -m venv /tmp/trt_e2e_venv --system-site-packages          # reuse system torch 2.12.0+cu130 (no torch re-download)
/tmp/trt_e2e_venv/bin/pip install --no-deps torch-tensorrt==2.12.0 --extra-index-url https://download.pytorch.org/whl/cu130
/tmp/trt_e2e_venv/bin/pip install tensorrt-cu13==10.16.1.11 dllist  # the MATCHING TRT 10.16 (NOT 11.x → libnvinfer.so.10)
```
Installed: `torch_tensorrt 2.12.0+cu130` + `tensorrt-cu13 10.16.1.11` →
`torch_tensorrt/lib/libtorchtrt_runtime.so` (1.3 MB) + `tensorrt_libs/libnvinfer.so.10` (860 MB). The
`executorch>=1.2.0` / bare-`tensorrt` pip-resolver warnings are benign (B48: not needed; `import tensorrt` works).

**Build/link contract** (`crates/waav-infer-backend-torch/build.rs`, unchanged): `WAAV_TORCHTRT_LIB` +
`WAAV_TENSORRT_LIB` (the venv's `torch_tensorrt/lib` + `tensorrt_libs`) force-link `libtorchtrt_runtime.so` +
`libnvinfer.so.10` + `_plugin.so.10` under a scoped `--no-as-needed` window, under `RUSTFLAGS="--cfg accel_tensorrt"`.
Verified the test binary's `DT_NEEDED` carries all three (`readelf -d`). Default build (no env) = byte-for-byte the
prior link line.

**PROOF gate — `cuda_torch_neutts_trt` (RUSTFLAGS="--cfg accel_tensorrt", --ignored):** PASS.
```
neutts TRT: loaded Fp16 engine .../neutts-air/trt/backbone_fp16.ts (no-Python CModule::load) — AR decode accelerated
trt_active = true
B49 BACKBONE accuracy (1 step, same KV+token): hidden corr = 0.999964, max|Δ| = 0.0781   (bar corr>0.999 — accuracy-preserving)
B49 RTF: eager 17.546s / RTF 3.970  |  TRT 5.746s / RTF 1.122  |  per-step speedup ~3.54×
```
- **Deserialized + ran the staged `.ts` engine on CUDA, no-Python** via `tch::CModule::load` (`trt_active=true`).
- **FP16 ~3.54× the eager** (well above the 1.6–2× bar) — a throughput lever.
- **Accuracy-preserving** per-step backbone (hidden corr 0.999964 > 0.999). TRT FP16 forks the greedy-AR sequence
  by design (lossy) → it is a THROUGHPUT lever, NOT a byte-id path.

**Eager byte-id INTACT (the critical "eager untouched" check) — `cuda_torch_neutts` (no accel_tensorrt):** PASS.
gate1 codec maxΔ=0, gate2 hidden maxΔ=0, gate3 logits maxΔ=0, **gate4 greedy CPU-f32 0/96 differ**, **gate5 greedy
CUDA-bf16 0/96 differ**, gate6 live synth RTF 0.673. The TRT install did not perturb the eager path one bit.

**Reboot-durability note:** the venv in `/tmp` is wiped on reboot, but that is acceptable — it is a compile/link-time
dep only (the `.ts` engines run no-Python at serve time). The `.ts` engines are durable; the 3-line reinstall above
restores the link in ~1 min. (A future hardening could relocate the venv under `~/.cache/waav-venvs/trt_e2e/` so even
the venv survives; the gate-doc recipes currently pin `/tmp/trt_e2e_venv` so that path was kept.)

---

## B. The 4 reboot-wiped goldens — REGENERATED, PERSISTED DURABLY, GATES BYTE-FAITHFUL

Each golden was regenerated via its **staged reference recipe** (the model's own reference engine on the SHARED
system torch, or a throwaway `--system-site-packages` reference venv — reference-only, NOT a serving path,
per [[waav-infer-no-venv-wrap]]) and persisted to `~/.cache/waav-models/<model>-golden/` (the durable pattern
`granite-speech-golden` uses), with a `gen_golden.py`/`dump_golden.py` reproducer beside it. Each gate's golden path
was repointed to that durable default (env override preserved).

| model | reference engine used | golden files (durable) | gate result |
|---|---|---|---|
| **ark-asr-0.6b** | ARK-ASR `trust_remote_code` HF `generate` on SHARED system torch (CPU-fp32; the sidecar IS the reference) | `transcript_cpu_fp32.txt`, `pcm16.f32` (reused from granite — same kokoro clip, md5 match), `gen_ids_fp32.npy`, `dump_golden.py` | **byte-identical** 100.0% char-identity (RTF 0.392) |
| **canary-qwen-2.5b** | NeMo `SALM.generate()` (CUDA bf16; `nemo_toolkit[asr]==2.7.3` throwaway venv) on the HF README widget LibriSpeech clips | `sample1.wav`, `sample2.wav`, `sample1.txt`, `sample2.txt`, `gen_golden.py` | **avg WER 0.0%** vs golden (bar <0.15), per-clip 0.0%/0.0% |
| **voxtral-tts** | sglang-omni standalone CPU-fp32 reference (`mistral_common 1.11.3` throwaway venv) on `consolidated.safetensors`, voice `casual_male` | `prompt.json`, `codes.npy`[24,37], `x0.npy`[24,1,36], `wav.npy`, `sampled.npy`, `prefill_last_hidden.npy`, `gen_golden.py` | **semantic 0/24, 24/24 bit-exact prefix, codec corr 0.999995**; frame0 maxΔ 4.17e-7; all 6+ tests green |
| **viitorvoice-nar** | port-faithful reference (embeds/heads/Gumbel in torch-CUDA fp32; backbone + DualCodec via shipped ONNX; seed-0 schedule_34) — system Python had everything, no venv | `codes.npy`[12,79], `wav.npy`, `prompt_row0.npy`, `gen_golden.py` | gate1 ids 34/34; gate2 codec maxΔ=0; **gate3 THE LAW codes 0/948**; gate4 wav maxΔ=0 (RTF 2.453) |

**Gate-file edits (all in `crates/waav-infer-backend-torch/tests/`, test-only, lib untouched):**
- `cuda_torch_ark.rs` — `golden_dir()` default `/tmp/ark_golden` → `$HOME/.cache/waav-models/ark-asr-golden` (`WAAV_ARK_GOLDEN` override).
- `cuda_torch_canary_qwen.rs` — hardcoded `/tmp/...` `CLIPS` const → `golden_dir()`+`clips()` under `$HOME/.cache/waav-models/canary-qwen-golden` (`WAAV_CANARY_GOLDEN`).
- `cuda_torch_voxtral_tts.rs` — `golden_dir()` default `/tmp/voxtral_golden` → `$HOME/.cache/waav-models/voxtral-tts-golden` (`WAAV_VOXTRAL_TTS_GOLDEN` preserved).
- `cuda_torch_viitorvoice.rs` — `const GOLDEN_DIR="/tmp/vv_ref_out"` → `golden_dir()` defaulting to `$HOME/.cache/waav-models/viitorvoice-nar-golden` (`WAAV_VIITORVOICE_GOLDEN`).

**Run commands (durable, no env needed):**
```bash
source gb10-env.sh
cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_ark         -- --ignored --nocapture --test-threads=1
cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_canary_qwen -- --ignored --nocapture --test-threads=1
cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_voxtral_tts -- --ignored --nocapture --test-threads=1
cargo test -p waav-infer-backend-torch --features cuda --test cuda_torch_viitorvoice -- --ignored --nocapture --test-threads=1
```

---

## Honesty / nothing un-regenerable

- **Nothing was un-regenerable.** All 4 goldens reproduced cleanly from their reference engines; the TRT `.ts`
  engines survived so no re-export was needed (the staged engines + the reinstall recipe are the full restore).
- voxtral-tts came back **stronger** than the historical golden (24/24 bit-exact frames vs the prior doc's 18/24) —
  the new seed/RNG draw avoids the late ±1 FSQ-boundary near-ties; the gate's bar (semantic-perfect + ≥12 prefix +
  codec corr) is comfortably met either way. The OSS checkpoint's encoder weights for voice-cloning remain absent
  (noted in the ONBOARD as not-needed for voice-id TTS) — not a regression.
- canary used NeMo (the documented reference); the README widget clips come from a stable public cdn-media URL and
  `gen_golden.py` regenerates the transcripts deterministically (greedy, 0.0% drift).

## Build/lib health (Rust touched → gated)
- `cargo test -p waav-infer-backend-torch --lib --features cuda` → **192 passed, 0 failed, 2 ignored.**
- `cargo clippy -p waav-infer-backend-torch --lib --features cuda` → clean; `--tests` → clean.
- Did NOT touch the concurrent agent's files (`cuda_torch_misotts.rs`, `cuda_torch_zonos2.rs`, `voxtral.rs`,
  `cuda_torch_voxtral_perf.rs`); no `git commit`; no `cargo fmt`.
