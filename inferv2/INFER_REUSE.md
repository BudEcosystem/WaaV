# WaaV Infer — Build-vs-Borrow Reuse Catalog

**Companion to `INFER_SPEC.md` (v1.0 r3.1).** This is the open-source reuse intelligence layer: every
existing repo / library / sub-component / reference pattern that compresses the WaaV Infer timeline,
raises its production maturity, or adds a relevant feature (telephony 8/12 kHz, diarization, noise
suppression, word-timestamps, few-step CFM…). Mature repos carry years of battle-tested edge-case
handling — capturing **that** is as valuable as the code.

**Version:** v1.0 · **Date:** 2026-06-13 · **Method:** 10 parallel deep web+GitHub research agents,
grounded against the spec and the cloned reference source under `research/`, with direct reading of
candle/moshi/sherpa-onnx/CosyVoice/Chatterbox/VoxCPM source. Full per-slice notes: `research/notes/reuse_*.md`
(rust_runtime, stt, tts, codecs_dsp, s2s_serving, flow_cfm, audio_io_telephony, packaging_registry,
serving_api_ops, g2p_text). ~140 artifacts catalogued; this is the synthesis + the spec-impacting deltas.

---

## 0. The thesis: reuse changes the build estimate

The spec's §17.3 build column lists the "genuinely new ~20%": batched AR backbone, scheduler, ledger,
lifecycle, manifests, protocol, porter kit. The reuse sweep **confirms that boundary and shrinks several
items inside it** — three findings move the needle most:

1. **`candle-transformers` already ships SNAC, DAC, EnCodec, full *streaming* Mimi, and MetaVoice as
   in-tree Rust** on the exact `candle::Tensor`/`Device` types the engine uses. candle 0.10 is already a
   chosen dep. So most of §11.2's *codec-decoder* work is **verification + a sliding-window streaming
   wrapper, not a build**. (`reuse_codecs_dsp.md`, `reuse_rust_runtime.md`)

2. **`parakeet-rs` already implements WaaV's entire STT-A path** — Parakeet-CTC/TDT (25-lang), Nemotron
   **cache-aware streaming**, **EOU end-of-utterance** detection, word timestamps, streaming diarization —
   on the **same EP set** WaaV targets (CUDA/TensorRT/MIGraphX/DirectML/WebGPU + CPU). The spec calls
   STT-A "the largest single M2 item"; this is the closest existing thing to all of Path-A and resolves
   OQ-4's cache-aware-streaming fallback. (`reuse_stt.md`)

3. **Two production Rust serving shells exist to copy wholesale**: HF **text-embeddings-inference (TEI)**
   = the only production candle+ORT-dual-backend server (Backend trait, token-budget batching, 429
   admission, disconnect detection — Apache-2.0), and **moshi-server `batched_asr.rs`** = the literal
   §8.3 fixed-slot scheduler (slot table, StreamMask padding, finalize-barrier marker heap, stale-slot
   guard, warmup tick — Apache-2.0, runs Kyutai Unmute in production). (`reuse_rust_runtime.md`,
   `reuse_s2s_serving.md`)

**Net:** the *new* code stays the scheduler-ledger-manifest-protocol spine plus the batched Llama/Qwen
backbone — but M0/M1 (P3 TTS + STT-A + DSP frontend + server skeleton + G2P/TN + CLI/registry) and M2's
codec/CFM stages are now overwhelmingly **assembly of mature, mostly-permissive, mostly-Rust parts**, not
greenfield. Conservative estimate: this reuse compresses M0–M2 by weeks and de-risks the two hardest
items (the dual-backend server shape and the fixed-slot scheduler) with production-proven references.

---

## 1. Master reuse map — by spec component

Legend — **mode**: VC vendor-crate · EM extract-module · FFI ffi-bind · REF reference-pattern ·
ONX onnx-export-source · DATA data-asset. **lic**: ✅ permissive · ⚠️ copyleft/conditional · ⛔ NC/excluded.

| Spec area | Top reuse | mode | lic | Milestone & impact |
|---|---|---|---|---|
| **§10 backend / dual-path server** | **TEI** `backends/core` Backend trait + router (candle+ORT in prod) | REF | ✅ Apache | M1 — the production shape of "candle+ORT behind one trait" |
| | **mistralrs-server-core** (router-builder, streamer, OpenAPI-gen) | REF | ✅ MIT | M1 — OpenAI-compat router shape (do not link: fork-pinned candle) |
| | candle `cudarc` 0.19 (CUDA 11.4→13, dynamic-load) | VC | ✅ | GB10 sm_121 without build-time CUDA |
| | tract+tract-pulse / rten / burn | REF | ✅ | named escape hatches (pure-Rust streaming/CPU/GPU-portability), revisit-triggers only |
| **§8 scheduler / staged engine** | **moshi-server `batched_asr.rs`** (fixed-slot tick loop) | EM | ✅ Apache | M2 — the literal FR-S2 reference; weeks off the hardest bring-up |
| | moshi-core `kv_cache`/`batched_transformer`/`streaming` (ScatteredKvCache, StreamMask) | VC | ✅ | M2 — per-slot fixed-quota KV (already chosen; restated) |
| | **lmdeploy "persistent batch"** + Triton sequence-batcher + vLLM-V1 token-budget | REF | ✅ | M2 — independent production validation of the fixed-slot + duty-piggyback design (KISS confirmation) |
| | flume (TEI-proven bounded MPMC) + tokio CancellationToken + audio_thread_priority (Mozilla) | VC | ✅/MPL | M1/M2 — the bounded stage queues + RT audio-emit thread (both already implied) |
| **§11.1 STT-A frame-sync** | **parakeet-rs** (CTC/TDT + Nemotron cache-aware streaming + EOU) | VC/EM | ✅ | **M1/M2 — collapses "the largest single M2 item"; resolves OQ-4** |
| | **sherpa-onnx** endpoint.cc + context-graph.cc + OnlineRecognizerResult | REF | ✅ Apache | M1/M2 — port the 3-rule endpointer, Aho-Corasick biasing (FR-A6), finality struct |
| | transcribe-rs `SpeechModel` trait; onnx-asr (IO-spec per family) | REF | ✅ | M1 — provider-trait + manifest-IO-sniffing priors |
| **§11.1 STT-B Whisper** | **candle-examples/whisper/main.rs** (temp-fallback, thresholds, timestamp rules, lang-detect) | REF/VC | ✅ | M2 — the decode policy already in Rust on the chosen candle |
| | **faster-whisper / CTranslate2** (vad_filter, hallucination_silence, cross-attn word-ts) | REF | ✅ MIT | M2/M4 — STT-B scenario-mgmt reference + int8 quality bar (OQ-1) |
| | **whisper_streaming / SimulStreaming / WhisperLiveKit** (LocalAgreement-2, AlignAtt) | REF | ✅ MIT | M4 — the exact §11.1 streaming-Whisper algorithms |
| **§11.2 codec decoders** | **candle-transformers `snac.rs`/`dac.rs`/`encodec.rs`/`mimi/`** (in-tree Rust) | EM/VC | ✅ | M2 — SNAC/DAC/EnCodec/Mimi decoders ready to lift; Mimi is fully streaming |
| | **onnx-community/snac_24khz-ONNX** (fp32/fp16/int8/bnb4 decoder) + bitbasti recipe | ONX/DATA | ✅ MIT | M2 — decoder-only SNAC on Path A; the per-quant re-verify assets |
| | moshi-core `StreamableConv1d` (mask-aware causal-conv state) | VC | ✅ | M2 — the battle-tested streaming-conv ring buffer the fixed-slot scheduler needs |
| | foldl/chatllm.cpp (ggml DAC/SNAC/WavTokenizer) | REF/FFI | ✅-ish | M4/M5 — the codec half of the Hexagon/Vulkan ggml road llama.cpp itself lacks |
| **§11.2 vocoders** | **sherpa-onnx vocoder factory** (Vocos+HiFi-GAN ONNX dispatch) | REF/FFI | ✅ Apache | M1/M2 — production vocoder wiring + EP fallback |
| | **istft-onnx** (mush42) | ONX/REF | ✅ MIT | M2 — closes the real "ONNX has no ISTFT op" gap for Vocos/iSTFTNet/Kokoro (porter recipe) |
| | Vocos ONNX (BSC-LT/wetdog) · HiFTNet/iSTFTNet (yl4579/deepvk) · BigVGAN (GPU-flag) | ONX | ✅/MIT | M2/M3 — the vocoder zoo as ONNX assets |
| **§11.2 Euler-CFM solver** | **Matcha-TTS `solve_euler`** (~30 LOC canonical reference) | REF | ✅ MIT | M2 — the golden loop semantics 4 families inherit |
| | **CosyVoice2 `CausalConditionalCFM`** (chunk-aware flow cache: −34 overlap, deterministic noise) | REF | ✅ Apache | **M2 — the streaming-equivalence enabler; no clean substitute** |
| | **candle `mmdit`/`flux`** (DiTBlock, AdaLN, modulate) | EM | ✅ | M2/M4 — the Rust DiT estimator building blocks (port, not from-scratch) |
| | VoxCPM `UnifiedCFM` (CFG-Zero★, sway, MeanFlow); F5-TTS-ONNX recipe; torchcfm goldens | REF/ONX | ✅(code) | M2/M4 — solver supersets + few-step distillation + numerical-parity goldens |
| **§11.2 speaker encoder** | **sherpa speaker-embedding-extractor + SpeakerEmbeddingManager** (CAMPPlus/wespeaker/3D-Speaker/NeMo, one dispatch) | REF/DATA | ✅ Apache | M2 — the SpeakerEmbeddingCache pattern + CAMPPlus ONNX (FR-E10 cloning AND diarization) |
| **§11.2 DSP frontend** | **kaldi-native-fbank** (Rust crate: fbank+Whisper-mel+STFT/iSTFT, online) · realfft/rustfft · mel_spec | VC | ✅ Apache | M0/M1 — drop-in FR-E7 frontend; mel_spec is bit-checked vs whisper.cpp (golden oracle) |
| **§11.1 TTS-P3 one-shot** | **Kokoros / kokorox** (full Rust Kokoro: ONNX fwd + chunk + stream + OpenAI server) | EM/REF | ✅ Apache | **M0/M1 — pulls first-audio forward; the CPU-floor gate is proven** |
| | piper-rs/sonata + **rhasspy/piper-voices** (35-lang voices.json catalog) | VC/DATA | ✅/per-voice | M2 — Piper path + a ready voice catalog (zero authoring) |
| | sherpa-onnx TTS (kokoro/vits/matcha/melo/kitten/supertonic impls + chunking/silence/lexicons) | REF | ✅ Apache | M1/M2 — the FR-E8 chunking + lexicon reference |
| **§11.1 TTS-P1 codec-LM** | **Orpheus** sliding-window SNAC decoder + bitbasti streaming recipe + orpheus-cpp (llama.cpp) | REF | ✅(code) | M2 — the streaming decode hot path + llama-cpp-2 serving recipe (backbone stays build) |
| | OuteTTS (WavTokenizer single-codebook, native in llama.cpp `llama-tts`) | REF/FFI | ✅ | M2 — proves end-to-end llama.cpp TTS serving |
| **§11.1 TTS-P1+P2 composite** | **CosyVoiceForOnnx** (Flow+HiFT made ONNX-exportable, **fp16 NaN fix**) | ONX/REF | ✅ Apache | **M2 — removes the single hardest porting blocker** |
| | chatterbox-turbo-ONNX (official) + Perth watermarker | ONX/REF | ✅ MIT | M3 — official ONNX path; the watermark *enable* (FR-M5 truthful provenance) |
| **§11.1 S2S (M5)** | moshi/unmute (full duplex stack) · CSM · GLM-4-Voice (single-codebook, simplest) · LFM2-Audio-1.5B (small, fits NFR-X) · Step-Audio2 | REF/ONX | ✅(mostly) | M5 — a spread of duplex references; GLM-4-Voice/LFM2 are the easiest first targets |
| **§6 interruption / turn** | **Pipecat** (interrupt = queue-jumping SystemFrame; flush-buffers-on-barge-in) | REF | ✅ BSD-2 | M1/M2 — the canonical interruption state machine for FR-E4 |
| | **smart-turn-v3** (ONNX, 12 ms CPU, **license-clean** EoT) | ONX | ✅✅ BSD-2 | M2/roadmap — the rare turn model with no model-license trap (vs LiveKit's restricted weights) |
| | **LiveKit ProcPool** (warm-idle pool, prewarm shared models, silent-worker-death detect) | REF | ✅ Apache | M1 — the §4.2b supervised-sidecar warm-pool + FR-O3 deep-probe lesson |
| | HF speech-to-speech (`CancelScope`+`stop_event` cooperative cancel) | REF | ✅ Apache | M2 — minimal cancellation fan-out across the staged engine |
| **FR-E8 text frontend / G2P / TN / segmenter** | **misaki-rs** (MIT, GPL-free Kokoro G2P with `default-features=false`) | VC | ✅ MIT | M1 — named kokoro G2P, no GPL helper |
| | **icu_segmenter (ICU4X) + srx** (UAX-29 + CJK/Thai word-break + LanguageTool abbreviation rules) | VC | ✅ | M1 — the *correct* FR-E8 segmenter (kills splits-on-Dr./e.g./3.14) |
| | **wetext-rs + rustfst** (pure-Rust WFST TN/ITN zh/en/ja) + NeMo/WeText FAR grammars (data) | VC/DATA | ✅ Apache | M2 — production TN/ITN with **zero C/C++** (satisfies §17.1); fills the unnamed full-TN gap |
| | **CharsiuG2P/DeepPhonemizer → ONNX** (100-lang neural G2P, no espeak) + phoonnx registry pattern | ONX/REF | ✅ Apache/MIT | M2+ — permissive multilingual phonemes on Path A; **shrinks the §16.4 GPL surface to legacy Piper only** |
| | jpreprocess (BSD-3 pure-Rust OpenJTalk, ja) · jieba-rs+pinyin (MIT, zh) | VC | ✅ | M2+ — ja/zh frontends with no GPL/C |
| | kitoken (C-free SentencePiece) · tokenizers (default-features=false) | VC | ✅ | M1/M2 — keeps tokenizers in the C-free `-components` crate |
| **FR-D/E6/E9 audio edge & telephony** | **rubato** (gateway's resampler — lift `StreamResampler` wholesale) | VC | ✅ MIT | M1 — edge resampling; the 8/12/16/24/48 matrix |
| | **audio-codec-algorithms** (G.711 µ-law/A-law, 0BSD, no_std) | VC | ✅ 0BSD | **M2 — the telephony 8 kHz codec edge (the spec named none)** |
| | **`opus` crate** (libopus FFI, BSD) — one codec spans 8/12/16/24/48 kHz, FEC/PLC/DTX | FFI | ✅ MIT/BSD | M1 — egress opus AND the Symphonia-opus-ingress gap-filler |
| | Symphonia (ingress decode) · hound (wav) · cpal (`waav run` mic) · rtrb (RT ring) | VC | ✅/MPL | M1 — the FR-E9 decode + device I/O edge |
| | **nnnoiseless** (pure-Rust RNNoise, 16 kHz) · DeepFilterNet via deepfilter-rt (ort, 48 kHz) · sonora (pure-Rust AEC/AGC, watch) | VC/REF | ✅ | M2/M4 — NEW: noise suppression for telephony/edge (gateway-owned, engine-adjacent) |
| **FR-M packaging / registry / supply chain** | **hf-hub** (Apache, used by candle/TEI) · safetensors · candle `gguf_file`+`tensor-tools` | VC/EM | ✅ | M0/M1 — pull substrate + format readers/writers + GGUF quantize writer (pure-Rust) |
| | ollama store mechanics (blobs/manifests CAS) · piper voices.json (catalog-index shape) | REF | ✅ MIT | M1 — content-addressed store + signed-catalog-index shape |
| | **sigstore-rs / prefix-dev/sigstore-rust** (offline OMS-bundle verify) | VC/EM | ✅ Apache | M2/M3 — FR-M5 keyless+offline signing verification (DSSE the integration risk) |
| | **cargo-deny + cargo-audit + cargo-cyclonedx** (CVE + **license-obligation allowlist** + SBOM) | VC(CI) | ✅ | M0 — §16.3 gate *and* FR-M5 license policy, verbatim |
| | oci-client + oci-spec (deferred `oci://` v2) · clap/indicatif/comfy-table/minijinja (CLI UX + chat_template) | VC | ✅ | M1/M3 — ollama-class CLI + the deferred OCI transport made cheap |
| **§13 API / §15 ops** | **async-openai** (Rust OpenAI types incl. Audio+Realtime-GA) | EM | ✅ MIT | M1 — the biggest OpenAI-compat accelerant; reuse types + CI conformance |
| | axum + tokio-tungstenite + tower-governor (rate-limit) + utoipa (OpenAPI/AsyncAPI from types) | VC | ✅ | M1 — HTTP/WS server + ingress limits (§13.6) + anti-drift spec gen (FR-A5) |
| | metrics-exporter-prometheus + opentelemetry-otlp quartet + tokio CancellationToken (RC6 drain) | VC | ✅ | M1 — standalone metrics + the §15.5 OTel span layer (gateway lacks OTel today) |

---

## 2. The "vendor / extract now" shortlist (what to actually pull in for M0–M2)

These are the crates/modules to add to the workspace early — all permissive, all reduce build:

**Engine & runtime:** candle-transformers (snac/dac/encodec/mimi/whisper modules — extract into
`waav-infer-components`), moshi-core (already chosen; the streaming-conv + scattered-KV), flume,
tokio-util, audio_thread_priority, cudarc (transitive), rayon (parallel ISQ).

**DSP & audio:** kaldi-native-fbank (Rust) or realfft+rustfft+mel_spec, rubato (lift gateway
`StreamResampler`), Symphonia, hound, cpal, rtrb, audio-codec-algorithms (G.711), `opus`.

**STT/TTS reuse:** parakeet-rs (feature-gated STT-A), Kokoros/kokorox modules (P3), SNAC ONNX decoder
asset, CosyVoiceForOnnx exports (M2 model assets).

**Text frontend:** misaki-rs (default-features=false), icu_segmenter, srx, wetext-rs+rustfst,
tokenizers (default-features=false) or kitoken, jieba-rs+pinyin, jpreprocess (ja).

**Packaging/registry/supply-chain:** hf-hub, safetensors, sha2, tar, zstd/flate2, sigstore-rs (+
prefix-dev/sigstore-rust for DSSE), oci-client/oci-spec (later), cargo-deny + cargo-audit +
cargo-cyclonedx (CI), clap, indicatif, comfy-table, minijinja, reqwest-retry.

**API/ops:** async-openai (type modules), axum, tokio-tungstenite, tower/tower-http, tower-governor,
metrics-exporter-prometheus, opentelemetry-otlp + tracing-opentelemetry, utoipa, subtle (const-time key
compare), nix (SO_PEERCRED).

**Reference-only (copy the shape, never link):** TEI (Backend trait + token-budget batching +
disconnect detect), moshi-server (the scheduler files), mistralrs-server-core (router shape) +
mistralrs-quant (ISQ design), sherpa-onnx (endpoint/context-graph/vocoder/speaker/lexicon C++),
Pipecat (interruption discipline), LiveKit ProcPool, Matcha/CosyVoice2 CFM, faster-whisper (decode
policy), bitbasti (Orpheus streaming).

---

## 3. Battle-tested edge cases worth importing (the scenario-management prize)

Years of production tuning, distilled — capture these as Rust ports or config defaults; each prevents a
class of 3am bug. (Grouped; full provenance in the per-slice notes.)

**Scheduler / serving (moshi-server, TEI, Triton/vLLM):**
- **Stale-slot guard** — when a fixed slot is reused by a new connection while an old request's output is
  still in flight, compare `ref_channel_id` to the slot's current `ChannelId` and **silently drop the
  stale output** (never deliver to the new user). The exact correctness hazard FR-S2's O(1) abort creates.
- **Finalize-barrier as a step-ordered marker max-heap** (`BinaryHeap<Marker>` keyed by step_idx) — acks
  emit *after* all audio fed before them is processed, across a batch where slots are at different steps.
  Non-obvious under batching; this *is* the FR-A2 three-level-finality machinery. **Import directly.**
- **`with_data` gate + StreamMask padding** — run the static-shape batched step only if ≥1 slot has a
  full frame; mask the empties (don't resize) so the batch shape stays constant (CUDA-graph-friendly).
- **Warmup runs the worst-case batch shape** with an all-true mask before serving (= FR-O2 / §9.2(b)).
- **Cascade-close by drop-propagation + a top-level `select!` timeout backstop** (no manual lifecycle
  bookkeeping) = FR-E4 "freed resources return to pools immediately."
- **TEI: detect client disconnect *before* spending GPU** (admission hygiene); **batch by token budget,
  not request count** (= §8.5 Sarathi piggyback).
- **lmdeploy "persistent batch" + Triton sequence-batcher** independently validate the fixed-slot design
  — two production engines converged on the same slot-batch the spec chose (KISS confirmation).

**Interruption / turn (Pipecat, HF s2s, LiveKit):**
- **Interrupt is a SystemFrame that survives interruption and jumps the data queue** — the single most
  important S2S primitive (= §7 "control jumps data queues", FR-E4). On interrupt every stage flushes its
  send buffer AND cancels in-flight work so no stale audio leaks (Pipecat `test_tts_frame_ordering`).
- **Cancellation taxonomy** (graceful end vs hard cancel vs error) = §13.5 `draining`/`clear`/`internal`.
- **LiveKit silent-worker-death** (issue #3841: workers die after prewarm, fail all jobs) — the exact
  failure mode FR-O3's deep probe + §4.2b crash-loop budget exist to catch. A logged production incident
  validating the design.

**STT (sherpa-onnx, faster-whisper, candle, kyutai DSM):**
- **3-rule endpointing** (rule1 2.4 s silence even w/o decode; rule2 1.2 s after decode; rule3 20 s max) —
  fed by `(num_frames, trailing_silence, frame_shift)`; the FR-G3 finalize logic. **The constants are gold.**
- **Adaptive VAD thresholds** — base 0.5; **raise to 0.9 once speech > 20 s** then reset; exit-threshold
  `max(thr−0.15, 0.01)` to kill silence-flicker.
- **Whisper temperature-fallback ladder** — regenerate at higher T when `avg_logprob < −1.0` OR
  `compression_ratio > 2.4`; no-speech when `no_speech_prob > 0.6` ∧ low logprob (already in candle).
- **VAD-gated chunking** (faster-whisper `vad_filter`) — skip non-speech *before* the decoder sees it: the
  single biggest real-world Whisper-quality lever; also speeds batching.
- **`condition_on_previous_text` off on low-confidence + `hallucination_silence_threshold`** — stop
  repetition cascades on silence (the live-streaming killer).
- **LocalAgreement-2 / AlignAtt** — commit only the stable longest-common-prefix across hypotheses (= the
  M4 streaming-Whisper algorithm; don't reinvent).
- **Audio-repetition penalty for repeated-word loops** (kyutai DSM #175) — AR decoders loop on repeated
  input; design in a repetition penalty for STT-C and any AR decode.
- **4–5 min CTC/TDT length cap** — non-streaming Conformer ONNX graphs degrade/OOM past ~5 min → the
  segmenter/VAD must chunk; encode the cap in the manifest.

**Codec / CFM / vocoder (moshi-core, CosyVoice2, candle, sherpa):**
- **Streaming causal-conv state with batch masking** (moshi `StreamableConv1d`) — masks carried conv
  state per-slot so a finished/reset stream doesn't bleed history into a neighbor; `bail!`s on varying
  step size. Reuse verbatim.
- **`reset_state` discipline** (candle Mimi) — `encode`/`decode` reset the transformer; `*_step` preserve
  it. Getting this wrong = audible clicks at chunk boundaries.
- **SNAC 7-token de-interleave** — slots 1&4→layer-2, 2,3,5,6→layer-3 with per-slot offset; "the single
  most error-prone line in an Orpheus path." Port the reference exactly + golden-test.
- **CosyVoice2 chunk-aware flow cache** — stack `(z, mu)` of the prompt + **last 34 mel frames** of the
  previous chunk; overwrite the leading frames of fresh noise → no audible chunk seams. The
  streaming-equivalence enabler; no clean substitute. Plus **deterministic precomputed noise** sliced per
  chunk (cross-chunk consistency).
- **CFG batch-doubling + "do not use concat (TRT memory-format trap), pre-allocate `x[2B]` and
  slice-assign" + dtype discipline** (CosyVoice/Chatterbox, verbatim repeated comments) — hours of TRT
  fp16 debugging baked in; informs §20's CFM-parity risk.
- **fp16 CFM-estimator NaN in `/decoder/estimator`** (CosyVoiceForOnnx fix, Nov 2025) — inherit the fix /
  fp32-keep-output-layer rule (= §10.2's cross-backend numerics warning).
- **delay-pattern: strip BOC/EOC with `where(code≥vocab→0)`, never `clamp`** (clamp turns EOC into a
  valid code → audible artifact).
- **Metadata-driven vocoder + speaker dispatch** (sherpa) — one model-metadata field selects HiFi-GAN vs
  Vocos, wespeaker vs CAM++ vs NeMo. Exactly "models are data."

**Audio I/O (gateway resampler, Opus, cpal):**
- **Resampler filter-history persistence + stale-state clear on >200 ms gap + utterance-boundary flush** —
  without flush, the END of every utterance (final stop consonants) is silently dropped (STT-accuracy
  killer). Already solved in `gateway/src/core/audio/resampler.rs` — **lift wholesale.**
- **8 kHz sub-frame chunking** — fixed 1024-frame chunks buffer 128 ms at 8 kHz before VAD sees audio;
  chunk at `in_rate/50` (~20 ms). **8 kHz is the rate that exposes fixed-chunk latency bugs.**
- **Degrade-to-passthrough, never drop audio** on resampler failure.
- **Opus output rate is freely chosen; the codec is rate-agnostic** (RFC 6716) — one Opus decoder absorbs
  the entire 8/12/16/24/48 kHz matrix without an external resampler. The single biggest multi-rate lever.
- **GRU/recurrent state as explicit ONNX I/O for streaming** (deepfilter-rt "split streaming") — the
  pattern for any streaming-RNN on Path A (Silero VAD, Smart Turn, DeepFilterNet).

**Packaging / supply chain (hf-hub, ollama, OMS, cargo-deny):**
- **HF cache layout is a portability contract** — hf-hub reproduces `models--org--name/{blobs,snapshots,
  refs,.no_exist}`, so WaaV's store is interoperable with an existing warm `~/.cache/huggingface`.
- **ollama blob dedup across manifests** → `waav rm` must refcount before deleting a blob.
- **Model files are attack surface with *active* CVEs** — GGUF integer-overflow→heap-OOB (CVE-2025-53630
  + its bypass), ONNX external-data path traversal (CVE-2022-25882 → 2024-27318 → 2026-27489, each a
  bypass of a `startswith("../")` check). **Imports:** canonicalize-and-confine paths inside the blob dir
  (not a prefix check), validate GGUF sizes vs file length before alloc, never load ORT custom-op dylibs
  from model-controlled paths, seed the §18.4 fuzz corpus from these PoCs.
- **`cargo-deny` is the obligation engine** — its `licenses`/`bans`/`sources` checks *are* the FR-M5
  permissive-allowlist + pinned-deps-no-second-version + trusted-registry policy. Wire deny.toml to *be*
  the §16 policy.
- **Verify signatures at pull only; load enforces recorded provenance** — keeps a stale TUF root from
  bricking an air-gapped fleet (already FR-M5; the crates deliver it).

---

## 4. License gating map (reuse is license-bounded)

| Tier | Examples | Reuse rule |
|---|---|---|
| **✅ Permissive (vendor/extract freely)** | candle, moshi-core, TEI, parakeet-rs, sherpa-onnx (Apache), kaldi-native-fbank, rubato, Symphonia, hf-hub, sigstore-rs, async-openai, axum, misaki-rs, icu_segmenter, wetext-rs/rustfst, jpreprocess, jieba-rs, audio-codec-algorithms (0BSD), opus(MIT)/libopus(BSD), Pipecat (BSD-2), smart-turn-v3 (BSD-2 code+model), Matcha-TTS/CosyVoice2/Chatterbox **code** | vendor or extract; standard attribution |
| **⚠️ Weak/file-level copyleft (link OK, isolate file changes)** | Symphonia/DeepFilterNet (MPL-2.0), audio_thread_priority (MPL-2.0), zstd (BSD/libzstd) | safe to depend on; note in deny.toml |
| **⚠️ Conditional / additional-restrictions (read LICENSE first)** | TEN-framework + ten-vad ("Apache + additional restrictions"; LPCNet BSD attribution), LiveKit **turn-detector weights** (LiveKit Model License — NOT redistributable; use smart-turn instead) | reference-pattern only; verify before any bundling; use the permissive alternative |
| **⚠️ Strong copyleft (separate process / reference-read only)** | **espeak-ng (GPL-3.0)** → separate `waav-g2p-espeak` process (ADR-13); **seed-vc (GPL-3.0)** → reference-read only, never copy code; espeakNG-rs pure-Rust port is *still* GPL | process-isolate or read-only |
| **⚠️ Model-license derivatives (obligations flow down)** | orpheus-3b (Llama-3.2 Community: "Built with Llama" + AUP), LLaMA-Omni (Llama-3.1), Supertonic *model* (OpenRAIL-M use-restriction), LFM2-Audio (LFM Open License) | carry the `[license]` obligation block (FR-M1); surface attribution |
| **⛔ NC / excluded weights (code may be a reference; weights not catalog-eligible)** | F5-TTS (CC-BY-NC weights / MIT code), Fish-Speech/OpenAudio-S1 (CC-BY-NC-SA), XTTS (Coqui CPML) | code = reference-pattern; weights excluded per FR-M8 unless license-clean retrain |

**Net licensing win:** the permissive-Rust G2P/TN stack (misaki-rs + CharsiuG2P/DeepPhonemizer-ONNX +
jpreprocess + jieba-rs + wetext-rs) **shrinks the GPL surface to only legacy Piper voices that
hard-require espeak** — a materially smaller §16.4 isolation footprint than the spec assumed.

---

## 5. New features these repos make cheap (beyond the current spec)

Catalogued so the roadmap can adopt them as low-cost adds (most are manifest flags or composable DAG nodes):

1. **8 kHz telephony + full multi-rate (8/12/16/22.05/24/44.1/48 kHz)** — `opus` (one rate-agnostic
   codec) + `audio-codec-algorithms` (G.711) + rubato edge resampling. **Directly answers the user's
   8/12 kHz ask.** Unlocks the contact-center self-host audience (§1.2). → spec FR-D edits (§7 below).
2. **Speaker diarization** (spec v1 non-goal → roadmap DAG node, now near-ready) — pyannote-rs / speakrs /
   sherpa pyannote-pipeline; CAMPPlus is *already* a §11.2 component (dual-use with cloning).
3. **Real-time noise suppression / speech enhancement** — nnnoiseless (16 kHz, cheap) / DeepFilterNet via
   deepfilter-rt (48 kHz, ort) / GTCRN (16 kHz) — composable pre-STT DAG node for telephony/noisy field.
4. **Semantic end-of-turn (endpointing)** — smart-turn-v3 (ONNX, 12 ms CPU, license-clean) beyond raw VAD.
5. **Word/segment timestamps** — faster-whisper cross-attn DTW + stable-ts (STT-B); moshi-server Marker
   step-index (STT-A) — closes the OpenAI-compat word-timestamp gap.
6. **Few-step / distilled CFM (MeanFlow, 1–4 steps)** — Chatterbox + VoxCPM carry the branch;
   `meanflow=true` + `n_timesteps=2` ⇒ ~5× cheaper P2 stage. Plus **CFG-Zero★** (free first step → TTFA
   win) and **sway sampling** (fewer NFE) — all manifest knobs, not engine changes.
7. **Voice style mixing/blending** (Kokoros `af_sky.4+af_nicole.5`) — a cheap FR-E10 voice-bank superset.
8. **Explicit duration + emotion control** (IndexTTS2) — a P1+P2 manifest capability flag.
9. **Multilingual VITS (35 langs) one-shot** via MeloTTS ONNX + sherpa lexicons — broadens P3 beyond EN.
10. **Single-codebook S2S** (GLM-4-Voice) — removes the depth decoder; the easiest M5 first target.
11. **Warm process pool** (LiveKit ProcPool) — faster cold-start under FR-M7/NFR-P5.
12. **Inverse text normalization for STT output (Riva-parity)** — wetext-rs ITN / text2num.
13. **OCI `oci://` enterprise air-gap transport** (Harbor/ECR/GHCR) — a few-hundred-LOC adapter via
    oci-client + oci-spec; promotes ADR-7's "v2 optional" to "v2 cheap."
14. **CycloneDX SBOM per model artifact** — exceeds ollama/LM Studio supply-chain posture.
15. **Warm-cache interop** — hf-hub shares the Python cache → zero re-download for users already on
    `transformers`.

---

## 6. Spec gaps the reuse sweep found (fold into the plan)

These are concrete corrections/additions the audio + codec + G2P slices surfaced (applied to the spec in §7):

1. **FR-E9 "opus via Symphonia" ingress is currently impossible** — Symphonia has **no Opus decoder** (0.6,
   June 2026, still "in work"). Fix: pair Symphonia with the `opus` crate (+`ogg`) for opus ingress; one
   libopus link serves ingress *and* egress.
2. **No telephony G.711 / 8 kHz codec named** despite §1.2's contact-center audience and FR-D1's 8 kHz.
   Fix: add `audio-codec-algorithms` (G.711 µ-law/A-law) to §17.3.
3. **FR-D1 omits 12 kHz and 22.05 kHz.** Fix: widen to {8, 12, 16, 22.05, 24, 44.1, 48} kHz and name
   **Opus** as the unifying rate-agnostic codec.
4. **DeepFilterNet is 48 kHz-only** — running it on the 16 kHz STT path is a hidden 16→48→16 round-trip.
   Fix: document the tax; prefer **nnnoiseless** on the STT-ingress lane.
5. **hound does not decode WAV µ-law/A-law payloads** — a `WAVE_FORMAT_MULAW` upload needs the G.711
   crate after hound parses the header. Fix: note the hound+G.711 composition.
6. **ONNX has no native ISTFT op** — every iSTFT vocoder (Vocos/iSTFTNet/Kokoro) needs the **istft-onnx**
   export recipe; make it a porter-kit recipe so it's not an M2 surprise.
7. **FR-E8 length caps should be token-budget-aware, not just char-count** (Kokoro-FastAPI 175/250/450
   phoneme-token budget prevents "rushed speech"); cross-reference the 20-char-first/120-char-rest caps.
8. **CFM solver should expose `meanflow`/`cfg_zero_star`/`t_scheduler∈{linear,cosine,sway,log-norm}` as
   manifest knobs** — the few-step + quality levers above; otherwise a distilled model can't "just work."

---

## 7. Source index

Per-slice notes (this synthesis's evidence base): `research/notes/reuse_{rust_runtime, stt, tts,
codecs_dsp, s2s_serving, flow_cfm, audio_io_telephony, packaging_registry, serving_api_ops, g2p_text}.md`
— each carries the full reuse table, the battle-tested-edge-cases subsection, and primary-source URLs
(GitHub/crates.io/HF/arXiv, verified June 2026). Cloned reference source: `research/{engines,rust,
toolkits,models}/`. Spec: `WaaV/inferv2/INFER_SPEC.md`. Prior corpus: `research/learnings.md` + the wave-1/wave-2
notes.

## Appendix — revision log
| Rev | Change |
|---|---|
| v1.0 | Initial synthesis from the 10-agent reuse sweep (140 artifacts); spec deltas in §6 folded into INFER_SPEC.md r4. |
</content>
