# 05 — Hardware & Hardware-Architecture Scenarios

Family: **Multi-hardware + hardware-architecture.** Every scenario is grounded in `INFER_ENGINE.md` §2 (substrate-aware HAL + the per-substrate batch-knee/placement table), §2.1 (the roofline law), §2.3 (static-conv-GOOD / dynamic-AR-BAD), §3.4 (zero-copy heterogeneous placement + the shared-bandwidth contention guard), §5.2 (precision×substrate), and the §1.x GB10 benchmarks. Axes tag the hardware (`hw:`), memory hierarchy (`mem:`), and execution model (`simd:` SIMT vs SIMD vs systolic/dataflow). Levels: **SIMPLE** (one substrate, one fact) → **INTERMEDIATE** (placement / precision / EP choice) → **COMPOUND** (two+ substrates, contention, zero-copy) → **EXTREME** (heterogeneous box under a frame deadline, degenerate physics).

---

## SIMPLE — one substrate, one property

### HW-1 — GB10 batch-1 leaves the GPU idle
- Level: simple
- Pipeline: TTS (AR codec-LM)
- Axes: hw:gb10, hw:gpu, mem:lpddr5x-273, simd:simt, batch-knee
- Scenario: A single live TTS stream runs a 0.5B codec-LM on the GB10 iGPU; the decode step measures 8.6 ms at batch 1 (§1.1), the GPU is massively underused.
- System must: Serve at B=1 with the CUDA-graphed step (1.21× @batch-1, §1.3) and advertise free slots — the flat-to-64 curve means 63 more streams cost almost nothing.
- If mishandled: Operator provisions one GB10 per call, wasting ~98% of the chip.

### HW-2 — CPU cannot serve a 0.5B AR codec-LM in realtime
- Level: simple
- Pipeline: TTS (AR codec-LM)
- Axes: hw:cpu, hw:arm-grace, mem:lpddr5x, simd:neon, batch-knee
- Scenario: With no GPU available the engine is asked to run a 0.5B codec-LM AR decode on Grace ARM (20 threads, fp32, no AMX); the step floor is ~210 ms (§1.7), ~24× the GPU and far over an 80 ms frame budget.
- System must: Refuse realtime AR-codec on a pure-CPU floor for a 0.5B+ model; CPU tier is for tiny/quantized models + feedforward stages, not 0.5B AR decode.
- If mishandled: Every frame underruns; audio is a continuous stutter.

### HW-3 — Hexagon NPU is strictly static, batch fixed at 1
- Level: simple
- Pipeline: STT (conv encoder) on phone
- Axes: hw:hexagon, hw:npu, mem:vtcm-8mb, simd:systolic, static-shape
- Scenario: A phone wants to run a Whisper conv encoder on the Hexagon HMX; the runtime is an AOT context binary with fixed shapes and batch=1, HMX reads only the ~8 MB VTCM scratchpad.
- System must: Compile a static-shape encoder graph (batch=1, fixed audio window) for QNN; never attempt dynamic batching or growing-KV on Hexagon.
- If mishandled: Shape mismatch at runtime or a fall-back recompile per request.

### HW-4 — B200 is wasteful for 1–4 streams
- Level: simple
- Pipeline: any AR
- Axes: hw:b200, hw:gpu, mem:hbm3e-8tb, simd:simt, batch-knee
- Scenario: A B200 (~8 TB/s HBM3e, ideal batch ~256–512) is handed a single voice call; under-occupancy is severe at small batch (§2.2).
- System must: Either pack hundreds of streams onto it (its design point) or place the single stream on a cheaper edge tier — a B200 at batch 1 is the most launch-sensitive and least cost-efficient choice.
- If mishandled: A datacenter GPU bills full price to do a phone's work.

### HW-5 — RTX is VRAM-capacity-bound before bandwidth
- Level: simple
- Pipeline: TTS/STT multi-stream
- Axes: hw:rtx, hw:gpu, mem:gddr-24gb, simd:simt, batch-knee
- Scenario: An RTX 4090 (24 GB GDDR, ideal batch tens) loads a 3B+ model plus per-stream KV; VRAM fills before the bandwidth knee is reached (§2.2).
- System must: Treat VRAM capacity as the binding constraint on RTX (quantize weights, cap slots by memory), not the compute crossover.
- If mishandled: OOM at admission, or the model won't load at all.

### HW-6 — Whisper encoder is a perfect systolic fit
- Level: simple
- Pipeline: STT (Whisper AED encoder)
- Axes: hw:npu, simd:systolic, static-shape, placement
- Scenario: A Whisper conv/attention encoder (fixed 30 s mel window, static shape, no growing KV) is offered to an NPU systolic array.
- System must: Place the encoder on the NPU — it maps perfectly to a dataflow array (dense MACs, no branching, no per-token host round-trip); both Qualcomm and Apple ship Whisper split this way (§2.3).
- If mishandled: Encoder runs on the CPU floor, wasting a free accelerator.

### HW-7 — quantize weights everywhere (the universal decode win)
- Level: simple
- Pipeline: any AR decode
- Axes: hw:any, mem:bandwidth-bound, precision:int8/int4, roofline
- Scenario: AR decode is memory-bandwidth-bound on every substrate (§2.1); weights streamed ÷ bandwidth dominates the step.
- System must: Quantize weights (int4 ≈4×, int8/fp8 ≈2× cheaper) for near-proportional decode speedup on CPU, GPU, NPU alike — and widen the flat batch region.
- If mishandled: fp16 weights stream needlessly, halving achievable concurrency and edge latency.

### HW-8 — fp8 is a DC-batch lever, not a batch-1 latency lever
- Level: simple
- Pipeline: TTS DC
- Axes: hw:b200, hw:hopper, precision:fp8, mem:hbm, batch-knee
- Scenario: An operator enables fp8 hoping for lower single-stream latency; measured fp8/bf16 GEMM is 0.62× at M=64 (slower) but 2.11× at M=4096 (§1.6).
- System must: Use bf16 for edge/batch-1 and reserve fp8/mxfp4 for the compute-bound DC large-batch regime; never flip a batch-1 stream to fp8 expecting a speedup.
- If mishandled: Edge latency gets *worse* after a well-intentioned fp8 toggle.

### HW-9 — Mimi codec decode is already GPU-efficient
- Level: simple
- Pipeline: TTS (codec/vocoder stage)
- Axes: hw:gpu, simd:simt, codec-stream, mem:hbm
- Scenario: The shared Mimi decoder runs 6.0 ms at batch 1 (RTF 0.003) and ~1.7× batch efficiency (§1.4) — it is not the bottleneck; the AR LM is.
- System must: Spend the batching/optimization budget on the AR stage; treat codec decode as a light, offloadable terminal stage.
- If mishandled: Engineering effort is misdirected at the already-fast vocoder while AR streams starve.

### HW-10 — MI300X has capacity to spare
- Level: simple
- Pipeline: multi-model co-residency
- Axes: hw:mi300x, hw:gpu, mem:hbm3-192gb, simd:simt, batch-knee
- Scenario: An MI300X (192 GB HBM3, 5.3 TB/s, 256 MB cache, ideal batch ~128–256) is asked to co-host several small voice models with large KV.
- System must: Exploit it as the "max co-resident small models + huge KV" substrate (§2.2) — capacity is a non-issue; pack many models, batch wide.
- If mishandled: It runs one model like an H200 and forfeits its capacity advantage.

### HW-11 — H200 wants concurrency, not one stream
- Level: simple
- Pipeline: batched STT/TTS
- Axes: hw:h200, hw:hopper, mem:hbm3e-4.8tb, simd:simt, batch-knee
- Scenario: An H200 (141 GB HBM3e, 4.8 TB/s, ideal batch ~64–256+) suffers under-occupancy at small batch.
- System must: Drive high-concurrency batched STT/TTS or AR decode at scale with CUDA-graph + seqlen buckets; fill it.
- If mishandled: Expensive Hopper silicon idles at batch 1.

### HW-12 — TPU/Edge-TPU needs static int8 graphs
- Level: simple
- Pipeline: STT encoder / KWS
- Axes: hw:tpu, hw:edge-tpu, hw:npu, simd:systolic, static-shape, precision:int8
- Scenario: An Edge-TPU is offered a voice frontend; it natively runs INT8/INT4 with static shapes only, on-chip SRAM + DRAM.
- System must: Map only static conv encoders / CNN vocoders / KWS in int8; never route AR decode here (BAD at AR, §2.2).
- If mishandled: The compiler rejects the dynamic AR graph or silently degrades it.

### HW-13 — Apple ANE is fp16/int8, static only
- Level: simple
- Pipeline: STT encoder on Mac/iPhone
- Axes: hw:ane, hw:apple, simd:systolic, static-shape, precision:fp16
- Scenario: An on-device Mac/iPhone wants the Whisper encoder on the ANE (fp16/int8, static shapes).
- System must: Pin the static encoder to ANE via CoreML EP and the AR decoder to GPU/CPU (§2.3); quantize to fp16/int8 to fit ANE's contract.
- If mishandled: A dynamic-shape graph is forced onto ANE and falls back, killing the latency win.

### HW-14 — LPDDR5X bandwidth sets the GB10 batch knee
- Level: simple
- Pipeline: AR decode
- Axes: hw:gb10, mem:lpddr5x-273, roofline, batch-knee
- Scenario: GB10's ~273 GB/s LPDDR5X is ~17–29× lower than a B200's HBM3e; the AR batch knee is set by bandwidth, not compute (§2.1, §2.2).
- System must: Size the slot count from `WeightBytes/273GB-s` against the frame budget — the knee is low and bandwidth-bound, so quantization helps most here.
- If mishandled: The scheduler assumes an HBM-class knee and oversubscribes slots into underruns.

### HW-15 — the roofline ridge point sets the ideal batch
- Level: simple
- Pipeline: any AR
- Axes: hw:any, roofline, mem:bandwidth, batch-knee
- Scenario: The ideal batch rises with a substrate's FLOPs:bandwidth ratio — a CPU/NPU (low ratio) saturates at batch 1–4; a B200 (high ratio) needs a *bigger* batch than an H200 just to fill (§2.1).
- System must: Compute each substrate's knee from its ridge point (peak-compute ÷ peak-bandwidth) rather than using one global batch size.
- If mishandled: A single hard-coded batch underfills the B200 and overruns the CPU.

### HW-16 — fp8/fp4 native only on Hopper/Blackwell
- Level: simple
- Pipeline: DC precision selection
- Axes: hw:hopper, hw:blackwell, precision:fp8/mxfp4/nvfp4, simd:simt
- Scenario: A manifest requests fp8 weights but the active GPU is pre-Hopper; fp8 tensor cores don't exist there, mxfp4/nvfp4 only on Blackwell (§5.2).
- System must: Resolve precision per active EP (`by_substrate[ep]`) so fp8/fp4 lands only on Hopper+/Blackwell, falling back to bf16 elsewhere.
- If mishandled: The fp8 graph fails to compile or silently emulates in software.

### HW-17 — AMX gives x86 CPU an int8 realtime path
- Level: simple
- Pipeline: small STT/TTS on x86
- Axes: hw:cpu-x86, simd:amx, precision:int8, mem:ddr5
- Scenario: An x86 server with AMX (bf16/int8, ~8× VNNI) is the only substrate; a small quantized model must serve a single stream.
- System must: Use AMX-int8 kernels for the CPU tier — this is the one CPU path that can approach realtime for small AR/codec where Grace ARM (no AMX) cannot (§1.7, §5.2).
- If mishandled: The engine assumes "CPU = too slow" and refuses a workload AMX could serve.

### HW-18 — Hexagon offers true int4 (HMX)
- Level: simple
- Pipeline: phone STT/KWS
- Axes: hw:hexagon, precision:int4, mem:vtcm, simd:systolic, static-shape
- Scenario: A phone needs the smallest possible static encoder; Hexagon HMX supports int8/int16 + true int4 (§5.2).
- System must: Ship an int4-quantized static encoder for Hexagon (accuracy-gated, §5.2) to fit VTCM and minimize bandwidth.
- If mishandled: An fp16 encoder blows the VTCM cap and can't map onto HMX.

### HW-19 — CPU is the guaranteed portability floor
- Level: simple
- Pipeline: any
- Axes: hw:cpu, mem:ddr, simd:any, ep-fallback
- Scenario: An accelerator kernel is missing for an op on the active backend; the dispatch chain hits `forward_native` (P-6).
- System must: Fall to the pure portable CPU path (ORT/ggml/ndarray) and emit a degrade telemetry event — an accelerator gap is never an `Err`.
- If mishandled: A missing kernel returns an error instead of degrading, taking the stream down.

### HW-20 — Intel Core Ultra has three engines on one bus
- Level: simple
- Pipeline: edge DAG
- Axes: hw:core-ultra, hw:npu, hw:igpu, mem:lpddr5x-136, simd:mixed
- Scenario: A Lunar Lake laptop (16/32 GB LPDDR5X, ~136.5 GB/s) exposes CPU + Xe2 iGPU + NPU, all sharing one ~136 GB/s bus (§2.2).
- System must: Route static stages to the NPU (OpenVINO), dynamic to Xe2, with the shared bus as the binding ceiling — three engines contend for one bandwidth pool.
- If mishandled: Two bandwidth-bound stages co-run on the bus and both miss the frame budget.

---

## INTERMEDIATE — placement, precision, EP fallback

### HW-21 — split Whisper: encoder on NPU, AR decoder on GPU
- Level: intermediate
- Pipeline: STT (Whisper AED)
- Axes: hw:npu, hw:gpu, simd:systolic+simt, placement, static-vs-dynamic
- Scenario: A Whisper AED model has a static conv encoder and a dynamic AR text decoder; on a heterogeneous box both substrates are available.
- System must: Place the encoder on the NPU and the AR decoder on the GPU/CPU (the canonical §2.3 split) — static-conv GOOD on systolic, dynamic-AR BAD.
- If mishandled: The whole model runs on the GPU (wasting the NPU) or the decoder is forced onto the NPU and breaks the static-shape contract.

### HW-22 — int8 weights on ORT-CUDA silently fall to CPU
- Level: intermediate
- Pipeline: STT/TTS on GB10
- Axes: hw:gb10, hw:gpu, precision:int8, ep:ort-cuda, mem:lpddr5x
- Scenario: An int8 checkpoint is loaded under the ORT CUDA-EP; `MatMulInteger`/Q-DQ silently partition to the CPU EP — measured 12 ms fp → 232 ms int8 (§5.2).
- System must: Resolve `by_substrate[ort-cuda]` to a CUDA-supported dtype (bf16/fp16) and route int8 to the TensorRT EP (static, S8S8) or the torch sidecar tier; never let an int8 file land on ORT-CUDA.
- If mishandled: A "memory optimization" makes inference ~19× slower with no error.

### HW-23 — GroupQueryAttention op is fp16/bf16/float only
- Level: intermediate
- Pipeline: AR decode (GQA model)
- Axes: hw:gpu, ep:ort-cuda, precision:int8, simd:simt
- Scenario: A GQA AR model is quantized to int8; ORT's fused `GroupQueryAttention` op accepts only fp16/bf16/float (§5.2).
- System must: Keep attention in fp16/bf16 even when GEMMs elsewhere are quantized; the quantized-KV PR was closed unmerged, so KV-quant on this op is not available via ORT-CUDA.
- If mishandled: The fused op rejects the int8 KV or de-fuses to a slow path.

### HW-24 — int8 Voxtral was validated on CPU, not ORT-CUDA
- Level: intermediate
- Pipeline: STT (Voxtral)
- Axes: hw:cpu, precision:int8, ep:cpu, simd:amx/vnni
- Scenario: WaaV's int8 Voxtral is byte-identical to plain onnxruntime int8 — but only on CPU (no aarch64 GPU wheel; ORT-CUDA can't int8-GEMM) (§5.2 + memory).
- System must: Run int8 Voxtral on the CPU/AMX tier or move it to TensorRT-EP/torch for GPU int8; document that the int8 win is a CPU-tier property under ORT.
- If mishandled: int8 Voxtral is scheduled on GB10 ORT-CUDA expecting a speedup and gets a slowdown.

### HW-25 — never FlashInfer on sm_120 aarch64
- Level: intermediate
- Pipeline: AR decode on GB10
- Axes: hw:gb10, hw:blackwell, simd:simt, kernel-routing
- Scenario: A kernel router considers FlashInfer for attention on the GB10 (sm_120, aarch64); measured ≈2× end-to-end regression (§2 HAL, §7).
- System must: Route attention to cuDNN/SDPA on Blackwell aarch64 (vLLM-Omni's own default) and exclude FlashInfer from the sm_120 candidate set entirely.
- If mishandled: A "faster attention kernel" halves end-to-end throughput on the device of record.

### HW-26 — CUDA-graph helps batch-1, hurts batch-32
- Level: intermediate
- Pipeline: AR decode
- Axes: hw:gb10, hw:gpu, simd:simt, kernel-routing, batch-knee
- Scenario: The same lockstep step is graphed at batch 1 (7.35 ms, 1.21× faster) and batch 32 (13.92 ms, 0.72× slower) (§1.3).
- System must: Tier kernels by batch — CUDA-graph at low-batch/edge, eager/compile at high-batch/DC; pick once at load per cohort batch size.
- If mishandled: A globally-on CUDA-graph slows the DC path 28%; a globally-off one slows the edge path.

### HW-27 — sm_120 CUDA-graph capture hangs → eager fallback is non-optional
- Level: intermediate
- Pipeline: AR decode on GB10
- Axes: hw:gb10, hw:sm120, simd:simt, ep-fallback, kernel-routing
- Scenario: On sm_120 graph capture exhibits hang-after-N-requests / capture-OOM-after-/health-passes / silent-corruption-over-varlen (catalog H4).
- System must: Resolve a per-kernel CUDA-graph support level, auto-downgrade to eager (never crash), and treat eager as a first-class path on GB10 — capture EXACT slot counts to avoid padding cliffs.
- If mishandled: The server crash-loops after warmup passes, or emits silently-corrupt audio.

### HW-28 — `enforce_eager` as an OOM/capture-failure escape
- Level: intermediate
- Pipeline: large model on low-VRAM GPU
- Axes: hw:rtx, hw:gpu, mem:gddr, simd:simt, ep-fallback
- Scenario: A 3B model + CUDA-graph + compile capture overflows a 24 GB RTX during warmup (capture costs real memory) (catalog C8/B).
- System must: Expose `enforce_eager` as a first-class config and auto-fall to eager on capture failure, freeing the graph-pool delta — then climb the OOM ladder (cpu-offload → layerwise → slicing).
- If mishandled: A capture OOM crashes the box instead of degrading to a working eager path.

### HW-29 — follow the immovable weights for placement
- Level: intermediate
- Pipeline: TTS DAG
- Axes: hw:gb10, hw:gpu, hw:npu, placement, mem:unified
- Scenario: AR weights (3–6 GB) are resident on the GPU; the small codec weights sit on the NPU; the placer must decide where each stage runs.
- System must: Pin each stage to where its load-once weights live (AR→GPU, codec→NPU) — rule (3) of the ggml decision order (§3.4); never migrate multi-GB weights per request.
- If mishandled: A stage runs on a substrate that must stream its weights across the bus every frame.

### HW-30 — paradigm×substrate affinity routing
- Level: intermediate
- Pipeline: full STT→TTS DAG
- Axes: hw:gpu, hw:npu, hw:cpu, placement, simd:mixed
- Scenario: A DAG has an AR backbone, a CFM head, a conv codec, and an encoder; the placer applies affinity rule (4).
- System must: Route AR→GPU, CFM→GPU, conv-codec→NPU/CPU, encoder→NPU (§3.4) — each paradigm to its best engine, with a guaranteed CPU fallback.
- If mishandled: The CFM runs on the NPU (compute-bound, no systolic fit) or the codec hogs GPU bandwidth needed for AR.

### HW-31 — degrade-to-CPU floor on accelerator fault
- Level: intermediate
- Pipeline: any
- Axes: hw:gpu, hw:cpu, ep-fallback, mem:any
- Scenario: A GPU EP raises mid-session (driver fault / OOM on a co-tenant); the op has a portable `forward_native`.
- System must: Degrade that op/stage to the CPU floor + emit telemetry, keeping the contract alive (P-6) rather than failing the request.
- If mishandled: A transient accelerator fault becomes a hard request failure.

### HW-32 — quant accuracy gate is per-substrate
- Level: intermediate
- Pipeline: any quantized model
- Axes: hw:any, precision:int8/fp8, accuracy-gate
- Scenario: An int8 variant verified on CPU is loaded onto an NPU with different rounding; the `verified{substrate,precision,metric}` stamp doesn't match.
- System must: Re-run the accuracy gate per (substrate, precision) before serving and refuse/fall back to `reference_precision` if unverified — a substrate-specific stamp, not a global one.
- If mishandled: A quant that passed on CPU ships silently-degraded audio on the NPU.

### HW-33 — the TTS quant gate must include MOS, not just WER
- Level: intermediate
- Pipeline: TTS quantization
- Axes: hw:any, precision:int8/int4, accuracy-gate, codec
- Scenario: An int4 TTS variant passes a text-only WER check but the AR drift across frames crashes perceptual quality (the WER-flat/MOS-crash signature) (§5.2).
- System must: Gate TTS quant with a perceptual/MOS check (plus streaming + concurrent layers, catalog I4) before promotion on any substrate.
- If mishandled: A WER-clean quant ships audible artifacts that no text metric caught.

### HW-34 — norms/RoPE/codec stay high-precision regardless of substrate
- Level: intermediate
- Pipeline: AR + codec
- Axes: hw:any, precision:mixed, codec, simd:any
- Scenario: A user sets a global int8 precision; the per-component defaults must keep norms, RoPE, sampling head, and the codec/vocoder high-precision (quant noise compounds across frames) (§5.2, catalog).
- System must: Apply per-architecture `component_precision` defaults so the GEMMs quantize but norms/RoPE/codec/head stay fp32/bf16 with zero user config.
- If mishandled: int8 RMSNorm/codec corrupts audio that a GEMM-only quant would have kept clean.

### HW-35 — telephony 8 kHz egress needs anti-alias downsample
- Level: intermediate
- Pipeline: TTS egress
- Axes: hw:cpu, mem:any, codec, resample, sample-rate
- Scenario: A 24 kHz codec output must reach an 8 kHz PSTN leg; naive decimation aliases (§5.1).
- System must: Run the model-SR→8 kHz resample as a post-batch CPU/NPU stage with anti-aliasing (sinc for fractional ratios), then repacketize to fixed 20 ms RTP via a jitter buffer.
- If mishandled: The phone leg gets a tinny, aliased voice.

### HW-36 — codec-decode + resample are off-clock, CPU-offloadable
- Level: intermediate
- Pipeline: TTS terminal stages
- Axes: hw:cpu, hw:npu, codec, placement, sample-rate
- Scenario: The AR stage ticks the frame clock on the GPU; codec decode and resample are downstream and not on the AR clock (§5.1).
- System must: Place codec-decode + resample on CPU/NPU off the AR clock — frees GPU bandwidth for more AR streams (the terminal codec node is the safe offload point, §3.2).
- If mishandled: Codec decode steals GPU bandwidth and head-of-line-blocks AR streams.

### HW-37 — diffusion CFM is compute-bound, batches sublinearly
- Level: intermediate
- Pipeline: TTS (chunk CFM)
- Axes: hw:gpu, simd:simt, diffusion, batch-knee, compute-bound
- Scenario: A chunk-CFM DiT step is compute-bound — only ~10× @64 efficiency, and a 10-step solve @B64 = 110 ms, exceeding a 40 ms frame budget (§1.5).
- System must: Amortize chunk-CFM over frames (chunked/lookahead), never run it per-frame; bucket by (model, latent-shape, NFE, CFG) on its own micro-batch stage.
- If mishandled: A per-frame CFM solve blows the frame budget and underruns.

### HW-38 — DDPM head collapses at high batch
- Level: intermediate
- Pipeline: TTS (VibeVoice-class DDPM)
- Axes: hw:gpu, simd:simt, diffusion, batch-knee, compute-bound
- Scenario: A 25-step DDPM head solve is 90 ms @B1 and 624 ms @B64, collapsing at B128 (§1.5).
- System must: Cap the diffusion-head batch well below the AR knee (it's a different, compute-bound curve) and lookahead-amortize; admission must budget the head as the bottleneck stage.
- If mishandled: Scaling the diffusion head like the AR LM destroys realtime for everyone in the cohort.

### HW-39 — nested per-frame patch batches like AR
- Level: intermediate
- Pipeline: TTS (dots.tts-class nested)
- Axes: hw:gpu, simd:simt, nested-ar-diffusion, batch-knee
- Scenario: A nested AR-outer + per-frame diffusion patch (tiny T) batches 38×@64 — it rides the AR axis because the tiny latent is launch-bound (§1.5, §3.3).
- System must: Keep the inner head fused inside the outer lockstep step (one StageNode) so all B slots are at the same inner step and the inner kernel is a single `[B,…]` batch — nesting is net-positive precisely because the head can't saturate the GPU alone.
- If mishandled: Splitting the inner loop into a separate stage balloons per-step latency.

### HW-40 — Edge-TPU/static NPU rejects growing-KV AR
- Level: intermediate
- Pipeline: STT/TTS on Edge-TPU
- Axes: hw:tpu, hw:npu, static-shape, simd:systolic, placement
- Scenario: An AR decoder with a per-token growing KV and data-dependent control flow is offered to an Edge-TPU.
- System must: Reject AR placement on the static NPU; route only the static encoder/vocoder there and keep AR on GPU/CPU-AMX (§2.2, §2.3).
- If mishandled: The static compiler errors, or pads to a fixed max and wastes the array on a contract it can't honor.

### HW-41 — power-of-two prefill chunks (tile quantization)
- Level: intermediate
- Pipeline: prefill firewall
- Axes: hw:gpu, simd:simt, prefill, mem:hbm
- Scenario: A prefill chunk of 257 tokens runs ~32% slower than 256 due to tile quantization (§4.5).
- System must: Keep prefill chunk token counts power-of-two and align the fused batch width to GPU tiles; chunk any prefill exceeding one frame-budget's tokens.
- If mishandled: An off-by-one token count silently adds a wave-quantization tax to every prefill.

### HW-42 — KV-quant scales streams only for big-KV models
- Level: intermediate
- Pipeline: full-duplex S2S
- Axes: hw:gpu, precision:kv-int4, mem:hbm, batch-knee
- Scenario: Moshi-7B (32 KV heads, ctx 3000) fits only 25 streams/40 GB at fp16 but 101 at int4; a 0.5B codec-LM is unaffected by KV-quant (§1.6).
- System must: Apply KV-quant as a concurrency lever only on big-KV models (gate it by KV bytes/stream); skip it for small codec-LMs where it's irrelevant.
- If mishandled: KV-quant is enabled on a tiny codec-LM (no benefit, accuracy risk) or omitted on Moshi-7B (4× fewer streams).

### HW-43 — GQA inflates per-token KV ~9×
- Level: intermediate
- Pipeline: AR decode
- Axes: hw:gpu, mem:hbm, precision:kv, batch-knee
- Scenario: A Qwen3-0.6B with 8 KV heads has KV/token ~9× a 2-KV-head 0.5B; the slot ceiling drops accordingly (§5.2).
- System must: Treat GQA head count as the biggest KV lever *before* quant — size slots from actual KV bytes/token, and reach for KV-quant where GQA is wide.
- If mishandled: Slot count is set from param count and the wide-GQA model OOMs at admission.

### HW-44 — 3B model on 8 GiB edge is practical only quantized
- Level: intermediate
- Pipeline: edge TTS
- Axes: hw:edge, mem:8gib, precision:int4, batch-knee
- Scenario: A 3B model is requested on an 8 GiB edge device; at fp16 the weights alone don't fit (§5.2).
- System must: Load a published int4/AWQ/GPTQ/GGUF checkpoint (manifest selects the variant, zero-code) so the 3B fits; on 80 GiB DC the same weights are a rounding error and quant becomes a pure KV/throughput lever.
- If mishandled: The 3B refuses to load on edge, or loads fp16 and OOMs.

### HW-45 — load published quantized checkpoints as-is
- Level: intermediate
- Pipeline: any
- Axes: hw:any, precision:awq/gptq/gguf/fp8, mem:any
- Scenario: A model ships AWQ + GPTQ + GGUF + fp8 variants; the substrate dictates which is loadable.
- System must: Select the variant via the manifest (`by_substrate`/`precision` resolution) with zero code — GGUF/int for CPU+ggml, fp8 for Hopper+, never an int8 file onto ORT-CUDA (§5.2).
- If mishandled: The wrong variant lands on the wrong substrate and either fails or runs on the slow EP.

### HW-46 — TensorRT EP is static, S8S8 only
- Level: intermediate
- Pipeline: int8 AR on GB10
- Axes: hw:gb10, hw:gpu, ep:tensorrt, precision:int8, static-shape
- Scenario: To reach int8 tensor cores on GB10 the engine must use the TensorRT EP, which is static-shape and S8S8 only (§5.2).
- System must: Build static-shape TRT engines for the fixed lockstep step (one shape per cohort batch size) when int8-on-GPU is required; otherwise use the torch sidecar tier (torchao/native fp8).
- If mishandled: A dynamic-shape int8 graph is handed to TRT-EP and won't build.

### HW-47 — torch sidecar is the natural home for GPU quant
- Level: intermediate
- Pipeline: AR/codec GPU quant
- Axes: hw:gpu, ep:torch, precision:fp8/int4, simd:simt
- Scenario: A model needs GPU fp8/int4 GEMM that ORT-CUDA can't provide; the torch sidecar owns its kernels (torchao/bnb) (§5.2).
- System must: Route GPU-quant paths to the Path-B torch tier (it owns kernels), keeping Path-A ONNX for the supported-dtype graphs; the sidecar reports its footprint+duty at handshake.
- If mishandled: GPU quant is forced through ONNX and silently CPU-partitions.

### HW-48 — F16 KV-cache empty tensors must be graph-driven dtype
- Level: intermediate
- Pipeline: STT (q4f16 on CUDA)
- Axes: hw:gpu, precision:q4f16, ep:ort-cuda, mem:hbm
- Scenario: A q4f16 Voxtral on CUDA needs enc pkv + past_padding_cache + dec zero_past as f16, while input_features/inputs_embeds/audio_embeds stay f32 (memory: voxtral q4f16 fix).
- System must: Drive empty-tensor KV dtype from `StaticGraph::input_types()` (graph-driven, generalizable) so q4f16 is zero-code beyond swapping weights; argmax_last already handles f16 logits.
- If mishandled: Hard-coded f32 KV empties mismatch the q4f16 graph and crash or mis-type.

### HW-49 — Moonshine-class tiny STT fits the CPU edge tier
- Level: intermediate
- Pipeline: STT (Moonshine)
- Axes: hw:cpu, mem:ddr, simd:neon/avx, latency>throughput
- Scenario: A single-stream edge box with only a CPU needs low-latency STT; Moonshine is a tiny model where latency beats throughput.
- System must: Place tiny STT on the CPU tier (§2.2 favored: edge/single-stream, latency>throughput) — the CPU floor serves these well even without an accelerator.
- If mishandled: A heavyweight model is chosen for an edge CPU that can't keep up.

### HW-50 — low frame-rate is the biggest realtime-throughput lever
- Level: intermediate
- Pipeline: codec choice
- Axes: hw:gpu, mem:bandwidth, frame-rate, batch-knee
- Scenario: Mimi 12.5 Hz (80 ms) allows batch 16–32 @0.5B; EnCodec-48k 150 Hz (6.7 ms) is sub-realtime even at batch 1 — a 12× frame-rate spread (§4.4).
- System must: Prefer low-frame-rate codecs for throughput; size the slot count from `0.8 · T_f / t_step` where `T_f = 1000/frame_rate`.
- If mishandled: A high-FR codec is chosen and no substrate can serve even one realtime stream.

---

## COMPOUND — two+ substrates, contention, zero-copy

### HW-51 — zero-copy stage handoff on GB10 coherent memory
- Level: compound
- Pipeline: AR→codec DAG
- Axes: hw:gb10, mem:nvlink-c2c-coherent, zero-copy, placement
- Scenario: The AR stage on the GPU produces `TokenFrame`s consumed by a codec stage on the CPU; both see the same coherent LPDDR (NVLink-C2C + ATS).
- System must: Pass a `ZeroCopyBuffer{ptr,buft,layout,owner,ready_event}` — every substrate advertises `SharedHostBufType` so the boundary crosses with ZERO copy (the copy degenerates to a pointer alias) (§3.4).
- If mishandled: A needless DMA copy is inserted per frame on a unified-memory box.

### HW-52 — the shared-bandwidth contention law (two bandwidth-bound MUST serialize)
- Level: compound
- Pipeline: AR + second bandwidth-bound stage
- Axes: hw:gb10, mem:lpddr5x-273-shared, contention, roofline
- Scenario: Two memory-bandwidth-bound stages (AR decode + a bandwidth-heavy second model) are placed on GB10's single ~273 GB/s LPDDR pool; concurrent engines DIVIDE the ceiling (§3.4).
- System must: Co-locate + time-share two bandwidth-bound stages (serialize them); only overlap a memory-bound stage with a compute-bound one (compute-bound ∥ bandwidth-bound is OK).
- If mishandled: Both bandwidth-bound stages run concurrently, each gets half the bus, and both miss the frame deadline.

### HW-53 — placement frees the GPU bus for AR streams
- Level: compound
- Pipeline: STT/TTS DAG
- Axes: hw:gb10, hw:npu, mem:shared-273, zero-copy, placement
- Scenario: Moving the conv-codec/encoder off the GPU onto the NPU frees ~273 GB/s LPDDR bandwidth the AR streams were contending for.
- System must: Place codec/encoder on the NPU so the freed bandwidth admits ≥1.3× more AR streams (the M4 accept criterion) — provided admission budgets the shared pool so the split doesn't oversubscribe the one ceiling.
- If mishandled: The codec stays on the GPU and caps AR concurrency far below the device's potential.

### HW-54 — shared-bandwidth admission ledger on unified memory
- Level: compound
- Pipeline: multi-stage DAG on GB10
- Axes: hw:gb10, mem:shared-273, admission, contention
- Scenario: Several stages across GPU+NPU+CPU draw from one coherent pool; admission must test the shared bandwidth, not just per-substrate compute.
- System must: Enforce admission test (3) — `Σ bandwidth_duty ≤ S·ceiling` across all stages on the shared pool — in addition to per-substrate compute duty (2) (§6).
- If mishandled: Per-substrate compute looks fine, the shared bus is oversubscribed, and frames drop across every stage.

### HW-55 — per-substrate compute duty ledgers don't share
- Level: compound
- Pipeline: AR on GPU + encoder on NPU
- Axes: hw:gpu, hw:npu, admission, duty-ledger
- Scenario: AR streams run on the GPU and encoders on the NPU; the two substrates' compute do not contend for each other's MACs (§6).
- System must: Keep one compute-duty ledger *per substrate* — admit against each independently (NPU full ≠ GPU full) while still summing the shared bandwidth pool.
- If mishandled: A single global compute ledger rejects GPU streams because the NPU is busy, or vice versa.

### HW-56 — Apple UMA zero-copy across GPU+ANE+CPU
- Level: compound
- Pipeline: on-device STT→TTS
- Axes: hw:apple, hw:ane, mem:uma, zero-copy, placement
- Scenario: An M-series Mac runs the encoder on ANE, AR on GPU, codec on CPU; UMA (120→800 GB/s tier-dependent) is coherent across all three (§2.2).
- System must: Use `SharedHostBufType` zero-copy across the UMA, place static→ANE / dynamic→GPU, and budget the shared UMA bandwidth as one pool.
- If mishandled: Buffers are copied between engines that share the same physical memory, and the shared-BW law is ignored.

### HW-57 — Intel Core Ultra zero-copy via OpenVINO
- Level: compound
- Pipeline: edge DAG
- Axes: hw:core-ultra, hw:npu, hw:igpu, mem:lpddr5x-136-shared, zero-copy
- Scenario: A Lunar Lake laptop runs a 3-stage DAG across CPU+Xe2+NPU on one ~136.5 GB/s bus.
- System must: Zero-copy hand off across the three engines (OpenVINO), place static→NPU / dynamic→Xe2, and treat the ~136 GB/s bus as the single hardest ceiling (lowest of the unified boxes).
- If mishandled: Three engines saturate one thin bus and the DAG can't sustain even one stream.

### HW-58 — discrete GPU falls back to async copy + double-buffer
- Level: compound
- Pipeline: AR→codec on a discrete-GPU box
- Axes: hw:rtx, hw:gpu, mem:gddr-discrete, zero-copy-fallback
- Scenario: On a discrete RTX (non-coherent), the consumer can't view the producer's buffer type; zero-copy isn't available (§3.4).
- System must: Fall back to async copy + event sync + double-buffering, copying only the live slice (the per-frame `TokenFrame`), not the whole tensor.
- If mishandled: A full-tensor synchronous copy per frame stalls the pipeline on PCIe.

### HW-59 — boundary minimization across substrates
- Level: compound
- Pipeline: multi-stage DAG
- Axes: hw:mixed, placement, zero-copy, contention
- Scenario: A naive placement scatters a DAG across GPU↔NPU↔CPU↔GPU, paying a cross-substrate edge cost at each hop.
- System must: Apply rule (6) — minimize cross-substrate boundaries — grouping adjacent stages on one substrate unless an affinity/bandwidth win justifies the hop.
- If mishandled: Excess boundary crossings add per-frame copy/sync cost that eats the frame budget on discrete boxes.

### HW-60 — CPU alone can't saturate GB10's bus
- Level: compound
- Pipeline: codec on CPU + AR on GPU
- Axes: hw:gb10, hw:cpu, mem:shared-273, contention, roofline
- Scenario: The Grace CPU by itself cannot saturate ~273 GB/s; a CPU-placed codec leaves headroom for GPU AR streams (§2.2 note "CPU alone can't saturate").
- System must: Exploit the headroom — place the codec on the CPU (it can't monopolize the bus), overlapping a compute-light CPU stage with bandwidth-bound GPU AR.
- If mishandled: The scheduler assumes the CPU codec contends fully and needlessly serializes it against AR.

### HW-61 — idle SMs run static stages on GB10
- Level: compound
- Pipeline: AR + encoder co-resident
- Axes: hw:gb10, hw:gpu, simd:simt, placement, batch-knee
- Scenario: At batch 1 the GB10 GPU is mostly idle (§1.1); a static encoder could run on the idle SMs instead of a separate engine.
- System must: Treat idle-SM capacity as a placement target for static conv stages (GPU=dynamic; idle-SMs=static, §2.2) — but still budget the shared bus they pull from.
- If mishandled: Idle SMs sit unused while a static stage waits on a busy substrate.

### HW-62 — overlap memory-bound AR with compute-bound conv-codec
- Level: compound
- Pipeline: AR + conv codec
- Axes: hw:gb10, mem:shared-273, contention, roofline
- Scenario: AR decode (memory-bound) and a small conv-codec (compute-bound) are candidates to co-run on the shared pool.
- System must: Overlap them — the complementary bottlenecks (one bandwidth, one compute) co-exist without dividing the bandwidth ceiling (the explicit §3.4 preference).
- If mishandled: They are serialized unnecessarily, halving throughput that the bottleneck asymmetry would have allowed.

### HW-63 — cohort by (model, frame-rate); never lockstep-mix clocks
- Level: compound
- Pipeline: mixed-model GPU
- Axes: hw:gpu, simd:simt, frame-rate, batch-knee
- Scenario: A 12.5 Hz Mimi stream and a 75 Hz codec stream share a GPU; they have no common realtime tick (§4.2).
- System must: Batch by (model, frame-rate) cohort and share the GPU *temporally* via the duty ledger — never fuse two frame-rate clocks into one lockstep step.
- If mishandled: A fused lockstep step paces both at the wrong clock; one underruns or over-generates.

### HW-64 — prefill spike must not break the frame cadence
- Level: compound
- Pipeline: admitting a new stream
- Axes: hw:gpu, simd:simt, prefill, contention
- Scenario: A new stream's prefill is mixed into the decode batch; naive prefill+decode hybrid inflates per-token TBT up to 28.3× = 17–22 dropped frames at an 80 ms budget (§4.5).
- System must: Admit ≤1 new stream's prefill per K frames and chunk prefill to one frame-budget's tokens (Sarathi token budget keyed on the audio frame deadline).
- If mishandled: One admission's prefill spike causes total dropout for every co-resident stream.

### HW-65 — heterogeneous parallelism: AR on GPU ∥ codec on NPU
- Level: compound
- Pipeline: AR→codec DAG
- Axes: hw:gpu, hw:npu, placement, zero-copy, pipeline-overlap
- Scenario: The AR thread lockstep-ticks B streams on the GPU while the codec thread micro-batches their frames on the NPU — real parallelism, not temporal interleaving (§3.2).
- System must: Run AR on GPU ∥ codec on NPU with zero-copy `TokenFrame` handoff and a bounded inter-stage queue (back-pressure parks the upstream stage, never drops).
- If mishandled: Putting both on one engine forces temporal interleaving and loses the heterogeneous speedup.

### HW-66 — admission tests the bottleneck stage, not the AR stage
- Level: compound
- Pipeline: AR + CFM/codec DAG
- Axes: hw:gpu, admission, duty-ledger, contention
- Scenario: The CFM/vocoder is the binding stage (often slower than AR), but admission only checked the AR stage's slots (§3.2, §6).
- System must: Test the bottleneck stage in admission (every stage carries its own SLO + duty entry); a full downstream queue parks AR, so admitting on AR-only over-commits the bottleneck.
- If mishandled: Streams admitted on AR capacity overflow the codec/CFM queue and glitch (RFC #2568 audio gaps).

### HW-67 — codec micro-batch must not inherit the AR batch size
- Level: compound
- Pipeline: AR (B≥4) + codec (B=1)
- Axes: hw:gpu, simd:simt, batch-knee, pipeline-overlap
- Scenario: The AR stage wants `max_num_seqs ≥ 4` to pipeline; the codec stage typically uses 1 — a uniform default causes audio gaps because the codec window round-robins (§3.2, catalog C6).
- System must: Pin per-stage batch sizes independently (AR≥4, codec=1) in the stage schema defaults — decoupled per-stage batching.
- If mishandled: The codec inherits B≥4, its window round-robins across requests, and every stream gets gaps under load.

### HW-68 — fused vs separate stage by feedback tightness
- Level: compound
- Pipeline: CosyVoice2 (3-node) vs dots.tts (2-node)
- Axes: hw:gpu, simd:simt, nested, placement
- Scenario: AR→code-predictor is tight (per-frame feedback) but talker→chunk-CFM→vocoder is loose (consumes completed chunks) (§3.3).
- System must: Fuse tight feedback into one node (dots.tts = `ar_talker{nested cfm} → audiovae`) and split loose feedback into separate nodes (CosyVoice2 = `ar_semantic → cfm_chunk → vocoder`) — same engine, expressed as data.
- If mishandled: Splitting a tight inner loop balloons per-step latency; fusing a loose chunk consumer blocks the pipeline.

### HW-69 — back-pressure parks upstream, never drops frames
- Level: compound
- Pipeline: AR→codec under load
- Axes: hw:gpu, pipeline-overlap, contention, admission
- Scenario: The codec queue fills; the AR stage keeps producing `TokenFrame`s.
- System must: Park (back-pressure) the AR stage on the bounded queue — never drop frames; admission must have already tested the bottleneck so parking is rare (§3.2).
- If mishandled: Either frames are dropped (audible glitch) or the queue grows unbounded (latency blow-up).

### HW-70 — two clocks: don't off-clock-batch the codec onto the AR tick
- Level: compound
- Pipeline: TTS multi-rate
- Axes: hw:gpu, hw:cpu, frame-rate, sample-rate, placement
- Scenario: The AR step (frame clock) and codec decode + resample (post-batch, off-clock) are conflated into one batched loop.
- System must: Keep the AR lockstep on the frame clock and run codec-decode + resample as separate post-batch stages off that clock (§5.1) — co-located only temporally.
- If mishandled: Codec decode paced to the AR tick stalls the AR batcher or mis-paces audio.

### HW-71 — DC-only: disaggregate prefill to a side pool
- Level: compound
- Pipeline: DC TTS at scale
- Axes: hw:b200, hw:gpu, prefill, mem:hbm
- Scenario: At DC scale, prefill spikes contend with decode on the same GPU; a side prefill pool (DistServe-style, ~20–30 ms KV transfer) is available (§4.5).
- System must: Disaggregate prefill to a side pool *only at DC scale* (the edge never pays this machinery, §8) — the KV transfer cost is justified only when prefill volume is high.
- If mishandled: Edge inherits DC prefill-disaggregation overhead for a single stream.

### HW-72 — DC spill: Llumnix constant-time KV migration
- Level: compound
- Pipeline: DC multi-replica
- Axes: hw:gpu, mem:hbm, spill, contention
- Scenario: One B200 replica saturates while a sibling has headroom; a stream must move without a glitch (§6).
- System must: Use Llumnix-style constant-time (~20–30 ms) append-only KV migration to rebalance across replicas — but one decode-step > one frame, so the migration drop must be playback-buffer-masked.
- If mishandled: Mid-stream migration drops ≥1 frame audibly, or rejects rather than rebalancing.

### HW-73 — cross-substrate edge type chooses the relay
- Level: compound
- Pipeline: AR→codec→vocoder DAG
- Axes: hw:mixed, zero-copy, placement, mem:any
- Scenario: Edges carry `TokenFrame` (AR→codec), `LatentChunk` (semantic→CFM), or `WholeTensor` (encoder→decoder); each crosses a substrate boundary differently (§3.1, §3.4).
- System must: Choose the relay per edge type and substrate pair — zero-copy alias on coherent memory, live-slice async copy on discrete — copying only what crosses (the per-frame frame, not the whole tensor).
- If mishandled: A `WholeTensor` edge is copied every frame instead of once, or a `TokenFrame` triggers a full-buffer DMA.

### HW-74 — duty ledger tie-break on current load
- Level: compound
- Pipeline: placement with two viable substrates
- Axes: hw:gpu, hw:npu, placement, duty-ledger
- Scenario: A static stage could run on the NPU or idle GPU SMs; both satisfy the capability predicate.
- System must: Tie-break via the duty ledger (rule 5) — place on whichever substrate currently has more bandwidth/compute headroom, respecting any manual `substrate` pin (never overridden).
- If mishandled: Both viable substrates get loaded blindly; one saturates while the other idles.

### HW-75 — manual substrate pin is never overridden
- Level: compound
- Pipeline: any placed DAG
- Axes: hw:any, placement, config
- Scenario: An operator pins a stage to `substrate=npu` for a known-good reason; the placer's affinity rules would have chosen the GPU.
- System must: Honor the manual pin unconditionally (§3.4) — the placer decides only when the hint is `any`.
- If mishandled: The placer overrides an explicit operator decision and breaks a validated deployment.

### HW-76 — compute-bound ∥ bandwidth-bound is the safe overlap
- Level: compound
- Pipeline: encoder (compute) + AR (bandwidth)
- Axes: hw:gb10, mem:shared-273, contention, roofline
- Scenario: A compute-bound conv encoder and bandwidth-bound AR decode are both ready to run on the shared pool.
- System must: Overlap them freely — their bottlenecks don't collide on the ~273 GB/s ceiling (the one safe concurrency on unified memory, §3.4).
- If mishandled: They're needlessly serialized, leaving both compute and bandwidth underused.

### HW-77 — RTX few-stream prosumer box sizing
- Level: compound
- Pipeline: TTS few-stream
- Axes: hw:rtx, hw:gpu, mem:gddr-24gb, batch-knee, contention
- Scenario: A prosumer single-box RTX 4090/5090 serves a handful of streams; VRAM caps the slot count, then GDDR bandwidth (~1–1.8 TB/s) the throughput.
- System must: Cap slots by VRAM first (quantize weights + KV), then by the bandwidth knee (tens of streams) — the few-stream single-box tier (§2.2).
- If mishandled: Slot count from bandwidth alone OOMs the 24 GB card.

### HW-78 — Gaudi/HPU as an alternate batched accelerator
- Level: compound
- Pipeline: batched STT/TTS
- Axes: hw:gaudi, hw:hpu, simd:systolic+vector, batch-knee, mem:hbm
- Scenario: A deployment targets Intel Gaudi/HPU (MME systolic + TPC vector engines, HBM) instead of CUDA; it's a batched accelerator with its own EP.
- System must: Route via the Gaudi EP through the `Backend` trait (runtime-probe → one active backend), batch wide for the systolic MME, and keep the portable `forward_native` floor for unsupported ops.
- If mishandled: A CUDA-only assumption blocks the HPU path; ops with no HPU kernel error instead of degrading.

### HW-79 — sniffer/placer must be cycle-safe on the DAG
- Level: compound
- Pipeline: DAG with shared tensor leaves
- Axes: hw:any, placement, zero-copy
- Scenario: A content-sniffer walks the DAG payload to decide placement/zero-copy and the tensor graph has shared leaves (catalog G10, prior WaaV sniffer scar).
- System must: Carry a `seen` set in any payload walk (cycle-safe) — directly matching the prior CRITICAL sniffer false-positive scar.
- If mishandled: An infinite loop or a false-positive misroutes a stage's placement.

### HW-80 — same-process fan-out clones the owned buffer
- Level: compound
- Pipeline: DAG fan-out (1→N stages)
- Axes: hw:any, zero-copy, placement, mem:any
- Scenario: One stage's owned `Payload` fans out to N downstream stages on the same process; sharing it by `Arc<Mutex<>>` reintroduces the aliasing bug (catalog G5).
- System must: Move ownership across in-process channels and clone-on-fan-out the owned container, sharing `Arc` only for immutable tensor leaves (the borrow checker enforces this for free if you don't reach for `Arc<Mutex>`).
- If mishandled: A mutation in one fan-out branch corrupts the buffer the others read.

---

## EXTREME — heterogeneous box under a frame deadline; degenerate physics

### HW-81 — the full heterogeneous box: AR on GPU, codec on CPU-AMX, STT-encoder on NPU, one LPDDR bus, one frame deadline
- Level: extreme
- Pipeline: STT-encoder → AR-TTS → codec, three substrates
- Axes: hw:gb10, hw:cpu-amx, hw:npu, mem:lpddr5x-273-shared, simd:simt+amx+systolic, contention, zero-copy
- Scenario: A single box (GB10 iGPU + Grace CPU + a hypothetical NPU) runs AR-TTS on the GPU, the codec on CPU-AMX, and a Whisper encoder on the NPU — all three contending one ~273 GB/s LPDDR bus under an 80 ms frame deadline.
- System must: Place each stage on its best engine (AR→GPU dynamic, codec→CPU-AMX, encoder→NPU systolic), zero-copy hand off across the coherent pool, and run the shared-bandwidth admission ledger so the *aggregate* bandwidth duty ≤ 0.8·ceiling — overlapping the compute-bound encoder/codec with bandwidth-bound AR, serializing any two bandwidth-bound stages.
- If mishandled: Three engines saturate the one bus simultaneously, every stage divides the ceiling, and the whole box underruns at once.

### HW-82 — two bandwidth-bound stages on a coherent pool exceed the ceiling
- Level: extreme
- Pipeline: AR-TTS + AR-STT decode co-resident
- Axes: hw:gb10, mem:lpddr5x-273-shared, contention, roofline, batch-knee
- Scenario: Both an AR-TTS decode and an AR-STT decode (both memory-bandwidth-bound) are admitted onto GB10's single 273 GB/s pool; together they demand >273 GB/s.
- System must: Detect that both are bandwidth-bound and serialize them (time-share via the duty ledger) — the contention law forbids two bandwidth-bound stages concurrently on one pool; reject the second's admission if serialization breaks its frame budget.
- If mishandled: Both run, each gets ~136 GB/s, both double their step time, and both glitch (the exact §3.4 contention failure).

### HW-83 — EP cascade fallback under a live frame deadline
- Level: extreme
- Pipeline: AR decode, GPU kernel fails mid-utterance
- Axes: hw:gb10, hw:gpu, hw:cpu, ep-fallback, simd:simt→neon, contention
- Scenario: Mid-utterance the CUDA-graph replay errors (sm_120 hang, catalog H4) on an active GB10 stream with a hard frame deadline.
- System must: Auto-downgrade that step to eager (non-optional on sm_120), and if the GPU is wholly lost, degrade the op to the CPU floor + telemetry — but the CPU can't sustain 0.5B AR realtime, so the correct response is reject/relegate the stream to a degraded queue, not silently miss frames.
- If mishandled: The graph-hang either crash-loops the server or the CPU "fallback" underruns every frame with no signal.

### HW-84 — heterogeneous combo where the immovable weights conflict
- Level: extreme
- Pipeline: AR + codec + encoder, limited GPU VRAM
- Axes: hw:gb10, hw:gpu, hw:npu, hw:cpu, placement, mem:unified, contention
- Scenario: AR's 3–6 GB weights pin it to the GPU, but the GPU VRAM slice is nearly full; the codec and encoder want the GPU too, yet only the AR weights fit there.
- System must: Pin AR to the GPU (follow immovable weights), evict codec→CPU-AMX and encoder→NPU by affinity, and verify the resulting cross-substrate bandwidth fits the shared ledger — placement is constrained by *both* weight residency and the shared-bus budget.
- If mishandled: The placer tries to co-locate everything on the GPU, OOMs VRAM, or oversubscribes the bus offloading.

### HW-85 — nested AR+variable-NFE inner head batched across a heterogeneous cohort
- Level: extreme
- Pipeline: FlashTTS-class (MTP-3 + 2-NFE meanflow head)
- Axes: hw:gpu, simd:simt, nested, variable-stride, batch-knee
- Scenario: A production model advances by MTP-3 (3 tokens/step) with an inner 2-NFE meanflow head — breaking both batchers in one model (catalog L5); streams may run different inner NFE.
- System must: Generalize lockstep to "advance a model-dependent variable stride" and compose the inner variable-NFE micro-batch *inside* one AR step across the B slots at the same inner step — two batchers composed per step, not one picked; only cohort streams sharing the inner NFE schedule.
- If mishandled: The fixed-stride lockstep mis-paces the patch-AR, and mixed-NFE streams can't share a tick → desync or wrong-step kernels.

### HW-86 — dynamic-frame-rate codec defeats fixed cohorting on shared hardware
- Level: extreme
- Pipeline: TTS with FlexiCodec (3–12.5 Hz, data-dependent per-frame)
- Axes: hw:gpu, simd:simt, frame-rate, batch-knee, contention
- Scenario: FlexiCodec's frame-rate is data-dependent per-utterance *and* per-frame, not known a-priori (catalog L6); streams in one cohort drift to different instantaneous rates.
- System must: Let lockstep advance a variable stride and tolerate an unknown-a-priori rate in the cohort key — re-cohort or stride-adapt per frame rather than assuming a fixed (model, frame-rate) constant.
- If mishandled: A fixed-rate cohort assumption pins all streams to one clock and the data-dependent ones underrun or over-emit.

### HW-87 — hybrid KV: radix prefix-cache on shared ref-audio + ring suffix
- Level: extreme
- Pipeline: multi-tenant cloned-voice TTS
- Axes: hw:gpu, mem:hbm, kv-hybrid, batch-knee, contention
- Scenario: A multi-tenant agent reuses one cloned voice across calls; a fixed per-slot ring recomputes the ref-audio/system-prompt KV every request, forfeiting ~86% cacheable work (catalog L1/L2, Fish S2).
- System must: Run a hybrid KV — a radix/paged prefix-cache for the deterministic ref/system prefix plus a ring for the per-utterance suffix — and fingerprint the injected conditioning (blake2b over all codebooks) into the cache key so different ref-audios don't cross-contaminate (catalog G1).
- If mishandled: Every request recomputes the shared prefix (massive waste) or RadixAttention matches on token-ids alone and emits the wrong voice under concurrency.

### HW-88 — intra-node spatial P/D vs chunked-prefill on GB10
- Level: extreme
- Pipeline: TTS with prefill + decode contention
- Axes: hw:gb10, hw:gpu, simd:simt, prefill, contention, roofline
- Scenario: Chunked prefill mixed into the decode batch causes an ~8× TBT tail spike (catalog L4, Nexus 250 ms vs 15 ms); intra-node SM-partition P/D could avoid it on one GPU.
- System must: Treat intra-node spatial P/D (SM partitioning) as a measured option against the chunked-prefill firewall — the frame-deadline metric (strict TPOT/relaxed TTFT) is exactly where spatial P/D wins; A/B it on GB10.
- If mishandled: Chunked prefill is assumed optimal, paying an 8× TBT spike that SM-partitioning would have avoided.

### HW-89 — masked idle slots are NOT free under heterogeneous residency
- Level: extreme
- Pipeline: lockstep batch with idle/barge-in slots
- Axes: hw:gpu, simd:simt, batch-knee, contention, mem:bandwidth
- Scenario: Under variable residency (barge-in, EOS, VAD-silence) a lockstep batch carries many masked-idle slots; the dense kernel still reads/writes every row, and idle-lane energy is ~48% of serving energy (catalog L8).
- System must: Either compact/repack active slots into a smaller live batch *or* explicitly budget the masked-slot bandwidth/energy cost in the duty ledger — masked rows are dense-kernel work, not zero-cost, when residency is heterogeneous.
- If mishandled: The slowest stream paces all, idle lanes burn bandwidth/energy, and the throughput model overstates capacity.

### HW-90 — KV-quant empty-dtype graph-driven across substrates under load
- Level: extreme
- Pipeline: q4f16 STT on CUDA, fp16 KV empties
- Axes: hw:gpu, precision:q4f16, ep:ort-cuda, mem:hbm, contention
- Scenario: A q4f16 model on CUDA needs enc pkv + past_padding + dec zero_past as f16 while feature/embed inputs stay f32, serving many concurrent streams (memory: voxtral q4f16).
- System must: Drive every empty-tensor KV dtype from `StaticGraph::input_types()` (graph-driven, generalizable) so the q4f16 graph is fed correctly per slot at scale — zero-code beyond the weight swap, and the only safe int8/4-on-CUDA path is via this f16 KV (ORT-CUDA can't int8-GEMM).
- If mishandled: A single hard-coded f32 KV empty mis-types the q4f16 graph and corrupts/crashes every concurrent stream.

### HW-91 — long-form context breaks the bounded-ring assumption mid-stream
- Level: extreme
- Pipeline: long-audio STT / many-turn agent
- Axes: hw:gpu, mem:hbm, kv-hybrid, frame-rate, contention
- Scenario: A 10-minute stream reaches 30k+ tokens; the fixed ring silently forgets (StreamingLLM wraparound instability), and generic LLM-KV eviction fails on audio (catalog L12).
- System must: Pin attention-sink tokens + provide a paged/full-context escape hatch for long-form — the bounded-ring "context is bounded" only holds ≤~4 min; beyond that the ring is lossy and needs the hybrid/paged path.
- If mishandled: A long call silently loses early context (wrong transcript / drifting voice) with no error.

### HW-92 — NaN logit on a heterogeneous AR step → reject the frame
- Level: extreme
- Pipeline: AR decode (sampler-free argmax)
- Axes: hw:gpu, simd:simt, numerics, contention
- Scenario: An fp16/quant numeric overflow produces a NaN logit; the argmax-based sampler can't raise and argmaxes the NaN to a garbage codec token = audible pop (catalog H1).
- System must: Run an always-on `logits.isnan().any()` reduction and reject-frame (repeat prev / codec-silence / greedy-resample) — the single most important numeric inversion; prefer bf16 over fp16 to avoid the >65504 overflow that creates the NaN.
- If mishandled: The NaN argmaxes to garbage, popping audio with zero error signal.

### HW-93 — masked-row input substitution kills the whole batch if skipped
- Level: extreme
- Pipeline: lockstep AR with idle slots
- Axes: hw:gpu, simd:simt, batch-knee, numerics
- Scenario: A 64-slot batch has idle/warming rows; without substituting a valid BOS token before the forward, the KV-gather reads sentinel −2/stale → CUDA illegal-memory/NaN that kills all 64 users (catalog F1).
- System must: Force masked-or-warming rows to the `initial`/BOS token via `where(is_init, initial, gathered)` before embedding (MASKED ≠ ABSENT) — make the masked row's data harmless, never skip it.
- If mishandled: One idle slot's sentinel poisons the dense kernel and takes down every concurrent stream on the device.

### HW-94 — transactional slot recycling prevents cross-user KV leak
- Level: extreme
- Pipeline: lockstep, slot reused across users
- Axes: hw:gpu, simd:simt, mem:hbm, privacy, batch-knee
- Scenario: User in slot 7 disconnects; a new user is admitted into slot 7 — without reset, their attention sees the old user's KV + word buffers = cross-user transcript contamination (catalog F3, a privacy disaster).
- System must: Run one transactional `reset_slot(i)` fanning out to KV pointers + conv rings + sampler + word buffers + offset + host state, with a monotonic `channel_id` dropping any output whose id ≠ live occupant — correctness relies on positions/indices=0 + mask making stale bytes unreachable.
- If mishandled: A recycled slot leaks one caller's content into another's output.

### HW-95 — ring-KV wraparound masks future tokens on a heterogeneous cohort
- Level: extreme
- Pipeline: long lockstep utterance
- Axes: hw:gpu, simd:simt, mem:hbm, batch-knee
- Scenario: Once `offset > context`, physical slot order ≠ time order; a naive causal mask `j≤i` attends to FUTURE tokens in recycled ring cells (catalog F4).
- System must: Store the logical position per cell and mask by `pos ≤ my_pos` (causal) AND window AND never-written⇒−1 (the Kyutai test vectors become unit tests) — the ring wrap is only correct because the mask makes stale cells unreachable.
- If mishandled: The model attends to future/garbage cells after wraparound, corrupting output on long streams.

### HW-96 — degenerate edge: N=1 inline mode pays no DC machinery
- Level: extreme
- Pipeline: single-stream edge
- Axes: hw:gb10, hw:cpu, mem:any, batch-knee, ep-fallback
- Scenario: `mode=edge`, one stream, no co-tenant; the full §6 scheduler (queues, tick loop, admission, ledger) would be pure overhead.
- System must: Run Inline mode — all stages inline on the calling thread in DAG order, B=1, no queues/tick/admission/ledger; nested loops still in-forward (§8). The same stage-forward runs with B=1; there is no second implementation.
- If mishandled: The edge box pays DC scheduling overhead it can never amortize at N=1.

### HW-97 — lazy promotion edge→stage-batched when a 2nd stream arrives
- Level: extreme
- Pipeline: edge box that gains a second caller
- Axes: hw:gb10, hw:gpu, batch-knee, admission, contention
- Scenario: A box running Inline (1 stream) suddenly gets a 2nd concurrent stream or a co-tenant model load (`mode=auto`).
- System must: Promote lazily to Stage-batched mode (the ledger spins up on demand) — the DAG/stages/placement hints are identical across modes; only the executor changes (§8).
- If mishandled: The box stays Inline and serializes the second stream, or pre-pays DC machinery before the trigger.

### HW-98 — config-scale the same DAG from GB10 to B200
- Level: extreme
- Pipeline: one manifest, two substrates
- Axes: hw:gb10, hw:b200, mem:lpddr5x-273↔hbm3e-8tb, precision:bf16↔fp8, batch-knee
- Scenario: The same model contract must run GB10 (273 GB/s, latency-only, N=1, bf16+CUDA-graph) and B200 (~8 TB/s, big lockstep N, fp8/mxfp4, push toward compute-bound) (§8).
- System must: Change only the batch ceiling and precision tier — same lockstep loop, same DAG, same nested loops; the substrate sets the ceiling, not the design.
- If mishandled: Two separate code paths diverge; the GB10 build inherits B200 batch sizes (overrun) or vice versa.

### HW-99 — startup feasibility check: reserve graph-pool delta before admitting
- Level: extreme
- Pipeline: GB10 boot with CUDA-graph
- Axes: hw:gb10, hw:sm120, simd:simt, mem:hbm, admission
- Scenario: CUDA-graph capture-OOMs *after* /health passes (catalog H4, #44209 sm_120 crash-loop); the graph pool delta wasn't reserved.
- System must: Reserve the CUDA-graph-pool delta and run a pre-capture feasibility check at boot (gpu_mem_util 0.90–0.92 after a per-stage profile-run) — fail at boot, not at request-1; readiness gates on warmup+calibration complete, not process-up.
- If mishandled: The server reports ready, then crash-loops on the first request's capture OOM.

### HW-100 — warm-up forces graph capture off the hot path
- Level: extreme
- Pipeline: cold GB10 start
- Axes: hw:gb10, hw:gpu, simd:simt, batch-knee, ep-fallback
- Scenario: A cold server captures the CUDA-graph on the first real request, paying seconds of capture latency on a live stream (catalog F6).
- System must: Warm up 2–3 steps with a full mask + `synchronize()` at startup — fills conv/KV boundary state and forces capture OFF the hot path; gate readiness on this completing.
- If mishandled: The first caller eats seconds of graph-capture latency mid-stream.

### HW-101 — heterogeneous box, GPU-fault recovery pins VRAM into the next process
- Level: extreme
- Pipeline: GPU sidecar crash on a heterogeneous box
- Axes: hw:gb10, hw:gpu, mem:hbm, ep-fallback, crash-recovery
- Scenario: The parent is SIGKILLed; the orphaned GPU sidecar (thread blocked inside a CUDA kernel) never polls a death-pipe and pins VRAM into the next process (catalog H7, #34643).
- System must: Set `PR_SET_PDEATHSIG` at worker entry (kernel-guaranteed even under SIGKILL), abort collectives before destroy, and order teardown SIGTERM→5s→SIGTERM→4s→SIGKILL — a death-pipe Event alone fails when a thread is in a kernel.
- If mishandled: A killed parent leaks the GPU's VRAM, breaking the next deployment on the box.

### HW-102 — progress watchdog keyed on last-audio-emitted, device-aware deadline
- Level: extreme
- Pipeline: heterogeneous box, stalled GPU stage
- Axes: hw:gb10, hw:gpu, hw:cpu, crash-recovery, contention
- Scenario: An AR stage is "alive but zero forward progress" (passes every health check); the per-inference deadline must differ by substrate (CPU needs 3600 s, a 1.5B AR-TTS step ≠ a CTC step) (catalog H9, #45135).
- System must: Track monotonic "last-audio-emitted-at" per session on an independent thread and kill/restart the sidecar if no audio for >N×frame-interval — with a DEVICE+MODEL-aware deadline, not a flat timeout.
- If mishandled: A wedged GPU stage passes health forever while audio silently stops; or a CPU stage is killed under a too-tight GPU-tuned deadline.

### HW-103 — zero-copy on coherent memory still costs shared bandwidth
- Level: extreme
- Pipeline: AR + codec + encoder zero-copy chain on GB10
- Axes: hw:gb10, mem:lpddr5x-273-shared, zero-copy, contention, roofline
- Scenario: Three stages hand off zero-copy on GB10's coherent pool; the transfer cost is gone but each stage still reads/writes the shared ~273 GB/s for its own compute.
- System must: Remember zero-copy removes the *transfer* cost, not the *shared-bandwidth* cost — treat aggregate memory bandwidth as a budgeted schedulable resource (§3.4 contention guard); the win is "placement frees the GPU bus," not "free concurrency."
- If mishandled: The scheduler assumes zero-copy means zero contention and oversubscribes the one ceiling.

### HW-104 — codec stage offloaded under a frame deadline while AR runs hot
- Level: extreme
- Pipeline: AR on GPU + codec offloaded to CPU mid-frame
- Axes: hw:gb10, hw:gpu, hw:cpu-amx, codec, placement, contention
- Scenario: To free GPU bandwidth for more AR streams, the terminal codec is offloaded to CPU-AMX, but the codec must still emit within the frame deadline so audio isn't late.
- System must: Offload the codec (the safe terminal node) to CPU/another EP with a windowed decode (left-context + crossfade) sized to meet the frame deadline — the highest-value cross-model dedup point (Mimi/DAC/HiggsV2 shared) — while admission budgets the freed-then-reused bandwidth.
- If mishandled: The offloaded codec misses the frame deadline and audio arrives late despite the freed GPU.

### HW-105 — heterogeneous cohort: same model, same frame-rate, freely co-batchable; mixed → temporal-only
- Level: extreme
- Pipeline: mixed-model multi-tenant GPU
- Axes: hw:gpu, simd:simt, frame-rate, batch-knee, contention
- Scenario: Two streams of the same model+frame-rate plus a third of a different frame-rate share one GPU.
- System must: Co-batch the same-(model,frame-rate) pair in one lockstep step and share the GPU with the third *temporally* via the duty ledger — same model ⟹ same frame-rate ⟹ freely co-batchable; different clocks never fuse (§4.2).
- If mishandled: The third stream is forced into the wrong-clock lockstep step and underruns or over-generates.

### HW-106 — admission rejects rather than glitches at saturation on the bottleneck substrate
- Level: extreme
- Pipeline: heterogeneous DAG at saturation
- Axes: hw:gb10, hw:gpu, hw:npu, admission, contention
- Scenario: The box is at the M4 accept boundary — the next stream would break the frame budget on the bottleneck stage or oversubscribe the shared bus.
- System must: Reject (typed 429/503 + Retry-After) rather than admit-and-degrade — the two-level test (per-substrate compute duty + shared-bandwidth duty + bottleneck-stage slots); shed is the backstop, admission is the mechanism (§6).
- If mishandled: Admit-and-degrade drops frames for *every* co-resident stream instead of cleanly rejecting one.

### HW-107 — graceful overload: relegate to a degraded queue, don't drop frames for everyone
- Level: extreme
- Pipeline: 50% overload on a shared box
- Axes: hw:gpu, admission, contention, crash-recovery
- Scenario: Sustained load pushes p99 past the bottleneck-stage SLO; naively dropping frames hits all streams (Niyama: 8.6% vs 80% SLO violations at 50% overload) (§6, catalog L9).
- System must: Stop admitting → shed Batch → only then shed newest Realtime ≤1/tick with 60 s hysteresis, relegating to a degraded queue — deadline-aware admission as PRIMARY, hard reject only at true saturation, cadence protected by the client playback buffer.
- If mishandled: Frames drop for everyone at overload instead of cleanly relegating the marginal streams.

### HW-108 — wall-clock pacing on a heterogeneous tick (don't busy-spin, don't starve co-located stages)
- Level: extreme
- Pipeline: lockstep AR ∥ co-located encoder
- Axes: hw:gb10, hw:cpu, simd:simt, contention, batch-knee
- Scenario: A busy AR loop on one core starves a co-located encoder forward (SGLang: ~600× slowdown from GIL-starvation; the Rust analog is core/runtime starvation) (catalog G3, F6).
- System must: Apply admissions/resets/control-plane first, compute the exec-mask, run the kernel only if `exec_mask.any()` else short-sleep 1–2 ms — block on `recv_timeout`/`Notify` when idle, hog the core only when busy (wall-clock paced).
- If mishandled: The AR loop pins a core and the co-located encoder slows to a crawl, collapsing STT throughput.

### HW-109 — heterogeneous fan-in deadlock when a branch won't fire
- Level: extreme
- Pipeline: text+audio S2S DAG (conditional branch)
- Axes: hw:gpu, hw:cpu, placement, crash-recovery
- Scenario: A vocoder waits for a fixed `[text, audio_encoder]` set, but a text-only request never produces audio-encoder output → deadlock (catalog G11).
- System must: Compute the expected source set per request via `wait_for_fn` (dynamic fan-in), constrain routing to the static topology, and support multi-terminal merge (text-only vs text+audio) — a request narrows its own terminals.
- If mishandled: A conditional branch that won't fire hangs the merge stage forever on the box.

### HW-110 — full-duplex S2S on one box: simultaneous ingest + emit per frame, barge-in within a tick
- Level: extreme
- Pipeline: Moshi-class full-duplex S2S
- Axes: hw:gpu, simd:simt, full-duplex, batch-knee, mem:hbm
- Scenario: A full-duplex Moshi-class model must, per frame, ingest user Mimi tokens into input slots and emit Moshi tokens from output slots simultaneously, with barge-in cancelling the LLM within ≤1 tick (§9, catalog F-series).
- System must: Run the RQ-Transformer depth decoder + multistream/delay engine + full-duplex I/O on the lockstep batcher (the user stream is always modeled), freeing the stream's slot/KV/window within ≤1 tick on a barge-in control message.
- If mishandled: Ingest and emit desync (the user stream isn't modeled), or barge-in doesn't cancel within a frame and the model talks over the user.

### HW-111 — acoustic-delay per-codebook ring off-by-one on a heterogeneous step
- Level: extreme
- Pipeline: full-duplex / multistream codec-AR
- Axes: hw:gpu, simd:simt, full-duplex, mem:hbm, numerics
- Scenario: A per-codebook delay ring sized `max_delay+1` collides the max-delay write with the oldest read; before `step < acoustic_delay` no real acoustic token exists yet (catalog F8).
- System must: Size the per-codebook ring `max_delay+2` (the +2 = off-by-one guard), write `(offset+delays[k])%CT`, read `(offset−max_delay+gen_delays[k])%CT`, and teacher-force codebooks≥1 to PAD in the warm-up window.
- If mishandled: The delay write/read collide or read a not-yet-existing acoustic token, corrupting the multistream output.

### HW-112 — future-step marker/flush so non-streaming-over-streaming-core isn't truncated
- Level: extreme
- Pipeline: one-shot POST over a streaming lockstep core
- Axes: hw:gpu, simd:simt, full-duplex, crash-recovery
- Scenario: A one-shot transcribe over the streaming core terminates on input-exhaustion before the delayed model emits the last words → truncated transcript (catalog F5).
- System must: Append real audio + a step-ordered marker + ~10 s trailing silence, terminate on the MARKER not input-exhaustion, and free the slot only after `offset ≥ real_end` (never on disconnect alone — the tail is still draining).
- If mishandled: The transcript is truncated at the clip's nominal end, dropping the delayed tail.

### HW-113 — slot-leak-on-disconnect across a heterogeneous lifecycle
- Level: extreme
- Pipeline: lockstep under flaky connections
- Axes: hw:gpu, simd:simt, crash-recovery, batch-knee
- Scenario: A WS client vanishes; relying solely on a disconnect callback (which can be missed) leaks the slot, KV, and codec window (catalog F9).
- System must: Free the slot from inside the step loop on ANY of {receiver closed, sender disconnected, send error, ping-timeout 20 s, idle-timeout 120 s}, exposing a used/total_slots gauge — multi-trigger, never single-callback.
- If mishandled: Slots leak under churn until the box rejects all new streams despite no real load.

### HW-114 — fp32 sampler/CFM numerics regardless of model dtype on every substrate
- Level: extreme
- Pipeline: AR sampler + CFM solver
- Axes: hw:gpu, hw:cpu, precision:fp32-sampler, numerics, simd:simt+amx
- Scenario: A bf16/fp16 model runs its sampler/CFM/ODE math in the model dtype; fp16 overflows >65504 → inf→NaN, tiny-temp exp overflows (catalog H5).
- System must: Run all sampler/CFM/ODE math in fp32 regardless of model dtype, prefer bf16 over fp16 for long-context attention, and apply the four guards (`_SAMPLING_EPS=1e-5`, `_MAX_TEMP` clamp, ≥1-survivor, NaN-safe pivot via `not (pivot < max)`).
- If mishandled: The sampler NaNs or div0s on a substrate where the model dtype overflows, popping audio.

### HW-115 — sampler must be graph-safe or run outside the captured region
- Level: extreme
- Pipeline: TTS AR with CUDA-graph
- Axes: hw:gpu, simd:simt, numerics, batch-knee
- Scenario: The lockstep step is CUDA-graphed for the batch-1 edge win, but TTS sampling needs `multinomial` (not graph-safe; only `argmax` is) (catalog C2/F7).
- System must: Sample OUTSIDE the captured region or use a graph-safe gumbel-argmax inside — and wrap graphed callables with a shape+scalar-identity assert that converts silent-stale-graph corruption into a loud crash.
- If mishandled: Graph capture silently breaks sampling (deterministic/garbage output) or forces eager and loses the edge win.

### HW-116 — Path-B sidecar per-slot state crosstalk under concurrent load
- Level: extreme
- Pipeline: torch sidecar holding Python codec/window state
- Axes: hw:gpu, ep:torch, mem:host, crash-recovery, batch-knee
- Scenario: The torch sidecar caches streaming generators / codec buffers / sliding-window pads / CUDA-graph state across `forward()` without keying by slot → crosstalk/truncation only under load (catalog C3/I5).
- System must: Key all sidecar per-slot state by slot-id (`self._state: dict[slot_id, State]`) and free on slot-reset — add a concurrent-crosstalk test; the Rust lockstep handles model KV per-slot but the sidecar's Python state must too.
- If mishandled: Concurrent requests cross-talk audio (one caller's voice/codec window bleeds into another's) only under load.

### HW-117 — zero D2H syncs in the heterogeneous per-frame loop
- Level: extreme
- Pipeline: torch sidecar AR/CFM/vocoder per-frame loop
- Axes: hw:gpu, ep:torch, simd:simt, contention, numerics
- Scenario: A naive sidecar calls `.item()/.cpu()/.tolist()` per step → "2400 syncs/request" → latency collapse (catalog I3/C5); the clean 9 ms/step assumption depends on this rule.
- System must: Keep every per-step loop GPU-sync-free (`dst.copy_(src)` not `dst.fill_(src.item())`, `torch.where`/masking not Python branches, `torch.compile(forward, fullgraph=False)`) and never call `empty_cache()` in the hot path — assert zero-D2H via a CUDA-event/profiler guard during decode.
- If mishandled: Per-frame D2H syncs collapse the GB10's measured 9 ms step into a stuttering mess.

### HW-118 — heterogeneous precision per stage on the same box
- Level: extreme
- Pipeline: AR (bf16) + GEMM-quant + fp32 codec on GB10
- Axes: hw:gb10, hw:gpu, hw:cpu, precision:mixed-per-stage, codec
- Scenario: On one box the AR GEMMs can tolerate int8/fp8 but norms/RoPE/sampling-head/codec must stay high-precision, and the active EP dictates which quant is even loadable (§5.2).
- System must: Apply per-component mixed precision (GEMMs quantized, norms/RoPE/codec/head fp32/bf16) resolved per-substrate (`by_substrate[ep]` so an int8 file never lands on ORT-CUDA) — two orthogonal levers (weight-quant for latency, KV-quant for streams) stacked on the fixed-slot scheduler.
- If mishandled: A uniform quant corrupts the codec/norms, or an int8 file CPU-partitions on ORT-CUDA and slows 19×.

### HW-119 — disable WS write-coalescing on a tight frame budget
- Level: extreme
- Pipeline: streaming egress on any substrate
- Axes: hw:any, transport, contention, frame-rate
- Scenario: Default WS write-coalescing adds 10s-of-ms jitter to an 80 ms frame budget (catalog F10); the engine flushes per frame.
- System must: Set `write_buffer_size(0)` on every streaming route (flush per frame), meter per-step wall time + per-stream buffer depth as first-class metrics, and have an explicit overrun policy (buffer+autoscale vs frame-drop — not free).
- If mishandled: Transport coalescing alone blows the frame budget even when inference is on time.

### HW-120 — the binary streaming-viability objective on a saturating box
- Level: extreme
- Pipeline: VoxServe-style risk-scheduled DAG
- Axes: hw:gpu, admission, contention, frame-rate
- Scenario: At saturation, once a frame will deliver in time, further latency reduction is worthless; beyond the deadline it's a violation (catalog L3, VoxServe 10–20× over vLLM/SGLang).
- System must: Schedule by a binary streaming-viability objective + risk-of-violation (soft-deadline), with per-request detokenizer-cache batching and async stage pipelining — cadence protected by the client playback buffer, no cross-replica migration needed intra-node.
- If mishandled: The scheduler over-optimizes already-viable streams' latency while at-risk streams miss their deadline.

### HW-121 — calibration measures the batch-knee per stage per substrate under co-load, without the profiler
- Level: extreme
- Pipeline: heterogeneous DAG calibration at boot
- Axes: hw:gb10, hw:gpu, hw:npu, hw:cpu, admission, contention
- Scenario: The duty ledger needs `T_step(B_active)` per stage per substrate under synthetic co-load; running calibration under torch-profiler distorts latency ("Command Buffer Full" is profiler overhead) (catalog B, §8.3b).
- System must: Measure `T_step(B_active)` per (stage, substrate) under synthetic co-load WITHOUT the profiler, exclude the first-request lazy-init, and persist keyed `sha256 × device × driver × warm-set` — the torch sidecar reports its footprint+duty at handshake.
- If mishandled: Profiler-distorted or single-stage calibration mis-sizes the knee and admission either over-commits or under-utilizes the box.

---

## Coverage

This family covers the full hardware/architecture surface of `INFER_ENGINE.md` §2 across 121 scenarios, laddered SIMPLE→INTERMEDIATE→COMPOUND→EXTREME.

- **Every substrate's batch-knee + placement (§2.2 table):** CPU x86-AMX/AVX-VNNI (HW-17) and ARM-NEON/SVE2/I8MM/Grace (HW-2, HW-49, HW-60); GPU H200 (HW-11), MI300X (HW-10), B200 (HW-4, HW-8), RTX-GDDR (HW-5, HW-28, HW-77); NPU Hexagon-HMX/VTCM (HW-3, HW-18), ANE (HW-13, HW-56), Intel-NPU/Core-Ultra (HW-20, HW-57), TPU/Edge-TPU (HW-12, HW-40); Gaudi/HPU (HW-78); GB10 (HW-1, HW-14, and most COMPOUND/EXTREME).
- **SIMD-vs-SIMT vs systolic/dataflow (§2.3):** static-conv-GOOD/dynamic-AR-BAD placement throughout (HW-6, HW-21, HW-40, HW-81); the four-front AR/systolic mismatch (HW-3, HW-40).
- **Memory hierarchy/type/bandwidth:** HBM3e 4.8–8 TB/s (HW-11, HW-4) vs LPDDR5X-273 (HW-14) vs GDDR (HW-5, HW-77) vs DDR5-AMX (HW-17); VTCM scratchpad (HW-3, HW-18); the roofline ridge-point batch-knee law (HW-7, HW-14, HW-15).
- **Unified/shared-memory + zero-copy + the shared-bandwidth contention law (§3.4):** coherent zero-copy on GB10/UMA/Core-Ultra (HW-51, HW-56, HW-57); discrete-GPU async-copy fallback (HW-58); compute-bound∥bandwidth-bound OK vs two-bandwidth-bound-MUST-serialize (HW-52, HW-62, HW-76, HW-82); zero-copy-still-costs-bandwidth (HW-103); shared-bandwidth admission ledger (HW-54, HW-55).
- **Precision×substrate (§5.2):** fp8 Hopper+/Blackwell (HW-8, HW-16); mxfp4/nvfp4 Blackwell; int8-not-via-ORT-CUDA→TRT-EP/torch (HW-22, HW-23, HW-24, HW-46, HW-47); AMX-int8 (HW-17); Hexagon-int4 (HW-18); per-component mixed + per-substrate resolution (HW-34, HW-118); q4f16 graph-driven KV dtype (HW-48, HW-90); KV-quant/GQA levers (HW-42, HW-43); accuracy gate per substrate incl. MOS (HW-32, HW-33).
- **GB10/sm120 specifics:** CUDA-graph-hangs→eager-fallback non-optional (HW-27, HW-83, HW-99, HW-100); FlashInfer banned on sm_120 (HW-25); graph helps-B1/hurts-B32 (HW-26); enforce_eager OOM escape (HW-28); GPU-fault VRAM-leak recovery (HW-101, HW-102).
- **EP fallback + degrade-to-CPU floor (P-6):** HW-19, HW-31, HW-78, HW-83.
- **Heterogeneous placement + per-stage best-engine + the contention guard:** the ggml decision order (HW-29, HW-30, HW-59, HW-74, HW-75); the headline EXTREME box (HW-81, HW-84); AR∥codec real parallelism (HW-65); bottleneck-aware admission (HW-66, HW-106); reject/relegate-don't-glitch (HW-106, HW-107, HW-120).
- **EXTREME degenerate physics + correctness-under-hardware:** masked≠absent input-substitution (HW-93), transactional slot recycle (HW-94), ring-KV wraparound (HW-95), NaN-reject-frame (HW-92), fp32 sampler (HW-114), graph-safe sampler (HW-115), zero-D2H (HW-117), sidecar crosstalk (HW-116), full-duplex/acoustic-delay (HW-110, HW-111, HW-112), and the literature-driven reframings (variable-stride lockstep HW-85, dynamic-FR codec HW-86, hybrid radix+ring KV HW-87, intra-node spatial P/D HW-88, masked-idle-not-free HW-89, long-form ring-lossy HW-91).

Deliberately deferred to sibling families (not duplicated here): pure pipeline/DAG-topology scenarios, transport/protocol, multi-tenancy/SLO policy, and model-onboarding/accuracy mechanics — referenced here only where the hardware substrate is the load-bearing axis.
