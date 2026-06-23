# Onboard: OpenMOSS-Team/MOSS-TTS-v1.5 (delay-pattern codec-LM TTS)

**Status: BLOCKED on the ONNX ceiling — NOT onboarded. The exact `OpenMOSS-Team/MOSS-TTS-v1.5`
repo EXISTS and is ungated, but it is an 8.5B Qwen3-backbone *delay-pattern* codec-LM that ships
PyTorch-only (17 GB safetensors). There is NO ONNX export of its backbone anywhere on the Hub
(the only family ONNX is the *Realtime* sibling's, and that mirror is access-gated to me). Its
architecture is NOT the Nano's global/local transformer, so `tts/moss.rs` and the cached Nano codec
CANNOT be reused. A real synth would require either a multi-day tch reimplementation of the 8.5B
Qwen3-delay backbone or a full PyTorch->ONNX export of it — neither is a config-add. Reported
honestly per LAW; small config/tokenizer files acquired, no Rust touched.**

| | |
|---|---|
| Model | MOSS-TTS-v1.5 — ~8.5B multilingual codec-LM TTS, 32 languages, native **24 kHz**, zero-shot voice cloning + long-form + Pinyin/IPA control |
| Arch | `moss_tts_delay` / `MossTTSDelayModel` — a **single Qwen3-8B** transformer (h4096, 36L, 32h/8kv GQA, head_dim128, θ1e6, vocab 155648) with **32 parallel VQ `lm_heads`** (MusicGen-style) and a **delay-pattern** AR generate loop; codec = `MOSS-Audio-Tokenizer-v2` (1.6B "Cat" causal transformer, 32 codebooks, 48 kHz) |
| Triage tier | MODERATE |
| Official repo | `OpenMOSS-Team/MOSS-TTS-v1.5` — **VERIFIED EXISTS, ungated** (`gated:False`, 123,198 downloads), 4-shard bf16 safetensors (16.98 GB ~= 8.5B params) + custom modeling |
| ONNX mirror | **NONE for the v1.5 / delay backbone** — verified absent. Only ONNX in the family is `pltobing/MOSS-TTS-Realtime-ONNX` (a *different* model — the Realtime global/local sibling), and that repo is **`gated:auto` -> 403 for this token** ("not in the authorized list"). Codec-only ONNX exists (`MOSS-Audio-Tokenizer-ONNX`) but it is the codec half only. |
| Onboarding | **NOT POSSIBLE as config+code reuse** — needs tch-reimpl of the 8.5B Qwen3-delay backbone OR a full backbone ONNX export. |
| Accuracy / RTF | **N/A — no real synth produced** (blocker hit before SMOKE). |
| Rust touched | **None.** `cargo`/`clippy` untouched (no source changes). |

---

## 1. HfApi verification (method step 1) — the deciding step

Ran `HfApi` (HF_TOKEN provided) against every plausible id. Findings:

- **`OpenMOSS-Team/MOSS-TTS-v1.5`** — EXISTS, `gated:False`, 123k downloads. The exact id (the prompt's
  alternate `MOSS-TTSD-v1.5` is **404** — TTSD only goes to v1.0; `fnlp/MOSS-TTSD-v0.5` exists but is the
  old org/dialogue line, not this).
- Files: `config.json`, `modeling_moss_tts.py`, `processing_moss_tts.py`, `configuration_moss_tts.py`,
  `tokenizer.json`, `model-0000{1..4}-of-00004.safetensors` (4.93+4.92+4.98+2.15 = **16.98 GB**, bf16
  -> ~8.5B params), `chat_template.jinja`, `tts_robust_normalizer_single_script.py`. **No `*.onnx`.**

### config.json (the architecture verdict)

```
model_type: moss_tts_delay        architectures: [MossTTSDelayModel]
language_config: Qwen3 (h4096, 36L, 32 attn / 8 kv heads, head_dim128, intermediate 12288,
                        rope_theta 1e6, vocab 155648, _name_or_path Qwen/Qwen3-8B)
n_vq: 32                          audio_vocab_size: 1024     audio_pad_code: 1024
sampling_rate: 24000              dtype: bfloat16
audio_assistant_delay_slot_token_id: 151662   audio_assistant_gen_slot_token_id: 151656
```

### modeling_moss_tts.py (decode contract — confirms NO global/local split)

`MossTTSDelayModel.__init__` builds **one** Qwen3 backbone + `nn.ModuleList(self.lm_heads)` of `n_vq+1`
heads (col 0 = text, cols 1..32 = audio codebooks). `forward` requires `input_ids` shape
`(B, S, 1 + n_vq)` and runs each head off the SAME `last_hidden_state` (MusicGen comment in-source).
`generate` is a **delay-pattern** loop driven by `delayed_lengths` / `audio_lengths` /
`audio_assistant_delay_slot_token_id`, with `pre_audio_mask` / `post_audio_mask` staggering the 32
codebooks by their delay offsets. **There is no local transformer and no per-frame fused sampler** —
fundamentally different from the Nano.

## 2. Why the proven reuse paths do NOT apply

The task hypothesis was "v1.5 likely reuses the moss/vieneu pattern (bigger backbone + MOSS codec)".
**Verified false on both halves:**

1. **Backbone pattern mismatch.** `tts/moss.rs` (and `tts/vieneu.rs` which reuses it) implement the
   **`global_local_transformer`** loop: a GPT-2 *global* transformer feeding a 1-layer *local*
   transformer that emits a frame per step, driven by the Nano's 5-graph browser ONNX
   (`prefill` + `decode_step` + `local_fixed_sampled_frame` + codec). v1.5 is
   `model_architecture: global_local_transformer`'s opposite — a **single shared-backbone, 32-head,
   delay-staggered** decoder. None of moss.rs's seams (`prefill`/`decode_step`/`frame` graphs,
   17-col row grid, `should_continue` fused sampler, 12-layer KV helper) map onto it.
   - For reference, the global/local pattern *does* survive into the **Realtime** sibling
     (`moss_tts_realtime`: Qwen3-1.7B global h2048/28L + 4-layer local, rvq=16) — but that is a
     different model than v1.5, and its only ONNX export (`pltobing/MOSS-TTS-Realtime-ONNX`) is gated.

2. **Codec mismatch.** The cached Nano codec
   (`~/.cache/waav-models/moss-tts-nano/codec/moss_audio_tokenizer_decode_full.onnx`, 45 MB) is the
   **16-codebook MOSS-Audio-Tokenizer-Nano**. v1.5 uses **MOSS-Audio-Tokenizer-v2** — a 1.6B 32-codebook
   "Cat" causal transformer (`number_channels:2`, `downsample_rate:3840`, 24-layer encoder). Wrong
   codebook count (16 vs 32) and wrong weights -> the cached codec cannot decode v1.5 codes.
   - A v1.5-compatible codec ONNX *does* exist (`OpenMOSS-Team/MOSS-Audio-Tokenizer-ONNX`,
     encoder+decoder, ungated) — so the **codec half is onboardable**; it's the **backbone half**
     that has no ONNX.

## 3. The blocker, precisely

The official "torch-free" deployment story for MOSS-TTS-v1.5 (its own
`MOSS-Audio-Tokenizer-ONNX` README) is **llama.cpp (GGUF) for the Qwen3-8B backbone + ONNX for the
audio tokenizer** — confirming the vendor did NOT export the backbone to ONNX (only the codec). WaaV
Infer serves backbones via ONNX `StaticGraph` (or its in-process **tch** runtime) and has **no GGUF /
llama.cpp runtime**, and the HARD RULE bars any per-venv/pip serving path
([[waav-infer-no-venv-wrap]]). So:

- **ONNX-direct (the Nano/Supertonic/parakeet fast path): impossible** — no backbone ONNX exists, and
  the one community export of a family member is access-gated.
- **GGUF path: out of scope** — WaaV has no llama.cpp backend, and adding one is a substrate decision,
  not a model onboard.
- **tch-reimpl (the legitimate path): viable but multi-day.** WaaV's `waav-infer-backend-torch` already
  serves Qwen3-backbone codec-TTS (`qwen3_tts.rs`, `cosyvoice3.rs`, `voxtral_tts.rs`, `s2_pro.rs`,
  `higgs.rs`), so the Qwen3-GQA scaffolding + a torch codec seam exist to build on. But v1.5 needs a
  *new* regime on top of that scaffolding — the **32-head delay-pattern decode** (`delayed_lengths`
  staggering, 32 parallel `lm_heads`, the `MOSS-Audio-Tokenizer-v2` 1.6B Cat codec wired as a torch
  module) — plus the 17 GB weight load and a Python-golden byte-faithful gate. That is a focused
  multi-day arc per the campaign's own "export/tch tier is genuine per-model engineering" finding, not
  the config-add this MODERATE-triage onboard was scoped as.

## 4. What was acquired (method step 2, partial)

Established the model dir `~/.cache/waav-models/moss-tts-v15/` and downloaded the **ungated small
files** (config + tokenizer + custom modeling), so a future tch arc can start from disk without
re-fetching: `config.json`, `tokenizer.json` (+ `_config`, `special_tokens_map`, `added_tokens`,
`vocab.json`, `merges.txt`), `processor_config.json`, `modeling_moss_tts.py`,
`configuration_moss_tts.py`, `processing_moss_tts.py`, `chat_template.jinja`,
`model.safetensors.index.json`. The 17 GB safetensors were **not** pulled (only useful once the tch
backbone arc is committed; ungated, so acquirable on demand).

## 5. Disposition / recommendation

- **MOSS-TTS-v1.5 -> defer to the tch backbone tier**, NOT the ONNX config tier. It is the same class
  of blocker the campaign already catalogued for CosyVoice3 / Qwen3-TTS / dia-class codec-LMs: a
  bleeding-edge codec-LM whose backbone has no clean full-pipeline ONNX. Re-tier it MODERATE-ONNX ->
  **HARD/tch** in `WaaV/INFER_TRIAGE.md`.
- **If a v1.5-class delay model is wanted soon as a real synth:** the lowest-effort *real* path is the
  **Realtime** sibling via `pltobing/MOSS-TTS-Realtime-ONNX` (full backbone + local + codec ONNX,
  global/local pattern -> **directly reuses the `tts/moss.rs` seam**), **iff access is granted** —
  request access on that gated repo, then it likely onboards as config + a small registry arm like the
  Nano did. (It is the *Realtime* model, not v1.5, but it is the only ungate-able real-synth path in
  the family today.)
- **The codec half is ready** whenever the backbone lands: `MOSS-Audio-Tokenizer-ONNX` (32-codebook v2,
  ungated) drops in as the ORT codec decoder via the existing codec-decode seam.

## 6. LAW compliance

- **Real synth + accuracy + RTF, or the precise blocker:** the precise blocker is delivered (no
  backbone ONNX; gated sibling mirror; arch != Nano so no reuse; tch-reimpl is multi-day). No
  fabricated synth/accuracy/RTF numbers.
- **No Rust touched** -> `cargo test -p waav-infer-core --lib` / clippy untouched (nothing to regress).
- **HfApi-verified first**, env sourced, `free -g` checked (44 GB free / 121 GB pool), HARD RULE
  honored (no venv/pip/GGUF serving path introduced).

---
*Onboard attempt 2026-06-23. Verdict: NOT ONBOARDED — honest blocker. Files: model dir seeded at
`~/.cache/waav-models/moss-tts-v15/` (small files only); no source changes.*
