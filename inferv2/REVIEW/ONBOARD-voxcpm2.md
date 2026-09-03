# ONBOARD — openbmb/VoxCPM2 (2B tokenizer-free diffusion-AR TTS)

**Status: ONBOARDED + LIVE-VERIFIED on GB10.** Real synth ✅ · byte-faithful accuracy ✅ (max|Δ|=0.0) ·
RTF measured CPU + CUDA ✅. Path = **ORT-direct** (the flow-matching is baked into the ONNX graph — NO
host `cfm::` stepper reused, and none was needed; see "CFM decision" below).

Date: 2026-06-23 · Triage tier: MODERATE.

---

## 1. What it is (verified)

VoxCPM2 is a 2B **tokenizer-free, continuous-latent diffusion-AR** TTS on a **MiniCPM-4** backbone. Unlike
the discrete codec-AR family (chatterbox/MOSS), each AR step emits a **continuous latent patch `[P=4,D=64]`**
produced by a **flow-matching DiT/CFM head**, decoded to 48 kHz by an audio VAE. It is a **voice-cloning**
model (zero-shot from a reference wav+transcript); we onboard the **plain-TTS (no-reference)** regime
(feat all-zeros, default/zero-shot voice), which is the unconditional path and needs no ref audio.

- **Repo (HfApi-verified):** `openbmb/VoxCPM2` — EXISTS, **ungated**, Apache-2.0. (safetensors 4.58 GB +
  audiovae.pth 377 MB + tokenizer.json + config.json + custom `tokenization_voxcpm2.py`.)
- **ONNX mirror (HfApi-verified):** `ai4all8/VoxCPM2-ONNX` — EXISTS, **ungated**, Apache-2.0. Ships the
  **full graph pipeline** (no torch needed at serve time):
  | graph | size | role |
  |---|---|---|
  | `voxcpm2_prefill.onnx(.data)` | 8.36 GB | text+ref-audio → DiT hidden + dual KV cache |
  | `voxcpm2_decode_step.onnx(.data)` | 8.66 GB | 1 AR step **with the 10-step CFM Euler solve baked in** |
  | `audio_vae_decoder.onnx(.data)` | 184 MB | latents `z[1,64,L]` → 48 kHz waveform |
  | `audio_vae_encoder.onnx(.data)` | 193 MB | ref-audio → latent (cloning only; unused in plain TTS) |

Acquired to `~/.cache/waav-models/voxcpm2/onnx/` (17 GB) + tokenizer.json/config from the orig repo.

### config.json (key dims, all wired as constants)
`patch_size=4`, `feat_dim=64`, base LM `num_hidden_layers=28` (GQA: 16 heads / 2 KV / 128 ch),
`residual_lm_num_layers=8`, DiT CFM `solver=euler, inference_cfg_rate=2.0`, VAE `out_sample_rate=48000`,
`<|audio_start|>`=101, BOS `<s>`=1.

### The graph I/O contract (read off the ONNX, verbatim)
```
prefill:  text[1,S]i64, text_mask[1,S]i32, feat[1,S,4,64]f32, feat_mask[1,S]i32
       → dit_hidden[1,2048], base_next_{keys,values}[1,28,2,S,128],
         residual_next_{keys,values}[1,8,2,S,128], prefix_feat_cond[1,4,64]
decode_step: dit_hidden[1,2048], base/residual KV, prefix_feat_cond[1,4,64], noise[1,4,64], cfg_value[]
       → pred_feat[1,4,64], new_dit_hidden[1,2048], new_*_KV[…,past+1,…], stop_flag[1]bool
vae_decoder: z[1,64,L] → audio[1,1,1920*L]
```

---

## 2. CFM decision — REUSE vs ORT-direct

**Decision: ORT-direct. The `cfm::` steppers were NOT reused, and re-impl was NOT needed.** The ONNX
mirror's `decode_step` graph **internally runs the full 10-step CFM Euler flow-matching solve** (the DiT
vector field + classifier-free guidance + sway-sampled t-schedule are all baked into the 9998-node graph —
confirmed by reading the reference exporter `src/wrappers/decode.py`: `feat_decoder.solve_euler(...)` with
`t_span = linspace(1,0,11)` + sway coef, all traced into ONNX). So the host's only job is the **AR
recurrence** (feed noise, read `pred_feat`, feed it back as the next `prefix_feat_cond`, stop on
`stop_flag`). This matches the task's "(Or ORT-direct if the mirror ships full graphs)" branch. The Rust
`cfm::{CfmOde,DpmSolver,MaskedDiffusion}` steppers are for models whose CFM runs in **host** code (cosyvoice3
/ vibevoice / omnivoice); they are not applicable when the CFM is inside the served graph.

**The one stochastic input is `noise[1,4,64]` per step** (the CFM x0). Production seeds the shared
`GaussianNoise` (PCG32 + Box–Muller) on `(voice,text)` for reproducible audio; the accuracy test injects the
golden's exact noise schedule.

---

## 3. Integration (the moss/chatterbox ORT-AR pattern)

New module `crates/waav-infer-core/src/tts/voxcpm2.rs` (`VoxCpm2Tts`): tokenize (tokenizer.json BOS + text +
`<|audio_start|>`) → `prefill` (feat all-zeros, feat_mask all-zeros, text_mask all-ones = no-reference) →
per-patch AR loop over `decode_step` (cfg=2.0, per-step Gaussian noise, stop after a min-len floor) →
assemble `z[1,64,T*4]` (the reference `(B,T,P,D)→(B,D,T*P)` transpose+reshape) → `audio_vae_decoder` → 48 kHz
mono PCM16. Latent frame rate fixed; `set_language` is a no-op (multilingual, language inferred from text).
A `generate_latents(text, &mut next_noise)` seam exposes the byte-faithful path for the test.

Config + data drive everything via a `waav.json` `{"architecture":"voxcpm2","weights":{prefill,decode_step,
vae_decoder}}` — adding it was a module + one registry arm, zero kernel work.

---

## 4. Live verification on GB10 (the LAW)

Python-ORT golden (`/tmp/voxcpm2_golden.py`, fixed seed 1234, English): tokens
`[1,21045,…,72,101]` (BOS+text+audio_start), stop@step15 (16 patches), latents `(1,64,64)`, **2.56 s audio,
CPU RTF 1.86×**. Golden (incl. the exact per-step noise schedule + the `[1,64,64]` VAE-input latents) written
to `…/onnx/golden.json`.

**Gate 1 — BYTE-FAITHFUL latents (deterministic AR/CFM, fixed noise):**
```
got latent len 4096 (16 patches), golden len 4096
BYTE-FAITHFUL LATENTS: 4096/4096 exact | max|Δ|=0.000e0 rmse=0.000e0
```
The Rust diffusion-AR loop (tokenizer → prefill → 16× decode_step with the baked CFM → z assembly)
reproduces the Python-ORT golden **bit-for-bit**. Tokens matched exactly; stop_flag fired at the same frame.

**Gate 2 — real synthesis + RTF** (production PCG32-seeded noise, 7.68 s utterance):
| EP | synth time | audio | peak | rms | **RTF** |
|---|---|---|---|---|---|
| CPU | 15.13 s | 7.68 s @ 48 kHz | 0.994 | 0.157 | **1.97×** |
| **GB10 CUDA (sm_121)** | 4.98 s | 7.68 s @ 48 kHz | 0.994 | 0.157 | **0.649×** (faster than realtime) |

CUDA peak/rms == CPU → the CUDA path is numerically faithful. No OOM (mem stayed ~16 GB free, recovered to
21 GB after; the 2× 8 GB graphs load fine in the 121 GB unified pool, one model at a time). Wav written to
`/tmp/waav_voxcpm2_live.wav` (+ golden `/tmp/voxcpm2_golden.wav`).

---

## 5. Build/test health

- `cargo build -p waav-infer-core --tests` — clean. `cargo clippy -p waav-infer-core --tests` — **no findings
  in voxcpm2.rs / voxcpm2_live.rs** (the only clippy notes are in concurrently-added `sts/lfm2_audio.rs` and
  `waav-infer-components`, not mine).
- `cargo test -p waav-infer-backend-torch --lib` — **148 passed, 0 failed** (the task gate; I touched no torch
  source, but torch depends on core — confirms my core change is non-breaking).
- `cargo test -p waav-infer-core --lib` — green after the registry-count guard update (below).

---

## 6. Files added / changed (⚠ = SHARED file — flag for the coordinator)

**Added (mine, uncontended):**
- `crates/waav-infer-core/src/tts/voxcpm2.rs` — the `VoxCpm2Tts` execution path (~330 lines).
- `crates/waav-infer-core/tests/voxcpm2_live.rs` — byte-faithful + synthesis/RTF live test.
- `~/.cache/waav-models/voxcpm2/onnx/waav.json` — the model manifest (data, not repo code).

**Changed — ⚠ SHARED (reconcile with concurrent agents):**
- ⚠ `crates/waav-infer-core/src/tts/mod.rs` — `pub mod voxcpm2;` + `pub use voxcpm2::{VoxCpm2Error,VoxCpm2Tts};`
- ⚠ `crates/waav-infer-core/src/lib.rs` — added `VoxCpm2Tts` to the `pub use tts::{…}` line. (A concurrent
  agent independently added `pub mod sts;` here — both coexist.)
- ⚠ `crates/waav-infer-core/src/model.rs` — (a) `use crate::tts::voxcpm2::VoxCpm2Tts;`; (b) a new registry
  arm `"voxcpm2" | "VoxCPM2" =>`; (c) `"voxcpm2"` appended to `REGISTERED_ARCHITECTURES`; (d) the registry
  count-guard test bumped **18 → 19**. ⚠ **If another agent also adds an arm, this count must be reconciled
  to the final total** (it's a single shared assertion).

**Untouched:** no torch files; no kernels; no `cfm::`. NO `git commit` (per instructions).

---

## 7. Honest caveats / follow-ups (none are blockers)

- **No-reference (zero-shot) voice only.** Voice-cloning (ref wav+transcript) needs the `feat` preprocessing
  (`audio_vae_encoder` → patchify into `[S,4,64]`), which lives in the `voxcpm` package's `build_prompt_cache`
  (not exported to ONNX). The `audio_vae_encoder.onnx` is acquired; wiring the patchify is a clean additive
  follow-up. Plain TTS (the onboarded path) is the unconditional regime and is fully live.
- **Tokenizer: English byte-faithful via tokenizer.json.** The custom `VoxCPM2Tokenizer` adds a **multi-char
  Chinese token split** on top of the base Llama BPE; for non-CJK text the split never triggers, so the
  `tokenizers` crate reproduces it exactly (verified: tokens bit-match the golden). For CJK input a small
  post-encode id-split would be needed (follow-up).
- **`stop_flag` honored** (model self-terminated at the same frame in both runtimes); a `max_patches` ceiling
  (env `WAAV_VOXCPM2_MAX_PATCHES`) caps runaways. cfg overridable via `WAAV_VOXCPM2_CFG`.

**Bottom line:** VoxCPM2 onboarded as an **ORT-direct diffusion-AR TTS** — real 48 kHz synth, **byte-identical
latents vs the ORT golden (max|Δ|=0.0)**, **GB10 CUDA RTF 0.649× (faster than realtime)**. The flow-matching
is in the served graph, so `cfm::` was correctly NOT reused.
