# WaaV End-to-End Observability & Profiling — Design + Verified Equation Spec

Synthesis of the three recon reports (`OBSERVABILITY-GATEWAY-MAP.md`, `OBSERVABILITY-INFER-MAP.md`,
`PERF-EQUATIONS-REFERENCE.md`) + the on-box calibration (`profiling/gb10_calibration.json`) into one buildable
design. The goal: a full, accurate, human+AI-readable perf-observability system from Gateway → Infer
(batching → layer → kernel) we use to find bottlenecks, decide levers, and run experiments — with **accurate
equations**, **no stubs/hardcoded constants**, **integrated across the key paths**, **live-tested on real models**.

## 0. What already exists (reuse, don't rebuild)

- **Gateway:** a mature turn profiler (`core/observability/`) — 7-stage timeline (`stt → stt_to_llm → llm_ttft →
  llm_sentence → tts_queue → tts_ttfb → egress`), emitted 4 ways (structured `waav::turn` log, Prometheus, rolling
  p50/p90/p99, SSE), exposed at `/debug/profile` (JSON) + `/debug/profile/stream` (SSE) + `/metrics`. **Already
  human+machine readable.** Bottleneck tagging (`llm_ttft` first-class) + `realtime_blockers`.
- **Infer:** 24 Prometheus metrics; distributed-trace **receiving** half (`otel.rs::turn_span` parents stage spans
  under a gateway `trace_id`, `protocol/trace.rs` W3C traceparent, GW-17); a roofline/duty model in the scheduler
  (`cuda_roofline`, `SubstrateRoofline`, `DutyLedger`, `masked_bandwidth_duty`); two test-only component profile
  hooks (`TorchDia2::profile_generate`, `Neutts::profile_step_breakdown`, CUDA-synced + warmup).
- **Tools:** nsys 2025.3, ncu, torch.profiler (torch 2.12), perf, bpftrace, CUDA 13.0.

## 1. The five gaps to close (the build)

1. **Calibrated peaks (DONE, `10a1f7b`).** Replaced the FP4-marketing `1e15` peak-FLOPs (11.6× too high) with the
   on-box MEASURED peaks; single source of truth; `profiling/calibrate_gb10.py` is re-runnable per box.
2. **Per-layer + per-kernel profiling integrated into Infer** (not test-only). Generalize the dia2/neutts component
   hooks into a serve-callable, CUDA-event-based, **layer-resolved** profiler (`Profiler` trait + a torch.profiler
   pass); wire `TraceContext::child()` **stage spans** into the serve loop (today the engine emits ZERO). Kernel tier
   = an nsys/ncu capture wrapper invoked on a chosen run.
3. **The analysis toolkit** (Python, `profiling/`): implement the verified equations (§3) over the calibration +
   nsys/ncu/torch.profiler outputs; emit ONE JSON record per profile (machine/AI) + a rendered table (human); a
   mechanical signature→lever rule set. This is the "makes analysis easier" deliverable.
4. **The gateway→infer seam.** The gateway must **inject** the W3C `traceparent` at the Infer hop (today it injects
   nothing → a turn's trace never spans both halves). Then ONE trace covers handshake → STT → LLM → TTS →
   intra-Infer batch/layer/kernel.
5. **Magic-constant fixes:** `compute_secs 0.010/0.005`, `RATED_STREAM_SERVE_SECS=0.5`, `one_frame_ms()=100` vs rated
   `T_f`, the global `T_f=0.040` (wrong for 80ms-frame Mimi) — derive from the warmup/per-model, not hardcode.

## 2. The profiling hierarchy (the layered model)

```
L0  HARDWARE      calibrated peaks (86 TFLOP/s bf16-fp32acc, 198.5 GB/s, 48 SM)   [gb10_calibration.json]
L1  TURN/REQUEST  end-of-speech → first-audio; gateway 7-stage + infer turn span  [one trace_id, the seam]
L2  STAGE         STT | LLM | TTS  (infer: prefill | AR-decode | codec/vocoder)    [TraceContext::child spans]
L3  BATCH         cohort width, slot occupancy, admission sheds, KV bytes           [waav_infer_* + serve.rs]
L4  LAYER         per-layer ms + % (backbone vs depformer vs sampling vs codec)     [torch.profiler / CUDA events]
L5  KERNEL        top-N kernels, occupancy, mem/compute throughput %, launch gaps   [nsys timeline + ncu sections]
```
Each level rolls up to the one above (kernel→layer→stage→turn) and carries the same `trace_id`.

## 3. The VERIFIED equation library (the accuracy contract — every analysis script implements EXACTLY these)

Constants come from `gb10_calibration.json` (NOT hardcoded). For GB10 as measured:
`PEAK_BF16_FP32ACC = 86.0e12 FLOP/s`, `PEAK_BW = 198.5e9 B/s` (achievable) / `273e9` (datasheet ceiling),
`RIDGE = PEAK_FLOPS/PEAK_BW`.

| Metric | Formula (units) | Notes |
|---|---|---|
| **RTF** | `wall_s / audio_s` (dimensionless) | <1 = real-time. Streaming: also TTFA + per-frame budget vs frame period `T_f`. |
| **Arithmetic intensity** | `AI = FLOPs / bytes_moved` (FLOP/byte) | per kernel/layer/step. |
| **Roofline attainable** | `min(PEAK_FLOPS, AI × PEAK_BW)` (FLOP/s) | bound = compute if AI>RIDGE else memory. |
| **Ridge point** | `PEAK_FLOPS / PEAK_BW` (FLOP/byte) | GB10 measured ≈ **433**. AI below ⇒ memory-bound. |
| **MFU** | `achieved_FLOPs / (PEAK_FLOPS × time)` | transformer step FLOPs ≈ `2·N + 4·L·s·d` per token (N=params). Denominator = the **measured bf16-fp32acc** peak. |
| **MBU** | `bytes_moved / (PEAK_BW × time)` | decode bytes ≈ `weights·dtype + KV_read`. B=1 AR is weight-bandwidth-bound. |
| **Achieved occupancy** | ncu `sm__warps_active.avg.pct_of_peak_sustained_active` | warps resident / max. |
| **Compute throughput %** | ncu `sm__throughput.avg.pct_of_peak_sustained_elapsed` | the SOL compute %. |
| **Memory throughput %** | ncu `gpu__compute_memory_throughput…` / `dram__throughput…` | the SOL memory %. |
| **DRAM bytes/s** | ncu `dram__bytes.sum.per_second` | vs PEAK_BW = the real MBU. |
| **Launch-bound signal** | nsys: Σ(inter-kernel gaps) / wall | high gap-fraction + low SOL = launch-bound. |
| **Bottleneck class** | compute% high → compute-bound; mem% high → memory-bound; both low + gaps → launch-bound | the lever selector. |
| **Amdahl** | `speedup = 1/((1-p) + p/s)` | the 1.3×-kernel → 1.1×-e2e dilution math (`p` = that component's wall fraction). |

**The signature → lever rules (mechanical, for the AI/human report):**
- memory-bound + high MBU → already near peak; lever = fewer bytes (quant/KV-compress/precision) or bigger batch.
- compute-bound + low MFU → lever = better kernels (TRT/fused) or larger GEMM (batch).
- launch-bound (gaps, low SOL) → lever = CUDA-graph / fusion / fewer launches.
- AR-serial (low occupancy, B=1) → lever = batch (throughput) — single-stream is data-dependency-bound.

## 4. Output schema (human + AI), one record per profile

```json
{ "trace_id","model","precision","batch",
  "rtf", "ttfa_ms", "audio_s", "wall_s",
  "stages":[{"name","ms","pct"}], "layers":[{"name","ms","pct","mfu","mbu","ai"}],
  "top_kernels":[{"name","ms","pct","occupancy","compute_pct","mem_pct","class"}],
  "peaks":{"flops_tflops","bw_gbs","ridge","source":"gb10_calibration.json"},
  "bottleneck":{"level","class","evidence"}, "recommended_lever":"…", "amdahl_headroom":… }
```
Plus a rendered human table (the same data) + roll-up to the gateway turn profile by `trace_id`.

## 5. Build order (no stubs; live-verified at each step)

1. ✅ calibration + peak-FLOPs fix (`10a1f7b`).
2. Analysis toolkit (Python) implementing §3 over the calibration + a captured profile — verified against a hand-worked
   example (so the equations are provably right before any model uses them).
3. Serve-callable layered profiler in Infer (L2 stage spans + L4 layer hooks generalized from dia2/neutts; an nsys/ncu
   capture wrapper for L5).
4. Magic-constant fixes (§1.5).
5. The gateway→infer traceparent injection (§1.4).
6. Live-test across models (dia2, csm, a STT, a fast TTS), iterate on real bottlenecks; confirm the report is
   actionable + the numbers cross-check against nsys/ncu ground truth.

Accuracy gate throughout: every derived number must reconcile with a tool's raw counter (ncu DRAM bytes/s ↔ our MBU;
nsys kernel sum ↔ our layer %); a mismatch means a wrong equation, fix before proceeding.
