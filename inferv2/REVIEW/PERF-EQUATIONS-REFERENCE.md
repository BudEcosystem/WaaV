# PERF-EQUATIONS-REFERENCE — Ground-truth hardware constants, equations & counter sources for WaaV Infer perf analysis

**Status:** v1.0 · **Date:** 2026-06-29 · **Device of record:** NVIDIA **GB10** Grace-Blackwell (sm_121, aarch64, 121 GB unified LPDDR5X) · **Stack:** torch 2.12 + CUDA 13.0, ONNX Runtime · **Hard requirement:** every equation below is the EXACT formula the perf-analysis scripts must implement. A wrong constant or formula invalidates every experiment conclusion, so each number is sourced and each formula carries units + a worked example.

> **How to use this file.** §1 = hardware constants (cite + how to re-measure). §2 = the core equations (roofline, MFU, MBU, occupancy, bottleneck class, RTF, Amdahl, decode arithmetic). §3 = how `nsys` / `ncu` / `torch.profiler` expose the raw counters the scripts parse. §4 = the per-profile report schema. **Never hardcode a peak you did not either cite here or measure with the §1.7/§1.8 microbenches.** When in doubt, measure on the box — vendor "up to" numbers are upper bounds, not achievable peaks.

---

## 1. GB10 hardware constants

### 1.1 The authoritative published numbers (cite these, do not guess)

| Constant | Value | Unit | Source |
|---|---|---|---|
| GPU architecture | Blackwell, 5th-gen Tensor Cores, 4th-gen RT | — | [S1][S2] |
| Compute capability | **sm_121** (CUDA CC 12.1) | — | [S3] |
| Streaming Multiprocessors (SM) | **48** | count | [S3][S4] |
| CUDA cores (FP32 lanes) | **6,144** (128/SM) | count | [S1][S2][S4] |
| Tensor Cores | **192** (4/SM, 5th-gen) | count | [S2][S4] |
| L2 cache | **24 MiB** (25,165,824 B) | bytes | [S3] |
| GPU boost clock | **≈2.5 GHz** (derived, see §1.4; not officially published) | Hz | derived |
| Memory type | **LPDDR5X**, unified CPU+GPU pool | — | [S1][S5] |
| Memory capacity | **128 GB** (≈121 GB usable) | bytes | [S1][S5] |
| Memory bus width | **256-bit** | bits | [S5] |
| Memory clock / rate | **4266 MHz** → **8533 MT/s** effective | — | [S5] |
| **Peak memory bandwidth (theoretical)** | **273** | GB/s | [S1][S5] |
| Measured GPU bandwidth (real) | **≈231** (≈85 % of 273) | GB/s | [S6] |
| DRAM latency (idle / under load) | **113 / 351–400** | ns | [S6] |
| FP4 (NVFP4) tensor, **sparse** | **1000 TOPS = 1 PFLOP** | FLOP/s | [S1][S5] |
| FP32 (CUDA cores, non-tensor) | **≈31** | TFLOP/s | [S7] |
| TDP (whole SoC) | **140** | W | [S2][S5] |

**Critical framing:** GB10 is **NOT an HBM datacenter GPU.** Its memory is **LPDDR5X at 273 GB/s** — roughly **12–15× lower** than an H100/B200 (≈3.3–8 TB/s HBM). For voice inference (AR decode, B≈1) the workload is **memory-bandwidth-bound**, so **273 GB/s — not the TFLOPs — is the constant that decides realtime.** Treat 273 GB/s as the single most load-bearing number in this file.

### 1.2 The Tensor-Core precision ladder (derived; the part most likely to be gotten wrong)

NVIDIA only publishes two GB10 compute anchors: **FP4 sparse = 1000 TOPS** and **FP32 ≈ 31 TFLOPs** [S1][S7]. Every other precision must be **derived** from the 5th-gen consumer-Blackwell (sm_120/121) per-Tensor-Core throughput ladder, which is fixed by the architecture and cross-checked against the RTX 5090 (same Tensor Core, 680 TC @ 2.41 GHz) [S8][S9].

**Per 5th-gen Tensor Core, per clock (dense; ×2 with 2:4 structured sparsity):**

| Precision | FLOP / TC / cycle | Note |
|---|---|---|
| FP4 (NVFP4) | **1024** | the headline format |
| FP6 / FP8 | **512** | |
| FP16 / BF16, **FP16 accumulate** | **256** | full rate |
| FP16 / BF16, **FP32 accumulate** | **128** | **HALF rate** — the GeForce/consumer FP32-acc throttle [S8] |
| TF32, FP32 accumulate | **64** | |

Validation of the ladder (anchored on the two published numbers):
- Clock from FP4 anchor: `clock = 500e12 / (192 TC × 1024) = 2.54 GHz`.
- Clock from FP32 anchor: `31e12 / (6144 × 2 FLOP/cyc) = 2.52 GHz`. **The two agree ⇒ boost ≈ 2.5 GHz, ladder is self-consistent.**
- RTX 5090 cross-check: FP4 dense `1024 × 680 × 2.41e9 = 1676 TFLOPs` (matches published 3352 TOPS sparse ÷2); FP16-FP32acc dense `128 × 680 × 2.41e9 = 209.5 TFLOPs` (matches the documented 209.5) [S9]. Ladder confirmed.

**GB10 peak dense Tensor TFLOPs (192 TC @ 2.54 GHz; sparse = ×2):**

| Precision | **Dense TFLOP/s** | Sparse TFLOP/s | When it applies |
|---|---|---|---|
| **FP4 (NVFP4)** | **500** | 1000 (= published TOPS) | quantized inference only — **NOT used by WaaV (no-quant)** |
| **FP8** | **250** | 500 | quantized inference only — not used by WaaV |
| **FP16 / BF16 (FP16 acc)** | **125** | 250 | only if accumulation in FP16 (rare; accuracy-risky) |
| **FP16 / BF16 (FP32 acc)** | **62.5** | 125 | **← the rate that matters for WaaV bit-faithful inference** |
| **TF32 (FP32 acc)** | **31.25** | 62.5 | TF32 matmul path |
| **FP32 (CUDA cores)** | **31** | — | non-tensor FP32 |

> **Use the right denominator for MFU/roofline.** WaaV runs **bit-faithful bf16/fp32 with FP32 accumulation** (the `#2274` fp32-reduction rule). The correct peak for that regime is **62.5 TFLOPs (BF16, FP32 accumulate)** — *not* 125 and definitely not the 500/1000 FP4 marketing number. Using FP4 TOPS as the MFU denominator would understate MFU by **8–16×** and is a classic, conclusion-breaking error.

### 1.3 Why 273 GB/s (and ~231 GB/s achievable)

`BW = bus_bytes × effective_MT/s = (256 bits / 8) × 8.533e9 = 32 B × 8.533e9 = 273.1 GB/s` [S5]. The chip *supports* LPDDR5X-9400 (→301 GB/s) but the DGX Spark **runs at 8533 MT/s = 273 GB/s** — this reconciles the 273-vs-301 confusion in secondary sources. Independent measurement (chipsandcheese) saw **≈231 GB/s GPU read bandwidth** and **CPU+GPU contention pushing latency from 113 ns to ~400 ns** [S6] — i.e. the **unified pool is shared**: heavy CPU traffic steals GPU bandwidth (relevant to the GB10 unified-memory OOM/contention scars). **Use 273 GB/s for theoretical roofline; use the measured ~231 GB/s (re-measure per §1.7) for an "achievable" roofline.**

### 1.4 Derived / unofficial constants — flagged, must verify on box

- **Boost clock ≈ 2.5 GHz** — derived (§1.2), not officially published. 140 W SoC TDP means **sustained clocks under load may be lower** (thermal/power throttle). Re-measure achieved TFLOPs via §1.8 rather than trusting 2.5 GHz.
- **FP16/FP8/TF32 TFLOPs** — derived from the ladder, not on NVIDIA's GB10 datasheet. Verify with §1.8 if a conclusion depends on the compute peak (rare for voice; almost everything is memory-bound).

### 1.5 SM micro-architecture facts that change the kernel strategy (sourced elsewhere in this repo, restated)

- **No FlashInfer / FA3 / FA4 on sm_12x** (Blackwell consumer): no TMEM/WGMMA, half-rate FP32-acc MMA, and an illegal-memory crash at GQA=16. Pin `scaled_dot_product_attention` to **cuDNN/flash** backend. (See `INFER_PERF.md`, `INFER_PERF_BENCH.md`.) This is a *kernel-selection* constant, not a perf-equation constant, but it bounds the attainable compute side of the roofline.

### 1.6 One-line constant block (copy into scripts)

```python
GB10 = dict(
    sm_count=48, cuda_cores=6144, tensor_cores=192, cc="sm_121",
    l2_bytes=25_165_824,                       # 24 MiB
    boost_clock_hz=2.54e9,                      # DERIVED (§1.2); verify via FLOPs microbench
    mem_bytes=128 * 2**30, mem_bus_bits=256, mem_mtps=8.533e9,
    bw_peak_GBs=273.0,                          # theoretical [S1][S5]
    bw_measured_GBs=231.0,                      # measured GPU read [S6]; re-measure §1.7
    # DENSE tensor peaks, TFLOP/s:
    tflops_fp4=500.0, tflops_fp8=250.0,
    tflops_bf16_fp16acc=125.0,
    tflops_bf16_fp32acc=62.5,                   # <-- WaaV bit-faithful denominator
    tflops_tf32=31.25, tflops_fp32_cuda=31.0,
)
# Default roofline peaks for WaaV no-quant voice:
PEAK_FLOPS = GB10["tflops_bf16_fp32acc"] * 1e12   # 62.5 TFLOP/s
PEAK_BW    = GB10["bw_peak_GBs"]          * 1e9    # 273 GB/s (use bw_measured for achievable)
```

### 1.7 Bandwidth microbench (measure, don't trust) — the most important measurement

A read-dominated copy/triad saturates LPDDR5X. Report achieved GB/s and `% of 273`.

```python
import torch, time
def measure_bw_GBs(nbytes=4<<30, iters=200, dtype=torch.float16):
    n = nbytes // dtype.itemsize
    a = torch.empty(n, dtype=dtype, device="cuda")
    b = torch.empty(n, dtype=dtype, device="cuda")
    for _ in range(10): b.copy_(a)                 # warm up
    torch.cuda.synchronize(); t0 = time.perf_counter()
    for _ in range(iters): b.copy_(a)              # 1 read + 1 write
    torch.cuda.synchronize(); dt = time.perf_counter() - t0
    moved = 2 * nbytes * iters                      # read a + write b
    return moved / dt / 1e9                          # GB/s
# Expect ~210-240 GB/s (≈77-88% of 273). Cross-check with CUDA-samples bandwidthTest
# (device-to-device) and `nsys` dram__bytes if available.
```
Cross-check: run under CPU memory pressure to observe contention (the §1.3 / 400 ns effect) — the *achievable* GPU bandwidth on a busy box is the honest roofline denominator.

### 1.8 FLOPs microbench (measure the achievable compute peak)

Large square GEMM saturates Tensor Cores; FLOPs = `2·M·N·K`. Sweep dtype + accumulate mode.

```python
import torch, time
def measure_tflops(M=8192, dtype=torch.bfloat16, iters=100, tf32=False):
    torch.backends.cuda.matmul.allow_tf32 = tf32
    a = torch.randn(M, M, dtype=dtype, device="cuda")
    b = torch.randn(M, M, dtype=dtype, device="cuda")
    for _ in range(10): c = a @ b                   # warm up (cuBLAS picks kernel)
    torch.cuda.synchronize(); t0 = time.perf_counter()
    for _ in range(iters): c = a @ b
    torch.cuda.synchronize(); dt = time.perf_counter() - t0
    return (2 * M**3 * iters) / dt / 1e12            # TFLOP/s
# bf16 (cuBLAS uses FP32 accumulate) -> expect ~ up to 62.5 TFLOP/s achievable ceiling.
# Misaligned dims (not multiple of 8/16) collapse this; pad to x8/x16 (INFER_PERF.md lever 5).
```

---

## 2. The core equations

Conventions: `B`=batch (concurrent streams), `N`=parameter count, `L`=#decoder layers, `d`=`d_model`, `f`=FFN hidden, `h`=#query heads, `g`=#KV heads (GQA; MHA ⇒ g=h), `d_h`=d/h head dim, `s`=KV/context length (tokens), `V`=vocab, `dtype_bytes`=bytes/element (bf16→2). FLOP counts use **1 multiply-add = 2 FLOPs**.

### 2.1 Roofline [S10]

```
arithmetic_intensity   AI = FLOPs_performed / bytes_moved_from_DRAM      [FLOP/byte]
attainable_performance P  = min(PEAK_FLOPS, AI × PEAK_BW)                [FLOP/s]
ridge_point            I* = PEAK_FLOPS / PEAK_BW                          [FLOP/byte]
```
- `AI < I*` ⇒ **memory-bound** (left of ridge; attainable = AI × PEAK_BW). `AI > I*` ⇒ **compute-bound** (right; attainable = PEAK_FLOPS).
- **GB10 ridge points** (PEAK_BW = 273 GB/s):
  - BF16 FP32-acc (62.5 TFLOPs): `I* = 62.5e12/273e9 =` **229 FLOP/byte**
  - FP8 (250 TFLOPs): **916 FLOP/byte** · FP4 (500): **1832 FLOP/byte** · FP32 CUDA (31): **114 FLOP/byte**
- **Worked example (decode step, B=1, bf16):** AI ≈ `2/dtype_bytes = 2/2 =` **1 FLOP/byte** (§2.8). `1 ≪ 229` ⇒ deeply memory-bound; attainable ≈ `1 × 273e9 = 0.27 TFLOP/s` (0.4 % of peak). **The GPU is ~idle at B=1; only batching or smaller weights move you right toward the ridge.**

### 2.2 MFU — Model FLOPs Utilization

```
MFU = achieved_model_FLOPs_per_sec / PEAK_FLOPS                          [dimensionless 0..1]
achieved_model_FLOPs_per_sec = FLOPs_per_token × tokens_per_sec
```
**Counting a decoder's FLOPs (the exact formula).** Forward pass over weights ≈ **2N per token** (the "2N rule": every parameter does 1 multiply + 1 add per token) [S11][S12]. Training adds the backward pass ⇒ **6N per token** (2N fwd + 4N bwd) [S11][S12] — inference uses **2N**, not 6N.

Per-layer, per-token decode breakdown at KV length `s` (gated FFN, GQA):

| Term | FLOPs / layer / token | In 2N? |
|---|---|---|
| QKV projection | `2·d·(d + 2·g·d_h)` (MHA: `6d²`) | yes (weights) |
| Output projection | `2·d²` | yes |
| FFN (gated, 3 matrices d×f) | `6·d·f` (non-gated: `4·d·f`) | yes |
| **Attention scores `Q·Kᵀ`** | `2·h·s·d_h = 2·s·d` | **no** |
| **Attention `scores·V`** | `2·h·s·d_h = 2·s·d` | **no** |

```
C_step(s) ≈ 2·N  +  4·L·s·d            [FLOPs per decoded token, all layers + lm_head]
            └6N/3┘   └ attention extra (scales with context s) ┘
```
- The `4·L·s·d` attention term is the part **not** captured by `2N`. For voice (bounded ctx `s ≲ 3000`, modest `d`) `2N` dominates; for long context it can rival `2N`.
- `lm_head`/embedding contribute `2·d·V` per token (already inside `N` if tied/counted).
- **Worked example:** 1.6 B-param codec-LM, bf16-fp32acc, decoding 50 tok/s/stream × B=16 = 800 tok/s. `C_token ≈ 2·1.6e9 = 3.2 GFLOP` (attention term small). achieved = `3.2e9 × 800 = 2.56 TFLOP/s`. `MFU = 2.56e12 / 62.5e12 =` **4.1 %**. Low MFU is *expected and correct* for memory-bound AR decode — it is the symptom, not a bug. (Cross-check with MBU §2.3, which should be high.)

### 2.3 MBU — Memory Bandwidth Utilization

```
MBU = bytes_moved / (PEAK_BW × wall_time)                               [dimensionless 0..1]
```
For a **memory-bound decode step** the bytes moved are dominated by streaming the weights once + reading the KV cache:
```
bytes_step ≈ N · dtype_bytes                 (weights, read once per step, FLAT in B)
           + B · 2 · L · g · d_h · s · kv_dtype_bytes   (KV read: K and V, all layers)
           + activations (small)
```
- **Worked example:** same 1.6 B model, bf16 (2 B), B=16, s=1500, L=24, g·d_h (KV dim)=512, kv bf16. Weights = `1.6e9 × 2 = 3.2 GB`. KV = `16 × 2 × 24 × 512 × 1500 × 2 = 1.18 GB`. `bytes_step ≈ 4.38 GB`. If `t_step = 20 ms`: `MBU = 4.38e9 / (273e9 × 0.020) =` **80 %**. **High MBU + low MFU = textbook memory-bound** — the correct realtime signature for AR voice decode. The lever is to *reduce bytes* (quant — forbidden) or *amortize weights over more tokens* (batch — allowed; raises MFU without raising weight bytes).

### 2.4 Occupancy, warp efficiency, SM utilization (what the profilers report)

| Metric | Definition | Meaning |
|---|---|---|
| **Achieved occupancy** | `avg active warps per SM / max warps per SM` (`sm__warps_active.avg.pct_of_peak_sustained_active`) | latency-hiding headroom. Low ⇒ not enough parallelism to hide memory latency (common at B=1). High occupancy ≠ high throughput. |
| **Warp execution efficiency** | `avg active threads per executed warp / 32` (`smsp__thread_inst_executed_per_inst_executed.ratio`) | branch/predication divergence loss. ~100 % for dense GEMM. |
| **SM (compute) utilization** | fraction of wall time SMs issue work (nsys timeline "GPU active"; ncu `sm__throughput…`) | how busy the SMs are. |
| **Compute (SM) Throughput %** | `sm__throughput.avg.pct_of_peak_sustained_elapsed` | % of peak math/issue achieved (ncu SpeedOfLight). |
| **Memory Throughput %** | `gpu__compute_memory_throughput.avg.pct_of_peak_sustained_elapsed` | % of peak memory pipeline achieved (ncu SpeedOfLight). |
| **DRAM Throughput %** | `dram__throughput.avg.pct_of_peak_sustained_elapsed` | % of LPDDR5X bandwidth achieved (≈ MBU at the kernel level). |

### 2.5 Bottleneck classification: launch- vs compute- vs memory-bound

**Decision rule (per kernel, from ncu SpeedOfLight) [S13]:**
```
Compute% ≡ sm__throughput.avg.pct_of_peak_sustained_elapsed
Memory%  ≡ gpu__compute_memory_throughput.avg.pct_of_peak_sustained_elapsed
if Memory%  ≥ ~60 and Memory% > Compute% :  MEMORY-bound   (lever: fewer bytes / batch / fuse / KV-resident)
if Compute% ≥ ~60 and Compute% > Memory% :  COMPUTE-bound  (lever: better GEMM, pad dims, right SDPA backend)
if both < ~60                            :  LATENCY/LAUNCH-bound (lever: CUDA-graph, fuse, raise occupancy)
```
**Launch-bound signal (from nsys timeline, the B=1 AR-decode failure mode):**
```
GPU_busy_fraction = Σ kernel_durations / wall_time_on_stream
launch_bound  ⇔  GPU_busy_fraction is LOW (e.g. <0.5)  AND  many tiny kernels
                  with inter-kernel GAPS ≈ host launch latency (~5–10 µs each)
```
A step that issues thousands of sub-microsecond kernels back-to-back is **dominated by per-launch CPU→GPU overhead, not by compute or memory** — the fix is a **CUDA graph** (replays the whole step as one launch) or kernel fusion. This is the `dia2 RTF-3.4 = 60.8 % GPU-idle, 4.13 M tiny launches` regime in this repo: classify it from the nsys gap fraction, not from ncu (which only sees inside a kernel).

### 2.6 RTF (real-time factor) and the streaming budget

```
RTF = processing_wall_time / audio_duration            [dimensionless;  < 1 ⇒ faster than realtime]
```
For **streaming** synthesis/recognition, RTF alone is insufficient — two latency constraints must both hold:
```
frame_period   = 1 / codec_frame_rate_Hz               (e.g. 12.5 Hz → 80 ms; 80 Hz → 12.5 ms)
steady_state:  t_per_frame_step  <  frame_period        (must keep up, per frame, not just on average)
first audio:   TTFA = t_prefill + t_first_decode_step + t_first_vocoder_chunk  <  target (e.g. 200 ms)
```
- `RTF < 1` is necessary but **not** sufficient: a system can average RTF 0.5 yet stall on a slow frame and glitch. Track **p50 and p99 `t_per_frame_step`** against `frame_period`.
- **Worked example:** 12.5 Hz codec ⇒ `frame_period = 80 ms`. If `t_per_frame_step = 18 ms` ⇒ `RTF_decode ≈ 18/80 = 0.225` (4.4× realtime headroom) and TTFA budget is whatever the product sets (e.g. 200 ms).

### 2.7 Amdahl's law — component speedup → end-to-end (the "1.3× → 1.1×" dilution) [S14]

```
overall_speedup = 1 / ( (1 − p) + p / s )
```
where `p` = fraction of end-to-end time spent in the component, `s` = that component's local speedup.
- **Worked example (the exact dilution):** a kernel is **p = 40 %** of runtime, sped up **s = 1.3×**: `overall = 1 / (0.60 + 0.40/1.3) = 1 / (0.60 + 0.3077) = 1/0.9077 =` **1.10×**. A 1.3× local win dilutes to 1.1× end-to-end because 60 % of the time was untouched. **Corollary:** always weight a proposed kernel win by `p` (its measured time share, §3.3) before believing it — and chase the *largest* `p` first.

### 2.8 AR codec-TTS decode arithmetic (B=1 weight-bandwidth-bound) — why batching is the only allowed lever

At **B = 1** an autoregressive decode **step** reads every weight once to emit one token:
```
bytes_step ≈ N · dtype_bytes + KV_bytes          (KV small at modest s)
FLOPs_step ≈ 2 · N
AI_step    = FLOPs/bytes ≈ 2N / (N · dtype_bytes) = 2 / dtype_bytes      [FLOP/byte]
           = 1.0 (bf16),  0.5 (fp32),  2.0 (fp8),  4.0 (fp4)   [AI=2/dtype_bytes: fp8=2/1, fp4=2/0.5; corrected]
```
Since `AI_step (=1 for bf16) ≪ ridge (=229)`, the step time is set by **bandwidth, not math**:
```
t_step ≈ (N · dtype_bytes) / PEAK_BW            (FLAT in B until the compute crossover)
```
- **GB10 example:** N=1.6 B, bf16 ⇒ `t_step ≈ 3.2e9 / 273e9 =` **11.7 ms** floor (≈ 85 tok/s) — independent of how few FLOPs you do.
- **Bytes/FLOP crossover (the batch knee).** Weight bytes are flat in B; compute grows with B. Memory-bound while `t_mem > t_compute`:
```
N·dtype_bytes / PEAK_BW  >  2·N·B / PEAK_FLOPS
⇒  B_crit = PEAK_FLOPS · dtype_bytes / (2 · PEAK_BW)
   GB10 bf16-fp32acc:  B_crit = 62.5e12 · 2 / (2 · 273e9) ≈ 229   (theoretical upper bound)
```
The **theoretical** crossover is high (~229), but the **empirical** efficiency knee is far lower (~B≈16–64 on real exported graphs) because (a) decode GEMMs don't hit peak TFLOPs, (b) GQA cuts KV bandwidth, and (c) real graphs re-stream host↔device KV every stride (`O(B·s·L)`, grows with B), capping the realized speedup at **~1.8× @ B≈16** before it regresses (see `INFER_PERF.md` §2 / `INFER_PERF_VALIDATION.md`). **Reconcile theory vs. reality:** quote `B_crit≈229` as the bandwidth-physics ceiling, but **size slots at the measured per-graph knee**, not at `B_crit`. The two levers that lower `t_step` — quantization (shrinks `dtype_bytes`) and speculation (skips steps) — are exactly the ones WaaV forbids, leaving **batching** as the sanctioned lever (and it's *available* precisely because the GPU is idle at B=1).

---

## 3. How the tools expose the raw counters (so scripts read real numbers, not guesses)

### 3.1 `nsys` (Nsight Systems) — timeline, kernel durations, launch-bound gaps [S15][S16]

```bash
# 1. Collect a timeline (CUDA + OS runtime + NVTX):
nsys profile -t cuda,nvtx,osrt --cuda-memory-usage=true \
     -o waav_prof  python serve_one_utterance.py

# 2. Export to SQLite (scripts parse this) and/or emit CSV reports:
nsys export --type sqlite waav_prof.nsys-rep            # → waav_prof.sqlite
nsys stats --report cuda_gpu_kern_sum  --format csv --output . waav_prof.nsys-rep  # per-kernel time%, total, instances, avg
nsys stats --report cuda_gpu_trace     --format csv --output . waav_prof.nsys-rep  # per-launch start, duration, stream
nsys stats --report cuda_api_sum       --format csv --output . waav_prof.nsys-rep  # launch API (cudaLaunchKernel) time
```
What the scripts compute from it:
- **Per-kernel ms + %** from `cuda_gpu_kern_sum` (Time(%), Total Time, Instances, Avg).
- **Launch-bound classification** from `cuda_gpu_trace`: per-stream `gap = next.start − (this.start + this.dur)`; `GPU_busy_fraction = Σdur / (end−start)`. Low busy fraction + many sub-µs kernels + gaps ≈ launch latency ⇒ launch-bound (§2.5).
- SQLite tables of interest: `CUPTI_ACTIVITY_KIND_KERNEL` (start, end, name), `CUPTI_ACTIVITY_KIND_RUNTIME` (cudaLaunchKernel duration), `StringIds`.

### 3.2 `ncu` (Nsight Compute) — per-kernel SOL %, occupancy, DRAM bandwidth, roofline [S13][S17]

```bash
# Targeted sections + CSV the scripts parse (profile a few representative kernels, not all — ncu is slow):
ncu --set roofline \
    --section SpeedOfLight --section Occupancy \
    --section MemoryWorkloadAnalysis --section ComputeWorkloadAnalysis \
    --kernel-name regex:"decode|attn|matmul|mlp" --launch-count 20 \
    --csv --log-file waav_ncu.csv  python serve_one_utterance.py

# Pull exact metrics directly (lighter weight):
ncu --metrics \
 sm__throughput.avg.pct_of_peak_sustained_elapsed,\
 gpu__compute_memory_throughput.avg.pct_of_peak_sustained_elapsed,\
 dram__throughput.avg.pct_of_peak_sustained_elapsed,\
 dram__bytes.sum,dram__bytes.sum.per_second,\
 sm__warps_active.avg.pct_of_peak_sustained_active,\
 smsp__thread_inst_executed_per_inst_executed.ratio,\
 gpu__time_duration.sum \
 --csv --log-file waav_metrics.csv  python serve_one_utterance.py
```
Metric → meaning (verify exact strings on the box with `ncu --query-metrics` / `ncu --list-sections`; names are stable across recent versions but version-check before trusting):

| Metric string | Reports |
|---|---|
| `sm__throughput.avg.pct_of_peak_sustained_elapsed` | **Compute (SM) Throughput %** |
| `gpu__compute_memory_throughput.avg.pct_of_peak_sustained_elapsed` | **Memory Throughput %** (SpeedOfLight headline) |
| `dram__throughput.avg.pct_of_peak_sustained_elapsed` | **DRAM (LPDDR5X) throughput %** ≈ kernel-level MBU |
| `dram__bytes.sum` / `dram__bytes.sum.per_second` | bytes moved / achieved **GB/s** (compare to 273) |
| `sm__warps_active.avg.pct_of_peak_sustained_active` | **Achieved Occupancy %** |
| `smsp__thread_inst_executed_per_inst_executed.ratio` | **Warp Execution Efficiency** (÷32) |
| `gpu__time_duration.sum` | kernel duration |
| Section `SpeedOfLight` | the Compute% vs Memory% summary → bottleneck class (§2.5) |
| Section `SpeedOfLight_RooflineChart` / `--set roofline` | the roofline (achieved AI + attainable point) |

CLI flags: `--csv` (comma-separated output), `--page raw` (all metrics/kernel) or `--page details` (sections), `--log-file <path>` (capture), `--set roofline` (collect roofline data), `--launch-count N` / `--kernel-name regex:…` (limit cost).

### 3.3 `torch.profiler` — per-op / per-layer CUDA time, the easy default [S18]

```python
from torch.profiler import profile, ProfilerActivity, schedule
with profile(activities=[ProfilerActivity.CPU, ProfilerActivity.CUDA],
             record_shapes=True, with_stack=True,
             schedule=schedule(wait=1, warmup=2, active=5)) as prof:
    for _ in range(8):
        model.decode_step(...); prof.step()

# Per-op table sorted by GPU time (the per-layer attribution scripts read):
print(prof.key_averages(group_by_input_shape=True)
          .table(sort_by="self_cuda_time_total", row_limit=25))
# Machine-readable:
for e in prof.key_averages():
    rec(e.key, e.self_cuda_time_total, e.cuda_time_total, e.count, e.input_shapes)
prof.export_chrome_trace("waav_trace.json")     # timeline (chrome://tracing / perfetto)
```
- `self_cuda_time_total` = time **in this op's own kernels** (excludes children) → use for the top-N op ranking. `cuda_time_total` = inclusive.
- Attribute to **layers** by wrapping modules in `torch.profiler.record_function("layer.{i}.attn")` (or use `with_stack=True` + the module stack) so the table groups by your named regions.
- Cheapest path for the per-stage/per-layer ms+% table in §4; escalate to `ncu` only when a stage is the bottleneck and you need the SOL/roofline class.

---

## 4. The per-profile report schema (what an AI agent + a human should see)

Every profile run emits **one record** with these fields, so the bottleneck and the lever are obvious without re-reading raw traces:

```jsonc
{
  "model": "chatterbox-codec-lm", "precision": "bf16/fp32acc", "batch": 16,
  "device": "GB10 sm_121", "peaks": {"bw_GBs": 273, "tflops": 62.5},

  "rtf": 0.22,                          // §2.6  wall/audio (<1 = realtime)
  "ttfa_ms": 180,                       // §2.6  time-to-first-audio
  "frame_period_ms": 80, "t_frame_p50_ms": 18, "t_frame_p99_ms": 31,   // keep-up check

  "stages": [                           // §3.3  per-stage/per-layer ms + %
    {"name":"prefill",   "ms": 40, "pct": 9},
    {"name":"decode_step_x N","ms": 360,"pct": 80},
    {"name":"vocoder",   "ms": 48, "pct": 11}
  ],

  "mfu": 0.041,                         // §2.2  achieved/peak FLOPs (LOW expected for AR decode)
  "mbu": 0.80,                          // §2.3  bytes/(bw·t)  (HIGH = memory-bound, the realtime signature)
  "arithmetic_intensity": 1.0,          // §2.1  FLOP/byte (vs ridge 229)
  "ridge_point": 229,
  "achieved_occupancy": 0.34,           // §2.4
  "warp_exec_efficiency": 0.98,
  "compute_throughput_pct": 22,         // ncu SpeedOfLight
  "memory_throughput_pct": 78,          // ncu SpeedOfLight
  "dram_bw_achieved_GBs": 213,          // ncu dram__bytes.sum.per_second  (vs 273)
  "gpu_busy_fraction": 0.62,            // nsys Σdur/wall  (LOW + tiny kernels = launch-bound)

  "bottleneck": "memory-bound",         // §2.5  {memory|compute|launch|latency}-bound
  "top_kernels": [                      // §3.1  nsys cuda_gpu_kern_sum top-N
    {"name":"sdpa_cudnn","pct":31,"ms":112},
    {"name":"gemm_mlp","pct":24,"ms":86}
  ],
  "recommended_lever": "batch to the per-graph knee (~B16) + keep KV device-resident; weights are flat in B (§2.8). NOT a custom kernel (already on the memory roofline)."
}
```
**Rules the analyzer applies** (so the recommendation is mechanical, not vibes):
- `mbu` high + `mfu` low + `AI < ridge` ⇒ **memory-bound** ⇒ lever = batch / KV-resident / GQA (cut bytes), never a hand-written attention kernel.
- `gpu_busy_fraction` low + many sub-µs kernels ⇒ **launch-bound** ⇒ lever = CUDA-graph / fusion.
- `compute_throughput_pct` high + `mfu` moderate ⇒ **compute-bound** ⇒ lever = SDPA-backend pin / GEMM dim-pad ×8/×16 / right dtype.
- Always discount a proposed kernel speedup by its stage `pct` via **Amdahl (§2.7)** before ranking it.

---

## Sources

- **[S1]** NVIDIA, *DGX Spark — Personal AI Supercomputer Powered by Blackwell* (product page): 1000 TOPS / 1 PFLOP FP4 sparse, 128 GB LPDDR5X, 273 GB/s, Blackwell + 5th-gen Tensor Cores. https://www.nvidia.com/en-us/products/workstations/dgx-spark/
- **[S2]** NVIDIA, *DGX Spark User Guide — Hardware Overview*: 6,144 CUDA cores, 20-core Arm, 256-bit @ 4266 MHz → 273 GB/s, 140 W TDP, 1 PFLOP FP4 sparse. https://docs.nvidia.com/dgx/dgx-spark/hardware.html
- **[S3]** Kubesimplify, *DGX Spark Unpacked: GB10, Unified Memory, sm_121, NVFP4* — CUDA CC 12.1 (sm_121), 48 SMs, 24 MiB (25,165,824 B) L2. https://blog.kubesimplify.com/day-3-the-dgx-spark-unpacked-gb10-unified-memory-sm-121-and-the-one-reason-this-hardware-exists
- **[S4]** TechPowerUp, *NVIDIA Dissects GB10 SoC* — 6,144 CUDA cores, 192 5th-gen Tensor Cores, 48 SMs. https://www.techpowerup.com/340385/nvidia-dissects-gb10-superchip-soc-with-20-cpu-cores-and-6-144-cuda-gpu-cores
- **[S5]** WccfTech, *NVIDIA Dissects GB10 Superchip* — LPDDR5X-9400 support, 256-bit, 140 W; DGX Spark runs 8533 MT/s → 273 GB/s. https://wccftech.com/nvidia-gb10-superchip-soc-3nm-20-arm-v9-2-cpu-cores-nvfp4-blackwell-gpu-lpddr5x-9400-memory-140w-tdp/
- **[S6]** Chips and Cheese, *Inside Nvidia GB10's Memory Subsystem* — measured ≈231 GB/s GPU bandwidth, 113 ns idle / ~400 ns contended DRAM latency, shared unified pool. https://chipsandcheese.com/p/inside-nvidia-gb10s-memory-subsystem
- **[S7]** Flopper.io / Tom's Hardware GB10 — FP32 ≈ 31 TFLOPs (CUDA cores). https://flopper.io/gpu/nvidia-gb10-grace-blackwell · https://www.tomshardware.com/pc-components/gpus/nvidia-dgx-spark-review/2
- **[S8]** NVIDIA, *RTX Blackwell GPU Architecture Whitepaper v1.1* — 5th-gen Tensor Cores, NVFP4; consumer Blackwell FP32-accumulate runs at half the FP16-accumulate rate. https://images.nvidia.com/aem-dam/Solutions/geforce/blackwell/nvidia-rtx-blackwell-gpu-architecture.pdf
- **[S9]** RTX 5090 reference figures (680 TC @ 2.41 GHz; FP4 sparse 3352 TOPS = 1676 dense; FP16/FP32-acc dense 209.5 TFLOPs; FP32 CUDA 104.8 TFLOPs) — used to validate the per-Tensor-Core ladder. Wikipedia *GeForce RTX 50 series*; Runpod / Spheron RTX 5090 spec pages; NVIDIA Developer Forums "RTX 5090 FP16 Tensor TFLOPS is ambiguous". https://en.wikipedia.org/wiki/GeForce_RTX_50_series · https://forums.developer.nvidia.com/t/rtx-5090-specs-fp16-tensor-tflops-is-ambiguous/351063
- **[S10]** Williams, Waterman, Patterson, *Roofline: An Insightful Visual Performance Model for Multicore Architectures*, Communications of the ACM 52(4):65–76, 2009 (operational/arithmetic intensity, attainable = min(peak, AI×BW), ridge point). https://people.eecs.berkeley.edu/~kubitron/cs252/handouts/papers/RooflineVyNoYellow.pdf
- **[S11]** EleutherAI, *Transformer Math 101* — C ≈ 6PD training (2PD fwd + 4PD bwd); inference forward ≈ 2N/token. https://blog.eleuther.ai/transformer-math/
- **[S12]** Kaplan et al., *Scaling Laws for Neural Language Models*, 2020 (Appendix: C_forward ≈ 2N/token; attention adds ~`4·L·s·d`). arXiv:2001.08361. https://arxiv.org/abs/2001.08361
- **[S13]** NVIDIA, *Nsight Compute — Kernel Profiling Guide* (SpeedOfLight: Compute Throughput % vs Memory Throughput %, Roofline section, the bottleneck heuristic). https://docs.nvidia.com/nsight-compute/ProfilingGuide/index.html · and *The Peak-Performance-Percentage Analysis Method*, NVIDIA Technical Blog. https://developer.nvidia.com/blog/the-peak-performance-analysis-method-for-optimizing-any-gpu-workload/
- **[S14]** Amdahl, *Validity of the single processor approach…*, AFIPS 1967 (overall_speedup = 1/((1−p)+p/s)).
- **[S15]** NVIDIA, *Nsight Systems — User Guide* (`nsys profile`, timeline, CUDA kernel trace). https://docs.nvidia.com/nsight-systems/UserGuide/index.html
- **[S16]** NVIDIA, *Nsight Systems — export & stats* (`nsys export --type sqlite`; `nsys stats --report cuda_gpu_kern_sum/cuda_gpu_trace/cuda_api_sum --format csv`). https://docs.nvidia.com/nsight-systems/UserGuide/index.html
- **[S17]** NVIDIA, *Nsight Compute CLI* (`--section`, `--set roofline`, `--metrics`, `--csv`, `--page raw|details`, `--log-file`, `--query-metrics`, `--list-sections`). https://docs.nvidia.com/nsight-compute/NsightComputeCli/index.html
- **[S18]** PyTorch, *torch.profiler* (`key_averages`, `self_cuda_time_total`, `export_chrome_trace`, `record_function`, `ProfilerActivity.CUDA`). https://pytorch.org/docs/stable/profiler.html

> **Verification note for implementers.** Re-run §1.7 (bandwidth) and §1.8 (FLOPs) on the actual box before trusting any compute peak in a conclusion — the 140 W sustained-clock throttle and unified-memory CPU/GPU contention mean the *achievable* peaks (use ~231 GB/s, and the §1.8-measured TFLOPs) are the honest roofline endpoints; the §1.1 table values are theoretical upper bounds. Confirm every `ncu` metric string with `ncu --query-metrics` for your installed Nsight version before parsing.
