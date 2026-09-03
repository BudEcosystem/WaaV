# B29 — tch ARK-ASR-0.6B: byte-identical to the Python sidecar reference

**Goal.** Wave-3 fan-out on the proven LLM-decoder-ASR seam (the voxtral template): port **ARK-ASR-0.6B**
(`ArkasrForConditionalGeneration`) from the Python torch sidecar (`torch_runtime/models/arkasr.py`) onto the
in-process **tch-rs** backend, **BYTE-IDENTICAL** to the sidecar reference, running on GB10 CUDA. Standing
rule: every divergence root-caused + fixed, never explained away.

## TL;DR / answer

- **BYTE-IDENTICAL — ACHIEVED and gated, on the FIRST run.** The tch CUDA (f16) transcript is **exactly** the
  sidecar golden, **100.0% char-identity (byte-for-byte)** on the kokoro clip:
  `"Hello world. This is W A V. Infer a portable voice inference engine running live on the GB10 Grace BL, a C K W E L L."`
  No debugging loop was needed — the architecture was replicated faithfully from the HF source and matched
  the reference end-to-end on the first execution.
- **The reference is the sidecar itself.** ARK-ASR has **no ONNX export** (it is a `trust_remote_code` torch
  model; the sidecar drives the model's own HF code, so the sidecar IS the reference engine). The gate
  asserts `torch_txt == sidecar_golden` exactly.
- **Unambiguous golden.** The sidecar's **CPU-fp32** and **CUDA-fp16** runs produce the *identical* transcript
  AND the *identical* 34 generated token ids on this clip (no near-tie flips) — so byte-identity is
  well-defined. Golden gen_ids: `[9707, 1879, 13, 1096, 374, 467, 362, 647, 13, 62658, 264, 22819, 7743,
  44378, 4712, 4303, 3887, 389, 279, 18865, 16, 15, 31071, 14850, 11, 264, 356, 730, 467, 468, 444, 444, 13,
  151645]`.
- **RTF 0.270** (<1). Load 3870 ms, infer 3248 ms on 12.05 s audio. clippy clean (`-p
  waav-infer-backend-torch --all-targets`, zero warnings). 29/29 torch unit tests green (6 new ark + 23
  existing, nothing regressed).
- **Touched ONLY** `crates/waav-infer-backend-torch/src/ark.rs` (new), `.../src/lib.rs` (mod + export + doc
  lines), `.../tests/cuda_torch_ark.rs` (new), `ci/heavy_live_tests.sh` (one gate entry). **Worktree SHA
  `2a350c6`** (branch `worktree-agent-af91d0cdae37a409e`, based on `a13754e` = B25).

## Architecture (faithful translation of HF `modeling_arkasr.py` + `modeling_audio.py`)

ARK-ASR is the canonical **LLM-decoder ASR** shape: a Whisper-style audio encoder + MLP adapter inject audio
embeddings into a **Qwen2-0.6B** decoder by *replacing* the `<|audio|>` (id 151663) placeholder tokens in the
prompt, then greedy AR transcript generation.

- **Audio tower** (`audio_encoder.whisper.*`, run in **f32**): a *modified* Whisper encoder.
  - **STANDARD Whisper conv stem - SYMMETRIC pad-1, NOT causal**: conv1 (k3 s1 **pad1**) keeps the length;
    conv2 (k3 **s2 pad1**) halves it as `floor((T+2-3)/2)+1`. This is plain `nn.Conv1d(...,padding=1)` - the
    **opposite** of voxtral's causal left-pad. gelu (erf) after each.
  - 32 transformer layers: MHA 20x64 (q/v/out_proj **bias**, k_proj **no** bias), pre-LN **LayerNorm**
    (weight+bias), gelu MLP->5120.
  - **PARTIAL INTERLEAVED RoPE**: `RotaryEmbedding(head_dim//2 = 32)` rotates only the **first 32 of 64**
    head dims, as `rot_dim/2` interleaved PAIRS `(x0,x1)` with the complex product
    `(a+bi)(c+di) = (ac-bd) + (ad+bc)i`, **theta=10000** (NOT 1e6; NOT rotate-half). The trailing 32 dims
    pass through.
  - The Whisper built-in final norm is `Identity()`; instead the **adapter's** `LayerNorm(1280)` is applied.
    **No** absolute `embed_positions` (the `use_rope` path skips them).
  - merge-4 (4x1280->5120, truncate seq to a multiple of 4) -> `adapting` MLP (linear 5120->1792, gelu,
    linear 1792->896) -> `audio_embeds[N,896]`.
- **Text decoder** (`model.*`, f16 on CUDA): **Qwen2** - 24 layers, hidden 896, GQA **14 heads / 2 kv**,
  head_dim 64, RoPE **theta=1e6** (full rotate-half), swiglu silu MLP->4864, **RMSNorm eps 1e-6**, q/k/v
  **bias** (Qwen2), o_proj no bias, **tied** embeddings (`lm_head.weight` == `model.embed_tokens.weight`).
  EOS=151645.
- **Prompt** (sidecar processor, tokenized `add_special_tokens=false`):
  `<|user|><|begin_of_audio|>` + Nx`<|audio|>` + `<|end_of_audio|>Please transcribe this audio.<|assistant|>`,
  where N = `((mel_frames+1)//2)//merge` = the encoder output token count. The encoder output is scattered
  into the N `<|audio|>` positions of `inputs_embeds`.

## Method - de-risk the frontend bit-exactly BEFORE the decode

LLM-decoder ASR has a long frontend (resample -> mel -> prompt-tokenize -> scatter). Each was validated
against the sidecar before trusting the decode, isolating any mismatch:

1. **Identical input PCM.** A tiny Rust emitter ran the suite's `EdgeResampler(24000->16000)` on the kokoro
   WAV -> `192800` samples (12.05 s) -> `/tmp/ark_golden/pcm16.f32`. The sidecar golden consumes that exact
   buffer, so both arms see byte-identical samples (the resampler is a Blackman-sinc FIR - replicating it in
   Python would have risked a mismatch).
2. **Mel.** `ark_log_mel` (natural-length Whisper 128-mel, center reflect-pad, **per-clip max** norm, drop the
   last frame - NO 30 s padding, since the processor uses `padding="longest"`) matches the sidecar's fp32
   `WhisperFeatureExtractor` `input_features` to **maxDelta ~ 2.5e-5** (an f32-vs-f64 STFT residual), with the
   **boundary frames exact (0.0)** - confirming the no-pad reflect-at-true-boundary is right.
3. **Prompt tokenization.** The Rust `tokenizers` encode of the prompt string yields the **exact 160 input
   ids** (byte-exact match to the sidecar `input_ids`), and `tokenizer.decode` of the golden gen_ids
   reproduces the golden transcript exactly.
4. **Token count.** `((1205+1)//2)//4 = 150` audio tokens == the encoder output N == the 150 `<|audio|>`
   placeholders. The scatter aligns 1:1.

With the frontend proven, the in-process decode produced the byte-identical transcript directly.

## The byte-identical playbook - per-bug-class checks

| # | Bug class | ARK status |
|---|-----------|------------|
| 1 | **FUSED libtorch ops** | RMSNorm/LayerNorm spelled with the f32-accumulate reduction (matching HF's f32-upcast Qwen2RMSNorm / nn.LayerNorm); Linear via the zero-copy matmul(w^T-view); SDPA built in the eager graph. Greedy + robust transcript -> no fused-vs-decomposed ULP flips observed (cf. B25's dia2 RMSNorm 1-ULP, which only mattered under sampling). |
| 2 | **bf16 vs f16 exact** | Weights are bf16 on disk; decoder runs **f16-on-CUDA** (f32 on CPU). The sidecar CPU-fp32 == CUDA-fp16 transcript+ids are identical, so the f16 decode is safe; the f32 final-logits decision (below) removes the only place a tie could flip. |
| 3 | **EXACT tokenizer** | Rust tokenizers reproduces the 160 prompt ids byte-exactly + decodes the golden ids to the golden string. |
| 4 | **RoPE inv_freq** | Two distinct RoPEs implemented exactly: encoder = **partial interleaved** (first 32/64 dims, complex-pair, theta=10000); decoder = **full rotate-half** (theta=1e6). Unit test pins the partial/interleaved contract (identity at pos 0, trailing dims pass through). |
| 5 | **TF32** | Not relied on - encoder f32 + final-decision f32; the f16 decode dtype is explicit. (tch is the same libtorch PyTorch loads, so any residual matmul behavior is shared with the reference.) |
| 6 | **GREEDY decode** | Final tied-lm_head projection + argmax run in **f32**; argmax breaks ties to the **lowest index** (`x > bv`, first-max) - NOT tch::Tensor::argmax (unspecified CUDA tie-break). Plus the sidecar's **bad-words suppression** (all_special_ids U {added <...> tokens} - EOS forced to -inf) reproduced exactly via get_added_tokens_decoder() (content + special flag). Unit test pins suppressed-max-skipped + first-max. |
| 7 | **causal-conv left-pad (the voxtral conv-stem bug)** | **CHECKED - ARK is the OPPOSITE of voxtral.** ark's stem is symmetric nn.Conv1d **pad-1 on BOTH** convs (not causal): conv1d(stride, padding=1) in libtorch. Verified live (correct 150 audio tokens -> correct embeds -> byte-identical transcript) and pinned by a unit test asserting the symmetric pad-1 stride-2 phase reads the even inputs {0,2,4,6,8}, that pad-2 (a wrong count) shifts the phase (the bug class), and that the output length = floor((T+2-3)/2)+1. |
| 8 | **batched** | Single-utterance gate (B=1); the decode reuses the voxtral perf patterns (ring-KV, GQA-native, zero-copy gemm), so it is batch-ready by construction. |

## Perf patterns (reused verbatim from `voxtral.rs`)

- **Device-resident ring-KV** per layer: `[1,kv_heads,max_seq,d]` allocated once; `index_copy_` at the write
  index, `narrow` read-back - no per-step cat, no O(n^2) realloc.
- **Zero-copy `[rows,in]@W^T` gemm**: `w.transpose(-1,-2)` is a strided view (cublas OP_T) - the up-to-294 MB
  tied embedding is never copied per step.
- **GQA-native decoder attention**: fold `n_rep = 7` into the query rows; K/V stay un-expanded at `kvh=2`.
- **Mask only on the multi-row prefill**; single-row decode steps pass `None` (ark's decoder has
  `sliding_window: null` -> pure causal, so the newest query's mask is all-zeros).
- **Encoder in f32** (runs once/clip -> RTF-neutral); **final lm_head + argmax in f32** (one matmul/step,
  cheap). The 24-layer decode stays f16-on-CUDA -> RTF 0.270.

## The gate

`crates/waav-infer-backend-torch/tests/cuda_torch_ark.rs::cuda_torch_ark_byte_identical` (`#[ignore]`'d; run
via `ci/heavy_live_tests.sh`): emits the exact 16 kHz PCM (same EdgeResampler the dumper used), loads the HF
snapshot dir directly (symlink-populated with model.safetensors + tokenizer.json), transcribes on CUDA,
asserts `torch_txt == golden` byte-for-byte and `RTF < 1`. Self-skips cleanly if the weights or the persisted
sidecar golden (`$WAAV_ARK_GOLDEN`, default `/tmp/ark_golden/transcript_cpu_fp32.txt`) are absent - mirroring
the cosyvoice3/dia2 golden gates. The golden is (re)produced by the persisted `/tmp/ark_golden/dump_golden.py`
(`source gb10-env.sh && WAAV_INFER_ROOT=$(pwd) HF_HUB_OFFLINE=1 python3 ...`).

## Verification artifacts (`/tmp/ark_golden/`)

- `transcript_cpu_fp32.txt` / `transcript_cuda_fp16.txt` - identical golden transcripts.
- `gen_ids_cpu_fp32.npy` / `gen_ids_cuda_fp16.npy` - identical 34-token greedy outputs.
- `input_ids.npy` (160) - matched byte-exactly by the Rust tokenizer.
- `audios_fp32.npy` [128,1205] - matched by ark_log_mel to maxDelta 2.5e-5 (boundary exact).
- `audio_embeds_fp32.npy` [1,150,896] - the sidecar adapter output (the byte-identical transcript proves the
  tch encoder reproduces it; any encoder error would shift embeds and flip tokens).

## Result

| Metric | Value |
|--------|-------|
| Byte-identical transcript | **YES - 100.0% char-identity vs sidecar golden** |
| Conv-stem (playbook #7) | symmetric nn.Conv1d pad-1 (NOT causal) - verified + unit-pinned |
| RTF (CUDA f16) | **0.270** (<1) |
| clippy | clean (-p waav-infer-backend-torch --all-targets, 0 warnings) |
| unit tests | 29/29 green (6 new ark) |
| files touched | src/ark.rs, src/lib.rs, tests/cuda_torch_ark.rs, ci/heavy_live_tests.sh |
| worktree SHA | **2a350c6** |
