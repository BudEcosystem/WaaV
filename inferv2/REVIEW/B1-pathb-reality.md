# B1 — Path B Reality: the truth about WaaV Infer's custom (non-ONNX) execution runtime

**Date:** 2026-06-21  **Mode:** READ-ONLY code/manifest investigation (no build).
**Question:** Does a real "vLLM for voice" Path B exist, or is it just designed? Are the no-venv hard
rule and the multi-hardware vision satisfied? What do the torch-backend models actually run on?

---

## TL;DR verdict

**A real Path B EXISTS and is wired live — but it is a PYTHON (torch + transformers) sidecar process, NOT
an embedded pure-Rust runtime.** It is **substantially BUILT, not merely designed** (this contradicts the
stale memory note "DESIGN for a PyTorch sidecar runtime").

- **Verdict: PARTIAL — real, wired, and no-venv-compliant *today*; NOT yet the portable multi-hardware
  "vLLM for voice" the vision demands.**
- It runs the models' **own HF code on the provider's original safetensors in ONE shared Python env**
  (no per-model venv is configured anywhere) — so the *spirit* of the no-venv rule is met.
- BUT the execution substrate is **CPython + torch + `transformers.generate()`**, which is:
  - **CUDA/ROCm/CPU-only** (no Hexagon/QNN, CoreML, OpenVINO, AMX-tuned NPU) → fails the portability bar
    the charter sets for "own flexible execution, not capped by ORT kernels."
  - **Not an owned execution engine** — the AR/KV/sampling loop is `model.generate()` inside HF, not a
    WaaV-controlled lockstep/step-bucket scheduler. The Rust engine sees the sidecar as an opaque
    `transcribe→text` / `synthesize→PCM` black box; none of the v2 engine machinery (fixed-slot lockstep,
    duty ledger by frame, prefix-KV residency) reaches inside it.
- The strict reading of the hard rule ("zero per-venv/pip *serving* paths; reimplement the architecture
  from the repo portably") is **only partially satisfied**: there is no *venv*, but there **is** a
  pip/CPython *serving* path (torch + transformers are runtime serving deps of the sidecar, not throwaway
  validation). The rule's deeper intent — *reimplement portably in the engine* — is **not** met for these
  models; they are served by driving the upstream framework, exactly the "wrap the package" shape the
  design doc itself (§10d/§10.4) flags as the anti-pattern to avoid.

So: **the seam, the supervision, the registry, and 14 working model runners are real and live-capable.
The "portable, owned, multi-hardware execution" half of the vision is not built** — Path B today is a
crash-isolated transformers harness, which the design explicitly calls deployment tier (2) of (1–4), the
"closer-to-production-than-venv but not the final portable answer" tier.

---

## 1. `torch_sidecar.rs` — verdict

**File:** `crates/waav-infer-server/src/torch_sidecar.rs` (37 KB, ~750 lines, ~265 of them production).

**What it is, exactly:** a **Rust client that spawns a separate Python child process** and exposes it as a
normal `SttModel` / `TtsModel`. It does **NOT** embed a runtime (no PyO3 — by deliberate design, see §4).

| Aspect | Reality |
|---|---|
| **Process model** | `Command::new(python)` → `python -m torch_runtime serve --model <m> --device <d> --dtype <dt> [--arch <a>]`, cwd = `$WAAV_TORCH_HOME`. One model ⇒ one child process ⇒ one `SidecarId`. |
| **Is it a venv/pip path?** | It *can* be (per-model `python` override exists, `engine.rs:97-99`), but **NO model configures one** — every torch `waav.json` omits `python`, so all default to `python3` (the single shared env). So **as configured today it is NOT a per-venv path**; it is a single-shared-env pip path. |
| **IPC** | Framed binary stdin/stdout (NOT WS/UDS despite the design saying UDS): `u32_be total \| u32_be json_len \| <json> \| <raw f32le PCM>`, symmetric. Binary-safe, no base64. |
| **Binary/script launched** | The Python package `torch_runtime` (in the repo at `waav-infer/torch_runtime/`), entry `__main__.py` → `cli.py serve`. |
| **Wired to the live seam?** | **YES.** `engine.rs::load_model_at` (the standalone/CLI/GUI load path) checks `read_torch_runtime(dir)` first; if `waav.json` has `{"runtime":{"backend":"torch"}}` it calls `load_torch_model` → `TorchSidecar::spawn` → wraps as `TorchSidecarStt`/`TorchSidecarTts` (`LoadedModel::Stt/Tts`). Same `SttModel`/`TtsModel` contract, so all callers above the seam are unaffected. |
| **Any model served live through it?** | The **GUI** (`gui/app.py`) drives 16 torch models resident in-process (it imports `torch_runtime` directly, real load/unload + inference). The **Rust sidecar path** is fully wired and the heavy-test harness exists; torch 2.12.0+cu130 (CUDA available) and transformers 5.12.0 are installed on this box, so the path is live-capable. (No build was run here to re-confirm an end-to-end Rust→Python round-trip, but every link in the chain is present and the deps resolve.) |
| **Resilience** | This is the *most* hardened part. Out-of-band `SidecarHeartbeat` watchdog (`waav-infer-runtime/src/watchdog.rs`), per-request bounded reaper (`read_frame_bounded`): a CUDA-wedged child is detected dead and the request **fails typed (StallTimeout/503) in bounded time, never an indefinite hang** (gate H7/G7). `authorize_serve` gates dispatch on liveness; `mark_dead` fans out so subsequent requests fail fast. 4 RED-gate tests stand up real `/bin/sh` sidecars (no torch needed) to prove the hang surface is closed. |

**Bottom line on the .rs file:** it is a *correct, production-grade IPC client + supervisor for a foreign
process*. It is NOT a custom kernel runtime; it is the **topology glue** that lets a Python process play
the model role. The crash-isolation (P-5) rationale is sound and well-executed.

---

## 2. Embedded ML backends — dependency table

Grepped every `crates/*/Cargo.toml` for `tch`, `torch-sys`, `candle-core/nn/transformers`, `ort`, `ggml`,
`llama`, `pyo3`, `cudarc`, `onnxruntime`.

| Backend | Status | Where |
|---|---|---|
| **`ort`** (ONNX Runtime, `load-dynamic`) | ✅ **ACTUAL, the ONLY native ML backend** | workspace dep (`=2.0.0-rc.12`, features `ndarray/api-24/load-dynamic/half`); used by `waav-infer-backend-ort` (Path A). Dylib supplied via `ORT_DYLIB_PATH`. |
| `onnxruntime` (0.9, the C-API crate) | ✅ present but **only a dylib pre-validator** | `waav-infer-backend-ort/Cargo.toml` — used solely to pre-check the ORT dylib is loadable before `ort` re-enters it. Not an inference path. |
| `candle-core` / `candle-nn` / `candle-transformers` | ❌ **NOT a dependency** | Named only in doc-comments as the *example* of "a backend we must not name from -core" (P-8 isolation rule). **Zero candle code in the tree.** The memory's "vendor moshi-core / candle reuse" is **aspirational, not built.** |
| `tch` / `torch-sys` (libtorch) | ❌ **NOT a dependency** | Not in any Cargo.toml. The "torch" in WaaV Infer is **Python torch in the sidecar**, never libtorch-in-Rust. |
| `pyo3` | ❌ **NOT a dependency** | **Deliberately rejected** (design §0.1: PyO3 "would embed CPython + torch CUDA in the Rust process, violating P-5 crash-containment and zero-C/C++"). The sidecar is the chosen alternative. |
| `ggml` / `llama.cpp` | ❌ NOT a dependency | Named only as the third hypothetical backend in P-8 doc-comments. |
| `cudarc` | ❌ NOT a dependency | Not present. |

**Conclusion:** WaaV Infer has **exactly one embedded/native ML backend: `ort` (ONNX Runtime).** There is
**no embedded torch (tch), no candle, no ggml, no in-process Python.** Every non-ONNX model runs in the
**out-of-process Python torch sidecar.** The "two-runtime engine" is real, but runtime #2 lives in a
different process and a different language.

---

## 3. The torch-backend models — list, runtime needs, registry handling

### 3a. How the registry handles `backend:torch`

There are **two registries** (Rust core vs the torch path); the dispatch happens *before* the core one:

- **`core/src/model.rs::load_model`** (the canonical config-arch registry, 16 arms) handles **ONLY
  `StaticGraph`/ONNX models**. It takes a `GraphLoader`, has **NO torch arm**, and `LoadedModel` is just
  `Tts | Stt` (no torch variant). A test (`registry_path_a_invariant_no_backend_named`) *enforces* that
  this module names no concrete backend. So **`load_model` itself never sees `backend:torch`.**
- **`server/src/engine.rs::load_model_at`** is the real entry. It checks `read_torch_runtime(dir)`
  **first** → if `runtime.backend == "torch"`, it diverts to `load_torch_model` (spawn sidecar) and
  **returns before ever calling `core::load_model`**. Otherwise it builds an `OrtGraphLoader` and calls
  the core registry. So the answer to "reject, sidecar, or what?" is: **diverted to the Python sidecar,
  one level above the core registry** — the core registry stays pure ONNX/Rust.
- **Python side mirror:** `torch_runtime/registry.py` (`@register(arch...)` + `build()`) is the Python
  analog of the config-arch dispatch — `architectures[0]` (or `waav.json` `runtime.architecture`) →
  registered runner class. "New model = config + weights, not engine code" holds on the torch side too.

### 3b. The torch models actually present (local manifests with `backend:torch`)

14 model dirs under `~/.cache/waav-models` carry `{"runtime":{"backend":"torch",...}}`. All load HF
safetensors as-is (no re-publish); dtype mostly `bf16`/`fp16`. Mapped to their registered runner + the
runtime each one *needs*:

| # | Model dir | Arch / runner | Task | Runtime need (arch family) | Loads via |
|---|---|---|---|---|---|
| 1 | `ark-asr-0.6b` | `ArkasrForConditionalGeneration` → `ArkAsrRunner` | STT | **LLM-decoder ASR** (Whisper-enc + MLP + Qwen2 AR) | transformers (`AutoModel...`) |
| 2 | `granite-speech-4.1-2b` | `GraniteSpeechForConditionalGeneration` → `GraniteSpeechRunner` | STT | **LLM-decoder ASR** (Conformer-CTC enc + projector + Granite LLM + speech LoRA) | transformers-native |
| 3 | `csm-1b-hf` | `csm` → `CsmRunner` | TTS | **AR codec-TTS** (Mimi RVQ + depth-transformer; Sesame CSM) | transformers (`CsmForConditionalGeneration`) |
| 4 | `dia-1.6b` | `dia` → `DiaRunner` | TTS | **AR codec-TTS** (DAC audio tokens, CFG sampling; Nari Dia) | transformers (`DiaForConditionalGeneration`) |
| 5 | `dia2-2b` | `dia2` → `Dia2Runner` | TTS | **AR codec-TTS + depformer** (fully **vendored** custom runtime, `vendor/dia2/`) | vendored model code |
| 6 | `dots-tts-base` | `dots_tts` → `DotsTtsRunner` | TTS | **AR + DiT/flow + BigVGAN vocoder** (vendored `vendor/dots_tts/`) | transformers + vendored |
| 7 | `dots-tts-soar` | `dots_tts` (SCA variant) | TTS | same as dots base (flow head variant) | transformers + vendored |
| 8 | `dots-tts-mf` | `dots_tts` (MeanFlow variant) | TTS | same as dots base (MeanFlow head) | transformers + vendored |
| 9 | `higgs-tts` | `higgs_tts` → `HiggsTtsRunner` | TTS | **AR multi-codebook codec-TTS** (Higgs 8-codebook delay @25fps; 4B) | transformers |
| 10 | `neutts-air` | `neutts_air` → `NeuttsAirRunner` | TTS | **AR codec-TTS** (on-device clone, 0.5B) | transformers |
| 11 | `qwen3-tts-12hz-06b` | `qwen3_tts` → `Qwen3TTSRunner` | TTS | **AR codec-TTS** (Qwen3-TTS, 9 speakers; **vendored** tokenizer `vendor/qwen3_tts/`) | transformers + vendored |
| 12 | `vibevoice-1.5b` | `vibevoice` → `VibeVoiceRunner` | TTS | **diffusion TTS** (LLM + diffusion head + DPM solver; **vendored** `vendor/vibevoice/`) | transformers + vendored |
| 13 | `omnivoice` | `omnivoice` → `OmniVoiceRunner` | TTS | **masked-diffusion-LM TTS** (fp32) | transformers |
| 14 | `cosyvoice3` | `cosyvoice3` → `CosyVoice3Runner` | TTS | **AR + CFM flow-matching + HiFi-GAN** (vendored `vendor/cosyvoice/`) | transformers + vendored |

**Registered-but-no-local-manifest runners (also in `models/`, GUI lists some):**
- `canary_qwen` (`SALM` → `CanaryQwenRunner`, STT, LLM-decoder) — `canary-qwen-2.5b` dir exists but its
  `waav.json` has **no** runtime block (so not currently routed to torch; runner is ready).
- `chatterbox` (`ChatterboxTTS` → `ChatterboxRunner`, TTS) — `chatterbox` / `chatterbox-turbo-onnx` dirs
  carry no torch runtime block (chatterbox is the design's "reclassified validated-reference" model;
  there is also an ONNX export variant).

**So:** 14 models are wired to Path B via local manifests **today**; 16 model runner classes are
registered (`models/__init__.py`). The brief's "23 torch-backend models" matches the **broader
onboarding triage** (`WaaV/INFER_TRIAGE.md` / the design's ~50-non-ONNX universe), of which **14 are
live-wired and 16 have runner code**; the rest (csm depth variants, hibiki, the deferred 7-9B duplex, S2S)
are designed/triaged but not yet runner-backed. **Runtime needs cluster into 4 families:** LLM-decoder ASR
(4), AR codec-TTS (the bulk), diffusion/masked-diffusion TTS (vibevoice, omnivoice), AR+flow/CFM TTS
(cosyvoice3, dots) — exactly the two-seam taxonomy (decoder + codec/CFM) the design predicted.

---

## 4. `INFER_TORCH_RUNTIME.md` — design summary + design-vs-built gap

**File:** `WaaV/inferv2/INFER_TORCH_RUNTIME.md` (241 lines, "Status: design 2026-06-14").

**The design (what it specifies):**
- **A Python torch sidecar** implementing `SttModel`/`TtsModel`, over WS-v1/UDS. **Explicitly NOT PyO3**
  (P-5 crash containment + zero-C/C++ `-core`), **NOT** a new `StaticGraph` (that seam is stateless).
- **"Borrow the patterns, not the frameworks"** — a *minimal* torch runtime using vLLM-Omni/SGLang-Omni's
  *model-definition interface* (3-method class + string registry) and stage ideas, **not** full vLLM.
- **Stack:** torch eager + SDPA + `torch.compile(dynamic=False)` + bucketed manual CUDA graphs.
  **No FlashInfer** (Blackwell-aarch64 regresses on it; batch-1 voice is launch-bound).
- **Two shared seams cover >80%:** (1) Qwen/Llama AR LLM-decoder + per-request KV; (2) codec/vocoder
  decoders (Mimi/Higgs/DAC/AudioVAE/BigVGAN) + the CFM solver already built for Supertonic. Seed with
  ARK-ASR (decoder) + csm-1b (Mimi codec).
- **§10/§10b/§10c/§10d — the production-hardening course-correction (the crux):** a pip-package-as-is
  satisfies NO production requirement. The deployment-form priority is **(1) ONNX/ORT** [full hw matrix,
  one install] → **(2) transformers in ONE shared env** [portable multi-model, CUDA/ROCm/CPU] → **(3)
  ported model class in a shared torch runtime** [vLLM-style shared layers + weights, when ONNX export is
  infeasible] → **(4) per-model venv = LAST RESORT / validation only.** Load the **provider's original
  safetensors** (like vLLM), do **NOT** wrap the provider's inference code (the "MOSS trap"), never depend
  on a vendor-locked runtime (NeMo/NIM). Missing pieces called out to build: **shared torch layers +
  shared codec decoders + a single-env torch model registry (NOT venv-per-model).**

**Is it BUILT? — PARTIALLY, and built at a DIFFERENT tier than the design's preferred one.**

| Design element | Built? | Reality |
|---|---|---|
| Python sidecar at the `SttModel`/`TtsModel` seam | ✅ **YES** | `torch_sidecar.rs` + `engine.rs` torch arm + `torch_runtime/server.py`. |
| Crash-isolation / heartbeat / bounded read (P-5) | ✅ **YES, exceeds design** | full watchdog FSM + reaper + typed-fail gates. |
| Wire protocol | ✅ YES (but **framed stdin/stdout**, *not* the designed WS-v1/UDS) | a simpler, equivalent binary framing. |
| Arch-string runner registry (config+weights, not code) | ✅ YES | `registry.py` mirrors the Rust dispatch; 16 runners. |
| Tier-(2): transformers in ONE shared env, provider weights as-is | ✅ **YES — this is what's actually built** | every runner is `from_pretrained(provider_repo, dtype=...)`. `compat.py` patches transformers gaps so old `trust_remote_code` code loads in the shared env "rather than a per-model venv silo." **No venv configured anywhere.** |
| SDPA pin (no silent math-kernel 40-135× degrade) | ✅ YES | `install_process_sdpa_pin()` process-wide before any load. |
| StaticCache + `torch.compile(dynamic=False)`, **gated on bit-identity** | ✅ YES | `apply_decode_accel` + `PATH_B_DECODE_ACCEL`; auto-reverts to eager for sampling TTS where compile flips a token (#2274). |
| R-5 no-in-loop-host-sync discipline (9 ms lockstep budget) | ✅ YES (as guards/analyzers) | `DecodeLoopGuard` + AST analyzer `find_host_syncs_in_loops`. |
| Tier-(3): **ported model classes onto SHARED torch layers** (the vLLM-style owned runtime) | ❌ **NOT built** | There is **no shared `layers/` lib** (Linear/Attn/RMSNorm/RoPE/Embed), **no shared decode loop**, **no shared codec-decoder lib**. Models drive **HF's own `generate()`** or **vendored upstream code** (`vendor/dia2`, `vendor/dots_tts`, `vendor/vibevoice`, `vendor/qwen3_tts`, `vendor/cosyvoice`). The "two shared seams everything amortizes onto" — the heart of the "vLLM for voice" claim — **does not exist**. Each runner re-drives its own/HF forward. |
| Bucketed manual CUDA graphs | ⚠️ partial | compile path exists; explicit manual CUDAGraph capture/replay buckets are not evident in the runners (the harder latency lever the design names as #1). |
| Owned AR/KV/sampling loop under the WaaV scheduler | ❌ **NOT built** | the AR loop is inside `model.generate()`, opaque to the Rust lockstep/step-bucket scheduler, duty ledger, and prefix-KV router. The sidecar is a black box: `text↔PCM`. |
| Multi-hardware portability (ROCm/Hexagon/CoreML/OpenVINO/NPU) | ❌ **NOT** for Path B | torch sidecar = CUDA/ROCm/CPU only. The design's own answer for portability is *"export to ONNX (tier 1)"* — i.e. portability is delegated to Path A, **Path B is not portable beyond torch's own targets.** |

**The gap in one sentence:** the design's tiers (1) ONNX and (2) transformers-shared-env are built and
live; tier (3) — *the actual owned, portable, shared-layer "vLLM for voice" runtime* — is **designed but
not built**, and what exists for the 14 torch models is tier (2): **a crash-isolated harness around
`transformers.generate()` / vendored upstream code**, which the design itself classifies as "closer to
production than venv, but NOT the final portable answer."

---

## 5. The no-venv hard rule — compliance + what a compliant runtime looks like

**The rule (memory):** "zero per-venv/pip *serving* paths; reimplement the architecture from the repo
portably; venv = throwaway validation only."

**Compliance assessment: PARTIAL — passes the letter on "no venv," fails the spirit on "reimplement
portably; no pip serving path."**

- ✅ **No venv is configured.** No `waav.json` sets `runtime.python`; all spawn `python3` (one shared
  env). `compat.py`'s own comment is explicit: it shims transformers "rather than a per-model venv silo (a
  portability + lock-in regression)." This is the deliberate, correct anti-venv stance. The escape hatch
  (per-model `python`) exists but is unused.
- ⚠️ **But there IS a pip/CPython serving path.** torch + transformers + librosa/soundfile + vendored
  model packages are **runtime serving dependencies** of the sidecar, in-process to the serving request
  path — not "throwaway validation." Under a strict reading of "zero pip serving paths," the torch sidecar
  *is* a pip serving path (a single, shared one — better than venv-per-model, but still a Python serving
  dependency the rule's deeper intent wants reimplemented away).
- ❌ **"Reimplement the architecture from the repo portably" is not done** for the 14 torch models. They
  are served by **driving the upstream framework/weights**, which is the design's tier (2)/(3) boundary —
  precisely the line the charter wants crossed *into* the engine. The "vLLM for voice / own flexible
  execution, not capped by ORT kernels" vision is therefore **not yet realized for non-ONNX models**: WaaV
  does not own their kernels or their decode loop; HF/torch does.

### What a COMPLIANT portable custom runtime would look like (for the 14–23 torch models)

Two viable architectures; the design points at the first, the charter's "portable everywhere" bar argues
for blending in the second:

**Option A — `candle` pure-Rust multi-backend runtime (the strongest no-venv answer).**
- Add `candle-core`/`candle-nn`/`candle-transformers` as the **second native backend** behind a new
  `ArStep`/codec seam in `-backend-*` (parallel to `-backend-ort`), keeping `-core` backend-agnostic (P-8).
- Implement the **two shared seams the design already specs**: (1) a shared AR LLM-decoder + ring/paged-KV
  + sampler in Rust/candle; (2) shared codec/vocoder decoders (Mimi/DAC/BigVGAN) + the CFM solver
  (Supertonic's is already in-tree). Each model becomes a thin config-arch class composing those — the
  vLLM `layers/` + `ModelRegistry` shape, **in Rust**.
- **Portability:** candle backs **CUDA + Metal + CPU (+ WASM)**; this **eliminates the Python serving
  path entirely** and folds the AR loop **into the WaaV lockstep/step-bucket scheduler, duty ledger, and
  prefix-KV router** — i.e. it finally makes these models first-class engine citizens, the actual "vLLM
  for voice." Precedent: the spec's own `moshi-core`/candle reuse note + `parakeet-rs`. **This is the
  rule-compliant target.** Cost: re-implementing each arch's forward in Rust/candle (real work, but the
  seam amortization is exactly what makes it bounded — ARK-ASR/csm seeds, then amortize).
- **Quantization:** candle supports GGUF/Q4/Q8; AWQ/GPTQ via community kernels — covers the multi-quant
  requirement without bitsandbytes-in-Python.

**Option B — embedded `tch`/libtorch (faster to reach, weaker on portability).**
- Link libtorch via `tch` inside a `-backend-torch` crate. Removes the *Python* serving path (no CPython,
  no transformers) but **violates the zero-C/C++ `-core` posture and the P-5 crash-containment rationale**
  (a libtorch CUDA `abort()` is again in-process and uncatchable) — the exact reasons the design rejected
  PyO3. Portability is still CUDA/ROCm/CPU only (no Hexagon/CoreML/OpenVINO). **Not recommended** except as
  a stopgap; it trades the Python dep for an in-process C++ crash surface and keeps the portability ceiling.

**Pragmatic sequencing (what it would actually take):**
1. **Keep the sidecar** as the validated-reference + the bring-up path (it already verifies bit-exact
   against HF in-process — the design's §7 strength). Reclassify it honestly as **tier (2), "portable
   across torch targets only, owned-execution NOT yet."**
2. **Build the two Rust/candle shared seams** (decoder + codec/CFM) behind a new engine seam, seed with
   **ARK-ASR (decoder)** and **csm-1b (Mimi codec)** exactly as the design's P1/P2 say — but in
   Rust/candle, not Python. Verify each against the sidecar (which is the reference).
3. **Amortize** the remaining AR/codec/flow models onto those seams (the design's P3 list). Diffusion
   (vibevoice/omnivoice) needs a third (step-bucket diffusion) seam — the v2 engine already has the
   step-bucket batcher concept to host it.
4. **Prefer ONNX export (tier 1)** wherever a model *can* export — it gives the full hardware matrix for
   free and is the only path to Hexagon/CoreML/OpenVINO/NPU. The portable-everywhere requirement is met by
   ONNX for exportable models + Rust/candle-CUDA/Metal/CPU for the un-exportable AR/codec/diffusion ones;
   **the Python sidecar is retired from the serving path** once a model's Rust seam lands.

---

## Appendix — key files

- **Rust sidecar client + supervisor:** `crates/waav-infer-server/src/torch_sidecar.rs`
- **Live torch wiring (the `load_model_at` torch arm):** `crates/waav-infer-server/src/engine.rs:79-161`
- **Core ONNX-only registry (no torch arm):** `crates/waav-infer-core/src/model.rs:396-...` (16 arms,
  `GraphLoader`/`StaticGraph`; `registry_path_a_invariant_no_backend_named` test enforces purity)
- **Sidecar watchdog / heartbeat FSM:** `crates/waav-infer-runtime/src/watchdog.rs` (SidecarHeartbeat,
  SidecarId, declare_unresponsive — H7/G7/J18)
- **The Python runtime:** `waav-infer/torch_runtime/` — `server.py` (framed protocol), `registry.py`
  (arch→runner), `base.py` (1716 LOC: runners + SDPA pin + decode-accel + host-sync guards + kernel
  discipline), `compat.py` (transformers shim, the "no venv silo" comment), `cli.py`, `models/*.py`
  (16 runners), `vendor/*` (dia2, dots_tts, vibevoice, qwen3_tts, cosyvoice upstream code)
- **Design doc:** `WaaV/inferv2/INFER_TORCH_RUNTIME.md` (esp. §0 decision, §4 internals, §10/§10b/§10c/§10d
  production-hardening + deployment-tier priority)
- **GUI (drives torch models resident, real load/unload):** `waav-infer/gui/app.py`
- **Live env (confirmed):** torch `2.12.0+cu130` (CUDA available), transformers `5.12.0`, Python 3.12
- **Torch model manifests:** `~/.cache/waav-models/{ark-asr-0.6b, granite-speech-4.1-2b, csm-1b-hf,
  dia-1.6b, dia2-2b, dots-tts-{base,soar,mf}, higgs-tts, neutts-air, qwen3-tts-12hz-06b, vibevoice-1.5b,
  omnivoice, cosyvoice3}/waav.json` (14 with `backend:torch`)
