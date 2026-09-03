# Onboard: bosonai/higgs-audio-v3-stt-v2 (Whisper→Qwen3 speech-LLM ASR)

**Status: ONBOARDED + LIVE-VERIFIED on GB10 CUDA. Transcripts BYTE-IDENTICAL to the bosonai reference.**

## Verdict (the LAW)

A real transcribe + WER + RTF on GB10, byte-faithful greedy:

| clip (LibriSpeech dummy) | dur | WaaV-Rust HYP | Reference HYP | identical? | WER vs GT |
|---|---|---|---|---|---|
| clip0 | 5.9 s | `mr quilter is the apostle of the middle classes and we are glad to welcome his gospel` | (same) | **YES** | 0.059* |
| clip1 | 4.8 s | `nor is mister quilter's manner less interesting than his matter` | (same) | **YES** | 0.000 |
| clip2 | 12.5 s | `he tells us that at this festive season of the year with christmas and roast beef looming before us similes drawn from eating and its results occur most readily to the mind` | (same) | **YES** | 0.000 |

- **Mean WER = 0.020** for BOTH WaaV-Rust and the reference engine (identical). The only residual is the
  `mr`↔`MISTER` normalizer artifact on clip0 — the transcripts are word-perfect.
- **All 3 transcripts are BYTE-IDENTICAL between the WaaV Rust engine and the bosonai reference** (greedy
  argmax, do_sample=False). This is the accuracy gate: WaaV reproduces the reference output exactly.
- **RTF on GB10 (warm, debug build, CUDA bf16):** clip0 0.45, clip1 0.41, clip2 0.33 — comfortably real-time
  (<1.0). Reference RTF (eager attn) 0.71/0.24/0.14. Load ≈4.7 s warm.

\* clip0's 0.059 is entirely the abbreviation-normalization mismatch (`mr` vs `MISTER`); both engines emit the
identical `mr` string, so there is ZERO WaaV-vs-reference disagreement.

## HfApi verification (the 3 triage candidates — all EXIST, all UNGATED)

| repo | params | size | gated | picked |
|---|---|---|---|---|
| `bosonai/higgs-audio-v3-stt` | 2.68 B | 5.37 GB | no | — (base, ships eval harness + `transcribe.py`) |
| `bosonai/higgs-audio-v3-stt-v2` | **2.07 B** | 5.37 GB | no | **YES** (smallest, same arch; one model at a time on GB10) |
| `bosonai/higgs-audio-v3-8b-stt-v2` | 8.91 B | 17.83 GB | no | — (8B; deferred, wires zero-code if needed) |

All three share the SAME `HiggsAudio3Model` / `model_type higgs_audio_3` architecture. The `-v2` 2.07B
checkpoint was acquired to `~/.cache/waav-models/higgs-stt/` (5.0 GB, 2 shards).

## Architecture (reused the higgs/granite/voxtral seams; the encoder is the new glue)

NOT the higgs-tts codec-AR backbone — this STT variant is a DISTINCT arch (the ASR direction):

- **audio tower** (`audio_tower.*`): a standard **OpenAI/HF Whisper-large-v3 encoder** — 2-conv stem (conv1
  128-mel→1280 k3 pad1, conv2 1280→1280 k3 **stride 2** pad1, both erf-gelu) + **learned absolute positions**
  (`embed_positions[1500,1280]`, NOT RoPE) + 32-layer pre-norm **bidirectional** tower (LayerNorm; 20 heads ×
  head_dim 64; ungated-GELU FFN 1280→5120→1280; q/v/out_proj bias, k_proj none) + **AvgPool1d(2)** time
  downsample + final layer_norm. *(The avg-pool-before-final-norm + learned positions are the higgs distinction
  vs the shared `crate::asr` RoPE-stem encoder → model-specific glue, but composes the shared `nn` primitives.)*
- **projector** (`audio_encoder_proj.*`): stride-2 depthwise temporal conv (`[1280,1,3]` groups=1280) →
  linear1 1280→2048 → **ReLU** → linear2 2048→2048.
- **text backbone** (`layers.{0..27}` + `embed_tokens` + `norm`): a **plain Qwen3-1.7B-Base** GQA decoder
  (28L, hidden 2048, 16 q / 8 kv × head_dim 128, per-head q/k RMSNorm, eps 1e-6, SwiGLU inter 6144, RoPE
  θ=1e6) — **reused the `crate::higgs` / `qwen3_tts` Qwen3 composition verbatim** (`nn::Backbone` +
  `build_qwen3_layer`-style glue), just different dims.
- **text head** (`audio_decoder_proj.text_lm_head.weight [151936,2048]`): a SEPARATE (un-tied) Linear;
  `logits = text_lm_head(norm(hidden))`.

**Did it reuse the higgs backbone?** YES for the Qwen3 decoder (the higgs/qwen3 `nn::Backbone` composition).
The Whisper mel encoder + projector are NEW glue (the higgs-TTS sibling has a DAC codec, not a Whisper tower),
but they compose only shared `nn::LayerNorm`/`nn::Linear`/`nn::Mlp(ungated GELU)`/`nn::sdpa_manual`. The mel
frontend is the SHARED `waav_infer_components::LogMelExtractor::new(128)` (Whisper-large-v3: n_fft 400, hop
160, periodic Hann, center reflect-pad, 128 Slaney mel, log10→clamp max-8→(x+4)/4).

## ASR forward (faithful to `transcribe.py` + `HiggsAudio3Model.forward`)

1. mel → audio tower → projector → `audio_feats[1,N,2048]`.
2. ChatML prompt: `<|im_start|>user\n{PROMPT}<|audio_bos|><|AUDIO|><|audio_eos|><|im_end|>\n<|im_start|>assistant\n`.
   The SINGLE `<|AUDIO|>` (id 151672) placeholder is filled with the N audio embeds (the reference's
   `merge_input_ids_with_audio_features` expands `token_placeholder_num[audio]=N`). **No embedding multiplier**
   (plain Qwen3, unlike granite ×12).
3. Qwen3 decoder over the merged embeds → `norm` → `text_lm_head` → **greedy argmax (f32, first-max
   tie-break)**; stop on `<|im_end|>` (151645) / `<|endoftext|>` (151643). Strip `<think>…</think>` CoT block.

### The load-bearing fix (4 s encoder chunking)

The config carries `chunk_size_seconds=4.0` + `vad_cut=true`. The reference collator ALWAYS splits the
waveform into ≤4 s windows (`_chunk_post_process_helper` over the single full-audio cut — `transcribe.py`
provides no real VAD cuts), whisper-encodes EACH window **separately**, duplicates the `<|AUDIO|>` placeholder
num_chunks times, and **concatenates** the per-window features. A naive single-window encode regressed clip0
(tail hallucination, WER 0.18) and clip2 (repetition loop, WER 0.59). Replicating the 4 s windowing inside
`encode()` (encode each ≤4 s window, concat the features, one greedy decode over the concatenated stream)
fixed both to **byte-identical / WER 0.000**. This is the single non-obvious detail; it is now correct.

## Faithfulness regime

bf16 on CUDA (the reference `torch_dtype=torch.bfloat16`), f32 on CPU — `tch` IS libtorch, so the same math ⇒
the same bytes. The 8-bug playbook scars handled: fused RMSNorm (`nn::RmsNorm::Fused`), bf16 dtype, Qwen
tokenizer.json, RoPE inv_freq f32-tensor-arange + f32 tables, TF32 on the f32 head projection, greedy
argmax(f32) first-max tie-break, symmetric conv padding, single batch row.

## Files

**Added:**
- `crates/waav-infer-backend-torch/src/higgs_stt.rs` — the model (`TorchHiggsStt`, `SttModel`).
- `crates/waav-infer-backend-torch/tests/higgs_stt_live.rs` — the ignored live WER/RTF smoke (CUDA-gated).

**Changed (FLAGGED — SHARED files, note for the coordinator):**
- `crates/waav-infer-backend-torch/src/lib.rs` — `pub mod higgs_stt;` + `pub use higgs_stt::{HiggsSttError, TorchHiggsStt};` (additive).
- `crates/waav-infer-server/src/engine.rs` — registry: added the `"higgs_stt" | "higgs_audio_3" | "HiggsAudio3Model"` arch arm + import + the unknown-arch error message (additive, mirrors the granite arm).

## Gates

- `cargo test -p waav-infer-backend-torch --lib` → **151 passed / 0 failed** (148 prior + 3 new higgs_stt).
- `cargo clippy -p waav-infer-backend-torch --lib --tests` → **clean** (no higgs_stt warnings).
- `cargo build -p waav-infer-server` → **clean** (engine.rs registry change compiles).
- Live: `cargo test -p waav-infer-backend-torch --test higgs_stt_live -- --ignored --nocapture`.

## Reference engine note

The bosonai custom `generate()` is hard-coupled to transformers==4.51.0 (the installed system transformers is
5.12.0; its `_sample`/`generation_kwargs`/whisper-mask APIs broke the custom loop). The reference was run by
installing `transformers==4.51.0` against the system torch 2.12 (`venv --system-site-packages`) with
`attn_implementation="eager"` (a torch-2.12 SDPA mask-contiguity quirk, not a model issue). Under that the
model's own `transcribe.py` ran the real audio tower + projector + Qwen3 forward + text_lm_head greedily — and
produced the transcripts above, byte-identical to WaaV-Rust.

## Manifest (to serve via the engine)

`waav.json` runtime block: `architecture: "higgs_stt"` (or `higgs_audio_3` / `HiggsAudio3Model`), weights =
the model dir (sharded safetensors + tokenizer.json). The engine dispatches to `TorchHiggsStt::load` →
`LoadedModel::Stt`.
