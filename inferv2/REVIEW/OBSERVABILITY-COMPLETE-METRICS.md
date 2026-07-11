# WaaV Observability — Complete Bottleneck Metrics (per-kernel efficiency + CPU-vs-GPU + memory)

**Status:** SHIPPED + live-validated on GB10 (dia2, voxtral) · **Date:** 2026-07-01 ·
Extends `OBSERVABILITY-DESIGN.md` / `PERF-EQUATIONS-REFERENCE.md`. The prior system had the equations,
the read_ncu/read_nsys parsers, the top-kernel table and the §4 record, but three CAPTURE gaps:
(1) ncu was perm-blocked so per-kernel efficiency was never populated; (2) the CPU was never traced, so
"is the CPU the bottleneck or the GPU" was unanswerable; (3) the profiler was time-only (no memory).
All three are now closed with real, reconciled numbers.

## Headline

**Yes — the system now captures, per run, every bottleneck signal, all cross-tool reconciled:**

- **Per-kernel** `{launch, duration, compute%, mem-BW%, occupancy, warp-eff, bytes-moved, GB/s, roofline
  class, lever}` — from ncu (unblocked via `sudo`), one row per profiled kernel.
- **CPU-vs-GPU verdict** — `{gpu-bound | cpu-bound | balanced | under-utilized}` from nsys CPU sampling +
  CUDA-API dispatch time + GPU-busy fraction + `/proc/stat` ground truth. The launch-bound case is named
  explicitly as a **CPU-side dispatch bottleneck**.
- **Memory consumed** — peak (this-run + absolute device-used/total), per-component reserved-growth, and
  per-kernel bytes-moved, from a cudart `cudaMemGetInfo` capture in the Rust profiler + ncu's L2 counter.
- **Host vs device split** — per-component CPU-dispatch wall vs GPU device time, in the §4 report.

**Honest limits (fully disclosed below):** GB10 exposes **no `dram__*` counters** (unified LPDDR5X) → the
per-kernel "bytes moved" is the **L2-traffic proxy `lts__t_bytes.sum`** (≈DRAM for >L2 streaming buffers,
an upper bound otherwise), labeled as such. The Rust memory number is the **reserved free-pool high-water**
(`cudaMemGetInfo` delta), not exact per-tensor `allocated` (tch 0.20 exposes no caching-allocator stats).

## What each new metric tells you + the lever it points to

| Signal (where) | Reads | Lever it selects |
|---|---|---|
| per-kernel compute% vs mem% (ncu SOL) | which pipe a kernel saturates | ≥60% compute → better GEMM / SDPA pin; ≥60% mem → cut bytes / batch; both <60 → CUDA-graph / fuse |
| per-kernel occupancy (ncu) | latency-hiding headroom | low + B=1 → batch; **high but low SOL → still latency-bound** (voxtral: occ 0.96, SOL 12/16% → launch-bound) |
| per-kernel bytes-moved + GB/s (ncu L2) | bandwidth consumed / kernel | GB/s near peak → memory-bound (already on roofline); far below → latency/launch-bound |
| CPU-vs-GPU verdict (nsys) | is the host or the GPU the limiter | cpu-bound/launch → **CUDA-graph the step**; gpu-bound → faster kernels/batch; balanced → both |
| CUDA-API dispatch fraction (nsys) | host time submitting to the GPU | high + GPU idle = the launch-bound host cost → fewer/bigger launches |
| peak memory + per-component Δreserved (Rust) | OOM headroom + which stage allocates | a stage growing the pool (dia2 depformer +1.1 GB KV) → the KV-residency / arena-cap target |
| host vs device per-component (Rust) | CPU-dispatch vs GPU-execute per stage | Σhost ≈ wall ⇒ dispatch-bound stage → graph/fuse it |

---

## TIER 1 — Per-kernel efficiency (live, dia2)

`ncu --target-processes all --set roofline --metrics <occ,warp,lts_bytes,SOL,dur>` via `sudo` (root
bypasses `ERR_NVGPUCTRPERM`). The wide `--page raw` CSV (metric strings as columns + a units row) is
parsed by `read_ncu`, which converts the displayed units (`Mbyte`/`us`/`Tbyte/s`) to SI.

```
PER-KERNEL EFFICIENCY (ncu SpeedOfLight/roofline)                        [dia2, GB10]
  kernel                                   dur_ms comp% mem% occ% warp%   bytes    GB/s  class
  void elementwise_kernel<128,2,DivFunctor  0.018   12   13  108   100    4.4MB    238  latency-bound
  void elementwise_kernel<128,2,DivFunctor  0.017   12   14  142   100    4.4MB    258  latency-bound
  void vectorized_elementwise_kernel<4,...  0.009    0    0   37   100    0.1MB      6  latency-bound
  bytes source: lts__t_bytes.sum (L2 proxy; GB10 exposes no DRAM counters)
```

Reading: sub-20µs kernels, compute% AND mem% both ≪60, GB/s far below the 198.5 peak → **latency/launch
bound** (not compute, not memory). Lever = CUDA-graph the step. (The dominant `Kernel2` cutlass GEMM, 48.8%
of GPU time in the nsys table, was outside this ncu launch window — steer `NCU_LAUNCH_SKIP`/`--kernel-name`
to profile it; see caveats.)

---

## TIER 2 — CPU-vs-GPU attribution (live)

nsys now captures the CPU: `--sample=cpu --trace=cuda,nvtx,osrt --cpuctxsw=process-tree` (needs
`kernel.perf_event_paranoid<=1`, set by the script via `sudo sysctl`). `read_nsys` extracts CPU-busy
(per-thread Running-sample count × sampling period / wall), the hot host functions (leaf-of-stack
symbols), and CUDA-API dispatch time; `cpu_gpu_attribution()` renders the verdict; `/proc/stat` snapshots
around the run are the ground-truth CPU %.

```
CPU-vs-GPU VERDICT: CPU-BOUND                                            [dia2]
  CPU-side dispatch bottleneck: GPU idle while the host is busy
  gpu_busy(wall)=0.06  gpu_busy(kern-span)=0.08  cpu_busy(hottest thread)=0.89
  cuda_api=0.59 (11297.9 ms)   /proc/stat: overall 9%, busiest core 19 at 62% of 20 cores

CPU-vs-GPU VERDICT: CPU-BOUND                                            [voxtral, RTF 0.61]
  gpu_busy(wall)=0.12  cpu_busy=0.72  cuda_api=0.52 (16150 ms)  occupancy=0.96
```

Both B=1 voice-decode models are diagnosed **launch/CPU-dispatch-bound** — independently rediscovering the
known dia2/voxtral regime (`INFER_PERF.md`: "many tiny kernels, GPU-idle") from live counters. The verdict
discriminates: it is severity-graded (dia2 GPU 94% idle vs voxtral 88% idle) and at the kernel level a
compute-saturating GEMM classifies **compute-bound** (probe: sm__throughput 71%), so the verdict is not
hard-wired to one answer.

---

## TIER 3 — Memory consumed (live, dia2)

Rust `nn::Profiler` now samples `cudaMemGetInfo` (cudart FFI) at every component boundary — peak used,
per-component reserved-growth, run-start baseline — plus per-component host wall vs device time.

```
MEMORY CONSUMED                                                          [dia2]
  peak this-run: 1.51 GB   peak device-used: 47.46 GB / 131 GB   [cudaMemGetInfo free-pool delta (reserved)]
    backbone     Δreserved   +207.2 MB   host  222.6 ms   dev 6774.0 ms
    depformer    Δreserved  +1183.9 MB   host  401.1 ms   dev 6049.2 ms   <- grows the pool (KV cache)
    codec        Δreserved   +111.7 MB   host   12.8 ms   dev   13.7 ms
  per-kernel bytes moved:  elementwise 4.4 MB @ 238 GB/s ...

HOST vs DEVICE (per-component): host=1161 ms  device=13372 ms  host_fraction=0.08

[voxtral] peak this-run 0.04 GB (steady-state — no pool growth during decode); peak device-used 60.2 GB
```

Reading: dia2's **depformer grows the reserved pool by ~1.2 GB** (the per-slot KV cache — the residency /
arena-cap target from the KV-ACCEL work); voxtral is flat (no decode-time growth). Peak device-used
(47 GB dia2 / 60 GB voxtral of 131 GB) is the direct OOM-headroom number.

---

## Reconciliations (derived number ↔ raw counter — the accuracy gate)

1. **Per-kernel bytes (SI) ↔ ncu raw counter.** `read_ncu`'s SI `mem_bytes` == raw `lts__t_bytes.sum`
   (in `Mbyte`) × 1e6, **exactly, 6/6 dia2 kernels**. The unit conversion is bit-exact.
2. **CPU-busy: nsys estimate ↔ `/proc/stat` kernel counter.** dia2 nsys busiest-thread `0.89` vs
   `/proc/stat` busiest-core `0.62`; voxtral `0.72` vs `0.46`. Both corroborate a **pegged host thread**
   (same direction/order). They differ because nsys attributes to the hottest *thread* (which migrates
   across cores) while `/proc/stat` is per-physical-*core* — a migrating thread reads lower per-core.
3. **The two "device time" views reconcile the launch-bound cost.** dia2: Rust `device_ms`(stream-wall,
   incl. intra-region launch gaps) = **13372 ms**; nsys `gpu_active_ms`(union of real kernel durations) =
   **1072 ms**; wall = 19280 ms → `gpu_busy(wall)=0.056`. The **12300 ms difference == GPU-idle launch
   gaps == the CPU-dispatch bottleneck** the nsys verdict independently flags. Three tools agree.
4. **Memory identity.** `peak_alloc_bytes == peak_used_bytes − baseline_used_bytes` exactly (dia2
   47.461 − 45.954 = 1.507 GB).

---

## Files

| File | Change |
|---|---|
| `profiling/parse_tools.py` | `read_ncu`: WIDE `--page raw` (units-row→SI) + LONG details support; `lts__t_bytes` DRAM proxy + `mem_bytes/mem_source`. `read_nsys`: `_read_nsys_cpu` (CPU-busy, hot host fns, CUDA-API dispatch, OSRT) + shared wall + `gpu_busy_fraction_wall`/`gpu_active_ms`. New `read_proc_stat_delta`. |
| `profiling/perf_equations.py` | `cpu_gpu_attribution()` (the mechanical verdict) + named thresholds. |
| `profiling/perf_equations_test.py` | +6 tests (verdict cases) → **40/40** green. |
| `profiling/analyze.py` | `_ncu_kernel_table`, `_cpu_gpu_attribution`, `_memory_report`, `_host_device_split`; `--infer-report`/`--proc-stat-before/after`; renders all three tiers. |
| `profiling/profile_model_kernels.sh` | `WAAV_NCU_SUDO=1` (+`sudo -E env`), `--set roofline`+`--metrics`, nsys `--sample=cpu`, `/proc/stat` snapshots, `perf_event_paranoid` auto-lower, runs `<model>_profile_report` for the L4 memory/host-device JSON, feeds all to analyze.py. |
| `crates/waav-infer-backend-torch/src/nn/profiler.rs` | cudart `cudaMemGetInfo` FFI; per-component `host_ns` + `alloc_delta`; profiler peak/baseline/total; `peak_alloc_bytes`/`peak_used_bytes`/`baseline_used_bytes`/`mem_total_bytes`/`mem_source` on `ProfileReport`, `host_ms`/`alloc_delta_bytes` on `LayerEntry`; table render. **Zero-overhead-when-off preserved** (`timed(None)` untouched; `WAAV_PROFILE_NOMEM` disables mem capture); `cargo build`+`clippy -D warnings` green; profiler unit tests 3/3. |

## Honest caveats (nothing hidden)

- **No DRAM counters on GB10** → per-kernel bytes = `lts__t_bytes.sum` (L2). ≈DRAM for >24 MiB streaming
  buffers (the AR-decode case); an upper bound for L2-resident buffers. Labeled `mem_source` everywhere.
- **ncu launch window** profiles a bounded slice (`--launch-skip/--launch-count`); it may land on
  elementwise kernels and miss the dominant GEMM (`Kernel2`). Steer with `NCU_LAUNCH_SKIP` or add
  `--kernel-name regex:` to profile the costliest kernel. The nsys time-share table always shows the true
  top kernel by wall.
- **Rust `host_ms` in CudaEvents mode** absorbs upstream async-drain at whichever region first forces a
  host sync (dia2 `sample` showed 12.5 s of drain). The **SyncWall** run gives the clean per-region host
  cost (and is what the written JSON carries). The authoritative CPU-vs-GPU answer is the **nsys** verdict.
- **Memory = reserved free-pool high-water**, not exact `allocated` (tch 0.20 has no allocator-stats API).
  It is the OOM-risk number (what `cudaMalloc` reserved), honestly scoped.
- **Permissions:** ncu needs `sudo` now; `NVreg_RestrictProfilingToAdminUsers=0` (already in
  `/etc/modprobe.d`) grants sudo-less ncu **after a reboot**. CPU sampling needs
  `kernel.perf_event_paranoid<=1` (transient `sysctl`, re-applied by the script; make permanent via
  `/etc/sysctl.d`).
