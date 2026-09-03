# ONBOARD — MOSS-Transcribe-Diarize (`mosstd`)

**Model**: OpenMOSS-Team/MOSS-Transcribe-Diarize (0.9B, Apache-2.0, released 2026-07-09) — end-to-end
**speaker-attributed long-form transcription + diarization** in ONE pass: Whisper-Medium encoder (24L/1024d)
→ 4× time-merge → VQAdaptor MLP → Qwen3-0.6B decoder (28L, GQA 16q/8kv, TIED head), greedy compact
`[start][Sxx]text[end]` output with digit time-anchor tokens interleaved in the audio span (12.5 tok/s,
5 s markers). arXiv 2601.01554. **SOTA claim**: best open CER/cpCER on multi-speaker long-form benchmarks.

**Substrate**: tch Path-B (torch-inprocess), per the Goal-D decision. New arch module
`crates/waav-infer-backend-torch/src/mosstd.rs` (`TorchMossTd` + `ring::TorchMossTdBatched`), registry key
`mosstd | moss_transcribe_diarize | MossTranscribeDiarizeForConditionalGeneration`.

## Verdict: VERIFIED — every gate green

| Gate | Result |
|---|---|
| Prompt ids vs HF processor (audio span + digit markers) | identical, 3 clips |
| Mel vs HF `input_features` | **maxΔ=0** (byte) |
| Encoder / adaptor seams | **maxΔ=0** both cells |
| Greedy ids, jfk (70 tok) + multispeaker (194 tok) | **byte-identical**, CUDA-bf16 AND CPU-f32 |
| Long-form 184 s (1015 tok) | prefix-identical to token 166 = a reference-side **EXACT bf16 logit tie** (HF top-2 both 15.500000); past it: text similarity 0.9993, 30 segments structure-identical (two-tier bar) |
| Force-solo ring oracle (4 ragged rows incl. duplicate) | 412 ids max\|Δ\|=0 vs TRUE one-shot solo + identical-input contract |
| Unit tests | components 97/97 (12 new parser tests), backend-torch 782/782 (10 new) |
| clippy | clean on all new code |
| Live serve | `/v1/audio/transcriptions` 8/8 parallel OK in both serve modes; CLI `transcribe`; GUI registry entry |
| Eval harness (`eval/mosstd_eval.py`, 16 LibriSpeech clips) | WaaV WER 0.0273 == ref WER 0.0273, **16/16 transcript agreement (100%)**, PASS |

## Performance (GB10, release, CUDA bf16, single stream)

| Clip | Audio | Wall | RTF |
|---|---|---|---|
| jfk | 11.0 s | 1.21 s | 0.110 |
| multispeaker | 35.8 s | 2.58 s | 0.072 |
| long-form | 184.4 s | 15.0 s | **0.081** |

fp16 ≈ bf16 (15.1 s long-form). CPU f32 RTF ~4.4 (the portability floor, correctness-gated). Model load
2.0 s.

## Auto-config (selecting the model on a hardware+dtype enables the verified-optimal options)

No env vars are required for the optimal setup — they are OVERRIDES only:

| Hardware | dtype (auto) | Concurrency (auto) | Verified |
|---|---|---|---|
| CUDA | bf16 native (reference regime, byte-gated) | **Fork-A1 batched ring DEFAULT-ON** (8 slots × 8192, ≈7.5 GiB; solo delegate serves REST so long-form is unaffected by the stepped-seam guard) | ring log + 8/8 parallel + 184 s long-form green |
| CPU | f32 (deterministic floor, byte-gated) | solo + coalescer (ring buys nothing without device-resident KV) | auto-solo log verified |
| CUDA + `WAAV_MOSSTD_BATCHED=0` | bf16 | solo forced (escape hatch) | override log verified |

Overrides: `WAAV_MOSSTD_DTYPE=fp16|f32`, `WAAV_MOSSTD_PRECISION=sandwich` (quality-tier forks),
`WAAV_MOSSTD_MAX_SLOTS/MAX_SEQ` (ring re-size, caps 24/8192), `WAAV_MOSSTD_MAX_POS/MAX_NEW/PROMPT`.
This is the fleet auto-config pattern (qwen3_tts/dia2/cosyvoice3 default-on rings; PerfMode hardware
auto-detect) applied to the STT track — flipped default-on only after BOTH §8.3 gates: force-solo oracle
green AND live-serve ring mode green.

## Precisions & portability

- `WAAV_MOSSTD_DTYPE=bf16(default)|fp16|f32` — CUDA compute dtype. bf16 = the reference regime
  (byte-gated); fp16 live-verified transcript-identical on the gate clips (WER-tier); f32-CUDA byte-gated.
- `WAAV_MOSSTD_PRECISION=native(default)|sandwich` — `nn::PrecisionMode`; sandwich loads attention
  projections f32 (dia2 recipe), live-verified.
- CPU always f32 — byte-gated (cpu_fp32 cell green). Off-CUDA portability = the tch CPU floor.
- Knobs: `WAAV_MOSSTD_MAX_POS` (RoPE/KV budget, default 32768 ≈ 43 min audio, hard cap 131072),
  `WAAV_MOSSTD_MAX_NEW`, `WAAV_MOSSTD_PROMPT` (or waav.json `prompt`), `WAAV_MOSSTD_MAX_SLOTS/MAX_SEQ`.
- Config-driven: processor_config.json (tokens/s, marker cadence, enable), generation_config.json
  (eos, max_new), waav.json `precision`/`prompt` — a sibling checkpoint of the same arch loads zero-code.

## Acceleration status (follows the STT-fleet pattern)

Fused SDPA everywhere (FusedCausalGqa decoder; the whisper tower rides the FUSED kernel with HF's
pre-scaled-q scale=1.0 spelling); TF32 context parity; batched `[N,…]` encoder (all 30 s chunks in one
forward); device-resident ragged-ring KV (opt-in). CUDA-graph/TRT: the LLM-decoder-ASR track
(voxtral/higgs_stt/granite/ark/canary/mosstd) does not yet route through AccelMapper/ModelSpec — that is
a fleet-wide workstream, not per-model; mosstd inherits it when the track lands. Qwen3 q/k-norm is
present (the TRT-safe predictor), so mosstd is a first-line TRT candidate then.

## The 5 new scars caught during onboarding (the 100%-correctness playbook grew)

1. **HF's mel is the TORCH path, not numpy** — with torch installed, `WhisperFeatureExtractor` computes
   `torch.stft` f32 (numpy path never runs). The pure-Rust extractor agrees only to ~2e-5 → NEW shared
   front-end `asr::hf_mel` (`hf_whisper_mel_cpu`): manual reflect-pad + center-less `stft`
   (verified bit-equal to `torch.stft(center=True)`), `abs()**2` spelling, per-chunk max clamp, and
   `components::mel::slaney_mel_filterbank` verified BITWISE == HF's f64→f32 filterbank (16080/16080).
2. **Fused-LN + at::linear in the whisper tower** — Decomposed-LN/matmul drift ~1.6e-5/layer × 24
   compounds; HF `nn.LayerNorm`/`nn.Linear` are the fused kernels ⇒ `LayerNorm::fused` +
   `Linear::at_linear` throughout tower + adaptor. (Bisection: per-stage golden dumps, conv→pos→L0/11/23→final.)
3. **HF Qwen3 RMSNorm is DECOMPOSED (weight-first, pow(2))**, not the fused `F.rms_norm` higgs's Boson
   reference uses — a per-vendor spelling, now `mk_rms` in mosstd = `decomposed(…, Pow, weight_first=true)`.
4. **RoPE tables in the COMPUTE dtype** — HF returns cos/sin `.to(x.dtype)`; f32 tables promote the
   rotate to f32 (a ~bf16-ulp logits shift) ⇒ `from_inv_freq_full(…, dt)`.
5. **Ring RoPE base position** (FLEET BUG): `RopeApply::Start` consumes `pos`, not the positions list —
   the ring seam passed `0` per stride → every decode rotated at position 0. Found because the
   force-solo oracle was hardened to compare against the TRUE one-shot solo (a singleton-ring reference
   is blind to cohort-consistent bugs). **The same bug existed in higgs_stt's ring** — fixed + its
   oracle re-run green (84 ids max|Δ|=0) + hardened the same way.

Also: fp32-sandwich weight-dtype fix (`ProjPrec::F32Sandwich` needs f32-loaded attention projections),
and a new engine STT boot arm (torch-inprocess `waav.json` STT dirs now load at serve boot — previously
only TTS had the pre-check; without it `--whisper-dir <mosstd>` boot-failed).

## Reuse ledger (modularise + discover-before-write)

REUSED: `nn::Backbone`/`TransformerLayer`/`Attention`(FusedCausalGqa)/`RmsNorm`/`LayerNorm`/`Linear`/
`Mlp`/`Rope`/`KvCache`/`RaggedSlotRing`/`LmHead`(tied)/`PrecisionMode`/`nn::sdpa`; `SttModel`/
`ArStepModel` seams; `SttCoalescer`; higgs_stt module shape + ring template; CI/oracle conventions.
NEW SHARED (not model-private): `asr::hf_mel` (any future HF-torch-mel model), `components::transcript`
(the WaaV-canonical diarized-STT format parser, 12 reference-parity tests), `slaney_mel_filterbank` made pub.
MODEL-PRIVATE: only the MOSS glue (chunk/trim/merge/adaptor, marker-span builder, scatter-merge, decode loop).

## Adversarial review (fresh-eyes agent) — 3 confirmed defects, ALL FIXED + re-gated

1. **Ring silent long-form truncation (Fork-A1 breach)** — the batched ring clamped the transcript budget
   to `max_seq − l` with no signal, truncating exactly the long-form audio this model exists for, and the
   suggested `WAAV_MOSSTD_MAX_SEQ` escape was impossible (hard-capped 8192 by `TORCH_RING_MAX_SEQ`).
   FIX: the higgs reject semantics — `prompt + solo_budget > ring context` → typed clean-reject steering
   to the solo path; the slot budget is pinned to EXACTLY the solo budget (never above/below). Engine
   comment corrected (real capacity ≈ 3.8 min/slot at full budget; 22.5 GiB ring footprint documented).
2. **Ring RoPE-table overrun panic** — `max_seq` (8192) was not bounded by `model.max_pos`; with
   `WAAV_MOSSTD_MAX_POS=4096` a mid-length clip's decode would `narrow` past the cos/sin tables → PANIC.
   FIX: `max_seq = max_seq.min(model.max_pos)` pinned at `TorchMossTdBatched::new`.
3. **Solo silent trim / silent-empty** — `max_new` clamped to `max_pos − l` silently; at `l == max_pos`
   an empty transcript was returned. FIX: shared `effective_max_new` — typed reject at zero budget,
   `tracing::warn!` on any clamp (live-verified: reject fires on a 184 s clip @ MAX_POS=2048; the warn +
   full transcription fire @ MAX_POS=600). Doc arithmetic corrected (≈34 min at full budget, ≈40 min ceiling).

Nits fixed: span-builder extracted to a free fn so unit tests exercise PRODUCTION code; prompt `.trim()`
(reference parity) + load-time reject of prompts containing audio placeholder tokens; `eos_token_id` list
form accepted; `transcribe` now delegates to `transcribe_ids` (decode-loop dedupe); adaptor shape check
without a device round-trip; gate long-form arm handles the strict-prefix case; format-string space runs.
Reviewer verified clean (line-by-line vs the reference): span-builder math incl. >40-min multi-digit
markers, chunk boundaries (480 000/480 001), scatter placeholder-count fail-closed, ring slot lifecycle,
transcript parser state machine (incl. the >32-char overflow duplicate-char quirk), engine boot
warn-vs-fail semantics, hf_mel op sequence. ALL gates re-run green after the fixes.

## Files

- `crates/waav-infer-backend-torch/src/mosstd.rs` (+ lib.rs exports)
- `crates/waav-infer-backend-torch/src/asr/hf_mel.rs` (+ asr/mod.rs)
- `crates/waav-infer-components/src/transcript.rs` (+ lib.rs), `mel.rs` (pub filterbank)
- `crates/waav-infer-server/src/engine.rs` (arch arm + STT boot pre-check)
- `crates/waav-infer-backend-torch/tests/{cuda_torch_mosstd.rs, mosstd_force_solo_codes.rs}`
- `crates/waav-infer-backend-torch/src/higgs_stt.rs` (fleet ring-pos fix) + `tests/higgs_stt_force_solo_codes.rs` (hardened oracle)
- `ci/heavy_live_tests.sh` (g3 + g3b entries), `eval/mosstd_eval.py`, `torch_runtime/dump_mosstd_golden.py`
- `crates/waav-infer-server/tests/fixtures/torch_inprocess/mosstd.waav.json`, `gui/app.py`

Goldens: `/tmp/mosstd_golden/{jfk,mosstd_multispeaker,mosstd_longform}/{cuda_bf16,cpu_fp32}/` via
`torch_runtime/dump_mosstd_golden.py <clip> <device> <dtype>` (WAAV_MOSSTD_DIR=model dir).
Model dir: `~/.cache/waav-models/moss-transcribe-diarize` (materialized snapshot + `waav.json`).
