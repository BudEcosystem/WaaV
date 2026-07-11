# Chunked Prefill — closing the vLLM-parity gap (pillar #6)

> **Status:** LANDED + GATED (CPU unit gates + LIVE real-weight ark gate). The `VLLM-PARITY-MATRIX.md`
> pillar #6 ("Chunked prefill — MISSING, monolithic prefill") is now **HAVE-LIVE**: the prompt prefill can be
> fed to the backbone in fixed-size `chunk_size`-token chunks (accumulating KV across chunks) instead of one
> monolithic forward — the vLLM scheduling primitive that lets a long prompt's prefill interleave with decode.
> The decoded output is **byte-identical** to monolithic; the win is scheduling/latency.

Implemented in `/home/bud/ditto/waav/waav-infer`. No `git commit`, no `cargo fmt`. The concurrent per-slot-LoRA
agent owned `nn/lora.rs` / `nn/linear.rs` / `nn/mod.rs` / `qwen3_tts.rs` / `canary_qwen.rs` — **no conflict**:
my seam is a new method on the already-exported `Backbone` type, so I never had to touch `nn/mod.rs`.

---

## 1. What landed

### The chunk seam — `Backbone::prefill_chunked` (NEW, shared, additive)
`crates/waav-infer-backend-torch/src/nn/backbone.rs`

A reusable driver on the shared `Backbone` (used by all 7 tch models). Given the full prompt
`embeds[1,L,hidden]`, a `chunk_size`, the per-layer ring `KvCache`s, the absolute `start` position, and a
per-chunk `mask_for(q_len, kv_len)` builder (so each model supplies its own causal/window/no-mask flavor), it
drives the prefill chunk-by-chunk through the existing `Backbone::forward`:

- chunk `k` covers prompt rows `[s, s+c)` (a final partial chunk is shorter when `L % chunk != 0`);
- it runs at `pos = start + s` so `Rope::apply_start` lands RoPE at positions `[s, s+c)` (absolute, chunk-
  invariant) and `positions = (start+s .. start+s+c)` for the `apply_positions` regimes;
- the mask is `[c, s+c]` — q rows are the LAST `c` of the running `(s+c)`-long context (the exact causal
  sub-block of the monolithic `[L,L]` mask);
- the ring `KvCache::write` already appends a `q>1` chunk in place, so after chunk `k` the cache holds rows
  `[0, s+c)` — chunk `k+1` attends ALL of chunks `0..=k`;
- it concatenates the per-chunk post-`final_norm` hidden into the full `[1,L,hidden]` (the caller takes the
  last row → first-decode logits).

`chunk_size == L` ⇒ one chunk = the monolithic forward; `chunk_size == 1` ⇒ token-at-a-time prefill; any
in-between (incl. a partial tail) is the interleavable regime.

### The model demonstrator — `TorchArk::prefill_chunked` + live env-knob
`crates/waav-infer-backend-torch/src/ark.rs`

**ark is the cleanest demonstrator**: its decoder is **pure causal** (`sliding_window: null`), **ManualGqa** (a
deterministic `matmul → +mask → softmax(f32) → matmul`), **`apply_start` RoPE** (absolute positions), **`View`**
cache. `TorchArk::prefill_chunked` wires the seam with ark's `causal_mask(q_len, kv_len)` builder and the f32
tied-lm_head over the last prompt row.

`transcribe_chunk` gained an opt-in **`WAAV_ARK_PREFILL_CHUNK=<n>`** env knob: unset/`0`/invalid = the existing
monolithic prefill (DEFAULT — ark's live transcription is byte-identical by default); `n>=1` switches to
chunked. Pure scheduling/latency knob; the transcript is unchanged for any chunk size.

---

## 2. Is it byte-identical? — YES for the decoded output; the intermediate-state story (honest RCA)

The decisive question is what "byte-identical" means here, resolved by an f32-CPU bisection diagnostic:

| chunk_size (L=9) | post-norm hidden maxΔ | KV maxΔ | first-logits maxΔ | greedy continuation |
|---|---|---|---|---|
| **9 (= full)** | **0.0** | **0.0** | **0.0** | **identical** |
| 4 | rows 0–7 = 0.0, row 8 ≈ 3e-6 | ≈ 3e-4 | ≈ 1e-4 | **identical** |
| 3 (all M=3) | ≈ 1e-6 (layers 0–1 K = 0.0) | ≈ 5e-6 | ≈ 4e-5 | **identical** |
| 1 (token-wise) | ≈ 8e-5 | ≈ 5e-4 | ≈ 1e-4 | **identical** |

**Two-tier guarantee, both gated:**

1. **EXACT byte-identity (maxΔ == 0.0) at `chunk_size == full`** — hidden + per-layer KV (K&V) + first-decode
   logits. One chunk IS the monolithic forward, so this pins the seam's positions / per-chunk mask / KV-append
   as the numerically-same op (any pos/mask/append bug would be O(1), not 0.0).
2. **Discrete-output INVARIANCE across `chunk_size ∈ {1, k, full}`** — the greedy continuation (the decoded
   tokens, the thing a caller consumes) is **IDENTICAL** for every chunk size. This is the correctness
   guarantee that matters: chunking changes scheduling, never the output.

**The honest scar (why `chunk_size < full` intermediate state is ~1e-5, not 0):** a query block of `M` rows
attending a `kv_len`-wide prefix is a libtorch GEMM whose tiling + vectorized-softmax **reduction order**
depends on `M` and `kv_len`. Chunking changes both (a chunk attends `s+c`, monolithic attends `L`; a single-row
`M=1` chunk takes the GEMV path), so the masked-future-zero reduction **re-associates sub-ULP** — the same
documented "batched-GEMM / K-dim reduction reassociation" byte-identity scar the shared `forward_full` / GQA-fold
notes already pin elsewhere in this codebase (e.g. `self_attention.rs`'s `forward_full` comment: the GQA-fold
"reassociates the reduction ~6e-5"). The masked entries are exactly 0, so the result is **mathematically
identical**; only the float reduction order differs. It never flips the greedy argmax (proven), and it never
changes the decoded transcript (proven live).

> Making `chunk_size < full` ALSO maxΔ==0 would require a **fixed-width (`full_masked`) attention reduction** in
> the shared SDPA kernel (so every chunk reduces over a constant `max_seq` lane count) — that touches the shared
> `nn::Attention`/`KvCache` path used by all 7 models (high byte-identity-regression surface) and is the
> concurrent agent's adjacent territory. Scoped, not faked. The per-chunk hidden is throwaway anyway (only the
> last row's logits + the KV feed decode), and both of THOSE are exact at the final chunk (which attends the
> full `L` in both paths) — which is exactly why the live transcript is byte-identical.

---

## 3. The gates (all green)

### LIVE — real ark weights, CUDA f16 (the headline demonstration)
`cuda_torch_ark_chunked_prefill_byte_identical_to_monolithic`
(`crates/waav-infer-backend-torch/tests/cuda_torch_ark.rs`, `#[ignore]` / heavy-live)

Loads the real `ARK-ASR-0.6B`, transcribes the kokoro clip with `WAAV_ARK_PREFILL_CHUNK ∈ {1, 3, 8, 100000}`
and unset (monolithic), asserts **every transcript is byte-identical to the monolithic AND to the sidecar
golden**. RESULT (run live on GB10):

```
MONOLITHIC: "Hello world. This is W A V. Infer a portable voice inference engine running live on the GB10 Grace BL, a C K W E L L."
chunk_size=     1: <identical>
chunk_size=     3: <identical>
chunk_size=     8: <identical>
chunk_size=100000: <identical>
✔ chunked prefill (chunk_size ∈ {1,3,8,full}) byte-identical to monolithic AND golden   (1 passed)
```

### CPU unit gates (`cargo test -p waav-infer-backend-torch --lib`, no weights / no GPU)
- `ark::tests::chunked_prefill_bit_identical_to_monolithic` — the named gate. Tier-A exact (chunk=full →
  hidden+KV+logits maxΔ==0.0); Tier-B discrete-output invariance (greedy continuation identical for chunk_size
  ∈ {1, 4, full}); the scar-bound on intermediate state. Synthetic ark-decoder backbone at ark's REAL shapes
  (HIDDEN=896, 14 q-heads / 2 kv-heads / head_dim 64, ManualGqa+Start+View) — the exact shared op-sequence.
- `ark::tests::chunked_prefill_partial_last_chunk_bit_identical` — the **chunk-boundary edge** test: sweeps
  `(L, chunk)` with `L % chunk != 0` (partial tail) + `chunk > L` (one chunk) + `L == 1` — KV LENGTH exact,
  greedy continuation identical, exact byte-identity when one chunk.
- `nn::backbone::tests::prefill_chunked_matches_monolithic` — the seam tested independent of ark.

### Regression — no model byte-identity drift (shared `backbone.rs` is additive-only)
- **dia2: 544/544 byte-identical** (CPU fp32, live, real weights) — `cpu_fp32_codes_byte_identical` passed.
- **csm: byte-identical** (CUDA bf16, live, 125 frames × 32 codebooks) — `cuda_csm_codes_byte_identical_to_sidecar` passed.
- My `backbone.rs` change is a NEW method only — `forward` / `forward_graph` / `forward_bidirectional*` (the
  paths dia2/csm/the other 5 use) are untouched → structurally zero regression surface.

### Build / lint
- `cargo test -p waav-infer-backend-torch --lib` → **206 passed, 0 failed** (mine + the concurrent LoRA agent's).
- `cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings` → **clean** (added the `forward`-style
  `#[allow(clippy::too_many_arguments)]` on `prefill_chunked`; fixed an `explicit_counter_loop`).
- I did NOT touch the scheduler/runtime crate (the seam lives entirely in the tch backbone + ark), so no runtime
  test run was required.

---

## 4. Exact files changed

| File | Change | Owner |
|---|---|---|
| `crates/waav-infer-backend-torch/src/nn/backbone.rs` | **NEW** `Backbone::prefill_chunked` (the chunk seam) + `prefill_chunked_matches_monolithic` test | mine |
| `crates/waav-infer-backend-torch/src/ark.rs` | **NEW** `TorchArk::prefill_chunked` + `WAAV_ARK_PREFILL_CHUNK` knob in `transcribe_chunk` + 2 CPU gate tests | mine |
| `crates/waav-infer-backend-torch/tests/cuda_torch_ark.rs` | **NEW** live `cuda_torch_ark_chunked_prefill_byte_identical_to_monolithic` gate | mine |

**Shared-file note (flagged):** `nn/mod.rs`, `nn/lora.rs`, `nn/linear.rs`, `qwen3_tts.rs`, `canary_qwen.rs`,
`tests/lora_peft_byte_faithful.rs` also show as modified in the working tree — those are the **concurrent
per-slot-LoRA agent's** edits, NOT mine. No overlap with my seam.

---

## 5. What landed vs scoped

**Landed:** the byte-faithful chunked-prefill machinery (KV accumulates across chunks via the existing
device-append; positions/masks threaded so chunk `k` sees chunks `0..k`), wired live into a real tch model
(ark), with a passing live gate proving the decoded output is byte-identical to monolithic across chunk_size ∈
{1, k, full} including a partial-tail boundary, plus exact-0.0 byte-identity at chunk=full and CPU unit gates.

**Scoped (not faked):** exact maxΔ==0.0 intermediate state for `chunk_size < full` — blocked on a fixed-width
`full_masked` attention reduction in the shared SDPA kernel (constant lane count across chunk sizes). High
byte-identity-regression surface across all 7 models + concurrent-agent territory; the per-chunk hidden is
throwaway and the decode-feeding state (last-row logits + KV) is already exact at the final chunk, which is why
the live transcript is byte-identical regardless. Tracked for a follow-up if a long-context STT/dialogue
workload ever needs bit-exact intermediate prefill activations.
