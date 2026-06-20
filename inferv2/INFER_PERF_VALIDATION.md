# WaaV Infer v2 — Performance + Accuracy Validation

**Status:** FAIL (perf) / PASS (accuracy) → **overall verdict: NOT YET (pass=false)**
**Platform:** NVIDIA GB10 (sm_121, compute_cap 12.1), ORT CUDA EP, all measurements LIVE on GPU
**Commit:** `abb1c10` (HEAD)
**Date:** 2026-06-20
**Harness:** `crates/waav-infer-server/tests/perf_bench.rs` (new), perf-lever A/B in `crates/waav-infer-backend-ort/src/lib.rs::perf_lever_run_bound_vs_run_on_real_chatterbox_lm_cuda`, `eval/dataset_wer.py` (new), plus re-run of existing live tests (`cascade_live`, `gb10_serves_16_concurrent`, `server_live`).

---

## 1. Summary verdict

The v2 lockstep engine + production spine **serves all three real registry models correctly at RTF < 1 with ≥ 4 concurrent streams** on live GB10, and **accuracy is intact end-to-end** (whisper-tiny.en proven byte-identical to a reference engine over 73 LibriSpeech-dummy clips, all four named bit-exact gates green). That is the load-bearing correctness result, and it holds.

**But the two headline performance-*improvement* theses do not reproduce on the real ONNX path:**

1. **Batched-forward throughput scaling (the "55×@64" lockstep thesis)** — **does NOT materialize.** Concurrency scaling on the real chatterbox codec-AR path is **1.00×**: wall-clock grows linearly with N and per-stream throughput is flat. The system serves 24 concurrent streams *correctly*, but throughput does not scale with concurrency, because the runtime steps each slot as a separate batch-1 `StaticGraph::run` — there is no batch-dim-N forward across slots.
2. **IoBinding KV-residency (`run_bound`, billed the "#1 engine perf change")** — **net-negative on the real graph.** On the real chatterbox 30-layer Llama LM, `run_bound` is 0.988× at empty-KV (noise) and **0.775× (29% *slower*)** at KV-depth 200, because the chatterbox feedback loop host-materializes the present-KV outputs every stride, immediately undoing the keep-on-device request.

Per the strict acceptance criterion — *every measured metric improved-or-no-regression* — these two flat/negative levers make **perf.pass = false, honestly.** No metric was fabricated and no regression was hidden. Accuracy passes cleanly. The combined verdict is therefore **pass = false** (perf gate not met), with a clear, code-verified root-cause for each miss and concrete follow-ups below.

---

## 2. Performance table (per model, live GB10, single-stream + concurrency)

| Model | Paradigm | Input | TTFT / first-audio (p50 / p95 / p99) | Single-stream RTF | Throughput | Max concurrency at RTF < 1 |
|---|---|---|---|---|---|---|
| **whisper-tiny.en** | STT / feedforward | 12.05 s audio | 746.4 / 766.1 / 766.1 ms¹ | **0.0618** (16.2× realtime) | ~16.5 audio-s/s; ~1.3 streams/s² | **N = 8** |
| **kokoro-82M-ONNX** | TTS / whole-utterance | 6.90 s @ 24 kHz, `af_heart` | 165.2 / 195.7 / 195.7 ms³ | **0.0243** (41.1× realtime) | ~5.4 streams/s; 110.4 audio-s in 2.96 s wall @ N16 | **≫ N = 16**⁴ |
| **chatterbox-ONNX** | codec-AR (Ar) | 24 kHz, 5.64 s audio out | 3804 ms⁵ (first chunk == full synth) | **0.675** | ~1.4–1.5 audio-s/s; ~0.25–0.28 streams/s | **N = 24** (tested ceiling)⁶ |

**Footnotes (honest caveats baked into the numbers):**

1. ¹ **whisper TTFT == full-transcript latency.** There is no incremental-frame emission on this path; "TTFT" is the time to the whole transcript. REPS=8, so the tail is under-sampled and **p95 == p99** (tail not resolved).
2. ² whisper **does not batch**: concurrent transcribes serialize on a single `Arc<Mutex<model>>`. "~1.3 streams/s" is single-model-bound, not scaled.
3. ³ **kokoro first-audio == whole-synth latency**; chunking is post-synth. REPS=8 → p95 == p99 (tail under-sampled).
4. ⁴ kokoro is fast enough (RTF 0.024) that even 16 serialized synths aggregate to RTF 0.027 — so the RTF<1 ceiling is well above the tested N=16, but this is *speed masking serialization*, not concurrent batching.
5. ⁵ **chatterbox "first audio chunk" (3804 ms) == full-synthesis latency, NOT a low first-token metric.** The `serve_codec_ar_stream` path runs the *entire* AR decode loop, then calls `decode_audio()` **once** post-loop and slices the buffer into 113 chunks. Inter-chunk deltas are therefore p50 ≈ 0.0 ms / p95 0.1 ms / p99 0.1 ms over 112 frames — ~0 ms because the chunks are buffer slices, not incrementally synthesized. These per-frame percentiles are **not representative of true streaming jitter.**
6. ⁶ N=24 is the tested ceiling, not a measured saturation point — the model serves 24 streams at RTF 0.723, but see §3(a): adding streams adds linear wall-clock, it does not raise throughput.

**Concurrency ramps (per-stream / aggregate RTF):**

- **whisper** (per-stream RTF): N1 = 0.061, N2 = 0.131, N4 = 0.249, N8 = 0.500, **N16 = 1.012** → crosses RTF=1 at N=16; sustainable at N=8.
- **kokoro** (aggregate RTF): N1 = 0.023, N2 = 0.025, N4 = 0.026, N8 = 0.030, N16 = 0.027 — flat, fast.
- **chatterbox** (batched lockstep `serve_codec_ar_streams`, RTF): N1 = 0.652, N2 = 0.664, N4 = 0.661, N8 = 0.714, N16 = 0.788, N24 = 0.723. Per-stream throughput flat at ~1.4–1.5 audio-s/s at **every** N.

---

## 3. Baseline / improvement deltas

### (a) Single-stream vs N-concurrent batched scaling — codec-AR (the lockstep thesis)

**Result: NO SCALING — 1.00×.** Single-stream peak = 1.5 audio-s/s; concurrent peak = 1.5 audio-s/s @ N=1. Wall-clock grows **linearly** with N (3.60 s @ N1 → 96.93 s @ N24 for 24× the work). Per-stream audio-s/s is constant across all N.

**Root cause (verified in code at `abb1c10`):**
- `ArStepModel::step` (`crates/waav-infer-runtime/src/arstep.rs:509`) is a **single-slot seam** — its contract advances *one* slot by *one* stride.
- `Driver::tick` (`crates/waav-infer-runtime/src/driver.rs:230`) loops `model.step()` **once per slot** (`for input in active.inputs() { … }`), so each slot is a **separate batch-1 `StaticGraph::run`**.
- There is **no batch-dim-N forward** across slots. "Lockstep" here means step-synchronized *rounds*, **not** a single batched kernel.

The **55×@64 batched-forward thesis** from `INFER_PERF.md` / `INFER_ENGINE.md` **does not materialize on the real chatterbox ONNX path.** Correctness (24 streams at RTF<1) holds; throughput-scaling does not.

### (b) Perf-lever `run_bound` IoBinding (StaticGraph seam, "#1 engine perf change") — ON vs OFF on the REAL chatterbox `language_model.onnx` (30-layer Llama), CUDA EP

| Scenario | `run` (OFF) | `run_bound` (ON) | Speedup |
|---|---|---|---|
| Empty-KV step | 12.37 ms/step | 12.22 ms/step | **0.988×** (within noise) |
| Mid-decode, KV-depth = 200 | **64.5 ms** | **83.3 ms** | **0.775×** (run_bound **29 % slower**) |

**Result: NO IMPROVEMENT — net-negative at depth.**

**Root cause (verified):** chatterbox `feedback_present_kv` (`crates/waav-infer-core/src/tts/chatterbox.rs`) **takes** the `present.*` outputs that `run_bound` asked to keep on-device and renames them to `past_key_values.*` as **host `NamedTensor`s** for the next step. `run_bound` returns `Vec<NamedTensor>` (host-materialized), so the keep-on-device request is **immediately undone by a host round-trip every stride.** The KV never stays device-resident across strides, and `run_bound` adds 60-output binding-setup overhead for zero benefit. The `INFER_PERF.md` headline does **not** reproduce as a `run`-vs-`run_bound` swap on this real graph + this ORT-CUDA build.

### (c) M1 vs v2 path

There is **no separate runnable "old M1 server."** The v2 lockstep engine + production spine is wired **into** the same M1 OpenAI-compat server (`lib.rs` is the M1 surface; `engine.rs` drives v2). The live M1-surface tests pass **over the v2 engine**:
- `cascade_live` STT→LLM→TTS: **RTF = 0.121**
- `server_live`: REST `/v1/audio/speech` with `x-waav-rtf` header + STT closed-loop **4/4 green**

So **"M1 path runs on the v2 engine" is verified**, but a head-to-head M1-vs-v2 speed delta is **not separable** (single codebase) and is not claimed.

### NET

RTF<1 at ≥ 4 concurrent **holds for all three real models.** But the two headline perf-*improvement* levers — batched-forward throughput scaling and IoBinding KV-residency — **do not improve vs baseline** on the real path: they are flat (1.00×) or net-negative (0.775×). Per the strict criterion (*every measured metric improved-or-no-regression*), **perf.pass = false, honestly.**

---

## 4. Accuracy validation

**accuracy.pass = TRUE.** All four named bit-exact gates green on live GB10 (CUDA EP active, each `cargo run` under `timeout -k 30 1800`).

### 4.1 Named bit-exact gates (engine wiring is accuracy-neutral)

| Gate | Crate / module | Result | What it proves |
|---|---|---|---|
| `path_a_run_bound_bit_identical_per_arm` | `waav-infer-core` stt/encdec | **PASS** | run_bound/IoBinding is **accuracy-neutral**, token-for-token, vs the stateless `run` loop, for the 2 onboarded ORT Path-A STT arms: **whisper + moonshine** (deterministic decoder doubles). |
| `flow_solve_bit_identical_to_run_loop` | `waav-infer-core` tts/supertonic | **PASS** | Flow-matching (Supertonic-class CFM) `run_bound` loop is **bit-identical** to the run-per-step reference loop. |
| `ar_compounding_emitted_codes_identical` | `waav-infer-runtime` harness | **PASS** | Codec-AR full-loop emitted **integer codes identical** pre/post a modeled perf transform (the #2274 compounding trap). Sibling `concurrent_output_bit_identical_to_serial` (B=6 ≥ 4 concurrent == serial per slot) **PASS** → *masked ≠ absent* holds. |
| `pcm16_round_trip_is_bit_faithful` | `waav-infer-provider` | **PASS** | Exhaustive i16 → PCM16LE → f32 → i16 over **all 65 536 values**, byte-faithful (STT-ingress / TTS-egress seam). |

> **Honest scope note:** the 4 named gates use **deterministic doubles** (`RecDecoder` / `RecEstimator` / `DeterministicAr`), so they verify engine **WIRING is accuracy-neutral**, not live-model numerics. The live whisper-vs-onnxruntime 73/73 byte-identity (below) is what proves bit-exactness on **real registry weights** end-to-end.

### 4.2 Live integrated-path bit-exact-vs-REFERENCE (real registry weights)

**whisper-tiny.en** (onnx-community) driven through the WaaV CLI `transcribe` (SttModel seam, CUDA EP) vs a **plain python-onnxruntime greedy reference on the SAME ONNX graphs**, over all 73 LibriSpeech-dummy clips:

- **WaaV-vs-Ref disagreement = 0.000 % WER / 0.000 % CER**
- **73/73 normalized-exact** AND **73/73 RAW** (casing + punctuation) **byte-identical** transcripts.

→ whisper-tiny.en is **proven bit-exact-to-reference** on the integrated live path. Live closed-loop also green on CUDA: whisper + moonshine transcribe (incl. > 30 s sequential chunking), Kokoro TTS (RTF 0.023), integrated cascade STT→LLM→TTS (RTF 0.124).

### 4.3 Real dataset WER (offline, no fabrication)

- **Dataset:** `hf-internal-testing/librispeech_asr_dummy`, `clean` / `validation`, 73 utterances @ 16 kHz with ground truth. Locally cached, loaded `HF_HUB_OFFLINE=1`.
- **Model:** whisper-tiny.en (onnx-community), WaaV CLI on CUDA EP.

| Metric | WaaV | Reference (python-onnxruntime, same ONNX) |
|---|---|---|
| **WER vs ground-truth** | **9.67 %** | **9.67 %** (identical) |
| CER | 4.24 % | — |
| RTF | 0.354 (481.0 s audio in 170.2 s) | — |
| **WaaV-vs-reference disagreement** | **0.000 %** (73/73 byte-identical) | — |

**No-regression proof:** the engine introduces **ZERO** quality degradation vs the reference. The 9.67 % is **entirely the whisper-tiny.en model + normalizer floor** on this adversarial 73-clip dummy set (sci-fi proper nouns Ruggedo/Kaliko/Brion, number-words like "twenties"), not engine error.

**Honest caveat on the absolute number:** 9.67 % is **higher** than whisper-tiny.en's headline ~5–7 % because (a) this is the 73-clip *dummy* set (not the 2620-utt full test-clean) and is adversarially proper-noun-heavy, and (b) an **inlined minimal English normalizer** was used (system `python3` lacks jiwer/transformers' canonical whisper normalizer; `whisper.cpp normalizers/` dir is empty) which does not expand number-words. Both effects inflate WER **equally** for WaaV and the reference, so the no-regression delta (0.00 %) is exact regardless. **WER-vs-published-figure is therefore NOT directly comparable; WER-vs-reference (0.00 %) is the rigorous engine metric.**

**Harness:** `eval/dataset_wer.py` (committed-ready; system `datasets` + `soundfile` + `numpy` only; built-in WER/CER + normalizer + optional `--ref-transcripts` engine-parity gate). The pre-existing `eval/stt_eval.py` needs jiwer + transformers (absent) so could not be used as-is.

---

## 5. Honest gaps + recommended follow-ups

### Performance gaps (NOT measured / known limits)

1. **Native-S2S full-duplex e2e: NOT MEASURABLE.** No `DuplexModel`/S2S model is registered in the engine (`model.rs` registry dispatches only whisper STT + chatterbox/kokoro/etc. TTS). `full_duplex_bench` uses a **`FakeStage` double with modeled virtual-clock latency budgets** (not GPU). The `DuplexModel::step(&SlotBatch)->Vec` **batched** seam exists in the runtime but has **no real model behind it** — so the *only* path with a true batched-forward seam is unexercised by any real model.
2. **Per-frame TTFT for TRUE incremental codec-AR: not wired.** `serve_codec_ar_stream` decodes the whole body once post-AR-loop then slices, so "first audio chunk" = full-synthesis latency (3804 ms) and inter-chunk deltas ≈ 0 ms (buffer slicing). A genuinely incremental codec→audio decode (per-frame vocoder) is not wired; TTFA is **not** a low-first-token metric.
3. **STT/TTS concurrency does not batch.** whisper `transcribe` and kokoro `synthesize` both serialize through a single `Arc<Mutex<model>>` — concurrent requests run sequentially. Throughput is single-model-bound. (kokoro is fast enough to mask this; whisper crosses RTF=1 at N=16.)
4. **tokens/s for the codec-AR loop not instrumented** in the serving harness (only frames / audio-seconds). LM step latency was measured in isolation (~12 ms empty-KV, ~64 ms @ depth-200) via the perf-lever bench but not threaded back as a per-stride tokens/s counter.
5. **Accuracy/bit-faithfulness not RE-verified for the perf run** of the other arms — relied on prior memory entries (voxtral byte-identical, supertonic maxΔ=0.0). The perf bench asserts non-empty real output + distinct paradigms, not WER/bit-exactness this pass. (Accuracy track DID re-verify whisper — see §4.)
6. **Small-N tail.** p99 percentiles for whisper/kokoro TTFT are over REPS=8 → p95 == p99 (tail under-sampled). chatterbox per-frame p50/p95/p99 are over 112 real frames but are **buffer-slice deltas**, not true streaming jitter.
7. **Lever coverage.** No quant / spec-decode / approx-attn used. SDPA-pin / GQA-native were not A/B'd (chatterbox LM is MHA 16/16, not GQA → GQA-native is N/A on this model). Only the `run_bound` IoBinding lever was A/B-measured on the real graph.

### Accuracy gaps

1. **Named gates are doubles, not live weights.** All 4 named gates assert over deterministic recording doubles (proving wiring accuracy-neutrality). The STT gap was covered by the live whisper-vs-onnxruntime 73/73 byte-identity, but **no equivalent live bit-exact-vs-reference run** was done this session for the OTHER registry arms (Supertonic TTS, Kokoro, SenseVoice-CTC, voxtral-realtime, moonshine, codec-AR/torch-sidecar TTS families). Memory records voxtral-realtime + Supertonic previously byte-identical to onnxruntime, but **not re-verified this session.**
2. **Only 1 of ~16 arms got a fresh live dataset-WER + ref-parity number** (whisper-tiny.en). Other STT arms (moonshine, sensevoice, voxtral, nemotron/canary/granite/parakeet families) were not WER-evaluated here. `eval/sensevoice_eval.py` / `supertonic_eval.py` / `parakeet_eval.py` exist but need jiwer + sherpa-onnx / kaldi_native_fbank / reference py engines (not all installed).
3. **Absolute WER not validatable vs published figure** — no canonical Whisper normalizer locally and only the 73-clip dummy is cached. The engine-vs-reference 0.00 % delta is the honest no-regression proof.
4. **Reference engine = hand-written python-onnxruntime CPU greedy loop** (validated vs ground truth + WaaV), NOT the transformers PyTorch pipeline (onnx-community dir has no `.safetensors`; pulling PyTorch weights needs network). Same-weights ONNX-vs-ONNX is the apples-to-apples engine comparison and is arguably the correct reference, but it is not the torch reference.

### Recommended follow-ups (ranked)

1. **Implement a real batched-forward seam for codec-AR.** Today `Driver::tick` issues N batch-1 `StaticGraph::run`s. Either (a) make `ArStepModel::step` accept a `SlotBatch` and run one batch-dim-N forward (the existing `DuplexModel::step(&SlotBatch)->Vec` seam is the template), or (b) honestly **re-scope the lockstep "55×@64" thesis** in `INFER_PERF.md`/`INFER_ENGINE.md` to "correct N-concurrent serving at RTF<1," which is what actually ships. **This is the #1 perf gap.**
2. **Retire or fix `run_bound` on the chatterbox feedback path.** Keep present-KV **device-resident** across strides (return `OrtValue` device handles, not host `NamedTensor`s) so the keep-on-device request survives the feedback rename — otherwise drop `run_bound` from this path (it is a 29 % regression at depth).
3. **Register one real DuplexModel / S2S model** so the only true batched seam is exercised on GPU and `full_duplex_bench` stops relying on a `FakeStage`.
4. **Wire a per-frame incremental vocoder** for codec-AR so TTFA is a genuine first-token latency (and the streaming-jitter percentiles become meaningful).
5. **Batch STT/TTS** (or document the single-model-mutex serialization as intended) so whisper concurrency past N=8 and kokoro throughput scale.
6. **Extend the live dataset-WER + ref-parity harness** to the other ~15 arms (moonshine/sensevoice/voxtral/supertonic/parakeet), installing jiwer/sherpa-onnx in the eval venv.

---

## 6. Methodology

- **Hardware/runtime:** NVIDIA GB10 (sm_121, compute_cap 12.1), ORT CUDA EP active. Every measurement is **LIVE on GPU** against **real registry models on CUDA** — no simulated GPU timings (the one modeled-clock path, `full_duplex_bench`/`FakeStage`, is explicitly flagged as not-measured in §5).
- **Commit:** `abb1c10` (HEAD).
- **Perf harness (new):** `crates/waav-infer-server/tests/perf_bench.rs` drives whisper / kokoro / chatterbox through the serving seams; reports TTFT/first-audio percentiles, single-stream RTF, and concurrency ramps.
- **Perf-lever A/B (new):** `crates/waav-infer-backend-ort/src/lib.rs::perf_lever_run_bound_vs_run_on_real_chatterbox_lm_cuda` times `run` vs `run_bound` on the **real** chatterbox `language_model.onnx` (30-layer Llama) at empty-KV and KV-depth 200.
- **Existing live tests re-run green:** `cascade_live` (RTF 0.121), `gb10_serves_16_concurrent` (RTF 0.646), `server_live` (4/4).
- **Accuracy gates:** four named bit-exact tests across `waav-infer-core` / `waav-infer-runtime` / `waav-infer-provider`, each `cargo run` under `timeout -k 30 1800`.
- **Live bit-exact-vs-reference & dataset-WER:** WaaV CLI `transcribe` (CUDA EP) vs a hand-written python-onnxruntime greedy reference on the same ONNX graphs, over the locally-cached `librispeech_asr_dummy` clean/validation (73 utt, `HF_HUB_OFFLINE=1`). Harness committed at `eval/dataset_wer.py`.
- **Root-cause claims** in §3 were verified against the source at `abb1c10`: `arstep.rs:509` (single-slot `step` seam), `driver.rs:230` (per-slot `step` loop = batch-1 forwards), `chatterbox.rs::feedback_present_kv` (host round-trip of present-KV).

### Code provenance / scope (honest)

- **Pre-existing uncommitted changes left untouched by the accuracy author** and present in the tree: `crates/waav-infer-backend-ort/src/lib.rs` (+130 lines, the run_bound-vs-run perf A/B on real chatterbox LM) and `crates/waav-infer-server/tests/perf_bench.rs` (new, 457 lines). They compile clean (`workspace --tests` builds) and are perf-only (no accuracy seams), but were unreviewed by the accuracy author.
- The accuracy author's only addition is `eval/dataset_wer.py` (committed-ready). No `StaticGraph`/`SttModel`/`TtsModel`/registry/torch-sidecar seam was touched.

---

## 7. Verdict

| Track | Pass? | One-line |
|---|---|---|
| **Perf** | **FAIL** | RTF<1 @ ≥4 concurrent holds for all 3 real models, but batched-forward scaling = **1.00×** (no scaling) and IoBinding `run_bound` = **0.775×** (net-negative at depth) → not improved-or-no-regression. |
| **Accuracy** | **PASS** | 4/4 named bit-exact gates green; whisper-tiny.en **0.000 % WaaV-vs-reference** disagreement (73/73 byte-identical), dataset WER 9.67 % == reference WER (zero engine degradation). |

**Combined: `pass = false`** (overall gate requires perf.pass AND accuracy.pass). The engine **serves correctly and is accuracy-neutral**; the two headline perf-improvement theses do **not** reproduce on the real path and need either a real batched seam (#1 follow-up) or an honest re-scope in the perf docs.
