# WaaV Infer — Model Onboarding Workflow

The repeatable, accuracy-gated process for adding a model to WaaV Infer. Goal: every model works
across hardware (EPs) + every quantization it ships, with output **verified identical to the model's
reference inference engine** on a local eval subset, at maximum performance — adding a model is
config (same architecture) or one registry arm + a thin module (new architecture), never engine
surgery.

## Phase 0 — Triage
- Confirm **< 10B params** and an **ONNX path** (`onnx-community/*` export, or a clean exporter). WaaV
  runs ONNX via the `StaticGraph` seam; pure-PyTorch/NeMo/GGUF/LLM models need an ONNX export first.
- Identify the **execution-path archetype**: STT-B (attention enc-dec: whisper/moonshine), STT-A
  (frame-sync CTC/RNNT/TDT: parakeet/nemotron), TTS-P3 (one-shot: kokoro), TTS-P1 (codec-LM AR),
  TTS flow-matching (supertonic/cosyvoice). Map to an existing shared component or note a new one.
- Identify the **reference engine** (what HF says the model runs on): transformers, faster-whisper,
  NeMo, sherpa-onnx, the model's own repo. This is the accuracy oracle.

## Phase 1 — Acquire
- `hf download <repo> <onnx files> config.json generation_config.json tokenizer.json preprocessor_config.json`
- Build a model dir; add `waav.json` for precision/weights when selecting a quant:
  `{"precision":"fp16"|"int8dyn"|...}` or `{"weights":{"encoder":"onnx/..."}}` or `{"architecture":"..."}`.

## Phase 2 — Integrate (minimal-engine-change)
- **Same architecture** (any size/variant/lang/quant): zero code — `config.json` dispatch handles it.
  Anything model-specific that the engine hardcodes (mel bins, special tokens, sample rate, …) is a
  bug → make it **config-driven** (read from the model's config), generalizing all variants at once.
- **New architecture**: add ONE arm in `core/src/model.rs` + a thin module that composes shared
  components (mel/raw frontend, `encdec` AR loop, KV cache, sampler, tokenizer, codec decoder,
  vocoder…). Add only the genuinely-new component (e.g. a new tokenizer, a CTC collapse, a CFM
  solver). Register it in `REGISTERED_ARCHITECTURES`.

## Phase 3 — Smoke (live, real model, GB10)
- `waav-infer transcribe <wav> --whisper-dir <dir>` (STT) / `waav-infer run "<text>" --out o.wav` (TTS).
- Must produce sane output on a known clip (e.g. `research/engines/whisper.cpp/samples/jfk.wav`).

## Phase 4 — Accuracy gate (vs reference engine)
- STT: `python waav-infer/eval/stt_eval.py --model-dir <dir> --ref-model <hf_id> --n 16 --ep cuda`
  - Runs WaaV CLI + the transformers reference on `librispeech_asr_dummy`, whisper-normalizes both,
    computes WER vs ground truth for each + WaaV-vs-ref disagreement.
  - **PASS = `|WER_waav − WER_ref| ≤ tolerance`** (default 3%); aim for **0.00% disagreement**
    (byte-identical) on clean speech — ONNX vs PyTorch numerics are tiny.
- TTS: speaker-sim / UTMOS-proxy deltas vs reference within bounds (harness TBD per model).
- Run for **every quantization** the model ships (fp32 / fp16 / int8 …) and **every EP** (cuda, cpu).

## Phase 5 — Performance
- Record RTF (and TTFA for TTS) per (model × precision × EP). Pick the fastest correct precision as
  the default; surface "not realtime-rated on this device" honestly.

## Phase 6 — Review + land
- Brutal-review the new module/changes (Workflow), fix findings, re-verify accuracy, full test suite
  green + clippy clean, update the catalog. → next model.

## Notes / gotchas discovered
- Mel bins are config-driven (`feature_size`): 80 for whisper tiny/base/small/medium, **128** for
  large-v3 / turbo. `LogMelExtractor::new(n_mels)`.
- Reference pipeline: pass the raw array (`{"raw": arr, "sampling_rate": sr}`), not a file path, to
  avoid an ffmpeg dependency.
- Quant exports using `MatMulNBits` (onnx-community whisper int8/quantized) fail in the pinned ORT
  build on both CPU+CUDA — an upstream export/ORT issue; use fp16 or ORT-dynamic-int8 instead.
