# Onboard: pnnbao-ump/VieNeu-TTS-v3-Turbo (Vietnamese RVQ codec-AR TTS)

**Status: ONBOARDED (new arch, minimal code) — live RTF on GB10 + BYTE-FAITHFUL accuracy. The Rust ORT
pipeline reproduces a Python-onnxruntime golden byte-for-byte (4800/4800 greedy RVQ codes = 300/300 frames ×
16 codebooks, 0 diffs) and the MOSS codec wav is byte-identical (maxΔ = 0.0 over 1.15M samples). Synthesizes
real 48 kHz Vietnamese audio end-to-end through the production registry seam, RTF 0.470 (release CPU-ORT).**

**NOT a NeuTTS reuse.** The prompt named NeuTTS-Air as the likely arch — it is NOT. VieNeu v3 Turbo is a
genuinely **new, from-scratch architecture** (`VieNeuV3TurboForTTS` / `model_type: vieneu_v3_turbo`) that
merely shares the NeuTTS-family *special-token vocabulary*. It is the structural **cousin of `moss_tts_nano`**
(MOSS-TTS-Nano), not of neutts — same 768/12 backbone, same 16-VQ frame, same MOSS-Audio-Tokenizer-Nano
48 kHz codec. Onboarded as a new core module in the moss/chatterbox pure-ORT pattern.

| | |
|---|---|
| Model | VieNeu-TTS v3 Turbo — Vietnamese / En-Vi (code-switching) codec-AR TTS, native **48 kHz**, 10 built-in voices + instant cloning, by Phạm Nguyễn Ngọc Bảo |
| Arch | `vieneu_v3_turbo` (`VieNeuV3TurboForTTS`) — **Qwen3-style** semantic backbone (h768, 12L, GQA 12q/4kv, head_dim 64, RoPE θ1e4, **q/k RMSNorm, no qkv-bias**) → **2-layer acoustic RVQ depth-decoder** (8 heads, head_dim 96, learned `slot_pos_emb`, NO RoPE) emitting a **16-codebook RVQ frame**/step |
| Triage tier | MODERATE |
| Official repo | `pnnbao-ump/VieNeu-TTS-v3-Turbo` — **ungated, public** — VERIFIED EXISTS (HfApi) |
| Codec | `OpenMOSS-Team/MOSS-Audio-Tokenizer-Nano` (48 kHz) — already cached as `moss-tts-nano` codec; reused |
| Onboarding | **new arch, minimal code** — one new core module (`tts/vieneu.rs`, ~640 lines) + a registry arm; reuses the moss codec ONNX, the shared `StaticGraph`/`NamedTensor` ORT seam, `GaussianNoise`, `audio` |
| Accuracy | **BYTE-FAITHFUL**: greedy RVQ codes 4800/4800 == Python-ORT golden (0 diffs, 300 frames × 16); codec wav maxΔ = 0.0 (1.15M samples) |
| Live RTF | **CPU-ORT 0.470** (release, GB10 ARM, 8 threads) — faster than real time. CUDA EP: see §7 |

---

## 1. HfApi verification (method step 1)

Verified real via `HfApi.model_info` + `list_repo_files` (HF_TOKEN). `pnnbao-ump/VieNeu-TTS-v3-Turbo`:
**gated=False, private=False**. Files: `config.json`, `model.safetensors` (262 MB bf16 PyTorch),
`tokenizer.json` (BPE, 419 phoneme/special tokens), `speaker_encoder.onnx` (mel→speaker emb, for cloning),
and the torch-free ONNX export `onnx/{vieneu_prefill,vieneu_decode_step,vieneu_acoustic_cached}.onnx` +
`vieneu_backbone_shared.data` (415 MB f16 external weights, shared by prefill+decode) +
`vieneu_v3_heads.npz` (tied embeddings/heads).

**Config diff vs neutts-air — this is the headline finding (NOT the same arch):**

| | neutts-air | VieNeu-v3-Turbo |
|---|---|---|
| arch | `Qwen2ForCausalLM` | `VieNeuV3TurboForTTS` (`vieneu_v3_turbo`) |
| backbone | Qwen2-0.5B, h896/24L/14q-2kv, **qkv-bias, no q/k-norm** | **Qwen3** h768/12L/12q-4kv, **q/k RMSNorm, no qkv-bias** |
| heads | flat **FSQ** token stream (65536-vocab tail) | **2-stage RVQ**: 16 codebooks × 1024 via an acoustic depth-decoder |
| codec | NeuCodec (24 kHz, in-dir ONNX) | **MOSS-Audio-Tokenizer-Nano (48 kHz)**, external repo |
| text vocab | 217 652 (Qwen2) | 419 (sea-g2p phonemes) |
| tie_word_emb | true | false (heads tied to embeddings separately) |

So neutts.rs (`crates/waav-infer-backend-torch/src/neutts.rs`) is **not reusable** — different backbone
recipe, different head topology, different codec. The correct cousin is `moss_tts_nano`.

## 2. Acquire (step 2)

`snapshot_download` → `~/.cache/waav-models/vieneu-tts-v3-turbo/` (the 4 ONNX artifacts + safetensors +
tokenizer + speaker_encoder). Composed into the registry layout (no model.safetensors path needed — the
ONNX export is the serving path, no-venv-compliant):

- `vieneu_v3_heads.npz` → extracted to **`text_emb_f32.bin`** `[419,768]` + **`audio_emb_f32.bin`**
  `[16,1024,768]` (flat little-endian f32 — the host embedding/head tables).
- `voices_v3_turbo.json` — the **10 built-in default voices** (each: `reserved_id` 13..42 + `[n_frames,16]`
  reference RVQ codes), copied from the `vieneu` pip wheel's `assets/` (the HF repo ships no voices file).
- MOSS codec decoder **symlinked** from the existing `moss-tts-nano` cache
  (`moss_audio_tokenizer_decode_full.onnx` + `_shared.data`) — zero re-download.
- `waav.json` — `{"architecture":"vieneu_v3_turbo","weights":{prefill,decode_step,acoustic,codec_decode}}`.

The reference algorithm was recovered from the **`vieneu` 3.0.5 pip wheel** (`_v3_turbo_engine/`,
`onnx_runtime_lite.py` — the author's torch-free ORT engine) — **read for the port only, never installed/
served** (no-venv rule [[waav-infer-no-venv-wrap]]). The wheel also carries the default-voice presets.

## 3. Integrate (step 3) — the pipeline

VieNeu's ONNX export is the **embeddings-external / heads-external / host-sampling** variant (unlike moss,
which bakes embeddings+heads+sampling into the graphs). The faithful port (`tts::vieneu`, the moss/chatterbox
pure-ORT host-loop pattern — every matmul in a shared ONNX graph, embeddings/heads/sampling in host Rust):

1. **prompt rows** `[T,17]`: col 0 = `[reserved_id, <|TEXT_PROMPT_START|>, phoneme ids…, <|TEXT_PROMPT_END|>]`;
   then the voice's reference RVQ codes under `<|audio_ref_slot|>` rows (cols 1..16). audio_pad elsewhere.
2. **embed** (host): `text_emb[row0] + Σ_ch audio_emb[ch][row_{ch+1}]` (pad-masked) → `inputs_embeds[1,T,768]`.
3. **prefill** (`vieneu_prefill.onnx`, once) → backbone `hidden` + 12-layer KV.
4. **per-frame loop**: the last backbone hidden drives the acoustic depth-decoder
   (`vieneu_acoustic_cached.onnx`, run 16× per frame with its own 2-layer KV): prefill slots
   `[h, text_emb[SGS]]` → codebook-0 logits = `hidden[0,1] @ audio_emb[0]ᵀ` → sample; for ch=1..15 feed
   `audio_emb[ch-1][code]` as one cached step → `hidden[0,0] @ audio_emb[ch]ᵀ` → sample. The slot-0 output's
   **text** head argmax == `<|SPEECH_GENERATION_END|>` ends the utterance. The 16 codes become the next
   backbone `decode_step` row (col 0 = `<|SPEECH_GENERATION_START|>`).
5. **codec decode**: `moss_audio_tokenizer_decode_full(codes[1,T,16]) → [1,2,48000·s]`, mono-downmixed.

Sampler chain (byte-faithful to `OnnxV3LiteEngine._sample`): `repetition_penalty(1.2, per-channel-seen) →
temperature(0.8) → top-k(25) → top-p(0.95) → softmax → inverse-CDF(u)`. Greedy = temperature 0 → argmax
(the RNG-free byte-identity gate). The `u`-schedule is a seedable PCG32 (`GaussianNoise`) keyed on
`(voice,text)` for reproducible production audio.

## 4. Smoke (step 4)

Real Vietnamese synth through the engine seam (`VieneuTts::synthesize`): 4.64 s of 48 kHz audio, non-silent
(peak 0.72, rms 0.17), 10 voices listed. Also reproduced end-to-end via the reference `OnnxV3LiteEngine`
(4.24 s, peak 0.61) → `/tmp/vieneu_ref_out.wav`.

## 5. Accuracy (step 5) — BYTE-FAITHFUL

Live test `crates/waav-infer-core/tests/vieneu_live.rs` (CPU EP, the byte-faithful regime):

- **gate 1 (THE BAR)** — GREEDY (temp 0 → argmax, RNG-free) RVQ codes for the same prompt the Python-ORT
  golden used: **0 differing codes over 300 frames × 16 codebooks (4800/4800 byte-identical)**. Both
  runtimes call the SAME ONNX graphs with the SAME host embedding/head math ⇒ identical codes (cross-runtime
  byte-identity, the codec-AR accuracy bar).
- **gate 2** — `decode_codes(greedy)` == the golden MOSS codec wav: **maxΔ = 0.0** over 1 152 000 samples
  (same ONNX, byte-identical by construction).
- **gate 3** — production sampled `synthesize`: non-silent, RTF reported.

Golden produced by the reference `OnnxV3LiteEngine` in greedy mode at the matching 300-frame cap →
`~/.cache/waav-models/vieneu-golden/{greedy_frames.npy, greedy_wav.npy, meta.json}`.

## 6. Perf (step 6) — RTF

| build | EP | RTF | note |
|---|---|---|---|
| debug | CPU-ORT (8 thr) | 2.615 | unoptimized |
| **release** | **CPU-ORT (8 thr)** | **0.470** | **≪ 1, faster than real time** |

GB10 ARM CPU, 4.64 s utterance in 2.18 s. The acoustic depth-decoder (16 ORT calls/frame) dominates.

## 7. CUDA EP note

The acoustic graph is **int8-quantized** (`DynamicQuantizeLinear`/`MatMulInteger`); the backbone graphs are
f16 with a shared `.data`. The Rust ORT backend has a CUDA EP, but the quantized acoustic nodes fall back to
CPU on CUDA EP regardless, and the host system's *Python* onnxruntime is CPU-only on aarch64 (no GPU wheel —
the known [[waav-build-network-env]] / voxtral constraint), so the byte-faithful golden was generated on CPU.
CPU-ORT release RTF 0.470 already clears the real-time bar; CUDA would only help the f16 backbone graphs.

## 8. Files (exact)

**New:**
- `crates/waav-infer-core/src/tts/vieneu.rs` — the model (~640 lines).
- `crates/waav-infer-core/tests/vieneu_live.rs` — the 3-gate live test.

**Shared touches (concurrent-agent files — minimal, additive):**
- `crates/waav-infer-core/src/tts/mod.rs` — `pub mod vieneu;` + `pub use vieneu::{VieneuError, VieneuTts};`
- `crates/waav-infer-core/src/lib.rs` — added `VieneuTts` to the `tts::{…}` re-export.
- `crates/waav-infer-core/src/model.rs` — `use crate::tts::vieneu::VieneuTts;` + the `"vieneu_v3_turbo" |
  "VieNeuV3TurboForTTS"` registry arm + `"vieneu_v3_turbo"` in `REGISTERED_ARCHITECTURES` + bumped the
  registry-count invariant test 21→22 (a genuinely new architecture arm — exactly what that counter grows for).

**Cache (not in repo):** `~/.cache/waav-models/vieneu-tts-v3-turbo/` (model + `*_f32.bin` heads +
`voices_v3_turbo.json` + `waav.json` + MOSS codec symlinks); `~/.cache/waav-models/vieneu-golden/`.

## 9. Gate results

- `cargo test -p waav-infer-core --lib` — **75 passed** (incl. the bumped registry invariant).
- `cargo test -p waav-infer-core --test vieneu_live -- --ignored` — **green** (3 gates: byte-faithful greedy
  codes, byte-faithful codec wav, real sampled synth + RTF).
- `cargo clippy -p waav-infer-core --all-targets` — **clean** (0 warnings).

## 10. Deferred / not done

- **Voice cloning** (`_encode_ref` via `speaker_encoder.onnx` + MOSS encode ONNX) — not wired; the engine
  `Tts` seam takes only `(text, voice, speed)`, so default-voice presets are the served path. Cloning would
  add the MOSS *encode* graph + the speaker encoder (both available) behind a `ref_audio` extension.
- **Phonemization** — the engine seam passes `text`; faithful Vietnamese phonemes require the author's
  `sea-g2p` (a separate G2P, not in `components::g2p`). The byte-faithful gate feeds a fixed phoneme string
  (isolating the AR math from the G2P choice, the neutts-gate idiom); a `sea-g2p` port or the existing
  `Phonemizer` at the boundary is the live-text follow-up.
- **Emotion cues** (`[cười]`/`[thở dài]`/`[hắng giọng]` → `<|emotion_k|>`) — the tokens exist; the inline-tag
  splitter is a thin text-preprocess add when the seam carries cue markup.
