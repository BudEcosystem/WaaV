# B24 — tch CosyVoice3 AR speech-tokens BYTE-IDENTICAL to the PyTorch sidecar

**Verdict: ACHIEVED.** The tch CosyVoice3 [A] AR speech-token sequence is now **byte-identical** to the PyTorch
sidecar — **123 tokens == 123 tokens, first divergence: None, EXACT MATCH** under the same seed (`manual_seed(0)`)
and text (`"WaaV brings real time voice to every application."`). The backbone logits are bit-identical at every
step (`max|Δ|=0` over the full 6761-token vocab) and the RAS sampler now consumes libtorch's shared MT19937 in
the sidecar's exact op-order, so the sampled tokens match byte-for-byte. Cross-process reproducible (a fresh
process again yields the same 123 tokens). The pre-existing CFM/vocoder bit-faithful layers are unchanged.

- File touched: `crates/waav-infer-backend-torch/src/cosyvoice3.rs` (+ its gate `tests/cuda_torch_cosyvoice3.rs`).
- Worktree branch: `worktree-agent-a0c67c86614537abf`. Commit SHA: see HEAD (recorded at commit time).

## Are the AR tokens byte-identical? — YES (exact match)

```
[3b] LLM byte-identity vs sidecar golden tokens + full e2e
     LLM: 123 tokens in 1418 ms | first [2307, 2440, 253, 325, 244, 2278, 9, 2196, 2521, 2277, 2269, 2268]
     vs golden: tch 123 tokens, sidecar 123 | first divergence: None
     ✓ AR speech-token sequence BYTE-IDENTICAL to the sidecar (123 tokens)
```

The sidecar IS deterministic on CUDA-bf16 (run-to-run identical, verified) — so the golden 123-token dump is a
firm reference, and "cross-process CUDA-bf16 sampling variance" (the old excuse) was a myth: the divergence was
entirely a set of **5 reproducible port defects**, each root-caused by localizing the FIRST divergent op/step
against the sidecar's dumped intermediates (never explained away).

## The precise sampling/precision fix (5 root causes, in the order found)

The starting state was 129/161-vs-123 tokens, same first token then drift. Root-causing by dumping the sidecar's
per-step logits, per-layer hidden states, and per-op sub-tensors, then diffing tch against them:

1. **Tokenizer (the dominant cause).** BlankEN ships **no `tokenizer.json`** (only `vocab.json` + `merges.txt`),
   so the loader fell back to a raw `BPE::from_file(...).build()` with **no pre-tokenizer**. BlankEN's
   `tokenizer_class` is `Qwen2Tokenizer` (GPT-2 byte-level BPE); the raw BPE tokenized the text completely
   differently → `tts_ids` came out the wrong length (the assembled LM input `emb` was **320** positions vs the
   sidecar's **317**) → every downstream logit diverged. Fix: build the tokenizer with a **ByteLevel
   pre-tokenizer** (`add_prefix_space=false`, GPT-2 split regex `use_regex=true`) + ByteLevel decoder, matching
   `AutoTokenizer`. Verified `tts_ids` → `[54,5305,53,12434,1931,882,7743,311,1449,3766,13]` (11 ids) byte-for-byte
   and `emb` sum 473.7375 == golden.

2. **bf16, not f16.** The backbone ran in `Kind::Half` (f16) on CUDA; the sidecar runs Qwen2 in **bf16**
   (`config.json torch_dtype:"bfloat16"`, `qwen.to(torch.bfloat16)`). f16 (5 exp / 10 mantissa) and bf16 (8/7)
   round differently → divergent logits. Fix: `llm_dt = Kind::BFloat16` on CUDA.

3. **`at::linear` (fused addmm), not `matmul`+`add`.** HF's `nn.Linear` issues a single fused `addmm(bias, x, Wᵀ)`;
   the hand-rolled `x.matmul(Wᵀ)` then a separate `+ b` rounds the bias add through an extra bf16 round-trip,
   drifting ~1 ULP/layer that compounded over 24 layers (layer-23 pre-norm `max|Δ|` 8.0 → **0.5** after this fix).
   Fix: `x.linear(&weight, bias)` (libtorch `at::linear`).

4. **RoPE `inv_freq` rounded through bf16 — THE first divergent op.** Localized via per-layer then per-op dumps:
   `ln_out` and `qproj_out` were byte-identical, but the **post-RoPE `q`** diverged (tch sum -34.48 vs golden
   -36.22). Cause: HF's `Qwen2RotaryEmbedding.inv_freq` is a registered **buffer**, so `model.to(bfloat16)`
   **rounds it to bf16** (e.g. `1/θ^(2/64)` = 0.6494… stored as **0.6484375**); the forward then upcasts to f32 to
   build cos/sin. Building cos/sin from the FULL-f32 `inv_freq` gave a different rotation. Fix: round `inv_freq`
   through the model dtype (`.to_kind(dt).to_kind(Float)`) before the freqs matmul. **This made the entire
   backbone byte-identical** — layer 0/23/prefill `max|Δ|=0`, step-0 AND step-1 logits `max|Δ|=0` over all 6761.

5. **RAS `win_size=10`, not 25 — the RNG-desync fix.** Even with bit-identical logits, the full sequence still
   diverged at step 1: a pure RNG desync. The constant `RAS_WIN` was `25`, but the sidecar call
   `ras_sampling(logp, out, 25, top_p=0.8, top_k=25)` passes **`sampling=25`** (a positional arg that
   `random_sampling` actually ignores) and leaves `win_size` at its **default 10**. With 25 the repeat window was
   wider AND the fire threshold was `25·0.1=2.5` (rep≥3) instead of `10·0.1=1.0` (rep≥1) — so the repeat-aware
   fallback fired at DIFFERENT steps, and the fallback's **extra `multinomial`** desynced the per-step RNG draw
   count from the sidecar. Fix: `RAS_WIN = 10`. (Confirmed `10*0.1 == 1.0` exactly in both Python and Rust, so
   `rep≥1` fires identically.)

Two further things were proven NON-issues during root-causing (so they were left as-is): (a) `torch.multinomial`
RNG consumption is identical whether the input is a fresh tensor or a `narrow` view (length-`keep` CUDA tensor) —
so the nucleus draw stays in sync; (b) the nucleus/random samplers already match the sidecar bit-for-bit given
identical logits (verified by feeding identical `logp` and getting identical pick sequences).

### First-divergent-step analysis (the decisive trace)

| Stage probed | tch vs sidecar golden | where it pointed |
|---|---|---|
| `emb` (LM input) | 320 vs **317** positions, sum 471.98 vs 473.74 | → tokenizer (cause 1) |
| per-layer prefill hidden | diverges from **layer 0** (`max|Δ|` 0.0078, growing) | → a per-op ULP source |
| layer-0 sub-ops | `ln_out` ✓, `qproj_out` ✓, **`q_roped` ✗** (-34.48 vs -36.22) | → RoPE (cause 4) |
| after RoPE fix | layer 0/23/prefill `max|Δ|=0`; step-0 + step-1 logits `max|Δ|=0` (full 6761) | backbone bit-identical |
| full sequence after RoPE fix | still 149-vs-123, first divergence **step 1** (identical logits) | → RNG desync (cause 5) |
| after `win_size=10` | **123 == 123, first divergence None, EXACT** | DONE |

## RTF (target < 1) — PASS

| Path | Time | Audio | RTF |
|---|---|---|---|
| **[A] AR loop** (123 tokens) | ~1.42 s | — | — |
| flow → mel (CFM 10-step) + HiFT vocoder | ~1.59 s | 4.98 s | **0.31–0.32** |
| **full e2e** (LLM + flow + vocoder) | ~2.6 s | 4.98 s | **0.51–0.53** |
| load | ~4.4 s | — | — |

(The e2e RTF *improved* from 0.85 → ~0.52 because the now-correct token count yields the sidecar's exact 4.98 s
audio instead of an over-long 161-token run. estimator EP = **cuda**, telemetry-confirmed.)

## The CFM/vocoder layers (unchanged, still bit-faithful)

- **[1] CFM seam** on the (now identical) tokens: mel `max|Δ| 0.0049`, `RMS(Δ) 0.00023` — the pure CUDA-EP vs
  sidecar-CPU-EP ONNX-estimator delta (the only cross-runtime difference; the front-end mu/spk/cond reproduce).
- **[2] HiFT vocoder** on the golden mel: corr 0.853, RMS 0.1789 vs golden 0.1782 (NSF phase-noise buffers are
  process-fixed, so structurally faithful, not sample-identical — the established bar for that stage only).
- **[3a] flow→vocoder determinism**: 119460/119460 identical samples across two within-process runs.

## Verification

- `cargo clippy -p waav-infer-backend-torch --all-targets --features cuda -- -D warnings` — **clean** (after a
  `cargo clean -p` full recompile).
- lib unit tests **5/5** cosyvoice3 (cosine schedule, CFG-Euler exact, weight-norm, Hann, PCM) pass.
- Live gate `cuda_torch_cosyvoice3` **PASS** with the new byte-identity assertion (`assert_eq!(tokens,
  golden_tokens)`), CFM seam, vocoder, determinism, and e2e RTF — run via `ci/heavy_live_tests.sh` with the
  goldens at `$WAAV_CV3_GOLDEN` (default `/tmp/cv3_golden`; a durable copy is persisted at
  `~/.cache/waav-models/cosyvoice3/.cv3_golden`).
- Touched ONLY `cosyvoice3.rs` + `tests/cuda_torch_cosyvoice3.rs` (the temporary per-op diagnostic scaffolding was
  removed before commit). voxtral.rs / dia2.rs / cohere / device.rs / lib.rs / other crates / torch_runtime/*.py:
  untouched.

## Notes for the merger

- The gate now HARD-asserts AR byte-identity when the goldens are present (degrades to a loose plausibility bound
  only if absent). The golden `speech_tokens.bin` (123 i64) + `mel.bin`/`wav.bin`/`shapes.json` are the sidecar
  dump for the fixed text+seed; regenerate with the sidecar (`torch_runtime/models/cosyvoice3.py` on the same
  text, `torch.manual_seed(0)`) on a fresh box, or point `$WAAV_CV3_GOLDEN` at the persisted copy.
- The four faithful idioms (bf16, `at::linear`, **bf16-rounded RoPE `inv_freq`**, fused `enable_gqa` SDPA) are the
  template for byte-identifying the rest of the Qwen/Llama-backbone family (dia2, qwen3-asr, higgs, etc.) against
  their sidecars — the RoPE `inv_freq` buffer-cast in particular is a silent, high-impact divergence to copy.
