# ONBOARD — LiquidAI/LFM2.5-Audio-1.5B-JP (Japanese S2S / STS, codec-AR audio LLM)

**Status: ONBOARDED ✅ — ZERO-CODE (config + weights only).** The Japanese variant of LFM2.5-Audio serves
through the EXACT SAME `lfm2_audio_asr`/`lfm2_audio_tts` registry archs and the SAME `Lfm2Audio` core as
the EN model (onboarded `c56a97d`). NO production Rust touched, NO new registry entry, NO
`REGISTERED_ARCHITECTURES` bump. Real JP round-trips live-verified on GB10, **byte-faithful** to the JP
onnxruntime golden. `cargo test -p waav-infer-core --lib` green (75/0, unchanged) + the new JP live test
3/3 + clippy clean.

---

## 1. Verification (HfApi-first) — and the ONE real obstacle

| Repo | Exists? | Gated | Contents |
|------|---------|-------|----------|
| `LiquidAI/LFM2.5-Audio-1.5B-JP` | **YES** | No (LFM Open License v1.0) | **PyTorch/safetensors ONLY** — `model.safetensors`, `audio_detokenizer/{config,model.safetensors}`, tokenizer/config. **No ONNX.** |
| `LiquidAI/LFM2.5-Audio-1.5B-JP-ONNX` | **NO (404)** | — | The triage cited `community:LiquidAI/LFM2.5-Audio-1.5B-ONNX` (the **EN** mirror) for the JP row; there is **no JP ONNX mirror**. |
| `LiquidAI/LFM2.5-Audio-1.5B-ONNX` | YES | No | The EN ONNX mirror the EN onboard used (5 graphs + 2 bins). |

**The obstacle**: WaaV's lfm2_audio path is ORT-direct (5 ONNX graphs + 2 raw embed tables) and JP ships no
ONNX. So JP is NOT a pure file-copy drop-in like a typical config+weights model — it required **exporting
the JP safetensors to the same 5 graphs** with LiquidAI's official `github.com/Liquid4All/onnx-export`
(`liquidonnx`) exporter (the same tool that produced the EN mirror). This is acquisition (an external
artifact-build step), NOT a WaaV serving path — no per-venv serving, exactly per the HARD RULE.

**Why it IS still zero-code for WaaV** (proven before exporting, via HfApi/safetensors metadata):
- **Architecture identical**: both `config.json` declare `Lfm2AudioForConditionalGeneration`,
  `model_type=lfm2`, no `trust_remote_code`. Every dimension (hidden 2048, vocab 65536, 8 codebooks,
  `layer_types` 10-conv/6-attn schedule, Conformer encoder, 6-layer depthformer) is identical.
- **Weights are a same-arch fine-tune**: JP `model.safetensors` has an **identical key set (931 tensors)
  with ZERO shape/dtype differences** vs EN (`get_safetensors_metadata` diff). So the exporter emits
  structurally-identical graphs — confirmed: every exported JP `.onnx`/`.bin` is **byte-for-byte the same
  size** as the EN mirror's (decoder.onnx 141570, audio_detokenizer 179962484, audio_encoder 480114605,
  vocoder_depthformer 472093619, embed_tokens.bin 536870912) — same node topology + I/O names, weights
  differ only in value.
- **Tokenizer byte-identical**: `tokenizer.json` / `tokenizer_config.json` / `special_tokens_map.json` /
  `chat_template.jinja` all MD5-identical JP==EN. So every hardcoded special-token constant in the WaaV
  core (`IM_END=7`, `AUDIO_START=128`, `EOS=7`, `<|startoftext|>`=1, `<|im_start|>`=6) is valid for JP.
- **The ONLY config delta is `interleaved_n_audio` 12→9** — and the WaaV core does **not consume** this
  field (verified by grep; the core uses an `in_audio` boolean state-switch, not the config cadence). So
  the delta does not affect the WaaV serving path at all.

## 2. What was done (acquire → integrate)

1. **Export** (throwaway validation venv, not a serving path): `uv sync` the `onnx-export` repo (base deps
   only — `--no-dev --no-extra dev --no-extra gpu`, to skip `liquid-audio`→`torchcodec` which has no
   aarch64 wheel), then `lfm2-audio-export LiquidAI/LFM2.5-Audio-1.5B-JP` (fp32, the greedy golden
   precision). Two small env-only shims were needed (NO WaaV impact): a `liquid_audio.utils.get_model_dir`
   stub (the detokenizer builder's only `liquid_audio` use — a `snapshot_download` wrapper) and a pure-
   numpy slaney `librosa.filters.mel` (the Python infer's only librosa use, for the ASR golden) — both
   because those heavy deps lack aarch64 wheels / need extra network. The Rust mel/ISTFT are independent.
2. **Stage** the exported `onnx/` (5 graphs + `audio_embedding.bin` + `embed_tokens.bin` + `mel_config.json`)
   + the JP tokenizer/config (pulled straight from HF) into `~/.cache/waav-models/lfm2.5-audio-1.5b-jp/`,
   mirroring the EN dir exactly.
3. **Manifest**: write `waav.json` `{"architecture":"lfm2_audio_asr", weights:{decoder, audio_encoder,
   depthformer→vocoder_depthformer, detokenizer→audio_detokenizer}}` + `.variants/waav_tts.json`
   (`lfm2_audio_tts`). The registry's existing `"lfm2_audio_asr" | "lfm2_audio_tts"` arm builds the
   `Lfm2Audio` core with ZERO changes.

## 3. Results — REAL JP round-trips (byte-faithful golden + GB10 RTF)

Golden = LiquidAI `liquidonnx` onnxruntime fp32, greedy (temp 0 / audio-temp 0 ⇒ deterministic argmax).
WaaV = `Lfm2Audio` core via the production ORT path. JP text: `こんにちは、これは音声合成のテストです。`

| Mode | Output | Accuracy vs golden | RTF (CUDA / GB10) | RTF (CPU) |
|------|--------|--------------------|-------------------|-----------|
| **TTS** (JP text→audio) | 43 frames → 82 560 samp (3.44 s @24kHz), rms 0.1362 peak 0.900 | **codes BYTE-IDENTICAL** — 43 frames, first `[1049,1700,1626,142,306,1030,666,1744]`, **sum 337487** == golden | **0.538** | 2.123 |
| **ASR** (audio→JP text) via registry → `LoadedModel::Stt` | `こんにちは。これは音声合成のテストです。` | **transcript IDENTICAL to the Python onnxruntime golden** | **0.259** | 0.338 |
| **S2S** `round_trip` (speech→text+speech) | reply `こんにちは！はい、これは音声合成のテストですね。何かお手伝いできることがあれば、いつでもお知らせください。` | deterministic; the model **understood the spoken Japanese and replied coherently in Japanese** | turn 1.07 s | turn 2.19 s |

- **Determinism**: greedy ⇒ deterministic. **CPU == CUDA byte-identical** (TTS codes sum 337487 + same
  first frame on both EPs). The JP golden (43 frames / sum 337487) is **distinct from the EN golden**
  (44 frames / sum 360103) — proving these are genuinely the JP weights, not EN.
- The ASR input is the model's own greedy JP TTS output resampled to 16 kHz (so the whole chain — JP
  TTS → JP ASR — is self-consistent and reproducible). The model round-trips its own speech to the input
  sentence (only 、→。 punctuation differs, as the model emits).
- The S2S reply was text-only (greedy with the default interleaved prompt didn't emit `<|audio_start|>`
  within the 100-token text cap) — **identical behavior to the EN model** (see EN onboard §3); the TTS path
  proves the full audio-out chain works byte-faithfully. The speech→speech *comprehension* loop works
  end-to-end in Japanese. GB10 detected (sm_121, unified 121 GB); no OOM (112 GB free at load).

## 4. Files added / changed

**ADDED (mine, clean — NOT committed per instructions):**
- `crates/waav-infer-server/tests/lfm2_audio_jp_registry.rs` — **NEW test-only file** (no shared touch):
  drives the JP dir through the registry + core; asserts the JP TTS golden codes byte-faithfully + JP ASR
  content + a deterministic S2S turn. CPU and CUDA.
- `~/.cache/waav-models/lfm2.5-audio-1.5b-jp/` (6.2 GB) — the exported JP ONNX (5 graphs + 2 bins +
  mel_config), JP tokenizer/config, `waav.json` (`lfm2_audio_asr`), `.variants/waav_tts.json`
  (`lfm2_audio_tts`), and `sample_asr_jp_16k.wav`. **Acquisition artifacts (not committed).**

**SHARED-FILE TOUCHES: NONE.** This onboarding is genuinely zero-code — no production Rust, no `model.rs`
registry edit, no `REGISTERED_ARCHITECTURES` change. So there is **no registry-count touch for the
coordinator to reconcile** (the JP reuses the EN archs `lfm2_audio_asr`/`lfm2_audio_tts` already in the
list, count unchanged). ⚠️ The working tree DOES show other tracked files modified
(`backend-torch/src/{lib,qwen3_tts,vibevoice}.rs`, `server/src/engine.rs`, two test files) — these are
**concurrent agents' work, NOT mine** (verified: none mention JP/lfm2_audio_jp). I added zero lines to any
tracked file.

## 5. Validation status

- `cargo test -p waav-infer-core --lib` → **75 passed, 0 failed** (8 ignored) — **unchanged** (zero-code;
  I added no Rust to this crate). This is the post-`c56a97d` baseline (the EN onboard's 71 grew with later
  commits).
- `cargo test -p waav-infer-server --test lfm2_audio_jp_registry --features torch` → **3/3 pass** on **CPU
  and CUDA** (JP TTS byte-faithful codes, JP ASR content match, JP S2S deterministic turn).
- `cargo clippy -p waav-infer-server --test lfm2_audio_jp_registry --features torch` → **clean** (0
  warnings on my file).

## 6. Reproduce

```bash
# 1. Export the JP graphs (one-time acquisition; throwaway venv, NOT a serving path):
git clone --depth 1 https://github.com/Liquid4All/onnx-export.git && cd onnx-export
uv sync --no-dev --no-extra dev --no-extra gpu     # skip liquid-audio→torchcodec (no aarch64 wheel)
#   + the two env-only shims in §2 (liquid_audio.utils.get_model_dir; pure-numpy librosa.filters.mel)
HF_TOKEN=… .venv/bin/lfm2-audio-export LiquidAI/LFM2.5-Audio-1.5B-JP --output-dir /tmp/jp_export
# 2. Stage onnx/ + JP tokenizer/config into ~/.cache/waav-models/lfm2.5-audio-1.5b-jp/ + write waav.json.
# 3. Serve / test ZERO-CODE:
source gb10-env.sh && LFM2_EP=cuda \
  cargo test -p waav-infer-server --test lfm2_audio_jp_registry --features torch -- --nocapture
```

## 7. Notes / follow-ups (optional)

- **A JP ONNX mirror could be published** (push `/tmp/jp_export/.../LFM2.5-Audio-1.5B-JP-ONNX` to HF as
  `…-JP-ONNX`) so future acquisition is a plain download — but that's a publishing decision, not needed
  for WaaV serving (the staged dir already serves).
- Quant variants (q4/q8/fp16) export with `--precision …` and load via the manifest `precision` field with
  zero code change — gated behind an accuracy stamp per WaaV policy (the golden here is fp32).
- The `round_trip` text-until-`<|audio_start|>` switch is the same simplistic one as EN; porting the
  reference `generate_interleaved` cadence (`interleaved_n_text=6`/`interleaved_n_audio=9` for JP) would
  make the assistant reliably SPEAK every turn — the same clean follow-up flagged in the EN onboard.
- `LiquidAI/LFM2.5-Audio-1.5B-JP-GGUF` also exists (GGUF-only, P4) — a separate (llama.cpp-shaped) onboard,
  out of scope here.
