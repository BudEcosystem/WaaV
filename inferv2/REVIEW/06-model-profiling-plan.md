# Phase-C Model Profiling Plan — WaaV Infer (this GB10 box)

**Generated:** 2026-06-21 (READ-ONLY inventory; no models loaded, no cargo run).
**Scope:** every model runnable by the *Rust ORT engine* on THIS box + how to real-inference, accuracy-check, and perf-profile each.
**Box:** GB10 / aarch64 (sbsa), 121 GB **unified** CPU+GPU pool (shared — concurrent build + live GPU test can OOM-crash the box; see guardrails §C).

---

## 0. How dispatch + profiling works (the seams)

- **Registry dispatch** (`crates/waav-infer-core/src/model.rs::load_model`): a model dir is `config + weights`. The arch key is `waav.json.architecture` (override) → else `config.json` `architectures[0]` / `model_type`. There are **16 registered architectures** (`REGISTERED_ARCHITECTURES`), each one `match` arm composing shared components. Size/variant/quant of a registered arch needs **zero** code change — weights resolve via `waav.json.weights{}` overrides or the `onnx/{stem}_{precision}.onnx` convention.
- **Precision/quant gate** (`admit_quant_variant`): a lossy quant (int8/q4/q4f16/…) is **never** default-admitted; it loads only behind a **passing per-substrate `QuantStamp`** in `waav.json`, else degrades to fp32. fp32/fp16 are the reference (no stamp).
- **CLI** (`crates/waav-infer-server/src/bin/waav_infer.rs`): one-shot real inference.
  - STT: `waav-infer transcribe <audio> --whisper-dir <model_dir> [--lang auto] [--ep cuda]` (the `--whisper-dir` flag loads **any** registered STT arch via the registry, not just whisper).
  - TTS: `waav-infer run "<text>" --tts-dir <model_dir> [--voice <v>] [--speed 1.0] --out out.wav [--ep cuda]`.
  - Cascade: `waav-infer run-dag <audio> --stt-dir <d> --tts-dir <d> --out dag_out.wav`.
  - Diarize/enhance: `waav-infer diarize|enhance` (separate ONNX paths, not in the 16-arch STT/TTS registry).
- **Accuracy harnesses** (`eval/`): drive the CLI over a real dataset, compare to ground-truth **and** a reference engine. 5 harnesses (§B).
- **Perf harness** (`crates/waav-infer-server/tests/perf_bench.rs`): drives the public `Engine` seams (`transcribe`/`synthesize`/`serve_codec_ar_streams`); captures **TTFT, per-frame p50/p95/p99, RTF, throughput (audio-s/s & streams/s), concurrency ramp N=1..24, peak-mem via `free -g`**. Covers only 4 models today (whisper/kokoro/chatterbox/supertonic).
- **Live test runner** (`ci/heavy_live_tests.sh`): each live-GPU gate runs **process-isolated** (`cargo test … --include-ignored --test-threads=1`, `timeout -k 30 1800`) because every live test `mem::forget`s its CUDA session (GB10 ORT-CUDA `Drop` SIGABRTs) → leaks accumulate → OOM if co-run.

---

## A. PER-MODEL PROFILING TABLE

### A1. STT — RUNNABLE NOW (weights present, registered arm)

| model | task | arch (arm) | precision | weights present? (size) | accuracy check (script + dataset + reference) | live inference cmd (exact, GB10) | perf metrics | concurrency test |
|---|---|---|---|---|---|---|---|---|
| **whisper-tiny.en** (onnx-community) | STT | `whisper` (encdec) | fp32 | ✅ enc 32M + dec-merged 114M (**147M**) | `eval/dataset_wer.py` + `eval/stt_eval.py`; LibriSpeech-dummy (73 clips, cached); ref=transformers `openai/whisper-tiny.en` **and** plain-onnxruntime. **Baseline: WER 9.67%, 0.00% vs-ref disagreement (73/73 byte-identical).** | `waav-infer transcribe <wav> --whisper-dir ~/.cache/huggingface/hub/models--onnx-community--whisper-tiny.en/snapshots/*/ --ep cuda` | **YES** — `perf_bench::bench_whisper_stt_ttft_throughput_concurrency` + `whisper_ragged_concurrent_batched_bit_identical_and_scales` (TTFT p50/95/99, RTF, ramp N=1..16, batched==per-slot) | YES (ragged batched == per-slot, scaling>1.15×) |
| whisper-base (onnx-community) | STT | `whisper` | fp32/fp16 | ✅ enc 79M/40M + dec 199M/100M (**2.1G** all variants) | `eval/dataset_wer.py` (no ref-engine transcripts yet → GAP for vs-ref parity); ref=transformers `openai/whisper-base` via stt_eval.py | `waav-infer transcribe <wav> --whisper-dir …/whisper-base/snapshots/*/ --ep cuda` | reuse perf_bench whisper harness (set `WHISPER_DIR`) — **no dedicated test** | inherited from whisper arm (batched seam) |
| whisper-large-v3 (onnx-community) | STT | `whisper` | fp16 | ✅ enc 1.2G + dec 1.7G (**2.9G**) | stt_eval.py ref=`openai/whisper-large-v3`; dataset LibriSpeech-dummy | `WAAV_PRECISION=fp16 waav-infer transcribe <wav> --whisper-dir …/whisper-large-v3-ONNX/snapshots/*/ --ep cuda` | reuse perf_bench whisper harness — **no dedicated test** | inherited |
| whisper-large-v3-turbo (onnx-community) | STT | `whisper` | fp16 | ✅ enc 1.2G + dec 329M (**5.2G** all variants) | stt_eval.py ref=`openai/whisper-large-v3-turbo` | `WAAV_PRECISION=fp16 waav-infer transcribe <wav> --whisper-dir …/whisper-large-v3-turbo/snapshots/*/ --ep cuda` | reuse perf_bench whisper — **no dedicated test** | inherited |
| **moonshine-base** (onnx-community) | STT | `moonshine` (encdec) | fp32 | ✅ enc 78M + dec 159M (**240M**) | **GAP — no dedicated harness.** Use `eval/dataset_wer.py` (dataset-WER) or the in-tree live test below. Live test: `whisper_live::moonshine_transcribes_synthesized_speech` (closed-loop, ≥5 content words) | `waav-infer transcribe <wav> --whisper-dir …/moonshine-base-ONNX/snapshots/*/ --ep cuda` | **GAP — none** | **GAP — none** |
| **parakeet-tdt-0.6b-v2** (istupakov) | STT | `nemo-conformer-tdt` (parakeet) | fp32 (int8 avail) | ✅ enc 42M + dec_joint 36M + int8 enc 652M (**3.0G** all) | `eval/parakeet_eval.py --arch nemo-conformer-tdt`; LibriSpeech-dummy; ref=`onnx_asr` (same istupakov ONNX); PASS=`|WER_waav−WER_ref|≤0.02` | `waav-infer transcribe <wav> --whisper-dir ~/.cache/huggingface/hub/models--istupakov--parakeet-tdt-0.6b-v2-onnx/snapshots/*/ --ep cuda` | **GAP — none** | **GAP — none** |
| parakeet-tdt-0.6b-v3 (istupakov) | STT | `nemo-conformer-tdt` | fp32/int8 | ✅ enc 42M + dec_joint 73M (**3.0G**) | parakeet_eval.py `--arch nemo-conformer-tdt` (8k multilingual vocab) | same cmd, v3 snapshot dir | GAP | GAP |
| parakeet-rnnt-0.6b (istupakov) | STT | `nemo-conformer-rnnt` (parakeet) | fp32/int8 | ✅ enc 42M + dec_joint 36M (**3.0G**) | parakeet_eval.py `--arch nemo-conformer-rnnt` | same cmd, rnnt snapshot dir | GAP | GAP |
| parakeet-ctc-0.6b (istupakov) | STT | `nemo-conformer-ctc` (nemo_ctc) | fp32 (int8 653M) | ✅ model 42M (**3.0G** w/ int8) | parakeet_eval.py `--arch nemo-conformer-ctc` | same cmd, ctc snapshot dir | GAP | GAP |
| nemotron-en RNNT (waav-models) | STT | `nemo-conformer-rnnt` | fp32 | ✅ enc + dec_joint + nemo128 (**2.4G**) | parakeet_eval.py `--arch nemo-conformer-rnnt` (English FastConformer) | `waav-infer transcribe <wav> --whisper-dir ~/.cache/waav-models/nemotron-en --ep cuda` | GAP | GAP |
| fastconformer-quran (waav-models) | STT | `nemo-conformer-rnnt` | fp32 | ✅ enc + dec_joint + nemo80 (**456M**) | GAP — Arabic/Quran, no LibriSpeech ground truth; use round-trip or domain set | `waav-infer transcribe <wav> --whisper-dir ~/.cache/waav-models/fastconformer-quran --ep cuda` | GAP | GAP |
| **canary-180m-flash** (istupakov) | STT | `nemo-conformer-aed` (canary) | fp32/int8 | ✅ enc 463M + dec 316M (**947M**) | **GAP — no dedicated harness** (AED multilingual ASR+translation). Use dataset_wer.py for EN | `waav-infer transcribe <wav> --whisper-dir ~/.cache/huggingface/hub/models--istupakov--canary-180m-flash-onnx/snapshots/*/ --ep cuda` | GAP | GAP |
| canary-1b-v2 (istupakov) | STT | `nemo-conformer-aed` | fp32/int8 | ✅ enc 4.7M(fp32)+859M(int8) + dec 676M (**4.7G**) | GAP | same cmd, canary-1b snapshot | GAP | GAP |
| **cohere-transcribe-03-2026** (onnx-community) | STT | `cohere_asr` (cohere) | fp16 | ✅ enc+dec fp16 + nemo128 (**3.9G**) | **GAP — no dedicated harness.** dataset_wer.py for EN | `WAAV_PRECISION=fp16 waav-infer transcribe <wav> --whisper-dir ~/.cache/huggingface/hub/models--onnx-community--cohere-transcribe-03-2026-ONNX/snapshots/*/ --ep cuda` | GAP | GAP |
| **sense-voice** (csukuangfj sherpa) | STT | `sense_voice_ctc` (sensevoice) | int8 | ✅ model.int8 (**229M**) | `eval/sensevoice_eval.py --sherpa-dir <dir>`; LibriSpeech-dummy; ref=sherpa-onnx (knf-fbank+LFR+CMVN+CTC); PASS=`|ΔWER|≤0.03`. (multilingual zh/en/ja/ko/yue, int-native lang) | `waav-infer transcribe <wav> --whisper-dir ~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-sense-voice-…/snapshots/*/ --lang en --ep cpu` | GAP | GAP |
| **funasr-nano** (waav-models) | STT | `funasr_nano` (LLM-decoder) | int8 | ✅ enc 227M + emb 149M + llm 573M (**973M**); has `test_wavs/` (28 clips) | **GAP — no dedicated harness.** dataset_wer.py for EN; or its own `test_wavs/` (zh/ja/en/yue, no GT) | `waav-infer transcribe <wav> --whisper-dir ~/.cache/waav-models/funasr-nano --ep cuda` | GAP (stepped/AR seam exists) | GAP |
| **voxtral-realtime** (waav-models) | STT | `voxtral_realtime` (LLM-decoder, lockstep) | q4f16 | ✅ enc 559M + dec 1.9G + emb 222M onnx_data (**11G** all variants) | **dedicated proof exists (banked):** voxtral int8 == plain-onnxruntime int8 **byte-identical** on 3 LibriSpeech clips (0.0000 WER disagreement). No reusable .py in `eval/`; gate is in onboarding notes. dataset_wer.py works for re-run | `waav-infer transcribe <wav> --whisper-dir ~/.cache/waav-models/voxtral-realtime --ep cuda` (q4f16 needs graph-driven KV dtype on CUDA — see memory note) | GAP (stepped/AR seam exists) | GAP |

### A2. TTS — RUNNABLE NOW (weights present, registered arm)

| model | task | arch (arm) | precision | weights present? (size) | accuracy check (intelligibility / equivalence) | live inference cmd (exact, GB10) | perf metrics | concurrency test |
|---|---|---|---|---|---|---|---|---|
| **Kokoro-82M** (onnx-community) | TTS | `kokoro`/`style_text_to_speech_2` | fp32 | ✅ model.onnx (**311M**, total dir 312M) | `eval/tts_roundtrip.py --tts-dir … --asr-dir <whisper>` (round-trip WER, 6 sentences). Live: `kokoro_live::kokoro_synthesizes_real_audio` + `whisper_live` closed-loop (bit-faithful vs python ONNX ref, durations 1..16). **NOTE: kokoro pinned to CPU EP** (CUDA LSTM divergence → 34GB OOM; RTF~0.17 on CPU) | `waav-infer run "The quick brown fox." --tts-dir ~/.cache/huggingface/hub/models--onnx-community--Kokoro-82M-v1.0-ONNX/snapshots/*/ --voice af_heart --out k.wav --ep cpu` | **YES** — `perf_bench::bench_kokoro_tts_first_audio_concurrency` (first-audio p50/95/99, RTF, ramp N=1..16) | YES (concurrency ramp); note: graph batch-pinned to 1, no batched override |
| **Supertonic-3** (Supertone) | TTS | `supertonic` (flow-matching) | fp32 | ✅ dur 3.6M + text 35M + vector 245M + vocoder 97M (**383M**) + 10 voice_styles (M1-5/F1-5) | `eval/supertonic_eval.py --ref-py /tmp/supertonic_ref/py --turbo-dir <whisper-turbo>`: (1) **numerical equivalence** vs ref ONNX with replayed noise (`$WAAV_TTS_NOISE_FILE`, maxΔ≤2e-2, corr≥0.999 — **bit-identical: maxΔ=0.0000 banked**); (2) transcribe-back WER. Also tts_roundtrip.py | `waav-infer run "The quick brown fox." --tts-dir ~/.cache/huggingface/hub/models--Supertone--supertonic-3/snapshots/*/ --voice M1 --out s.wav --ep cuda` | **YES** — `perf_bench::tts_oneshot_concurrent_synthesize_deserializes_and_bit_identical` (ragged batched==per-request sample-for-sample, scaling>1.2×) | YES (ragged concurrent batched bit-identical + scales) |
| **chatterbox** (onnx-community/waav-models) | TTS | `chatterbox` (codec-AR) | fp32 (fp16/q4/q4f16 avail) | ✅ speech_enc 591M + lang_model 2.1G + cond_dec 534M + embed 62M onnx_data (**4.7G**) + `default_voice.wav` | `eval/tts_roundtrip.py` (round-trip WER). NOTE: S3Gen decoder **non-causal** ⇒ TTFA == whole-body. Bit-identity proven via ragged-batch gates (live) | `CHATTERBOX_DIR=~/.cache/waav-models/chatterbox-onnx waav-infer run "Hello." --tts-dir ~/.cache/waav-models/chatterbox-onnx --out c.wav --ep cuda` | **YES** — `perf_bench::bench_chatterbox_codec_ar_full` (TTFT, per-frame p50/95/99, batched ramp N=1..24, max-concurrency-RTF<1≥4) | YES (lockstep batcher: `live_ragged_batched_forward_bit_identical_and_scales`, ~1.8× @ B=8) |
| chatterbox-turbo (waav-models) | TTS | `chatterbox` | fp32 (+ fp16/q4/q4f16/quantized) | ✅ cond_dec 1.9M + lang 203K + speech_enc 1.2M (**6.9G** all variants) | tts_roundtrip.py (reuses chatterbox arm) | `CHATTERBOX_DIR=~/.cache/waav-models/chatterbox-turbo-onnx waav-infer run "Hello." --tts-dir ~/.cache/waav-models/chatterbox-turbo-onnx --out ct.wav --ep cuda` | reuse chatterbox perf harness (set `CHATTERBOX_DIR`) — same arm | inherited (same lockstep batcher) |
| **MeloTTS-English** (csukuangfj/waav-models) | TTS | `melo_vits` (VITS) | fp32 | ✅ model.onnx + tokens.txt (**168M**) | **GAP — no dedicated harness.** tts_roundtrip.py (round-trip WER) | `waav-infer run "The quick brown fox." --tts-dir ~/.cache/waav-models/melo-tts-en --out m.wav --ep cuda` | **GAP — none** | **GAP — none** |

### A3. S2S / Native-duplex — arm exists, NO real model weights present

| model | task | arch (arm) | weights present? | accuracy / live / perf |
|---|---|---|---|---|
| (synthetic duplex) | S2S | `s2s::duplex_codec_ar::CodecArDuplexModel` (`DuplexStepModel` seam) | **NO discrete S2S model dir** — the arm is exercised by `s2s_duplex_ragged_concurrent_batched_bit_identical_and_scales` using the **chatterbox codec-AR backbone** as the duplex backbone (live, in `heavy_live_tests.sh`). The torch-backend S2S dirs (csm-1b, dia, kyutai-mimi) are **NOT runnable in the Rust engine** (§A4). | Live: the duplex SlotBatch gate (ragged==per-slot + latency). Perf: covered inside that gate. **No standalone S2S model to profile in Phase C via the Rust engine.** |

### A4. PRESENT ON DISK but NOT runnable by the Rust ORT engine (excluded from Phase C profiling)

These have `waav.json` with `"runtime":{"backend":"torch",…}` (or `status: blocked`) — there is **no registered Rust arm** for their architecture; they are torch-sidecar/Path-B designs (see `INFER_TORCH_RUNTIME.md`), not ORT-engine models. **Do NOT attempt CLI inference** (registry returns `unsupported architecture`).

`ark-asr-0.6b` (Ark/torch), `canary-qwen-2.5b` (blocked-SALM), `granite-speech-4.1-2b` (torch), `cosyvoice3` (torch, 9.1G), `csm-1b-hf` (torch, 6.7G), `dia-1.6b` (torch, 6.1G), `dia2-2b` (torch, 7.2G), `dots-tts-base/-mf/-soar` (torch, 4.9G ea), `higgs-tts` (torch), `neutts-air` (torch, 1.5G), `omnivoice` (torch, 3.1G), `qwen3-tts-12hz-06b` (torch, 2.4G), `vibevoice-1.5b` (torch, 5.1G). **Status:** future torch-runtime / portable-reimplement targets, NOT Phase-C ORT profiling.

### A5. Diarize / Enhance (separate ONNX CLI paths, not in the 16-arch registry — optional Phase-C add-on)

| model | task | weights present? | live cmd | accuracy |
|---|---|---|---|---|
| pyannote-community-1 (waav-models) | diarize | ✅ seg 8M + emb 24M (**32M**) | `waav-infer diarize <wav> --seg-model ~/.cache/waav-models/pyannote-community-1/segmentation-community-1.onnx --emb-model …/embedding_model.onnx --ep cuda` | `eval/dia_testset.json` (10 scripted dialogs, no audio); onboarding-verified on jfk+kokoro 2-spk clip |
| dpdfnet2/4/8, clear, tse-tasnet, neucodec-onnx, kyutai-mimi-onnx | enhance/codec | ✅ (9.8M–2.2G) | `waav-infer enhance <wav> --model <onnx> --ep cuda` | GAP — no WER-style harness; PESQ/STOI not wired |

---

## B. ACCURACY-HARNESS COVERAGE (the `eval/` corpus)

| harness | models it serves | dataset (location) | reference engine | gate |
|---|---|---|---|---|
| `eval/dataset_wer.py` | **ANY** STT arm (CLI-driven, dependency-light) | LibriSpeech-dummy clean/validation, 73 clips — **CACHED** at `~/.cache/huggingface/{datasets,hub/datasets--}hf-internal-testing--librispeech_asr_dummy` | optional `--ref-transcripts <json>` (e.g. plain-onnxruntime) | WER ≤ expected+tol AND vs-ref disagreement ≤ ref-tol |
| `eval/stt_eval.py` | whisper-class (transformers ref) | same LibriSpeech-dummy | transformers `pipeline(ASR, <hf_id>)` | `|WER_waav−WER_ref| ≤ 0.03` |
| `eval/parakeet_eval.py` | nemo tdt/rnnt/ctc (parakeet/nemo_ctc) | same LibriSpeech-dummy | `onnx_asr` (same istupakov ONNX) | `|ΔWER| ≤ 0.02` |
| `eval/sensevoice_eval.py` | sense_voice_ctc | same LibriSpeech-dummy | sherpa-onnx pipeline (knf-fbank+LFR+CMVN+CTC) | `|ΔWER| ≤ 0.03` |
| `eval/supertonic_eval.py` | supertonic | 6 built-in sentences | ref ONNX (`/tmp/supertonic_ref/py`, **must rebuild**) + whisper-turbo transcribe-back | maxΔ≤2e-2, corr≥0.999, |ΔWER|≤3% |
| `eval/tts_roundtrip.py` | **ANY** TTS arm | 6 built-in sentences | a WaaV STT model (round-trip) | low round-trip WER (reporting) |

**Accuracy-harness GAPS (arms with NO dedicated harness):** `moonshine`, `canary` (AED), `cohere_asr`, `funasr_nano`, `voxtral_realtime` (banked proof but no reusable `.py`), `melo_vits`. → For STT, `dataset_wer.py` covers all of them on English LibriSpeech (no per-model ref engine, but a real-dataset WER no-regression number). For TTS, `tts_roundtrip.py` covers melo.

---

## C. EXECUTION ORDER + MEMORY-SAFE COMMANDS (the Phase-C runbook)

**Hard invariants (from MEMORY: GB10 unified-pool OOM scars):**
1. **Never** run a release build concurrently with a live GPU test (both draw the same 121 GB pool → box crash). A regression build is currently in flight — **wait for it**.
2. Always `source gb10-env.sh` first (sets `ORT_DYLIB_PATH`, `LD_LIBRARY_PATH`, `CARGO_BUILD_JOBS=6`, `RUST_TEST_THREADS=4`).
3. Every live model test runs **`--test-threads=1`**, **process-isolated** (one `cargo test` invocation per gate), **`timeout -k 30 1800`-wrapped** — exactly the `ci/heavy_live_tests.sh` pattern (it leaks each CUDA session by design).
4. CLI one-shots (`transcribe`/`run`) are single-instance + `process::exit(0)` → flat-bounded; safe to run back-to-back. Accuracy harnesses spawn the CLI per clip → also safe (one model instance at a time).
5. Watch `free -g` between heavy gates; abort if `avail` < ~25 GB.

**Recommended order (smallest/fastest first, grouped so peak resident never stacks):**

**Group 1 — tiny ONNX STT/TTS, accuracy via CLI harness (lowest mem, fastest, highest signal):**
```bash
source /home/bud/ditto/waav/waav-infer/gb10-env.sh
cd /home/bud/ditto/waav/waav-infer
# build the CLI ONCE (debug, low mem) BEFORE any live test — never during one:
cargo build -p waav-infer-server --bin waav-infer
# whisper-tiny.en (147M) — the anchored baseline (expect WER 9.67%, 0 vs-ref disagreement):
HF_HUB_OFFLINE=1 timeout 1200 python3 eval/dataset_wer.py \
  --whisper-dir ~/.cache/huggingface/hub/models--onnx-community--whisper-tiny.en/snapshots/*/ \
  --ep cuda --expected-wer 0.0967 --tolerance 0.005
# sense-voice (229M, int8, CPU) — sherpa ref:
timeout 1200 python3 eval/sensevoice_eval.py \
  --waav-dir ~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-sense-voice-*/snapshots/*/ \
  --sherpa-dir ~/.cache/huggingface/hub/models--csukuangfj--sherpa-onnx-sense-voice-*/snapshots/*/
# moonshine-base (240M) — dataset WER (no ref engine):
HF_HUB_OFFLINE=1 timeout 1200 python3 eval/dataset_wer.py \
  --whisper-dir ~/.cache/huggingface/hub/models--onnx-community--moonshine-base-ONNX/snapshots/*/ --ep cuda
# melo-tts-en (168M) + Kokoro (312M, CPU) — TTS round-trip:
timeout 1200 python3 eval/tts_roundtrip.py --tts-dir ~/.cache/waav-models/melo-tts-en \
  --asr-dir ~/.cache/huggingface/hub/models--onnx-community--whisper-tiny.en/snapshots/*/ --ep cuda
```

**Group 2 — NeMo transducer/CTC/AED family (~0.5–4.7G, share `onnx_asr` ref):**
```bash
# parakeet trio + nemotron-en + canary (run each separately; CLI per-clip = bounded):
for arch in nemo-conformer-tdt nemo-conformer-rnnt nemo-conformer-ctc; do
  timeout 1200 python3 eval/parakeet_eval.py --arch $arch \
    --waav-dir <matching istupakov snapshot dir> --ep cuda --n 16
done
# canary-180m-flash / cohere-transcribe (no ref) → dataset_wer.py on English.
```

**Group 3 — large STT (fp16, 2.9–11G; one at a time, watch `free -g`):**
whisper-large-v3, whisper-large-v3-turbo, voxtral-realtime (q4f16). `WAAV_PRECISION=fp16 … dataset_wer.py`. For voxtral q4f16-on-CUDA, the KV-cache empty-tensor dtype must be graph-driven (banked design note) — validate on CPU first if CUDA dtype seam not yet landed.

**Group 4 — LIVE PERF + concurrency (process-isolated, the OOM-sensitive group; run LAST, never during a build):**
```bash
source /home/bud/ditto/waav/waav-infer/gb10-env.sh
cd /home/bud/ditto/waav/waav-infer
# the canonical isolated runner (each gate its own process, timeout-wrapped, --test-threads=1):
ci/heavy_live_tests.sh
# covers: whisper STT TTFT+ramp, whisper ragged-batched, kokoro first-audio+ramp,
#         chatterbox codec-AR (TTFT+per-frame pctiles+ramp N=1..24), supertonic one-shot concurrent,
#         duplex-S2S ragged, the live serving-path batcher gates.
# OR run a single model's perf gate isolated, e.g.:
timeout -k 30 1800 cargo test -p waav-infer-server --test perf_bench \
  bench_kokoro_tts_first_audio_concurrency -- --exact --include-ignored --test-threads=1 --nocapture
```

**Phase-C PERF coverage TODO (models with weights but NO perf/concurrency test — the gaps to add to `perf_bench.rs`):** moonshine, parakeet (tdt/rnnt/ctc), nemotron-en, canary, cohere, sense-voice, funasr-nano, voxtral-realtime, melo. The `perf_bench` harness only loads via `resolve_whisper_dir`/`resolve_kokoro_dir`/`CHATTERBOX_DIR`/`SUPERTONIC_DIR`; adding a model = point an Engine at its dir + the same TTFT/RTF/ramp scaffold (no per-model fork — the `Engine::transcribe`/`synthesize` seams are generic).

---

## D. DATASETS — present vs MISSING

| dataset | used by | status |
|---|---|---|
| **hf-internal-testing/librispeech_asr_dummy** (clean/validation, 73 clips, 16kHz + GT) | dataset_wer / stt_eval / parakeet_eval / sensevoice_eval | ✅ **CACHED** (`~/.cache/huggingface/datasets/…` + `…/hub/datasets--…`) — works offline (`HF_HUB_OFFLINE=1`) |
| openslr/librispeech_asr (full dev-clean) | (optional larger WER) | ⚠️ cache dir exists (`~/.cache/huggingface/hub/datasets--openslr--librispeech_asr`) but **`.no_exist` marker present** — likely metadata only, full audio NOT downloaded. Verify before relying on it; the 73-clip dummy is the working set. |
| supertonic ref engine `py/helper.py` (`/tmp/supertonic_ref/py`) | supertonic_eval | ❌ **MISSING** (`/tmp` is ephemeral) — must rebuild from the Supertone repo before the numerical-equivalence gate. The voice_styles JSONs ARE present in the model snapshot. |
| `eval/dia_testset.json` (10 scripted dialogs, text-only) | diarize/dialog smoke | ✅ present (no audio — synthesize via TTS) |
| built-in sentence sets | supertonic_eval / tts_roundtrip | ✅ inlined in the scripts |
| `assets/kokoro_m1_sample.wav` (the perf-bench probe audio) | perf_bench (whisper/chatterbox/supertonic input) | ✅ present (578K) |
| funasr-nano `test_wavs/` (28 zh/ja/en/yue clips, no GT) | funasr smoke | ✅ present (intelligibility-only, no WER GT) |

**No new downloads required for Group 1–3 English accuracy** (LibriSpeech-dummy cached). **One rebuild needed:** the Supertonic reference `py/helper.py` for the equivalence gate (intelligibility gate works without it).

---

## E. SUMMARY (for the caller)

**Total models RUNNABLE NOW on this box (weights present + registered Rust ORT arm): 22.**
- **STT = 15:** whisper-{tiny.en, base, large-v3, large-v3-turbo}, moonshine-base, parakeet-{tdt-v2, tdt-v3, rnnt, ctc}, nemotron-en(RNNT), fastconformer-quran(RNNT), canary-{180m-flash, 1b-v2}, cohere-transcribe, sense-voice, funasr-nano, voxtral-realtime → **(that's 17 dirs across the 8 STT arms `whisper/moonshine/nemo-tdt/nemo-rnnt/nemo-ctc/aed/cohere/sensevoice/funasr/voxtral`; count 15 distinct named STT models + 2 extra nemo dirs).**
- **TTS = 6:** Kokoro-82M, Supertonic-3, chatterbox, chatterbox-turbo, MeloTTS-English (+ chatterbox is also the S2S-duplex backbone).
- **S2S = 0 standalone** (duplex arm proven via the chatterbox backbone; no discrete S2S ORT model — the csm/dia/kyutai dirs are torch-backend, not runnable).
- **+ on disk but NOT ORT-runnable: 14 torch-backend dirs** (cosyvoice3, csm, dia/dia2, dots-tts×3, vibevoice, qwen3-tts, omnivoice, neutts-air, higgs, granite, ark, canary-qwen) — future torch-runtime targets, excluded from Phase C.
- **+ diarize/enhance: pyannote-community-1 + 6 enhance ONNX** (separate CLI paths, optional add-on).

**Accuracy-harness coverage:** 5 reference-grade harnesses cover **whisper, parakeet/nemo-CTC, sense-voice, supertonic**; `dataset_wer.py`/`tts_roundtrip.py` give real-dataset WER for **any** STT/TTS arm. **Dedicated-harness GAPS:** moonshine, canary(AED), cohere, funasr-nano, voxtral(banked but no .py), melo — all coverable today by the generic harnesses on English LibriSpeech.

**Perf/concurrency-test coverage:** only **4 of 22** (whisper, kokoro, chatterbox, supertonic) have a live `perf_bench` gate (TTFT/RTF/percentiles/ramp/peak-mem). **GAP = 18 models** with no perf test — the Phase-C build item is to extend `perf_bench.rs` (generic `Engine` seam, no per-model fork) to moonshine, the parakeet/nemo family, canary, cohere, sense-voice, funasr-nano, voxtral, melo.

**Recommended Phase-C order:** **Group 1** tiny ONNX (whisper-tiny→sense-voice→moonshine→melo/kokoro accuracy, lowest mem, highest signal) → **Group 2** NeMo transducer/CTC/AED family (parakeet trio + nemotron + canary/cohere) → **Group 3** large fp16 STT one-at-a-time (whisper-large×2, voxtral-q4f16, watch `free -g`) → **Group 4** live perf+concurrency LAST via `ci/heavy_live_tests.sh` (process-isolated, timeout-wrapped, `--test-threads=1`). **Build the CLI once up front; never co-run a build with a live GPU test** (unified-pool OOM). Only missing artifact: rebuild the Supertonic reference `py/helper.py`; all English accuracy datasets are cached.
