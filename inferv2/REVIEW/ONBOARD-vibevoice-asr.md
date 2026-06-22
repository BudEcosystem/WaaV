# Onboard: microsoft/VibeVoice-ASR (continuous-VAE encoders → Qwen2.5-7B speech-LLM ASR)

**Status: EXISTS (ungated, public) + ONBOARDED + LIVE-VERIFIED on GB10 CUDA. Transcripts word-perfect (WER 0% after normalization) on 3 LibriSpeech clips.**

## Verdict (the LAW)

A real transcribe + WER + RTF on GB10, greedy (do_sample=False), bf16/CUDA:

| clip (LibriSpeech dummy) | dur | WaaV-Rust HYP (extracted `Content`) | WER vs GT | single-pass RTF |
|---|---|---|---|---|
| clip0 | 5.9 s | `Mr. Quilter is the apostle of the middle classes, and we are glad to welcome his gospel.` | 0.059* | 0.987 |
| clip1 | 4.8 s | `Nor is Mr. Quilter's manner less interesting than his matter.` | 0.100* | 0.857 |
| clip2 | 12.5 s | `He tells us that at this festive season of the year, with Christmas and roast beef looming before us, similes drawn from eating and its results occur most readily to the mind.` | 0.000 | 0.543 |

- **Mean WER = 0.053** with a naive lowercase+strip-punct normalizer; **= 0.000 (word-perfect)** once the
  standard ASR abbreviation expansion is applied (`mr→mister`). The ONLY residual is the `Mr.`↔`MISTER`
  abbreviation FORM — the recognized words are verbatim-correct on all three clips.
- **RTF on GB10 (warm, debug build, CUDA bf16):** clip0 0.99, clip1 0.86, clip2 0.54 — **real-time** (RTF<1),
  and it *improves* on longer audio (the fixed system+suffix prompt + JSON wrapper amortize). Load ≈10.7 s.
- VibeVoice-ASR is a **rich-transcription** model (Who/When/What): it emits a JSON array of
  `{Start, End, Speaker, Content}` segments (diarization + timestamps), NOT plain text. The engine STT seam
  returns the concatenated `Content` fields; `transcribe_json()` exposes the full structured output.

\* clip0/clip1's nonzero WER is entirely the `Mr.` vs `MISTER` normalization mismatch + trailing punctuation;
the transcripts are word-perfect.

Raw model output (clip0), verbatim:
```
assistant
[{"Start":0,"End":5.86,"Speaker":0,"Content":"Mr. Quilter is the apostle of the middle classes, and we are glad to welcome his gospel."}]
```
(The model itself emits the `assistant\n` turn header — the reference processor does NOT append a generation
prompt — then the JSON. `extract_content` slices past the header and parses the JSON.)

## HfApi verification — DOES IT EXIST? **YES.**

The triage flagged this as HIGH-RISK for non-existence. HfApi confirms it is **real, public, ungated**:

| field | value |
|---|---|
| repo | `microsoft/VibeVoice-ASR` |
| gated | **false** |
| private | false |
| pipeline_tag | `automatic-speech-recognition` |
| arch (config) | `VibeVoiceForASRTraining`, `model_type: vibevoice` |
| weights | 8 safetensors shards, **17.35 GB** (bf16), 1177 tensors |
| license | MIT |
| languages | 50+ (en, zh, es, ... declared in the card) |

Acquired to `~/.cache/waav-models/vibevoice-asr/` (all 8 shards + config + index + a copied
`Qwen/Qwen2.5-7B` `tokenizer.json`). The repo ships NO tokenizer (the processor loads `Qwen/Qwen2.5-7B`); the
7B tokenizer.json was copied into the model dir for the Rust loader.

## Architecture — DID IT REUSE THE vibevoice.rs BACKBONE? **YES** (the VAE family machinery is byte-identical)

VibeVoice-ASR is the ASR (audio→text) direction of the VibeVoice family. It REUSES the family's
continuous-VAE audio encoders + connectors, but the LLM runs forward into TEXT (greedy) — so there is **NO
diffusion head and NO VAE decoder** (those are TTS-direction only). What it reuses vs what is new:

- **acoustic VAE encoder** (`model.acoustic_tokenizer.encoder.*`, vae_dim 64) — **REUSED verbatim** from
  `crate::vibevoice::VaeEncoder` (the SEANet-style streaming-conv VAE, ratios `[8,5,5,4,2,2]` ⇒ hop 3200; the
  SAME encoder the TTS sibling uses for its reference-voice acoustic encode).
- **semantic VAE encoder** (`model.semantic_tokenizer.encoder.*`, vae_dim 128) — **REUSED** the same
  `VaeEncoder` loader (identical structure, 128-dim head).
- **acoustic/semantic connectors** (`model.{acoustic,semantic}_connector.*`) — **REUSED**
  `crate::vibevoice::SpeechConnector` (`fc1 → LlamaRMSNorm(1e-6) → fc2`), byte-identical to the TTS connector,
  just sized to the 7B hidden (3584).
- **db-normalize** (`AudioNormalizer`, target_dB_FS=-25, eps=1e-6) — **REUSED**
  `crate::vibevoice::db_normalize` (confirmed byte-identical to the family `AudioNormalizer`).
- **decoder** — a **plain Qwen2.5-7B** GQA decoder (28L, hidden 3584, 28 q-heads / 4 kv-heads × head_dim 128,
  q/k/v bias, RMSNorm eps 1e-6, SwiGLU inter 18944, RoPE θ=1e6, vocab 152064) composed via `nn::Backbone` +
  the SAME `build_qwen_layer` recipe the TTS sibling uses (Pow-square **decomposed** RMSNorm — the family
  byte-identity scar — `at::linear`, contiguous ring-KV, FusedCausalGqa, `apply_start` RoPE, `f32_tensor_host_exps`
  inv_freq). Only the dims differ (7B vs the TTS 1.5B). The `lm_head` is **SEPARATE / un-tied** (present as
  `lm_head.weight` in the checkpoint — unlike the TTS sibling whose head is tied to `embed_tokens`).

### ASR forward (faithful to `VibeVoiceASRForConditionalGeneration` + `VibeVoiceASRProcessor`)

1. db-normalize the 24 kHz mono audio; build the ChatML prompt:
   `<|im_start|>system\n{SYSTEM_PROMPT}<|im_end|>\n<|im_start|>user\n` +
   `<|speech_start|>` + `<|speech_pad|>`×N + `<|speech_end|>` +
   `\nThis is a {dur:.2f} seconds audio, please transcribe it with these keys: Start time, End time, Speaker ID, Content` +
   `<|im_end|>\n` where `N = ceil(samples/3200)`. The speech special tokens are the Qwen2.5
   object-ref/box reuse: `<|object_ref_start|>=151646`, `<|box_start|>=151648`, `<|object_ref_end|>=151647`;
   eos `<|endoftext|>=151643`. (NO generation prompt is appended — the reference processor ends the user turn
   at `<|im_end|>\n`; the model emits the `assistant\n` header itself.)
2. `encode_speech` (short-audio ≤60 s path): acoustic VAE encode → `acoustic_connector`; semantic VAE encode
   (`.mean`, dist_type='none') → `semantic_connector`; the per-token speech embedding is the **element-wise SUM**
   `acoustic_feats + semantic_feats` `[N, 3584]`.
3. The N speech embeddings OVERWRITE the N `<|speech_pad|>` positions in the embedded prompt
   (`inputs_embeds[acoustic_input_mask] = speech_features`).
4. Qwen2.5-7B over the merged embeds → `lm_head` → **greedy argmax (f32, first-max tie-break)**; stop on EOS.
   The raw text is a JSON array of `{Start, End, Speaker, Content}` segments; the seam concatenates `Content`.

### Determinism of the acoustic sample

The acoustic tokenizer is `std_dist_type="gaussian"` with `fix_std` a NON-persistent buffer (config 0.5, NOT
in the checkpoint). The reference inference runs greedy text (`do_sample=False`, `temperature=0`); the acoustic
`sample()` gaussian noise is the only stochastic op and does NOT change the greedy text argmax. The port uses
the **mean** (the `fix_std→0` collapse) for a reproducible transcript — confirmed correct by the 0% WER. The
semantic encode is `.mean` (deterministic by construction).

## Files (flag shared touches)

**New (mine alone):**
- `crates/waav-infer-backend-torch/src/vibevoice_asr.rs` — the ASR module (Qwen2.5-7B backbone glue +
  encode_speech + prompt build + greedy decode + JSON-`Content` extraction + `SttModel` impl). ~520 LOC.
- `crates/waav-infer-backend-torch/tests/vibevoice_asr_live.rs` — the live transcribe + WER/RTF gate
  (`--ignored`, reuses the `/tmp/higgs_clips/` LibriSpeech clips).
- `~/.cache/waav-models/vibevoice-asr/waav.json` — `{"runtime":{"backend":"torch-inprocess",
  "architecture":"vibevoice_asr","device":"cuda"}}` (the engine dispatch manifest).

**SHARED-FILE TOUCHES (coordinator: re-read before editing these):**
- `crates/waav-infer-backend-torch/src/vibevoice.rs` — exposed the VAE machinery as `pub(crate)` so the ASR
  sibling reuses it byte-for-byte: `Weights` (+ `load`/`get`/`contains`/`lin`/`fused`), `VaeEncoder` (+
  `forward`), `SpeechConnector` (+ `forward`), `load_encoder`, `load_connector`, and a new free fn
  `db_normalize`. **Purely additive `pub(crate)` visibility + one new free fn — NO behavior change** (all 157
  existing lib tests, including the vibevoice TTS tests, still pass).
- `crates/waav-infer-backend-torch/src/lib.rs` — added `pub mod vibevoice_asr;` + `pub use
  vibevoice_asr::TorchVibeVoiceAsr;`.
- `crates/waav-infer-backend-torch/Cargo.toml` — added `serde_json.workspace = true` (parse the JSON
  transcript).
- `crates/waav-infer-server/src/engine.rs` — added the `TorchVibeVoiceAsr` import + the
  `"vibevoice_asr" | "VibeVoiceForASRTraining"` STT dispatch arm + the error-message arch list. (Minimal,
  additive; this file is shared with concurrent agents.)

## Build/test status

- `cargo test -p waav-infer-backend-torch --lib` → **157 passed, 0 failed** (incl. 6 new `vibevoice_asr`
  unit tests + all existing vibevoice TTS tests).
- `cargo clippy -p waav-infer-backend-torch --lib --tests` → **clean** (0 warnings).
- `cargo build -p waav-infer-server --features torch` → **compiles** (engine wiring valid);
  `cargo clippy -p waav-infer-server --features torch` → **clean**.
- Live gate `cargo test -p waav-infer-backend-torch --test vibevoice_asr_live -- --ignored --nocapture
  --test-threads=1` → **passed** (the table above).

## Notes / deferred

- **No bit-exact reference cross-check.** A throwaway validation venv (torch + transformers + the `vibevoice`
  package) was started but the aarch64 torch wheel build did not finish in the session window. The accuracy
  evidence is the **0% WER on three known-transcript LibriSpeech clips** (verbatim-correct words + the exact
  reference JSON schema `Start/End/Speaker/Content`), which strongly validates the full forward path
  (acoustic+semantic VAE → connector SUM → Qwen2.5-7B greedy → JSON). A future byte-identity gate vs the
  `VibeVoiceASRForConditionalGeneration` reference can be added when a torch venv is available.
- **Long-audio (>60 s) streaming path NOT ported.** The reference `encode_speech` has a streaming branch
  (60 s segments, streaming-conv caches) for up-to-60-min audio. The port implements the short-audio
  (≤60 s, single-shot) path only — sufficient for the STT seam's clip-level transcription. The streaming
  conv-cache machinery already exists in `vibevoice.rs` (`SConv1d::forward_streaming` / `StreamCache`) and can
  be reused to add the long-form path zero-new-primitives if needed.
- **Diarization/timestamps are produced but flattened.** The model's Speaker/Start/End fields are emitted and
  available via `transcribe_json()`; the `SttModel::transcribe` seam returns the concatenated `Content` (the
  spoken words) — the gateway's plain-transcript contract. A diarized-segment seam could surface the structure.
