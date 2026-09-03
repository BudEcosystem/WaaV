# WaaV Infer — Batching Strategies: Full Perf + Accuracy Analysis (SYNTHESIS)

**2026-06-24, GB10 (Grace‑Blackwell sm_121) + aarch64 CPU.** Live analysis of **both** WaaV‑specific batching
strategies across **Path A (ONNX Runtime)** and **Path B (in‑process tch/libtorch)**. Source reports:
`BATCHING-ANALYSIS-PATHA.md`, `-PATHB.md`, `-S2S.md`.

## TL;DR
Both batching seams are **correct** (batched‑cohort output is bit‑identical to per‑stream solo). The honest gaps
are about **wiring and scaling**, not correctness:
1. **Only Path‑A ONNX (chatterbox) is wired into the live serve batcher.** Path‑B tch codec‑AR and S2S serve **solo
   (B=1)** today — the batched seams exist and are proven but aren't on any serve path.
2. **The wired path is the worst‑scaling one** — chatterbox CUDA peaks ~1.8×@B16 and **regresses to ~1.0×@B64**
   because the LM round‑trips KV through the host every stride. The device‑resident tch path scales **~30×@B64** but
   isn't wired.
3. **A real P0 was found + fixed** (`65ba7ec`): the live batcher **crashed on the default `q4f16` config**.
4. **bf16 batched ≠ solo flips codes** (GEMM reduction‑order floor) — latent today, but it gates wiring any tch
   model into a bf16 lockstep batcher.

## Accuracy — bit‑identity of batched cohort vs per‑stream solo

| Strategy | Path | Model | GB10 CUDA | aarch64 CPU |
|---|---|---|---|---|
| codec‑AR lockstep | A (ONNX) | chatterbox | ✅ codes identical (ragged + 16‑wide) | ✅ identical (GQA identity is EP‑agnostic) |
| flow‑TTS cohort | A (ONNX) | supertonic | ✅ maxΔ=0.0 | — |
| STT cohort | A (ONNX) | whisper‑tiny | — | ✅ transcripts identical |
| codec‑AR batched probe | B (tch) | qwen3‑tts | f32 ✅ max\|Δ\|=0; **bf16 ⚠️ flips argmax (1/4@B4)** | f32 ✅ (≤B16) |
| S2S duplex | B (tch) | CodecArDuplex (ragged 4‑session) | ✅ token‑for‑token identical | — |

**The bf16 caveat (B23 scar):** batched and B=1 GEMMs reduce in a different order; in bf16 the ~0.3–4.2 logit Δ can
flip a code. Not a production bug today (no tch batched‑serving path; CFG B=2 stays byte‑identical: dia2 608/608,
csm 544/544) — but it must be addressed (f32‑accumulate batched, or accept the floor) before a tch bf16 batcher.

## Performance — scaling curve (per‑stride wall, batched vs per‑slot loop)

| Strategy / model | HW | Peak speedup | Knee | Note |
|---|---|---|---|---|
| chatterbox codec‑AR (Path A, **wired**) | GB10 CUDA | **1.77×@B16** → 1.06×@B64 | B≈16 | host‑KV re‑stream dominates past knee; **55×@64 thesis FALSE** |
| chatterbox codec‑AR (Path A) | aarch64 CPU | **4.14×@B8** | >B8 | scales *better* than CUDA (no H2D/D2H wall); ~4–8× higher absolute latency |
| supertonic flow‑TTS (Path A) | GB10 CUDA | 2.33×@B8 | — | step‑bucket cohort |
| whisper STT (Path A) | aarch64 CPU | 1.19× | — | equal‑context cohort |
| qwen3‑tts probe (Path B, **device‑resident, not wired**) | GB10 CUDA bf16 | **30.2×@B64** | B≈16 | flat ~11 ms through B16; ~55% of the 55× roofline |
| qwen3‑tts probe (Path B) | aarch64 CPU f32 | 12.85×@B64 | — | soft curve |
| S2S duplex (Path B, not wired) | GB10 CUDA | 2.61×@N=8 (0.88×@N=2) | N≈4 | first‑frame latency 81.9 ms @4 tenants (200 ms budget) |

**The headline perf truth:** device‑resident KV scales ~30×; host‑KV re‑stream caps at ~1.8× and then regresses.
The live system runs the host‑KV variant.

## Bugs / gaps / opportunities (ranked, RCA'd)

| # | Sev | Item | State |
|---|---|---|---|
| B0 | 🔴 P0 | Batcher crashed on default `q4f16` LM (F16 KV vs hard‑coded `.as_f32()`) — concurrent cohort only | **FIXED `65ba7ec`** (bit‑exact F16↔f32 round‑trip) |
| G1 | 🟠 HIGH (perf) | Host‑KV re‑stream caps Path‑A scaling at ~1.8×; **device‑resident ring‑KV re‑export** is the only path to near‑linear | scoped — #1 perf lever |
| G2 | 🟠 HIGH (perf) | **No tch model wired to lockstep `step_batch`** (all B=1/B=2‑CFG); the 30×‑scaling device‑resident path is unused | scoped |
| G3 | 🟠 HIGH (perf) | **S2S serve is solo** — `CodecArDuplexModel` only in tests; hibiki has **no batched form**; no S2S analog of `codec_ar_batcher` | scoped |
| G4 | 🟡 MED (accuracy) | bf16 batched≠solo flips codes — blocks a tch bf16 batcher (G2) | needs f32‑accumulate or accept floor |
| G5 | 🟡 MED (perf) | `q4f16` LM worsens the host‑KV bottleneck (host‑bound, N=16 didn't finish in 15 min vs fp32 ~94 s) → **keep codec‑AR LM at fp32 for serve** | config recommendation |
| G6 | 🟢 LOW | No `/metrics` for live cohort width (a `step_batch` cohort histogram, ~10 lines) | do‑now observability |
| G7 | 🟢 LOW | RIGHT‑pad KV waste (~27% on staggered S2S cohorts); CUDA‑graph shape‑bound vs dynamic batch (bucket B) | bounded |
| G8 | 🟡 MED (reliability) | `live_gb10_batcher` gate OOMs the **unbounded ORT‑CUDA arena** under GPU pressure (loads 3 chatterbox instances for the batched‑vs‑solo comparison; 18.3 GB avail vs 21.7 GB req in the vocoder Conv). Pre‑existing (proven by stash‑rebuild), the documented GB10 unified‑pool arena issue. Mostly a TEST‑harness memory issue (prod loads 1 model + batches concurrent streams), but the live batching gate flakes under load. **Fix:** cap the codec‑AR ONNX EP arena (`gpu_mem_limit` / `kSameAsRequested`, the 950d491 pattern), or lower the gate's instance count. | found (pre‑existing) |

## Prioritized next steps
1. **G6 (do‑now):** add the `step_batch` cohort‑width histogram to `/metrics` — observability for the live batcher.
2. **G5 (do‑now):** default the codec‑AR serve LM to fp32 (document; `q4f16` is host‑bound‑worse until G1).
3. **G1 (the real lever):** device‑resident ring‑KV for the codec‑AR LM (ONNX re‑export or the tch device‑resident
   path) → recover near‑linear scaling; unblocks G2.
4. **G2/G4:** wire a tch model into the device‑resident lockstep batcher (30× target), resolving bf16‑batched first.
5. **G3:** an S2S analog of `codec_ar_batcher` (the seam + bit‑identity + latency are proven; only serve wiring) +
   hibiki's batched form (substantial).

## Both hardware (GB10 + aarch64 CPU)
Accuracy is HW‑invariant (the GQA left‑align identity is a graph property). Perf differs sharply: **CPU batching
scales *better* relatively** (chatterbox 4.14×@B8 CPU vs 1.77× CUDA — no H2D/D2H wall) but at ~4–8× higher absolute
latency; CUDA wins absolute throughput where KV stays device‑resident. This argues for **per‑backend batch‑knee
tuning** (the CPU knee is later than CUDA's).
