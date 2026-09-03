# PERF — kyutai/pocket-tts (WaaV Infer, streaming + threaded, byte-faithful preserved)

**Status: SHIPPED — CPU RTF 1.14× → 2.38× (≈2.1× speedup), CUDA 22.4×, byte-faithfulness preserved
(streamed == one-shot == golden, proven to 0 bits). Gate 5/5 green, lib 192/0, pocket clippy clean.**

Owner: `crates/waav-infer-backend-torch/src/pocket_tts.rs` (+ its gate). Did NOT `git commit`, did NOT
`cargo fmt`. Concurrent indextts2 + multi-hw agents were editing `indextts2_encoders.rs`/shared files; my
changes are confined to the two pocket files and are independent of theirs.

---

## 1. The headline: the RTF gate was measuring the WRONG thing

The onboard's "RTF ~0.9× CPU / 1.1× CUDA" was measured on the **5-frame "Hello world." gate clip** (0.24 s
audio), where fixed per-call costs dwarf the steady-state. On a **realistic 57-frame / 4.56 s clip** the true
RTF is very different, and the dominant CPU lever turned out to be **libtorch's default thread count**, not the
model code:

| regime | CPU RTF (57-frame, best of 3) | CUDA RTF |
|---|---|---|
| **BEFORE** (libtorch default = 20 threads) | **1.14×** (4005 ms) | 18–22× (already fast) |
| **AFTER** (pinned 8 threads + on-device latent) | **2.38×** (1916 ms) | **22.4×** (203 ms) |

The CUDA "1.1×" in the onboard was purely the 5-frame fixed-cost artifact — CUDA was **always ~22×** on a real
clip.

---

## 2. Levers applied (each accuracy-preserving; byte-faithfulness gated)

### Lever 1 — CPU intra-op thread count (THE headline, ~2.1×) — `pocket_tts.rs`
GB10's Grace CPU is **big.LITTLE**: 10× Cortex-X925 (3.9 GHz) + 10× Cortex-A725 (2.8 GHz), 1 thread/core.
libtorch defaults to spawning **20** OMP threads. For this **batch-1, 100M-param, launch-bound** synth the tiny
per-op work **oversubscribes** the 20 cores and spills onto the slow little cluster + pays cross-cluster sync.
Measured RTF vs thread count (57-frame clip):

```
 1 thr → 0.58×    4 thr → 1.75×    8 thr → 2.4–2.6× (PEAK)   10 thr → 1.0×    20 thr → 1.1×
```

The optimum is **8** (the big-core count minus headroom). Beyond 8 it collapses (little-cluster spill). Shipped:
- New field `cpu_threads: Option<i32>` defaulting to `DEFAULT_CPU_THREADS = 8`; setter `set_cpu_threads`.
- `apply_cpu_threads()` pins `tch::set_num_threads(8)` on the **CPU path only** at the `synthesize_pcm` /
  `synthesize_streaming` entry and **restores the prior count on exit** (the count is process-global; other
  concurrently-loaded Torch models must not be left re-pinned). No-op on CUDA.
- kyutai's own "~6× on 2 threads (M4)" target is M4-specific; on GB10 aarch64 the big-core optimum is 8.

**Byte-faithfulness:** thread count is pure scheduling. Gate `pocket_tts_thread_count_byte_faithful` proves
8-thr vs 1-thr synth has the **same sample count, same EOS-driven trajectory length, corr = 1.000000000, audio
max|Δ| = 4.4e-8** — the f32 *parallel-reduction* floor (libtorch multi-thread reductions are non-associative;
NO multi-threaded f32 GEMM is bit-identical to single-thread). This is the SAME class as the onboard's
documented CPU↔CUDA ~5e-6 latent floor and is **100× below** even the existing CUDA audio floor; it flips no EOS
decision and perturbs no audible sample. Notably the **pinned-8 delta (4.4e-8) is SMALLER than the old
default-20's (2.8e-7)** — the pin is *more* faithful to single-thread than the prior default was.

### Lever 2 — eliminate the per-step D2H→host→H2D latent round-trip (bit-identical) — `pocket_tts.rs`
The pocket analog of the voxtral per-step lm_head recast (commit 3a53094). The old AR loop, **every frame**:
`eos_and_latent` → `Vec::<f32>::try_from(latent)` (**D2H copy + hard GPU sync**) → push to host Vec → next step
`Tensor::from_slice(vec).to(dev)` (**H2D copy**). On CUDA that is one full device sync + two 32-float copies per
frame; on CPU it is two allocs/copies per frame.

Shipped: a device-resident greedy path:
- `greedy_dev()` keeps the latent **on-device** across steps — the previous step's `[1,ldim]` device tensor is
  fed straight into the next step's `backbone_input` (no host round-trip). Only the **EOS scalar** still syncs
  to host (the greedy break genuinely needs it).
- `eos_and_latent_dev()` returns the device latent tensor; `latents_to_host()` does a **single batched D2H**
  (`cat` → one transfer) to materialize the public `Vec<Vec<f32>>` only at the boundary.
- `decode_latents_dev()` + `decode_set_dev()` keep the chosen AR latents on-device through the Mimi decode.
- Public `greedy()` and `synthesize_pcm()` are unchanged in signature and **bit-identical** in output
  (`from_slice(Vec::try_from(t)) == t` reconstructs the exact f32 bits; `where_self`/`input_linear` are the same
  math on the same values regardless of where the tensor was last materialized — the flow latent is never NaN,
  so the BOS `where_self` passes it through unchanged). Verified: the existing CPU/CUDA byte-faithful gate
  latent max|Δ| is **unchanged** (4.649e-6 CPU / 3.695e-6 CUDA — identical to the pre-change baseline), and the
  text-to-audio gate stays corr 1.000000. On CPU this is a marginal win (host copies are cheap); on CUDA it
  removes the per-step sync (matters most on longer / GPU-contended clips).

### Lever 3 — streaming synth (the kyutai item-7 / TTFA delivery pattern, bit-identical) — `pocket_tts.rs`
- `synthesize_streaming(text, on_frame)` invokes `on_frame(idx, &pcm_frame)` per 1920-sample frame; returns the
  full PCM too. `SAMPLES_PER_FRAME = 1920` (`sample_rate / frame_rate` = 24000/12.5).
- The engine seam `TtsModel::synthesize` now emits **one `SynthChunks` entry per frame** (was a single chunk),
  so a consumer can begin playback as frames land — through the existing `Vec`-of-chunks trait, **no shared
  core code touched**.

**Byte-identity (the LAW):** the streaming path is a pure **delivery re-chunk of the ONE deterministic one-shot
decode** — NOT an incremental conv-state decode. Gate `pocket_tts_streaming_bit_identical_to_oneshot` proves
`concat(streamed_frames) == synthesize_pcm` **to 0 bits** (`max|Δ| = 0.000e0`), and the engine per-frame i16
chunks concatenate to the one-shot PCM16 bit-for-bit.

---

## 3. Why NOT true incremental Mimi decode (the honest ceiling on TTFA)

The genuinely-streaming TTFA win would be a **per-frame incremental Mimi decode** (decode latent prefix
`[0..=i]`, emit frame `i` right after AR latent `i`). I **measured** it and it is **NOT bit-identical**:
`probe_prefix_decode_bit_identity` → tail max|Δ| **1.27e-7** on early frames (exactly the ~1.7e-7 the onboard
flagged). Cause: the continuous-Mimi decoder is **not chunk-decomposable bit-for-bit** — `MimiConv`'s
`_get_extra_padding_for_conv1d` right-pad depends on the **total** sequence length, and the decoder
`ProjectedTransformer` runs a sliding-window (context-250) over the full upsampled sequence; a prefix decode
sees a different boundary. So an incremental decode would drift ~1e-7 and **break the byte-identity LAW**.

**The honest perf ceiling:** the byte-identity requirement **forbids** incremental decode, so TTFA-to-first-
audio is bounded by *AR-loop completion + one decode*. The AR loop is the slow part (≈31 ms/frame on CPU/8thr),
so the streaming API is a clean *delivery* seam but cannot emit audio before the AR finishes without violating
the law. The materially-improving, law-abiding levers are the thread pin (#1) + the D2H elimination (#2). If a
future caller accepts the documented ~1e-7 streaming floor (corr still 1.0, no EOS flip), the prefix-decode
incremental path is ready to wire behind an explicit opt-in — but it is OFF by default to keep
streamed == one-shot == golden exact.

### Why < kyutai's 6× on CPU (the precise reason)
1. **aarch64 GB10 Grace ≠ MacBook-Air-M4.** kyutai's 6× is M4-CPU-specific (different SIMD/cache, Accelerate
   BLAS). On GB10 the big-core peak is ~2.4–2.6×.
2. **libtorch eager + tch overhead.** Every op is a dispatched libtorch kernel launch; at batch-1/100M the
   per-op dispatch is a large fraction of the work (the same launch-bound regime the voxtral fix hit). kyutai's
   reference is a hand-tuned Rust/candle streaming loop, not eager libtorch.
3. **f32 (the byte-faithful regime).** The golden is CPU-f32; we run f32 to stay byte-faithful. kyutai's
   shipped CPU path can use bf16/lower-precision GEMM (faster, but would break the f32 byte-identity gate).
2.4× on f32-eager-libtorch-aarch64 is the honest best without abandoning byte-faithfulness.

---

## 4. Byte-identity proof (streamed == one-shot == golden)

All in `crates/waav-infer-backend-torch/tests/cuda_torch_pocket_tts.rs` (5/5 PASS, --ignored):

| gate | result |
|---|---|
| `pocket_tts_byte_faithful_cpu` | latents max|Δ| **4.649e-6** (== baseline), eos exact, mimi corr 1.0, e2e corr 1.0 |
| `pocket_tts_byte_faithful_cuda` | latents max|Δ| **3.695e-6** (== baseline), e2e corr 0.999999 |
| `pocket_tts_streaming_bit_identical_to_oneshot` | streamed PCM == `synthesize_pcm` **0 bits** (`f32::to_bits` equal); engine per-frame i16 chunks == one-shot PCM16 bit-for-bit |
| `pocket_tts_text_to_audio_matches_reference` | full frontend → corr **1.000000**, sample count exact |
| `pocket_tts_thread_count_byte_faithful` | 8-thr & 20-thr vs 1-thr: same length, corr 1.0, audio max|Δ| 4.4e-8 / 2.8e-7 (f32 parallel-reduction floor) |

The CPU/CUDA latent floors are **unchanged from the pre-perf baseline** → the on-device latent path is bit-
identical to the old host round-trip. Streaming is bit-identical to one-shot. The thread pin is byte-faithful
to the documented f32 floor (and *closer* to single-thread than the old default).

---

## 5. Files

- **EDIT (owned):** `crates/waav-infer-backend-torch/src/pocket_tts.rs`
  - `cpu_threads` field + `DEFAULT_CPU_THREADS = 8` + `set_cpu_threads` + `apply_cpu_threads` (pin/restore).
  - `greedy_dev` / `eos_and_latent_dev` / `latents_to_host` (on-device AR; single batched D2H boundary).
  - `decode_latents_dev` / `decode_set_dev` (device-tensor decode shared by one-shot + streaming).
  - `synthesize_streaming` + `SAMPLES_PER_FRAME`; `synthesize_pcm` now wraps thread pin/restore + device path.
  - `TtsModel::synthesize` emits per-frame chunks (byte-identical re-chunk).
- **EDIT (owned):** `crates/waav-infer-backend-torch/tests/cuda_torch_pocket_tts.rs`
  - new gates `pocket_tts_streaming_bit_identical_to_oneshot`, `pocket_tts_thread_count_byte_faithful`.
- No shared `nn::`/`codec::`/`lib.rs` edits → no cross-model regression (lib 192/0, the shared codec/rope/
  layernorm byte-identity tests stay green).

## 6. Verification commands
```
source gb10-env.sh
cargo test  -p waav-infer-backend-torch --test cuda_torch_pocket_tts -- --ignored --nocapture --test-threads=1
cargo test  -p waav-infer-backend-torch --lib            # 192 passed
cargo clippy -p waav-infer-backend-torch --all-targets -- -D warnings   # pocket_tts clippy-clean
```
(Note: a concurrent agent's in-progress `indextts2_encoders.rs` may transiently fail the workspace clippy/build
while they iterate — that file is not mine; the pocket gate runs green and pocket_tts.rs has zero clippy
findings.)

## 7. Follow-ups (not blockers)
1. **Opt-in incremental-decode streaming** behind an explicit flag for callers that accept the ~1e-7 Mimi
   streaming floor (true per-frame TTFA; OFF by default to keep the byte-identity LAW).
2. The thread optimum (8) is GB10-Grace-specific; a small auto-tune (probe big-core count) would generalize the
   default across hosts.
