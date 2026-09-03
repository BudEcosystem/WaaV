# ONBOARD — Qwen3-TTS-12Hz-1.7B-CustomVoice

**Date:** 2026-06-23 · **Box:** GB10 (121 GB shared, CUDA sm_121) · **Arch:** `qwen3_tts` (Qwen3TTSForConditionalGeneration)

## Verdict

**ONBOARDED — `+code` (minimal, config-driven; the SAME loader now serves both 0.6B and 1.7B).** It is NOT
the pure zero-code waav.json path the 0.6B was: the 1.7B has a **genuine structural difference** (a wider
talker + a new `small_to_mtp_projection` bridge). The change is fully config/weight-driven (zero per-size
code beyond the one bridge), confined to the model's own file + one engine dispatch arm.

- **LIVE on GB10 CUDA-bf16:** RTF **0.667** (2.67 s wall for 4.00 s audio, 96000 samples @ 24 kHz).
- **Accuracy:** the deterministic byte-identity LAW **PASSES** — talker hidden **Δ==0 over 2048 dims** at
  BOTH the prefill last position AND the first decode step vs the CUDA-bf16 reference engine (the qwen3_tts
  bar). step0 codec argmax exact (1995, logit 32.75==32.75). Codec decode corr **0.9999**.
- **Regression:** the 0.6B path is **still byte-identical** (Δ==0 over 1024 dims) through the refactored
  shared loader — RTF 0.555.

## HfApi verification (method step 1)

| Repo | Exists | Gated | Used? |
|---|---|---|---|
| `Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice` (official safetensors) | YES | No | **YES** — the weights |
| `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | YES | No | reference (sibling) |
| `elbruno/Qwen3-TTS-12Hz-1.7B-CustomVoice-ONNX` | YES | No | **NO** |

The triage cited the elbruno ONNX mirror — it **does exist** (talker_prefill/talker_decode/code_predictor/
vocoder + embeddings/), but the WaaV qwen3_tts impl serves the **official safetensors** repo (native tch),
NOT the ONNX mirror (the 0.6B does the same). So the ONNX mirror is irrelevant; the official 1.7B safetensors
were acquired to `~/.cache/waav-models/qwen3-tts-12hz-17b/` (3.6 G talker + 651 M codec).

## The structural delta (why it is NOT zero-code)

Diffed 1.7B vs 0.6B config.json + safetensors headers. Only the **talker** scales; everything else
(28/5 layers, 16/8 heads, head_dim 128, vocabs, the 12 Hz codec — byte-for-byte the same 651 M file) is
fixed. Two consequences, both faithful to the vendored `Qwen3TTSTalkerCodePredictor` in
`torch_runtime/vendor/qwen3_tts/core/models/modeling_qwen3_tts.py`:

| tensor | 0.6B | 1.7B |
|---|---|---|
| talker `hidden_size` | 1024 | **2048** |
| talker `intermediate_size` | 3072 | **6144** |
| talker `codec_embedding` / `codec_head` | [3072,1024] | [3072,**2048**] |
| CP `codec_embedding[i]` (= talker_hidden wide) | [2048,1024] | [2048,**2048**] |
| **`talker.code_predictor.small_to_mtp_projection`** | **ABSENT** (`nn.Identity`) | **[1024,2048] +bias** |
| CP backbone hidden (q/k/v/o/mlp) | 1024 | 1024 (unchanged) |

`small_to_mtp_projection` is the talker→CP bridge: it projects EVERY CP-backbone input (the
`[past_hidden, last_id_hidden]` prefill cat AND each per-codebook decode embedding) from talker-width (2048)
down to the CP's 1024. In the 0.6B (widths equal) the vendor uses `nn.Identity` and ships no weight — so the
0.6B path stays a clone (byte-identical).

## Integration (method step 3) — minimal, config-driven

Added a `Variant` (read from `config.json`'s `talker_config.hidden_size`/`intermediate_size`) and threaded
its dims through the talker + CP + the two generate loops, plus a `small_to_mtp` `Option<Linear>` applied at
each CP-backbone input. New sizes load with ZERO further code — the dims are read, the bridge is keyed on
weight presence (`talker_hidden != cp_hidden`), and a load-time `codec_embedding` shape guard cross-checks the
config-read width against the actual weights.

One config-parse subtlety (caught live by the shape guard, then fixed + unit-tested): `talker_config` opens
with the nested `code_predictor_config` object FIRST, which ALSO has `hidden_size`/`intermediate_size`
(1024/3072) — so the hand-parser (no serde_json in this `-backend-*` crate, matching the existing
tokenizer-config discipline) skips past the nested object to its matching `}` before reading the talker's own
scalars.

### Files changed (for the coordinator to commit)

1. **`crates/waav-infer-backend-torch/src/qwen3_tts.rs`** — model's own file. Added `Variant` +
   `from_config_dir` + `parse_int_after`/`skip_braced_object` hand-parser; `Talker`/`CodePredictor` made
   dim-aware (`hidden`/`embed_dim`); `CodePredictor.small_to_mtp` + `to_mtp()` applied at every CP input;
   load-time shape guards; +1 unit test (`config_parse_skips_nested_code_predictor`). 0.6B path unchanged
   (Variant::DEFAULT == 0.6B; `small_to_mtp` None ⇒ clone). Removed two now-unused consts
   (`talker_cfg::{HIDDEN,INTER}`).
2. **`crates/waav-infer-server/src/engine.rs`** — SHARED CODE (coordinator: sequence this). Added the
   `qwen3_tts` arm to the torch-inprocess dispatch (`match cfg.architecture`) → `TorchQwen3Tts::load`
   (TtsModel). This wires BOTH the 0.6B and 1.7B into the engine's `waav.json` path — previously qwen3_tts was
   reachable only via the test harness (the 0.6B onboard never wired the engine arm). +1 import.

### Files added (not code)

- `~/.cache/waav-models/qwen3-tts-12hz-17b/waav.json` — `{backend: torch, architecture: qwen3_tts, dtype: bf16}`.
- 1.7B weights at `~/.cache/waav-models/qwen3-tts-12hz-17b/` (acquired, not committed).

> NOTE: the other files in `git diff --stat` (kaldi_fbank, noise, model.rs, stt/mod, tts/mod, backend-api)
> are PRE-EXISTING uncommitted changes from concurrent work — NOT mine. My scope is the two files above.

## Smoke + accuracy + perf (method steps 4–6) — LIVE on GB10

Golden dumped from the reference engine (`torch_runtime/dump_qwen3tts_golden.py cuda bf16`, seed 0, the
vendored modeling which natively runs `small_to_mtp_projection`) → `/tmp/qwen3tts_golden_17b`.
Gate: `cargo test -p waav-infer-backend-torch --test cuda_torch_qwen3_tts -- --ignored` with
`WAAV_Q3TTS_DIR`/`WAAV_Q3TTS_GOLDEN` pointed at the 1.7B.

```
[L1] prompt ids match (24 tokens)
[L2] step0 codec argmax=1995 (logit 32.7500 vs golden 32.7500)
[L3a] LAW PASSED: PREFILL talker hidden BYTE-IDENTICAL (Δ==0 over 2048 dims)
[L3b] LAW PASSED: FIRST-DECODE talker hidden BYTE-IDENTICAL (Δ==0 over 2048 dims)
[L3c] greedy tracks the CUDA-bf16 fused-SDPA golden for 40 frames (T=53==53); tail div = (40,7,1328,189)
[L4] seeded-sampled tracks the sidecar golden for 38 frames (T=50==50)
[L5] codec decode on golden codes: 96000 samples, max|Δ|=0.01953 corr=0.9999
[RTF] qwen3-tts CUDA-bf16: 2.67s wall for 4.00s audio (96000 samples) → RTF 0.667
```

**Interpretation.** L3a/L3b (Δ==0) is the deterministic byte-identity LAW and it PASSES — the talker hidden,
which is downstream of the wider talker, the `small_to_mtp_projection`, the CP sub-talker and the
codec-feedback sum, is bit-exact vs the reference. The L3c (greedy, 40/53) and L4 (sampled, 38/50) tail
divergences are the EXACT same proven bf16-SDPA-kernel-tie floor the 0.6B exhibits (the sidecar's own
cuda-bf16 greedy disagrees with itself across SDPA backends) — NOT a port bug; frame counts match (53==53,
50==50). The codec (shared 651 M file) decodes corr 0.9999.

## Tests / lint

- `cargo test -p waav-infer-backend-torch --lib` → **148 passed** (was 147; +1 parser test). Clean.
- `cargo clippy -p waav-infer-backend-torch -p waav-infer-server --lib` → **clean** (the only workspace
  warnings are pre-existing in waav-infer-core `moss.rs`, unrelated).
- 0.6B regression gate (default dirs) → PASS, Δ==0 over 1024 dims.

## No-venv compliance

The python sidecar was used ONLY to dump the reference golden (throwaway accuracy validation, per
[[waav-infer-no-venv-wrap]]). The serving path is 100% native tch in-process; no per-venv/pip serving.
