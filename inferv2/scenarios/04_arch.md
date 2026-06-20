# WaaV Infer — Scenario Catalog 04: Multi-Architecture / Model-Paradigm / Nested-Paradigm

> **Family:** how each model paradigm — and every combination of them — interacts with the engine's two batchers (frame-sync **lockstep** for AR/RNNT/MTP, **step-bucket** for flow/diffusion/masked/encoder) and the heterogeneous stage-DAG scheduler. Grounded in `INFER_ENGINE.md` §1.5 (paradigm → batch profile), §3.3 (nested in-forward), §4.2 (two batchers + nesting rule + cohort key), §4.3 (per-slot ring KV), §5 (precision/sample-rate), and the failure catalog F (Moshi lockstep traps), G (stage/relay), I (vLLM-core), L (literature: THIRD execution class, variable-NFE inner solve, FlexiCodec, MTP-not-spec-decode).
>
> Paradigms in scope: **AR codec-LM** (memory-bound, lockstep, near-linear batch 55×@64) · **flow-matching CFM** (compute-bound, N-step, CFG×2, bucket, 10×@64) · **diffusion DDPM** (25-step, 624ms@B64) · **masked-diffusion** (parallel-unmask, fixed-K) · **encoder-decoder STT** (compute-bound encoder + AR/CTC/RNNT decoder) · the **HYBRIDS** (AR-outer + flow/diffusion-inner per token; multi-AR/MTP talker+sub-talker; Moshi temporal+depth; AR + separate-diffusion-vocoder; AR-codec-LM + neural-codec-decode delay-pattern) · the **THIRD execution class** (AR-outer + generative-inner head with per-stream variable inner-NFE) · dynamic-frame-rate codecs (FlexiCodec).
>
> Levels: **SIMPLE** (single paradigm, single stream/cohort) → **INTERMEDIATE** (single paradigm under batch/precision/cohort stress) → **COMPOUND** (≥2 paradigms or nested) → **EXTREME** (heterogeneous co-residency, multiple clocks/NFEs on one box).

---

## SIMPLE — single paradigm, the batcher each one lands in

### ARCH-1 — AR codec-LM single stream rides lockstep degenerately
- Level: SIMPLE
- Pipeline: AR codec-LM TTS (Orpheus/Voxtral-TTS class) → Mimi/SNAC decode
- Axes: arch:AR, batch:1, mode:inline, frame-rate:12.5Hz
- Scenario: One live TTS stream at Mimi 12.5 Hz (80 ms frame). No co-tenant.
- System must: run Inline mode (B=1 degenerate lockstep), CUDA-graph the fixed-shape step (1.21× @B1, §1.3), pace at wall-clock frame period; no queues, no ledger, no admission.
- If mishandled: spin up DC machinery for one stream, paying tick-loop + ledger overhead the edge never needed.

### ARCH-2 — AR codec-LM is the canonical lockstep workload
- Level: SIMPLE
- Pipeline: AR codec-LM TTS → codec decode
- Axes: arch:AR, batch:N, mode:lockstep
- Scenario: 16 same-model TTS streams arrive, all Mimi 12.5 Hz.
- System must: place all 16 in one fixed-slot rectangular batch (memory-bound decode → flat 1→64, 55×@64), advance one frame/tick under one exec-mask, per-slot ring KV.
- If mishandled: serialize streams (one Mutex per model = ONE in-flight, today's `engine.rs` floor) → 16× the latency, no batch benefit.

### ARCH-3 — Flow-matching CFM TTS lands in step-bucket, not lockstep
- Level: SIMPLE
- Pipeline: text → semantic AR → **chunk-CFM** (F5/Matcha/CosyVoice-CFM class) → vocoder
- Axes: arch:flow, bucket, CFG×2, NFE:fixed
- Scenario: One CFM TTS request, 10-step Euler solve, CFG enabled.
- System must: route the CFM stage to the step-bucket batcher (compute-bound, ~10×@64), fold cond+uncond into batch dim (×2), precompute the timestep schedule + masks ONCE at load (shape-stable).
- If mishandled: try to lockstep the CFM at frame rate → a 10-step@B64 solve = 110 ms blows the 40 ms frame budget (§1.5).

### ARCH-4 — Diffusion DDPM head is the compute-bound worst case
- Level: SIMPLE
- Pipeline: AR semantic → **DDPM acoustic head** (VibeVoice class, D768/L16, 25-step) → vocoder
- Axes: arch:diffusion, bucket, NFE:25
- Scenario: One DDPM-head TTS request, 25 denoise steps.
- System must: bucket by (model, latent-shape, step-schedule, CFG); amortize the 25-step solve over a chunk (chunked/lookahead), never per-frame (25-step@B1 = 90 ms; @B64 = 624 ms, collapses @B128, §1.5).
- If mishandled: per-frame diffusion → instant frame-budget blowout; or co-batch to B128 → OOM/collapse.

### ARCH-5 — Masked-diffusion parallel-unmask is fixed-K, not N-step ODE
- Level: SIMPLE
- Pipeline: text → **masked-diffusion acoustic** (SoundStorm/MaskGCT class) → codec decode
- Axes: arch:masked, bucket, K:fixed
- Scenario: One masked-parallel TTS request; K confidence-ordered unmask iterations over a fixed token grid.
- System must: route to step-bucket (bucketed by sequence length, K iterations), parallel-unmask (no KV, no per-frame emit), precompute the per-iteration schedule.
- If mishandled: treat K iterations as a continuous-batching KV path (no KV exists → crash) or as lockstep frames (length ≠ frame count).

### ARCH-6 — Encoder-decoder STT: compute-bound encoder + AR decoder are TWO different batchers
- Level: SIMPLE
- Pipeline: audio → **conv/transformer encoder** → **AR (AED) decoder** (Whisper/Voxtral/Qwen2-Audio class)
- Axes: arch:AR, arch:encoder, two-batcher
- Scenario: One Whisper-AED transcription.
- System must: micro-batch the encoder (step-bucket, length-bucketed one-shot) and run the AR decoder on the token-AR path (paged-KV + admit/evict — text length ≠ frame count, §4.1), NOT lockstep.
- If mishandled: force the AED decoder into frame-sync lockstep → text token count has no frame clock, slots never free correctly.

### ARCH-7 — Frame-sync STT (CTC/RNNT/TDT) IS lockstep, unlike AED STT
- Level: SIMPLE
- Pipeline: audio → conv encoder → **CTC/RNNT/TDT decoder** (parakeet/FastConformer-T class)
- Axes: arch:RNNT, lockstep, frame-rate
- Scenario: One parakeet streaming STT, frame-synchronous emit.
- System must: lockstep-batch the chunk-fed encoder + frame-sync decoder (best per §4.1 — emits per frame), cache-aware encoder state (deltas only, bounded mem; the Nemotron 560-stream contract, L11).
- If mishandled: treat it like AED STT (paged-KV) and forfeit the lockstep win + the cache-aware streaming-encoder contract.

### ARCH-8 — Codec/vocoder decode is the shared terminal stage, off the AR clock
- Level: SIMPLE
- Pipeline: any AR/flow producer → **Mimi/DAC/SNAC/Vocos/BigVGAN decode**
- Axes: arch:feedforward, codec_stream, offload-safe
- Scenario: AR LM emits codec tokens; the codec decoder turns them into PCM.
- System must: run the codec as a streaming-vocoder archetype (independent queue, windowed decode, left-context + crossfade), micro-batch it INDEPENDENTLY of the AR batch (codec already GPU-efficient, RTF 0.003 @B1, §1.4); it's the one stage safe to offload to CPU/NPU + the top cross-model dedup point.
- If mishandled: bind codec batch size to the AR batch (uniform default) → codec window round-robins under concurrency → audio gaps (RFC #2568, §3.2 / catalog C6).

### ARCH-9 — NFE=1 distillation collapses the step-bucket to feedforward
- Level: SIMPLE
- Pipeline: text → AR semantic → **1-step distilled flow head** (IntMeanFlow/DMOSpeech2/consistency class)
- Axes: arch:flow, NFE:1, bucket→feedforward
- Scenario: A distilled CFM that solves in a single function evaluation (NFE=1).
- System must: recognize NFE=1 → the step-bucket degenerates to a single feedforward pass (no solver loop, no per-step schedule); bucket key must accept N=1 (L15 — step count is no longer fixed/universal).
- If mishandled: hard-code a multi-step solver loop / CFG-fold that assumes NFE>1 → wasted passes or a 1-trip loop with full solver overhead.

### ARCH-10 — Token-AR STT wants paging; AR codec-LM TTS wants a ring (same "AR" word, opposite KV)
- Level: SIMPLE
- Pipeline: (a) token-AR STT decode vs (b) AR codec-LM TTS decode
- Axes: arch:AR, kv:paged, kv:ring
- Scenario: Two "AR" models, but one transcribes (unknown variable text length) and one synthesizes (bounded frame-count utterance).
- System must: use a fixed per-slot ring for the codec-LM (bounded context, zero reservation waste, §4.3) and reach for paged-KV ONLY on the long-variable-transcript token-AR STT path.
- If mishandled: paged-KV everywhere (block-table gather jitter on the codec path → frame-deadline misses) or ring everywhere (transcript overruns the ring → truncation).

### ARCH-11 — AR + separate diffusion vocoder is a 2-stage DAG, not one forward
- Level: SIMPLE
- Pipeline: AR semantic LM → (separate) **diffusion/flow vocoder** stage
- Axes: arch:AR, arch:diffusion, dag:multi-node, feedback:loose
- Scenario: AR talker emits semantic codes consumed by a downstream diffusion vocoder that processes COMPLETED chunks.
- System must: split into ≥2 DAG nodes (feedback is loose — consumes completed chunks → separate node, §3.3); lockstep the AR node, step-bucket the diffusion vocoder node, pipeline-overlap them.
- If mishandled: fuse them into one forward → the diffusion solve stalls the AR frame clock (tight coupling where none exists).

### ARCH-12 — MTP/depth decoder is direct-emit lockstep, NOT draft-spec-decode
- Level: SIMPLE
- Pipeline: AR temporal backbone → **MTP / Depformer code-predictor** (Moshi/VocalNet/Qwen3-TTS class)
- Axes: arch:MTP, lockstep, no-spec-decode
- Scenario: An acoustic AR model emits K codebook tokens per frame via a multi-token-prediction head.
- System must: treat the Depformer/MTP head as the direct-emit mechanism (2-5× quality-neutral, L14) — it PRESERVES the rectangular lockstep; weights-per-step depth decoder runs K forwards/frame INSIDE the step.
- If mishandled: bolt EAGLE/Medusa draft-spec-decode on the acoustic path → 0.98× NET SLOWDOWN (token→audio is many-to-one; verify-reject destroys the rectangular batch, L13).

### ARCH-13 — A delay-pattern codec-LM needs max_delay lag before any real acoustic token exists
- Level: SIMPLE
- Pipeline: AR codec-LM with **delay-pattern** multi-codebook (MusicGen/Parler/delay-RVQ class) → codec decode
- Axes: arch:AR, delay-pattern, codebook-ring
- Scenario: Codebooks are emitted on a staggered delay schedule; codebook k is valid only after step ≥ its delay.
- System must: size the per-codebook ring to `max_delay+2` (the +2 = off-by-one guard so max-delay write & oldest read don't collide, F8); teacher-force codebooks ≥1 to PAD during `step < acoustic_delay`; reverse the delay pattern before codec decode.
- If mishandled: read codebook k before its delay → garbage/PAD codes decoded as audio; or ring sized max_delay → write/read collision corrupts the oldest frame.

### ARCH-14 — A model declares two clocks; the engine derives everything from (SR, FR)
- Level: SIMPLE
- Pipeline: any codec model with intrinsic (sample_rate, frame_rate)
- Axes: two-clocks, frame-rate, sample-rate
- Scenario: A model is 24 kHz audio at 12.5 Hz frame rate.
- System must: derive step budget (T_f = 1000/12.5 = 80 ms), cohort key (model, 12.5 Hz), samples_per_frame (24000/12.5 = 1920), the resample chain, and duty from those two declared constants (§5.1); codec-decode + resample are POST-batch stages off the AR clock.
- If mishandled: conflate SR with FR → wrong frame budget (e.g. budget the step against 24 kHz sample period) → false admission + underruns.

---

## INTERMEDIATE — one paradigm under batch / precision / cohort / context stress

### ARCH-15 — Two AR streams on different frame-rate clocks cannot share a lockstep tick
- Level: INTERMEDIATE
- Pipeline: AR codec-LM @12.5 Hz + AR codec-LM @25 Hz
- Axes: arch:AR, cohort:frame-rate, no-mix-clocks
- Scenario: A Mimi-12.5 Hz stream and a 25 Hz codec stream of the SAME architecture arrive together.
- System must: cohort by (model, frame_rate) — these get SEPARATE lockstep batches that time-share the GPU via the duty ledger, never a fused step (§4.2: a 12.5 Hz and 75 Hz stream have no common realtime tick).
- If mishandled: co-batch them in one rectangular tick → one clock starves; the 25 Hz stream emits every other tick or the 12.5 Hz stream double-emits → cadence corruption.

### ARCH-16 — EnCodec-48k @150 Hz is sub-realtime even at batch 1
- Level: INTERMEDIATE
- Pipeline: AR codec-LM on a 150 Hz codec (EnCodec-48k class)
- Axes: arch:AR, frame-rate:150Hz, budget-violation
- Scenario: A model whose codec runs at 150 Hz → 6.7 ms frame period.
- System must: recognize the 9–13 ms decode step EXCEEDS the 6.7 ms budget even at batch 1 (§4.4 — frame-rate spread is 12×; low FR is the biggest throughput lever); reject as non-realtime on this substrate OR route to a faster substrate; never admit-and-underrun.
- If mishandled: admit it as realtime → every single frame overruns → continuous audible dropout from stream one.

### ARCH-17 — Idle masked slots are NOT free under heterogeneous residency
- Level: INTERMEDIATE
- Pipeline: AR lockstep batch with some streams ended/idle
- Axes: arch:AR, lockstep, masked-slot-cost
- Scenario: A 32-slot batch where 20 streams finished but slots aren't recycled; exec-mask is 12-active/20-idle.
- System must: keep masked slots in the rectangular batch (MASKED ≠ ABSENT, F1) but EITHER compact/repack active slots OR explicitly budget the masked-slot energy/bandwidth cost (padding 13%@BS1→40%@BS32; idle-lane energy ~48% of serving, L8).
- If mishandled: assume masked = zero-cost → the dense kernel still reads/writes 20 dead rows → slowest-stream-paces-all + 40% wasted bandwidth on a shared-273 GB/s box.

### ARCH-18 — Masked AR rows need a substituted valid token before the forward
- Level: INTERMEDIATE
- Pipeline: AR lockstep batch with idle/warming slots
- Axes: arch:AR, lockstep, F1, correctness
- Scenario: A batch tick where some slots are idle or still warming up.
- System must: force masked-or-warming rows to the `initial`/BOS token via `where(is_init, initial, gathered)` BEFORE embedding (`is_init |= ~exec_mask`, F1), else the KV-gather reads sentinel/stale → CUDA illegal-memory/NaN that kills ALL 64 users.
- If mishandled: let an idle row carry sentinel `-2`/stale codes → one bad row NaNs the whole batch → every active stream drops simultaneously.

### ARCH-19 — Ring-KV wraparound must mask by logical position, not physical slot
- Level: INTERMEDIATE
- Pipeline: AR codec-LM, long utterance past ctx length
- Axes: arch:AR, kv:ring, F4, wraparound
- Scenario: A stream whose offset exceeds the ring context (`offset > context`) so physical slot order ≠ time order.
- System must: reconstruct per-cell logical position, mask by `pos <= my_pos` (causal) AND window AND never-written⇒-1 (F4); bake the Kyutai pre-wrap/exact-fill/post-wrap/mixed-mask test vectors.
- If mishandled: naive causal mask `j ≤ i` on physical indices attends to FUTURE tokens in recycled cells → acoustic corruption that only appears after the ring wraps (long utterances).

### ARCH-20 — CFG-folding is not universal across flow variants
- Level: INTERMEDIATE
- Pipeline: CFM/flow head, mixed CFG-on and CFG-free streams
- Axes: arch:flow, CFG, bucket-key
- Scenario: Some streams use classifier-free guidance (CFG×2 forwards); some use a CFG-free flow (2504.20334 class).
- System must: include CFG in the bucket key `(model, latent-shape, step-schedule, CFG)` so CFG-on (×2 batch) and CFG-free streams bucket SEPARATELY (L15 — CFG-folding NOT universal); pass a seeded generator so CFG-parallel doesn't diverge from sequential.
- If mishandled: fold all into one ×2 batch → CFG-free streams pay the doubled forward for nothing, or CFG-on streams lose their guidance batch lane → wrong audio.

### ARCH-21 — Sampling needs multinomial; the CUDA graph only allows argmax
- Level: INTERMEDIATE
- Pipeline: AR codec-LM lockstep step under CUDA graph
- Axes: arch:AR, cuda-graph, sampler, C2
- Scenario: A graphed lockstep step that must SAMPLE codec tokens (TTS quality needs multinomial, not argmax).
- System must: sample OUTSIDE the captured region OR use a graph-safe gumbel-argmax inside (C2/F7 resolution); keep all sampler math fp32 regardless of model dtype (H5).
- If mishandled: capture multinomial into the graph → silent breakage or forced eager (losing the 1.21×@B1 edge win), or argmax-in-graph → deterministic flat TTS prosody.

### ARCH-22 — NaN logit on the acoustic path must reject the frame, not argmax garbage
- Level: INTERMEDIATE
- Pipeline: AR codec-LM lockstep, numerically unstable step
- Axes: arch:AR, numerics, H1, reject-frame
- Scenario: A decode step produces a NaN/Inf logit row.
- System must: run an always-on `logits.isnan().any()` (one cheap reduction) and REJECT THE FRAME (repeat prev / codec-silence / greedy-resample) — the H1 policy inversion vs vLLM's argmax-of-NaN garbage.
- If mishandled: argmax a NaN row → a garbage codec token → audible pop with ZERO error signal (vLLM's default-off NaN detector).

### ARCH-23 — Step latency vs KV-context stays benign; size slots by compute, not the KV wall
- Level: INTERMEDIATE
- Pipeline: AR codec-LM over a long utterance
- Axes: arch:AR, kv:context, sizing
- Scenario: A single utterance grows KV context to 1024 frames at batch 32.
- System must: size the slot table by the COMPUTE crossover, not the KV wall (ctx 1024 @B32 = 13.2 ms < 40 ms budget, §1.2 — voice AR is compute-bound long before KV-capacity-bound, the OPPOSITE of long-context text LLMs).
- If mishandled: over-provision KV capacity (text-LLM intuition) and under-provision slots → leave throughput on the table while believing KV is the limit.

### ARCH-24 — fp8 is a DC-batch throughput lever, not a batch-1 latency lever
- Level: INTERMEDIATE
- Pipeline: AR codec-LM, precision selection per batch regime
- Axes: arch:AR, precision:fp8, batch-tiered
- Scenario: An edge box (batch 1) and a DC box (batch 4096) both consider fp8 weights.
- System must: use bf16 for edge/batch-1 (fp8/bf16 GEMM = 0.62× @M=64 — fp8 is SLOWER) and fp8/mxfp4 only for DC/large-batch (2.1× @M=4096), §1.6/§5.2; precision tiers like kernels.
- If mishandled: ship fp8 to the edge for a "speedup" → a measured 1.6× latency REGRESSION at batch 1.

### ARCH-25 — int8 weights must never land on the ORT CUDA-EP
- Level: INTERMEDIATE
- Pipeline: any model, int8 checkpoint, GB10 CUDA via ORT
- Axes: precision:int8, substrate:ORT-CUDA, format-mismatch
- Scenario: An int8 model variant is selected on the ORT CUDA-EP.
- System must: resolve precision per-substrate (`$WAAV_PRECISION → by_substrate[ep] → precision → fp32`) so an int8 file routes to bf16 on ORT-CUDA (the EP silently partitions `MatMulInteger`/Q-DQ to CPU → 12 ms fp → 232 ms int8, §5.2); reach int8/fp4 tensor cores via the TensorRT-EP (static S8S8) or the torch sidecar (torchao native).
- If mishandled: int8 on ORT-CUDA → 19× slowdown from a silent CPU-EP fallback while believing it's a GPU quant win.

### ARCH-26 — KV-quant is the concurrency lever for big-KV models, irrelevant for small codec-LMs
- Level: INTERMEDIATE
- Pipeline: (a) Moshi-7B (32 KV heads, ctx3000) vs (b) Qwen2-0.5B codec-LM (2 KV heads)
- Axes: arch:AR, kv:quant, gqa
- Scenario: Two AR models, one big-KV one small-KV, both want more concurrent streams.
- System must: apply int4 KV-quant to Moshi-7B (25→101 streams, §1.6 — the dominant concurrency lever) and NOT bother for the 0.5B (25 MB/stream → 1589 streams already; KV is a rounding error); note GQA is the biggest KV lever BEFORE quant.
- If mishandled: KV-quant the small codec-LM (pure noise risk, zero stream gain) or leave Moshi-7B at fp16 KV (cap 25 streams when 101 was reachable).

### ARCH-27 — The codec/vocoder must stay high-precision even when the LM is quantized
- Level: INTERMEDIATE
- Pipeline: quantized AR LM → fp32 codec decode
- Axes: precision:mixed, codec:fp32, accuracy
- Scenario: The big LM GEMMs are int8/fp8; the codec decoder inherits the same dtype.
- System must: keep norms, RoPE, sampling, AND the codec/vocoder high-precision (per-component mixed; quant noise compounds across AR frames — the funasr int8-decode-divergence lesson; vLLM-Omni fp32 codec, §5.2); per-architecture defaults encode this with zero user config.
- If mishandled: autocast/quantize the codec → silent audio-quality degradation (WER-flat / MOS-crash signature) that an offline-text gate passes.

### ARCH-28 — A TTS quant gate must include a perceptual/MOS check, not just WER
- Level: INTERMEDIATE
- Pipeline: any TTS, quantized variant, load-time gate
- Axes: precision:quant, accuracy-gate, MOS
- Scenario: An AWQ/fp8 TTS variant is presented for serving.
- System must: gate it vs `reference_precision` on fixtures with a PERCEPTUAL/MOS check (not text-only WER, §5.2) + streaming-playback + concurrent layers (the validation pyramid I4); persist a `verified{substrate,precision,metric}` stamp; refuse/fallback + `waav_quant_gate_failed` if unverified.
- If mishandled: a WER-only gate passes the exact AR-drift bug WaaV hit (WER-flat, MOS-crash) → degraded voice ships silently.

### ARCH-29 — Chunk-CFM must amortize the solve over frames, never run per-frame
- Level: INTERMEDIATE
- Pipeline: AR semantic → chunk-CFM (CosyVoice2 class) → vocoder
- Axes: arch:flow, chunking, lookahead, budget
- Scenario: A CFM that would otherwise solve once per output frame.
- System must: chunk the CFM (left-context + lookahead), solving once per CHUNK of frames, not per frame (a 10-step@B64 solve = 110 ms ≫ 40 ms frame; §1.5 — chunk-level diffusion must be amortized over frames); set `codec_chunk_frames=25, codec_left_context_frames=25` overlap.
- If mishandled: per-frame CFM solve → every frame overruns by ~3× the budget → total dropout under any load.

### ARCH-30 — Streaming egress from any paradigm must be delta, not cumulative
- Level: INTERMEDIATE
- Pipeline: any streaming TTS (AR or flow or diffusion) → WS frames
- Axes: streaming, delta-correctness, C1/I1
- Scenario: A model that streams audio incrementally to a live client.
- System must: emit DELTA only (new samples per chunk), audited emit→consolidate→consume; assert `offline_concat == stream_concat` byte-identical (I1/C1 — cumulative re-decode is O(N²), "the MOST COMMON silent bug").
- If mishandled: cumulative emit → users hear replays/truncation while offline RTF passes; O(N²) compute on long utterances.

### ARCH-31 — Per-step loops on the torch sidecar must be GPU-sync-free
- Level: INTERMEDIATE
- Pipeline: any Path-B torch model, per-frame decode loop
- Axes: arch:AR, sidecar, I3, no-D2H-sync
- Scenario: A torch runner whose per-frame loop calls `.item()/.cpu()/.tolist()`.
- System must: keep every per-step loop sync-free (no D2H; `dst.copy_(src)` not `fill_(item())`; `torch.where` not Python branches, I3); assert zero D2H syncs via a CUDA-event/profiler guard during decode.
- If mishandled: "10 steps × 60 frames × 4 ops = 2400 syncs/request" → the clean 9 ms step assumption collapses → latency blowup invisible offline.

### ARCH-32 — Bucket the encoder by sequence length; 257 ≠ 256
- Level: INTERMEDIATE
- Pipeline: STT conv/transformer encoder, varied audio lengths
- Axes: arch:encoder, bucket, tile-quantization
- Scenario: Encoder inputs of 200, 256, 257, 500 frames arrive together.
- System must: length-bucket the encoder micro-batch (one-shot graph per bucket, §3.2) and keep chunk token counts power-of-two (257 is ~32% slower than 256 — tile quantization, §4.5); streaming & non-streaming never mix in a batch.
- If mishandled: one ragged batch padded to max → wasted compute; or a 257-length bucket pays a 32% tile-quantization tax every batch.

### ARCH-33 — Dynamic-frame-rate codec (FlexiCodec) breaks fixed-rate cohorting
- Level: INTERMEDIATE
- Pipeline: AR codec-LM on a variable-stride codec (FlexiCodec, 3–12.5 Hz)
- Axes: arch:AR, frame-rate:dynamic, cohort, L6
- Scenario: A codec whose frame rate is data-dependent per-utterance AND per-frame, NOT known a-priori (FlexiCodec, 2510.00981).
- System must: let lockstep "advance a variable stride"; the cohort key must tolerate an unknown-a-priori rate (regroup as the rate is observed, L6) — the static (SR, FR) declaration is insufficient.
- If mishandled: cohort by a fixed declared FR → the moment the codec changes stride mid-utterance, the stream desyncs from its cohort's tick → cadence break.

### ARCH-34 — A static-conv vocoder belongs on the NPU; an AR decode does not
- Level: INTERMEDIATE
- Pipeline: AR LM (GPU) + CNN vocoder (NPU)
- Axes: arch:AR, arch:feedforward, placement, §2.3
- Scenario: A heterogeneous box with an NPU; an AR codec-LM + a CNN vocoder stage.
- System must: place AR/dynamic on the GPU and the static conv vocoder (fixed-shape) on the NPU/CPU-AMX/idle-SMs (§2.3 — both Qualcomm & Apple split Whisper exactly this way: encoder on DSP/ANE, AR decoder on CPU/GPU).
- If mishandled: AR decode on the NPU → breaks the static-shape contract (variable per-token shape, growing KV, data-dependent control, per-token host round-trip) → no realtime.

### ARCH-35 — Step-bucket per-request N can vary (length-decoupled solvers)
- Level: INTERMEDIATE
- Pipeline: flow/masked head with per-request step count
- Axes: arch:flow, arch:masked, NFE:variable, L15
- Scenario: One request uses NFE=4, another NFE=2 (a runtime quality dial), and a LLaDA-TTS stream whose cost = T passes INDEPENDENT of length.
- System must: make the bucket key accept per-request variable N (incl N=1) and length-decoupled step counts (L15); bucket streams of the SAME N together, run distinct N for distinct buckets.
- If mishandled: a single fixed-N solver loop → wrong step count for some streams (over/under-denoised audio) or a forced common-N that hurts quality.

### ARCH-36 — Token pacing is free under lockstep, wasteful under continuous batching
- Level: INTERMEDIATE
- Pipeline: AR codec-LM TTS, steady-state
- Axes: arch:AR, lockstep, pacing, §4.6
- Scenario: A TTS that a client consumes at ~3.3 tok/s.
- System must: pace at exactly the consumption rate by construction (frame-synchronous loop = free Andes "Token Pacer", §4.6) — emit one frame per tick, no over-generation.
- If mishandled: run continuous-batching TTS → over-generate ~2.3× surplus GPU, wasting compute the client never consumes.

### ARCH-37 — Warm-up 2-3 lockstep steps before readiness, to fill state + capture the graph off-path
- Level: INTERMEDIATE
- Pipeline: AR codec-LM, server startup
- Axes: arch:AR, warmup, F6, readiness
- Scenario: A fresh server about to serve its first request.
- System must: run 2-3 warm-up steps with a full mask + `synchronize()` (fills conv/KV boundary state + forces CUDA-graph capture OFF the hot path, F6) and gate `/readyz` on warmup+calibration complete (C7), not process-up.
- If mishandled: first request pays seconds of graph-capture (#44209: capture-OOMs AFTER /health passes → crash-loop on sm120) → first-stream cliff.

### ARCH-38 — Compact slot capture: graph the exact slot counts, no 257-padding
- Level: INTERMEDIATE
- Pipeline: AR lockstep, CUDA-graph ladder
- Axes: arch:AR, cuda-graph, H4, power-of-2-cliff
- Scenario: Capturing graphs for a 64-slot lockstep batcher.
- System must: capture EXACT cohort slot counts (1,2,4,…,N_slots) → 0 padding → sidesteps the 257→272/257→512 power-of-2 cliff entirely (H4); single shared graph pool, capture LARGEST cohort first, `dst.zero_()` padded slots (#43810: padding wrote -1 into a real KV slot).
- If mishandled: pad to the next power-of-2 → tile-quantization tax + #43810 padding-corrupts-real-slot class bugs.

### ARCH-39 — enforce_eager must be a first-class OOM / capture-failure escape
- Level: INTERMEDIATE
- Pipeline: any graphed paradigm, low-VRAM or capture failure
- Axes: cuda-graph, eager-fallback, C8/H4
- Scenario: CUDA-graph + compile capture OOMs or the kernel is FULL-graph-unsupported on sm120.
- System must: expose `enforce_eager` as a config knob + auto-downgrade-to-eager (per-kernel `AttentionCGSupport` MIN across groups, NEVER crash, H4/C8); capture cost is real memory and must be budgeted/reserved before admitting.
- If mishandled: hard-fail on capture OOM (#40969 hang-after-6-requests, #45425 silent corruption FULL graph over varlen) instead of degrading to eager.

---

## COMPOUND — ≥2 paradigms, or nested AR+inner, in one model / one batch

### ARCH-40 — Nested AR-outer + CFM-inner stays INSIDE one stage's forward
- Level: COMPOUND
- Pipeline: **dots.tts class** — AR talker {nested per-frame CFM} → audio-VAE
- Axes: arch:nested, arch:AR, arch:flow, feedback:tight
- Scenario: An AR talker that runs an inner CFM solve per frame, with feedback written back in-call.
- System must: keep the inner loop INSIDE one batched forward as ONE `StageNode` with `[stage.nested]{inner_paradigm=flow, inner_steps, inner_batch=fused}` (§3.3 — AR→code-predictor is tight per-frame feedback → one node; the DAG sees one node).
- If mishandled: split the inner CFM into a cross-process stage → per-step latency balloons (SGLang merges Talker+MTP for exactly this reason) → frame-budget blowout.

### ARCH-41 — The nested inner head batches across the OUTER lockstep batch
- Level: COMPOUND
- Pipeline: nested AR + per-frame diffusion patch (T4 latent)
- Axes: arch:nested, lockstep, step-bucket, fused-batch
- Scenario: At outer frame t, all B active slots are at the same inner step k (frame-synchronous).
- System must: batch the inner ODE/diffusion step as a single kernel `[B,…]` across the outer batch (the nested per-frame patch batches 38×@64 because the tiny latent is launch-bound — nesting is net-POSITIVE precisely because the inner head can't saturate the GPU alone, §3.3/§1.5).
- If mishandled: run the inner solve per-slot serially → forfeit the 38× batch win → the nested forward becomes B× slower.

### ARCH-42 — Schedulability folds the inner loop into the outer step time
- Level: COMPOUND
- Pipeline: nested AR + inner N-step head
- Axes: arch:nested, scheduling, T_step
- Scenario: Admission must decide if a nested-model stream fits the frame budget.
- System must: compute `T_step = T_ar + inner_steps × T_inner` (calibration times the WHOLE nested forward, §3.3) and admit only if `T_step ≤ 0.8 × frame_period`.
- If mishandled: admit against `T_ar` alone (ignoring the inner solve) → every frame overruns by `inner_steps × T_inner` → underrun.

### ARCH-43 — CosyVoice2 is a 3-node DAG; dots.tts is a 2-node DAG; same engine, data-driven
- Level: COMPOUND
- Pipeline: (a) CosyVoice2 `ar_semantic → cfm_chunk → vocoder` vs (b) dots.tts `ar_talker{nested cfm} → audiovae`
- Axes: arch:nested, dag, feedback:loose-vs-tight, P-7
- Scenario: Two hybrid TTS models with different inner-coupling tightness.
- System must: express CosyVoice2 as 3 separate nodes (talker→chunk-CFM→vocoder is LOOSE — consumes completed chunks) and dots.tts as 2 nodes (AR→CFM is TIGHT — fused per-frame) purely via manifest data (§3.3 — the dividing line is feedback tightness).
- If mishandled: hard-code one topology → either fuse CosyVoice2's loose CFM (stalls the AR clock) or split dots.tts's tight CFM (latency balloon).

### ARCH-44 — The THIRD execution class: AR-outer + generative-inner with PER-STREAM variable NFE
- Level: COMPOUND
- Pipeline: **CALM/SALAD/FELLE/VoxCPM/DiTAR class** — AR-outer + inner diffusion/flow where inner-NFE is a runtime dial
- Axes: arch:nested, NFE:per-stream-variable, third-class, L5
- Scenario: Stream A solves its inner head at NFE=10; stream B (quality-relaxed) at NFE=2 — in the SAME outer lockstep batch.
- System must: compose TWO batchers per outer step — the lockstep outer tick fans each tick's B hidden-states into a per-stream variable-NFE inner micro-batch (streams at different NFE can't share a lockstep inner tick → sub-bucket the inner by NFE, L5); the nested batcher composes, never picks one.
- If mishandled: force a common inner-NFE → either over-denoise B (wasted compute) or under-denoise A (degraded audio); or try to lockstep the inner across different NFE → desync.

### ARCH-45 — DiTAR patch-AR advances by a PATCH, not a frame (variable stride)
- Level: COMPOUND
- Pipeline: **DiTAR class** — patch-AR outer + inner DiT ODE (NFE 10→2)
- Axes: arch:nested, stride:patch, arch:flow, L5
- Scenario: A model that advances the AR loop by a multi-frame PATCH and runs an inner DiT ODE per patch.
- System must: generalize lockstep to "advance a model-dependent variable stride" (patch ≠ frame, L5) — the outer clock ticks per patch; cohort by (model, patch-rate); inner ODE is a step-bucket per patch.
- If mishandled: assume one AR step = one frame → the patch stride desyncs the cohort tick and miscomputes the frame budget.

### ARCH-46 — FlashTTS breaks BOTH batchers in one model: MTP-3 + 2-NFE meanflow head
- Level: COMPOUND
- Pipeline: **FlashTTS class** — MTP-3 (3 tokens/step) outer + 2-NFE meanflow inner head
- Axes: arch:MTP, arch:nested, arch:flow, two-violations, L5
- Scenario: One production model that emits 3 tokens/step AND runs a 2-NFE inner flow per step.
- System must: handle MTP-3 as direct-emit lockstep (3 codebook tokens/step, rectangular-preserving, L14) WHILE composing the 2-NFE meanflow inner as a fused step-bucket per step (both batchers active in one node, L5).
- If mishandled: pick one batcher → either drop the MTP multi-emit (3× slower) or fail to batch the 2-NFE inner (per-slot serial).

### ARCH-47 — Moshi temporal + depth: two AR loops, one node, summed multi-codebook embeddings
- Level: COMPOUND
- Pipeline: **Moshi RQ-Transformer** — temporal transformer (1 fwd/frame) + Depformer (K fwds/frame, weights-per-step)
- Axes: arch:MTP, arch:nested, multi-AR, §9.4
- Scenario: A full-duplex S2S model with a temporal backbone and a per-frame depth decoder.
- System must: run the temporal transformer once/frame + the Depformer K-fwds/frame INSIDE the step (weights-per-step), summing multi-codebook input embeddings (§9.4); the depth loop batches across the outer lockstep batch.
- If mishandled: treat the Depformer as a separate stage → per-frame feedback latency balloon; or forget weights-per-step → wrong depth tokens.

### ARCH-48 — qwen3-tts talker + sub-talker is multi-AR/MTP, merged into one stage
- Level: COMPOUND
- Pipeline: **qwen3-tts class** — talker AR + sub-talker (MTP) head
- Axes: arch:MTP, multi-AR, nested, G/SGLang
- Scenario: A talker that drives a sub-talker MTP head per token.
- System must: merge Talker+MTP into ONE stage (SGLang-Omni merges them because "the per-step latency would balloon" if separated, §3.3); the sub-talker is the MTP mechanism (2-5× quality-neutral, L14), batched across the outer batch.
- If mishandled: separate the sub-talker into a process hop → per-token cross-process latency destroys the frame budget.

### ARCH-49 — AR-codec-LM + neural-codec-decode: the delay-pattern reverse with max_delay lag
- Level: COMPOUND
- Pipeline: AR codec-LM (delay-pattern) → neural codec decode (Mimi/DAC)
- Axes: arch:AR, delay-pattern, codec, F8, lag
- Scenario: The codec decoder consumes delay-patterned codebooks the AR LM emits with a max_delay stagger.
- System must: reverse the delay pattern (re-align codebooks to a common frame) before the codec decode; the codec node lags the AR node by max_delay frames; per-codebook ring sized max_delay+2 (F8); first-audio includes the acoustic-delay term (§4.4).
- If mishandled: feed staggered codebooks straight to the codec → garbled audio; or forget the lag → decode reads not-yet-emitted codebooks.

### ARCH-50 — Can't co-batch AR + diffusion in ONE step (different physics)
- Level: COMPOUND
- Pipeline: AR codec-LM stream + diffusion-head stream, same model family
- Axes: arch:AR, arch:diffusion, no-mix-paradigm
- Scenario: An AR stream (memory-bound, 1 step/frame) and a diffusion stream (compute-bound, N-step solve) arrive together.
- System must: route them to SEPARATE batchers (lockstep vs step-bucket) sharing the GPU TEMPORALLY via the duty ledger — NEVER a fused step (§4.2 — they have incompatible step semantics and bottlenecks).
- If mishandled: cram both into one kernel batch → the diffusion N-step solve stalls the AR frame clock, or the AR single-step starves the diffusion solver → both miss deadlines.

### ARCH-51 — Talker→chunk-CFM→vocoder loose chain pipelines across stages
- Level: COMPOUND
- Pipeline: AR talker → chunk-CFM → streaming vocoder (3-node)
- Axes: arch:AR, arch:flow, arch:feedforward, pipeline-overlap
- Scenario: Three nodes where stage N+1 of request A can run while stage N of request B runs.
- System must: give each node its own thread + bounded queue + batch policy (AR lockstep, CFM step-bucket, vocoder streaming-window) so the codec thread micro-batches frames while the AR thread lockstep-ticks (§3.2 — pipeline overlap is the payoff).
- If mishandled: one batch loop across all three stages → the codec head-of-line-blocks the AR (the exact bug the per-stage decoupling prevents).

### ARCH-52 — AR stage wants max_num_seqs≥4; codec stage wants 1 — independent batch sizes
- Level: COMPOUND
- Pipeline: AR stage + codec stage, concurrent streams
- Axes: arch:AR, arch:feedforward, batch-size-per-stage, C6
- Scenario: Multiple concurrent TTS streams through an AR→codec DAG.
- System must: set AR `max_num_seqs ≥ 4` (to pipeline) and codec `= 1` independently (§3.2/C6 — a uniform default causes audio gaps because the codec window round-robins, RFC #2568).
- If mishandled: codec inherits the AR batch size → codec window round-robins across requests → audible gaps only under concurrency.

### ARCH-53 — Prefix-cache key must fingerprint injected ref-audio, not just token ids
- Level: COMPOUND
- Pipeline: voice-clone TTS (ref-audio pasted at placeholder positions) → AR LM
- Axes: arch:AR, kv:prefix, G1/L1, contamination
- Scenario: Two requests, same text, DIFFERENT ref-audio, on a paged/radix-prefix path.
- System must: set `extra_key = blake2b(full N-codebook ref sequence)` over ALL codebooks (cb0-only collides, G1) so different ref-audios don't share KV; `extra_key=None` for zero-shot so legit prefix-sharing survives.
- If mishandled: token-ids match (placeholder `-100` positions) → RadixAttention concludes prefixes match → cross-contaminates KV → silent WRONG-VOICE output only under concurrency.

### ARCH-54 — Hybrid KV: radix prefix-cache for ref/system prefix + ring for the utterance suffix
- Level: COMPOUND
- Pipeline: cloned-voice TTS, repeated same-voice requests
- Axes: arch:AR, kv:hybrid, L1, prefix-reuse
- Scenario: Repeated requests reusing the same voice → 86.4% avg / >90% peak prefix-cache hit (Fish S2, L1).
- System must: use a HYBRID KV — radix/paged prefix-cache for the deterministic ref-audio+system-prompt prefix + a fixed ring for the per-utterance suffix (L1 retires "prefix sharing ~zero"); a pure per-slot ring forfeits ~86% cacheable work.
- If mishandled: ring-only → recompute ref-audio/system-prompt KV EVERY request → forfeit the top commercial workload's (multi-tenant cloned-voice agent) cache.

### ARCH-55 — STT→translate→TTS DAG needs dynamic fan-in or conditional branches deadlock
- Level: COMPOUND
- Pipeline: STT encoder/decoder → MT → TTS (AR+codec)
- Axes: arch:encoder, arch:AR, dag:fan-in, G11
- Scenario: A cascade where a text-only branch produces no audio-encoder output.
- System must: compute the expected source set PER REQUEST via `wait_for_fn(req, from, data) → expected_sources` (G11 — fixed `wait_for=[a,b,c]` deadlocks when a branch won't fire); constrain `route_fn` to statically-declared `next` (analyzable topology); multi-terminal for text+audio.
- If mishandled: fixed fan-in waits forever for the absent audio branch → the whole request hangs.

### ARCH-56 — A vocoder may receive AR stream-chunks BEFORE its own payload
- Level: COMPOUND
- Pipeline: AR producer ∥ vocoder consumer (parallel paths)
- Axes: arch:AR, arch:feedforward, out-of-order, G/SGLang
- Scenario: A vocoder gets codec stream-chunks before its request payload arrives (parallel paths, no cross-path ordering).
- System must: make pre-payload stream acceptance EXPLICIT opt-in (`can_accept_stream_before_payload`, else hard-fail not silent-corrupt); monotone `chunk_id` per (req, target); the vocoder latches the codec contract from whichever (payload|chunk-meta) arrives first.
- If mishandled: silently process chunks against a default/wrong codec contract → corrupt audio.

### ARCH-57 — Nested co-eviction: drop an EOS stream from ALL inner loops the same tick
- Level: COMPOUND
- Pipeline: nested AR + inner head, a slot hits EOS mid-batch
- Axes: arch:nested, co-eviction, F3, same-tick
- Scenario: Slot 7 in a nested batch reaches end-of-stream while other slots continue.
- System must: co-evict slot 7 from the OUTER lockstep AND every inner loop (depth/CFM/diffusion) in the SAME tick via one transactional `reset_slot(7)` (fan out to KV pointers, conv rings, sampler, inner-solver state, word buffers, offset, F3); monotonic channel-id drops any late output for the old occupant.
- If mishandled: evict from the outer but leave the inner solver carrying slot 7's latent → next admit into slot 7 inherits stale inner state → cross-user contamination.

### ARCH-58 — Tiny-T compute-bound inner head batches LIKE AR (38×@64), not like chunk-diffusion
- Level: COMPOUND
- Pipeline: nested AR + per-frame patch (T4) vs standalone chunk-CFM (T64)
- Axes: arch:nested, arch:flow, batch-profile, §1.5
- Scenario: The same flow math at T4 (nested per-frame) vs T64 (standalone chunk).
- System must: recognize the tiny-T inner patch is LAUNCH-bound → batches 38×@64 (AR-like), whereas the T64 chunk-CFM is compute-bound → only 10×@64 (§1.5); admit nested streams with the AR-like profile, chunk streams with the sublinear profile.
- If mishandled: assume "diffusion = sublinear" and under-admit nested streams (leave 28× batch headroom on the table) or over-admit chunk-CFM streams (blow the budget).

### ARCH-59 — Mixing a streaming and a non-streaming request in one micro-batch is forbidden
- Level: COMPOUND
- Pipeline: encoder/CFM micro-batch, mixed streaming + batch requests
- Axes: arch:encoder, arch:flow, batch-hygiene, G/SGLang
- Scenario: A live streaming request and a one-shot batch request both want the same CFM/encoder bucket.
- System must: NEVER mix streaming & non-streaming in one batch (G/SGLang micro-batch collector rule); separate buckets even at the same length/N; `max_batch_size=4, max_batch_wait_ms=2`.
- If mishandled: co-batch them → the streaming request's cadence is held hostage to the batch request's completion → first-audio jitter.

### ARCH-60 — Same-process fan-out must clone-on-fan-out, share Arc only for immutable leaves
- Level: COMPOUND
- Pipeline: one AR result → N downstream stages (codec + EoT head + VAD head)
- Axes: arch:nested, fan-out, G5, aliasing
- Scenario: One frame's hidden-state fanned to multiple consumer stages in-process.
- System must: MOVE ownership (clone the owned container on fan-out) and share `Arc` only for immutable tensor leaves (G5 — `Arc<Mutex<Payload>>` on fan-out reintroduces the aliasing hazard); serialize only cross-process.
- If mishandled: fan out by `Arc<Mutex>` reference → one stage mutating the payload corrupts the others → crosstalk.

### ARCH-61 — Encoder colocated with the hot AR loop must not starve it
- Level: COMPOUND
- Pipeline: AR decode (hot) + STT encoder forward (co-located)
- Axes: arch:AR, arch:encoder, starvation, G3
- Scenario: A busy AR lockstep loop and an encoder forward sharing a process/core.
- System must: block on `recv_timeout`/`Notify` when idle (hog the core only when busy, 2 ms sleep, G3/F6); prefer stage=process for hot-AR vs flaky-encoder (SGLang moved to ENCODER DISAGGREGATION because colocation slowed the encoder ~600×); starvation-load-test any colocation.
- If mishandled: `loop { try_recv() }` busy-spin → starves the co-located encoder → audio QPS collapses (>10→<0.5).

### ARCH-62 — Barge-in cancels every nested inner loop's slot within one tick
- Level: COMPOUND
- Pipeline: nested AR+CFM stream, user barge-in
- Axes: arch:nested, barge-in, G2/G9, control-plane
- Scenario: The user interrupts mid-utterance; the engine must cancel an in-flight nested forward.
- System must: route barge-in as a RELIABLE (per-stage ack, not fire-and-forget PUB/SUB, G9) control message that jumps every stage's queue + frees the slot/KV/inner-solver-state within ≤1 tick (§6); cancelled ≠ completed (distinguishable terminal frame, G2).
- If mishandled: best-effort abort drops the message (ZMQ PUB drops to late SUBs, G9) → the inner solver keeps generating into a cancelled slot → wasted compute + late audio after barge-in.

### ARCH-63 — A nested model under CUDA-graph: graph the outer step, eager the variable inner
- Level: COMPOUND
- Pipeline: nested AR (fixed step) + inner variable-NFE head
- Axes: arch:nested, cuda-graph, H4, mixed-capture
- Scenario: A nested model where the outer AR step is fixed-shape but the inner NFE varies per stream.
- System must: FULL-graph the AR outer step (fixed/uniform, capture once/cohort) and run the variable-NFE inner as eager-or-piecewise (H4 — AR lockstep = FULL-graph fast path; varlen/sampling = eager); resolve MIN graph-support across groups.
- If mishandled: try to capture the variable-NFE inner into the outer graph → shape-change HARD ERROR every time the NFE dial moves (F7 replay guard).

### ARCH-64 — A 2-node nested DAG must stream first-audio sub-300 ms
- Level: COMPOUND
- Pipeline: dots.tts 2-node `ar_talker{nested cfm} → audiovae`
- Axes: arch:nested, TTFA, streaming, M3
- Scenario: A live first-audio request through a nested-CFM 2-node DAG.
- System must: stream first-audio sub-300 ms (M3 accept criterion) via incremental egress + TTFA ramp (first chunk larger for quality, then smaller for latency, §3.2); first-audio = `frame_period + acoustic_delay·frame_period + step_time`.
- If mishandled: buffer the whole nested forward before first emit → first-audio in the seconds → fails the realtime bar.

### ARCH-65 — Acoustic-delay off-by-one in the nested codebook ring
- Level: COMPOUND
- Pipeline: nested/delay-pattern AR, codebook ring
- Axes: arch:AR, delay-pattern, F8, off-by-one
- Scenario: The per-codebook acoustic-delay ring at the boundary where the max-delay write meets the oldest read.
- System must: size the cache depth = `max_delay+2` (the +2 prevents the write/read collision, F8); write `(offset+delays[k])%CT`, read `(offset-max_delay+gen_delays[k])%CT`; teacher-force PAD before `step < acoustic_delay`.
- If mishandled: size = max_delay → the newest write clobbers the oldest read → a corrupted codebook frame surfaces as a periodic click.

### ARCH-66 — Speculative-decode is scoped to the long-context token-AR-STT paging path only
- Level: COMPOUND
- Pipeline: (a) acoustic AR-TTS vs (b) long-context token-AR-STT
- Axes: arch:AR, spec-decode, L13, scoped
- Scenario: A request to add speculative decoding "for speed."
- System must: BAN exact draft-spec-decode on the acoustic-token path (0.98× net SLOWDOWN, token→audio many-to-one, L13) but ALLOW sparse-KV spec-decode on the long-context token-AR-STT PAGING path (MagicDec 2.51×@batch32-256 for KV-memory-bound, L13).
- If mishandled: blanket-ban (forfeit the STT-paging win) or blanket-enable (the acoustic path regresses + the rectangular lockstep is destroyed).

### ARCH-67 — Long-form TTS overflows the ring → pin attention-sink + paged escape hatch
- Level: COMPOUND
- Pipeline: long-form AR-TTS / long-audio STT, ctx > ring
- Axes: arch:AR, kv:ring-lossy, L12, attention-sink
- Scenario: A 10-minute audiobook synthesis → 30k+ tokens, past the ring context.
- System must: pin attention-sink tokens + provide a paged/full-ctx escape hatch for long-form (L12 — "context is bounded" breaks >4 min; StreamingLLM sliding-window forgetting/wraparound instability without pinned sinks); generic LLM-KV eviction FAILS on audio (AudioKV).
- If mishandled: rely on the ring alone → silent lossy forgetting + wraparound instability → degrading prosody/transcript over a long utterance.

### ARCH-68 — Cross-model codec dedup: Mimi/DAC/HiggsV2 are shared decoders
- Level: COMPOUND
- Pipeline: multiple TTS models, same neural codec decoder
- Axes: arch:feedforward, codec, dedup, §3.2
- Scenario: Three loaded TTS models that all decode through Mimi.
- System must: dedup the shared codec decoder (one loaded Mimi/DAC/HiggsV2 instance serving all producers, §3.2 — the terminal codec node is the highest-value cross-model dedup point); micro-batch frames from all producers into the one codec engine.
- If mishandled: load three copies of Mimi → 3× the codec VRAM + three under-fed codec micro-batches instead of one well-fed one.

### ARCH-69 — Full-duplex S2S models the user stream too: barge-in is always-on
- Level: COMPOUND
- Pipeline: Moshi-class full-duplex S2S
- Axes: arch:AR, full-duplex, §9.6, barge-in
- Scenario: A full-duplex conversation where the user can speak at any time.
- System must: per frame, ingest user Mimi tokens into input slots AND emit Moshi tokens from output slots simultaneously (§9.6 — barge-in = the user stream is ALWAYS modeled); K=2Q+1 interleaved streams with per-stream delays, one knob switches STT/TTS/S2S/translation (§9.5).
- If mishandled: treat the user stream as an out-of-band VAD signal → can't model true overlap → jittery turn-taking (the BayLing-Duplex rejection of naive per-frame synchrony, L7).

### ARCH-70 — Variable stream lifetime (EOS/VAD/barge-in) is first-class, not "no length variance"
- Level: COMPOUND
- Pipeline: full-duplex S2S with PAD/EPAD/SILENCE state tokens
- Axes: arch:AR, residency:variable, L7, turn-taking
- Scenario: Streams with variable LIFETIME from barge-in/EOS/VAD-silence/async turn-taking.
- System must: handle heterogeneous residency first-class (drop the "fixes per-request token rate" / "no length variance" framing, L7); model turn-taking via PAD/EPAD/SILENCE state tokens (BayLing-Duplex BEAT Moshi: overlap 2.07→1.10 s, L7); recycle slots transactionally as lifetimes end.
- If mishandled: assume fixed residency → the slowest-stream-paces-all under variable lifetime → idle slots accumulate + jittery turn-taking.

### ARCH-71 — Marker/flush: a non-streaming request over a streaming-core needs trailing-silence flush
- Level: COMPOUND
- Pipeline: one-shot POST transcription over a streaming STT core
- Axes: arch:RNNT, marker/flush, F5, delay-pipeline
- Scenario: A whole-clip POST request served by a frame-sync streaming STT with an asr_delay.
- System must: append real audio + a marker + 10 s of trailing silence (`vec![0f32;240000]`) to flush the delay pipeline, and terminate on the MARKER not input-exhaustion (F5); the "stream done" marker fires at `now + asr_delay + buffered_frames` via a step-ordered heap.
- If mishandled: stop at input-exhaustion → the delayed model never emits the last words → truncated transcript at the clip end.

### ARCH-72 — Out-of-order codec chunks need monotone chunk_id reordering
- Level: COMPOUND
- Pipeline: AR producer → codec consumer, parallel transport
- Axes: arch:AR, arch:feedforward, ordering, G6
- Scenario: Codec chunks arrive at the vocoder out of order (parallel paths).
- System must: keep the ordered egress queue UNBOUNDED (never drop → never reorder audio) with sender-side credit backpressure (G6); monotone `chunk_id` per (req, target) re-orders before decode.
- If mishandled: drop or reorder chunks → audible audio scramble/gaps; or bound the egress queue and drop → lost frames.

### ARCH-73 — Two CFM streams at different latent SHAPE can't share a step-bucket
- Level: COMPOUND
- Pipeline: two flow models with different latent dims (D512 vs D768)
- Axes: arch:flow, bucket-key, latent-shape
- Scenario: A CosyVoice-CFM (D512) and a VibeVoice-DDPM (D768) both in flight.
- System must: bucket by `(model, latent-shape, step-schedule, CFG)` (§4.2) — different latent shapes get DIFFERENT buckets even both being "flow/diffusion"; run each bucket's fixed N/K independently.
- If mishandled: co-batch different latent shapes → shape-mismatch crash or a padded-to-max batch wasting compute.

### ARCH-74 — A masked-diffusion stream and an AR stream share GPU temporally, not in-batch
- Level: COMPOUND
- Pipeline: SoundStorm masked-parallel + AR codec-LM
- Axes: arch:masked, arch:AR, temporal-share, duty-ledger
- Scenario: A masked-diffusion TTS (fixed-K parallel) and an AR TTS (per-frame lockstep) co-resident.
- System must: run each on its own batcher (step-bucket K-iter vs lockstep) sharing the GPU TEMPORALLY via the duty ledger (§4.2 cohorts share temporally, not within a fused step); admit only if `Σ duty ≤ S` per substrate.
- If mishandled: interleave a K-iteration masked pass into the AR frame tick → the masked pass blows the AR frame budget or vice-versa.

### ARCH-75 — A model with an extra EoT/VAD linear head adds a cheap per-step fan-out
- Level: COMPOUND
- Pipeline: AR backbone + generic per-step linear heads (semantic-VAD / end-of-turn)
- Axes: arch:AR, extra-heads, §9.7, fan-out
- Scenario: A streaming model that also emits a per-frame end-of-turn / VAD signal.
- System must: run the extra heads as generic per-step linear heads off the same hidden-state (§9.7), fanned out (clone-on-fan-out, G5) — cheap, in-step, no separate stage.
- If mishandled: make the EoT head a separate cross-process stage → per-frame latency + ordering complexity for a one-matmul head.

### ARCH-76 — Encoder-decoder STT: the encoder is one-shot micro-batch, the decoder is its own loop
- Level: COMPOUND
- Pipeline: Whisper-AED — encoder (compute-bound) → AR decoder (token-AR, paged)
- Axes: arch:encoder, arch:AR, two-batcher, dag
- Scenario: Concurrent Whisper-AED transcriptions of varied audio lengths.
- System must: length-bucket the encoder as a one-shot micro-batch stage (compute-bound, §3.2) feeding the AR decoder on the paged token-AR path (admit/evict, §4.1); the two stages pipeline-overlap with independent batch policies.
- If mishandled: one uniform batch policy across encoder+decoder → either the encoder waits on decoder slots or the decoder starves on encoder batching.

### ARCH-77 — SenseVoice-CTC (non-AR STT) lands in step-bucket, not lockstep-AR
- Level: COMPOUND
- Pipeline: SenseVoice-CTC class — encoder + CTC head (no AR decode)
- Axes: arch:encoder, arch:CTC, bucket
- Scenario: A non-autoregressive CTC STT (one forward, no token loop).
- System must: route the whole model to step-bucket (length-bucketed one-shot, no KV, no AR loop, §4.1 — CTC is frame-sync but has no per-token AR cost); it can ride the lockstep encoder axis if co-located but needs no decoder loop.
- If mishandled: spin up an AR decode loop / KV table for a model that has neither → wasted machinery + wrong batching.

### ARCH-78 — Per-slot sidecar state for the inner codec/sliding-window must be slot-keyed
- Level: COMPOUND
- Pipeline: Path-B torch nested model holding Python codec/sliding-window state
- Axes: arch:nested, sidecar, C3/I5, crosstalk
- Scenario: A torch sidecar serving multiple slots that each hold streaming codec/sliding-window/CUDA-graph state.
- System must: key the sidecar Python state by slot-id (`self._state: dict[slot_id, State]`) and free on slot-reset (C3/I5 — the model can't own a shared buffer across slots); add a concurrent-crosstalk test.
- If mishandled: a shared codec/sliding-window buffer across slots → crosstalk/truncation only under load ("a shared buffer silently corrupts audio across concurrent requests").

### ARCH-79 — Bucket coefficients/timesteps/masks precomputed once when shape+steps fixed
- Level: COMPOUND
- Pipeline: CFM/diffusion step-bucket, fixed NFE
- Axes: arch:flow, arch:diffusion, precompute, B/catalog
- Scenario: A step-bucket batcher running a fixed-NFE solve repeatedly.
- System must: precompute the CFM/diffusion timestep schedule, causal masks, and position tensors ONCE at load and reuse (lockstep + fixed NFE = shape-stable, catalog B); never call `empty_cache()` in the per-frame loop (sync/idle stall).
- If mishandled: recompute the schedule/masks per solve → per-step host overhead + an `empty_cache` sync bubble that idles the GPU.

### ARCH-80 — Mixed-paradigm one model: encoder (bucket) + AR (lockstep) + CFM-vocoder (bucket)
- Level: COMPOUND
- Pipeline: a single model whose stages span all three batchers
- Axes: arch:encoder, arch:AR, arch:flow, suitability-matrix
- Scenario: One model sits in DIFFERENT batcher boxes per stage (the §4.1 suitability-matrix insight).
- System must: place the encoder stage in step-bucket, the AR stage in lockstep, the CFM-vocoder stage in step-bucket — same model, three batch policies declared per stage in the manifest (§4.1 — "one model sits in different boxes per stage").
- If mishandled: pick ONE batcher for the whole model → two of the three stages get the wrong batch profile → budget blowout or starvation.

---

## EXTREME — heterogeneous co-residency, multiple clocks/NFEs on one box

### ARCH-81 — Four paradigms co-resident on one GB10, each on its own clock/NFE
- Level: EXTREME
- Pipeline: AR-codec-LM stream + nested AR+CFM stream + masked-diffusion stream + STT encoder, ALL on one GB10
- Axes: arch:AR, arch:nested, arch:flow, arch:masked, arch:encoder, co-residency, duty-ledger
- Scenario: The headline case — a memory-bound AR-codec-LM (12.5 Hz lockstep), a nested AR+CFM (variable inner-NFE), a masked-diffusion TTS (fixed-K), and a compute-bound STT encoder, each on its own clock/NFE, sharing one GB10.
- System must: run FOUR micro-engines (two lockstep cohorts + two step-bucket cohorts), each on a dedicated thread with thread-affine device state; admit IFF `Σ_realtime duty(stage) ≤ S` on the GPU AND `Σ bandwidth_duty ≤ S·273 GB/s` on the shared LPDDR (§6 contention guard); the bottleneck stage (likely the masked-diffusion or nested-CFM) is the binding constraint, not the AR stage.
- If mishandled: AR-only admission the diffusion/CFM can't sustain (§6 — the exact bug the per-substrate duty ledger prevents) → the compute-bound paradigms blow the budget while the AR stage looks fine.

### ARCH-82 — Shared-bandwidth contention: zero-copy removes transfer cost but not the 273 GB/s ceiling
- Level: EXTREME
- Pipeline: AR (GPU, bandwidth-bound) + conv-codec (NPU) + encoder (NPU), all on GB10 unified memory
- Axes: arch:AR, arch:feedforward, arch:encoder, bandwidth-arbiter, §3.4
- Scenario: Stages placed across GPU+NPU on GB10's coherent ~273 GB/s LPDDR; concurrent engines DIVIDE the one ceiling.
- System must: treat aggregate memory bandwidth as a BUDGETED schedulable resource (§3.4 contention guard); prefer to overlap a memory-bound stage (AR decode) with a compute-bound one (conv-codec); co-locate + time-share when BOTH saturate bandwidth; admission budgets the shared bandwidth so the split doesn't oversubscribe.
- If mishandled: place codec/encoder on the NPU believing it's "free parallelism" → all three engines saturate the shared 273 GB/s → aggregate slowdown worse than co-locating on the GPU.

### ARCH-83 — Two different inner-NFE dials in ONE nested lockstep batch
- Level: EXTREME
- Pipeline: nested AR + variable-NFE inner head, batch of streams at NFE ∈ {2,4,8,10}
- Axes: arch:nested, NFE:per-stream-variable, third-class, sub-bucket, L5
- Scenario: An 8-slot nested batch where streams request inner-NFE 2, 4, 8, 10 simultaneously.
- System must: at outer frame t, sub-bucket the inner solve by NFE (streams at the same NFE share an inner micro-batch; different NFEs run as separate inner passes within the step, L5); the outer lockstep tick still advances all 8 together (one frame), only the inner composes 4 sub-batches.
- If mishandled: force a common NFE across the 8 → over/under-denoise; or try one inner lockstep tick across all NFEs → streams at NFE=2 finish while NFE=10 streams are mid-solve → desync within the step.

### ARCH-84 — Co-residency across THREE frame-rate clocks (12.5 / 25 / 75 Hz)
- Level: EXTREME
- Pipeline: three AR cohorts at 12.5, 25, and 75 Hz on one box
- Axes: arch:AR, cohort:frame-rate, multi-clock, §4.2
- Scenario: Three same-architecture-family but different-codec streams at 12.5/25/75 Hz, plus their codec stages.
- System must: maintain THREE separate lockstep tick loops (no common realtime tick across the 6× clock spread, §4.2), each paced to its own period; the duty ledger time-shares the GPU across all three; the 75 Hz cohort has the tightest 13.3 ms budget so the smallest batch knee.
- If mishandled: a single global tick → the 75 Hz cohort emits late every tick (its 13.3 ms budget < the slow cohort's tick) → cascading underruns on the fast cohort.

### ARCH-85 — A diffusion-head stream's N-step solve must not be admitted into the AR frame budget
- Level: EXTREME
- Pipeline: AR-codec-LM cohort (40 ms budget) + VibeVoice-DDPM stream (25-step) sharing GPU time
- Axes: arch:AR, arch:diffusion, bottleneck-admission, §6
- Scenario: A 25-step DDPM solve (624 ms@B64) co-resident with AR streams on a 40 ms frame clock.
- System must: schedule the DDPM solve as a step-bucket workload amortized over a chunk that spans MANY AR frames, and admit it against ITS OWN chunk deadline (the bottleneck-stage SLO, §6), reserving GPU duty so the AR cohort's per-tick budget is untouched.
- If mishandled: admit the DDPM stream against the AR 40 ms budget → its 624 ms solve monopolizes the GPU for ~15 AR ticks → 15 dropped frames across every AR stream.

### ARCH-86 — DC scale: lockstep AR steady-state + token-AR-STT continuous-batch + non-AR step-bucket, all big-N
- Level: EXTREME
- Pipeline: B200 fleet — AR-codec-LM (big lockstep N) + token-AR-STT (paged continuous-batch) + CFM/encoder (big step-bucket)
- Axes: arch:AR, arch:encoder, dc-scale, mode:stage-batched, §8
- Scenario: Thousands of streams across all three batching methods at B200 scale.
- System must: run AR steady-state at big lockstep N + token-AR-STT on paged continuous-batch + non-AR stages on big step-bucket (§4.1 DC row — all three live primitives, disaggregated by stage); fp8/mxfp4 DC precision tier (compute-bound regime, 2.1×); Llumnix constant-time KV migration for spill/rebalance (§8/§6).
- If mishandled: force lockstep on the token-AR-STT (text length ≠ frames) or continuous-batch on the AR-codec steady-state (variable-length machinery for a fixed-rate workload) → wrong batcher per stage at fleet scale.

### ARCH-87 — Mixed-precision per-component AND per-substrate in one co-resident batch
- Level: EXTREME
- Pipeline: AR LM (fp8 GEMM, fp32 norms/RoPE/sampler) on GPU + codec (fp32) on NPU + encoder (int8) on NPU
- Axes: precision:mixed, substrate:multi, §5.2
- Scenario: One DAG where each stage needs a different precision on a different substrate.
- System must: resolve precision per-component (fp8 LM GEMMs, fp32 norms/RoPE/sampler/codec) AND per-substrate (`by_substrate[ep]` — int8 encoder on the NPU's int8 tensor cores, never on ORT-CUDA, §5.2); the accuracy gate stamps each (substrate, precision) combo.
- If mishandled: one global precision → either fp8 corrupts the codec/norms (AR-drift) or int8 lands on ORT-CUDA (19× CPU-fallback) → mixed failures across stages.

### ARCH-88 — Nested model + separate diffusion vocoder + STT encoder: 3 paradigms, mixed coupling
- Level: EXTREME
- Pipeline: nested AR{CFM-inner} → (loose) diffusion vocoder + a co-resident STT encoder
- Axes: arch:nested, arch:diffusion, arch:encoder, coupling:mixed
- Scenario: A TTS with a TIGHT inner CFM AND a LOOSE downstream diffusion vocoder, co-resident with an STT encoder for a duplex agent.
- System must: fuse the inner CFM into the AR node (tight, §3.3), split the diffusion vocoder into a separate step-bucket node (loose), micro-batch the STT encoder independently; THREE batch policies + the duty ledger budgets all of them on the shared GPU.
- If mishandled: mis-assign coupling (split the tight inner → latency balloon; fuse the loose vocoder → AR clock stall) AND let the encoder starve the AR loop (G3 busy-spin).

### ARCH-89 — FlexiCodec dynamic-rate stream co-resident with fixed-rate streams
- Level: EXTREME
- Pipeline: FlexiCodec (3–12.5 Hz dynamic) + Mimi-12.5 Hz fixed, co-resident
- Axes: arch:AR, frame-rate:dynamic, cohort:regroup, L6
- Scenario: A FlexiCodec stream whose rate drops to 3 Hz mid-utterance, alongside steady 12.5 Hz streams.
- System must: re-cohort the FlexiCodec stream as its observed rate changes (cohort key tolerates unknown-a-priori + per-frame-variable rate, L6); when it's at 12.5 Hz it can co-batch with the Mimi cohort, when it drops to 3 Hz it moves to (or forms) the 3 Hz cohort — lockstep advances its variable stride.
- If mishandled: pin it to a fixed cohort → at 3 Hz it desyncs the 12.5 Hz tick (emits every 4th tick) → either it underruns or it drags the whole cohort to 3 Hz.

### ARCH-90 — One nested forward composes TWO batchers; admission times the WHOLE forward
- Level: EXTREME
- Pipeline: nested AR-outer (lockstep) + variable-NFE flow-inner (step-bucket) in one node
- Axes: arch:nested, two-batcher-composed, T_step, L5
- Scenario: The nested batcher must compose a lockstep outer + a step-bucket inner PER STEP (not pick one).
- System must: compose both batchers inside one node — lockstep fans B hidden-states into a step-bucket inner micro-batch (2B with CFG, §4.2 nesting rule) — and admit against `T_step = T_ar + inner_steps × T_inner` calibrated on the WHOLE nested forward under co-load (§6 calibration).
- If mishandled: calibrate the outer and inner separately and sum nominal → miss the co-load contention (the inner step-bucket competes with the outer for bandwidth) → optimistic admission → underrun under real co-residency.

### ARCH-91 — Crash blast-radius: the hot AR stage and a flaky diffusion vocoder are separate processes
- Level: EXTREME
- Pipeline: AR stage (process A) + diffusion vocoder (process B) + STT encoder (process C)
- Axes: arch:AR, arch:diffusion, arch:encoder, crash-isolation, G7/H6
- Scenario: A diffusion vocoder OOMs/crashes mid-fleet.
- System must: keep the hot AR stage and the flaky diffusion/encoder stages as SEPARATE processes (G7 — no per-stage failure isolation inside one process group); 3-layer crash detection (scheduler-thread handler + background-task done-callbacks + 5 s process-liveness, G7/H6); a dead vocoder fails its in-flight requests, NEVER hangs the AR streams; `PR_SET_PDEATHSIG` so a SIGKILL'd vocoder doesn't pin VRAM (H7).
- If mishandled: colocate all three in one process → the vocoder crash exits the whole group → every AR stream dies; or no death-sentinel → parent answers /health 200 while the vocoder is dead (#39863).

### ARCH-92 — Progress watchdog per paradigm: an AR underrun ≠ a stalled diffusion solve
- Level: EXTREME
- Pipeline: AR + nested-CFM + diffusion co-resident, per-session watchdog
- Axes: arch:AR, arch:flow, arch:diffusion, watchdog, H9
- Scenario: Distinguishing "no audio progress" across paradigms with very different step times.
- System must: key the watchdog on last-audio-emitted-at T per session, with a DEVICE+MODEL-aware deadline (H9 — a 1.5B AR-TTS step ≠ a CTC step ≠ a 25-step DDPM solve; #45135: not a flat 300 s); an independent thread kills/restarts a stage with no progress > N×its-own-frame-interval.
- If mishandled: one flat deadline across paradigms → either the slow diffusion solve is falsely killed mid-chunk or a genuinely-stalled AR loop runs forever (the #39863 "alive but zero forward progress" blind spot).

### ARCH-93 — Deadline-aware graceful degradation across paradigms beats blanket reject
- Level: EXTREME
- Pipeline: AR + CFM + masked, fleet at 50% overload
- Axes: arch:AR, arch:flow, arch:masked, graceful-degradation, L9
- Scenario: Overload where some paradigms are cheaper to degrade than others.
- System must: degrade gracefully PER paradigm (Niyama: 95%+ deadlines @ 50% overload vs <20% reject, L9) — e.g. drop the CFM NFE (quality-brownout, BrownoutServe 74%→7% @ ~5% acc loss) or relegate masked-diffusion streams to a degraded queue, before hard-rejecting; cadence protected by the CLIENT PLAYBACK BUFFER; reject only at TRUE saturation.
- If mishandled: blanket reject-don't-glitch at the first breach → reject streams that a small NFE-drop would have saved (the crude baseline deadline-aware schedulers beat, L9).

### ARCH-94 — Intra-node spatial P/D for a prefill spike vs the chunked-prefill firewall
- Level: EXTREME
- Pipeline: AR cohort + a new stream's prefill spike, GB10 SM-partition option
- Axes: arch:AR, prefill, intra-node-P/D, L4/L10
- Scenario: A prefill spike that would inflate per-token TBT if chunked into the decode batch.
- System must: A/B intra-node spatial P/D (SM-partition the prefill off the decode SMs — Nexus 20× lower TTFT, 2.5× lower TBT; chunked-prefill mixed batch = 250 ms vs 15 ms decode-only, >8× TBT spike, L4) against the chunked-prefill firewall on GB10; if chunking, use a KV-LENGTH-aware predicted-latency budget (token-count ≠ compute, DuetServe Obs.2, L10), power-of-two chunks.
- If mishandled: chunk the prefill into the AR decode batch with a flat token-budget → an >8× TBT tail spike (17–22 dropped frames at 80 ms) that intra-node spatial partitioning would have avoided.

### ARCH-95 — Co-resident lockstep + paged token-AR-STT: two KV regimes on one device
- Level: EXTREME
- Pipeline: AR-codec-LM TTS (ring KV) + Whisper-AED STT (paged KV), co-resident
- Axes: arch:AR, kv:ring, kv:paged, co-residency, §4.3
- Scenario: A duplex agent: TTS on the ring-KV lockstep path + STT on the paged token-AR path, same GPU.
- System must: run BOTH KV regimes side-by-side (fixed per-slot ring for the codec-LM, paged-KV + admit/evict for the long-variable STT transcript, §4.3); separate admission (slot-fit for the ring, block-watermark for the paged); the duty ledger budgets both.
- If mishandled: force one KV regime on both → ring on the STT (transcript overruns) or paging on the codec-LM (block-table gather jitter → frame-deadline misses).

### ARCH-96 — Full-duplex S2S + cloned-voice TTS + streaming STT: three workloads, one Moshi-class box
- Level: EXTREME
- Pipeline: Moshi full-duplex S2S + dots.tts-nested cloned-voice + parakeet streaming STT
- Axes: arch:AR, arch:nested, arch:RNNT, full-duplex, §9
- Scenario: A single box running the full Kyutai-class workload set plus a nested cloned-voice TTS and a frame-sync STT.
- System must: drive the RQ-Transformer depth decoder (Moshi temporal+depth, §9.4) + multistream/delay engine (§9.5) for the S2S, a hybrid-KV nested-CFM for the cloned voice (ARCH-54), and a cache-aware streaming encoder for the STT (L11) — three cohorts, one duty ledger, one shared-bandwidth budget; admission tests the bottleneck across all three.
- If mishandled: AR-only admission ignoring the nested-CFM inner solve and the masked/diffusion bottleneck → the heaviest paradigm blows the budget while the lockstep AR looks healthy.

### ARCH-97 — Co-eviction across a 3-deep nesting (outer AR + depth + inner CFM) in one tick
- Level: EXTREME
- Pipeline: AR temporal + Depformer depth + inner CFM, a slot hits EOS
- Axes: arch:nested, arch:MTP, arch:flow, co-eviction, F3
- Scenario: A triple-nested model (temporal → depth → inner flow) where slot 5 reaches EOS mid-batch.
- System must: drop slot 5 from ALL THREE nested loops the SAME tick via one transactional `reset_slot(5)` fanning out to: temporal KV ring, depth-decoder per-step state, inner-CFM latent/solver state, sampler RNG, conv rings, word buffers, offset (F3); channel-id guard drops any late emit for the old occupant.
- If mishandled: evict from the temporal loop but leave the depth or inner-CFM state live → the next admit into slot 5 inherits stale depth/latent state → cross-user audio contamination (a privacy disaster, F3).

### ARCH-98 — Mode promotion mid-flight: a 2nd paradigm-distinct stream arrives at an Inline edge box
- Level: EXTREME
- Pipeline: Inline single AR stream → a 2nd, paradigm-distinct (CFM) stream arrives
- Axes: arch:AR, arch:flow, mode:auto-promote, §8
- Scenario: `mode=auto` starts Inline (B=1) for one AR stream; a second stream that needs the step-bucket CFM path arrives.
- System must: lazily promote to Stage-batched (spin up the ledger + per-stage micro-engines on demand, §8) WITHOUT changing the DAG/stages/nesting/placement (only the executor differs — Inline calls the same stage-forward with B=1); admit the CFM stream against the now-active duty ledger.
- If mishandled: stay Inline and serialize the two distinct-paradigm streams on the calling thread → the CFM solve blocks the AR frame clock; or rebuild the model graph on promotion (there is no second implementation — §8).

### ARCH-99 — Calibration must measure each paradigm's T_step under SYNTHETIC CO-LOAD
- Level: EXTREME
- Pipeline: AR + CFM + diffusion + encoder, calibration lifecycle
- Axes: arch:AR, arch:flow, arch:diffusion, calibration, §6/§8.3b
- Scenario: Building the duty ledger for a box that will co-host four paradigms.
- System must: calibrate `T_step(B_active)` PER stage PER substrate UNDER synthetic co-load (§6/§8.3b — the AR step's time changes when a CFM solve is contending for bandwidth), persist keyed `sha256 × device × driver × warm-set`; measure WITHOUT the profiler (it distorts latency, catalog B); the torch sidecar reports its footprint+duty at handshake.
- If mishandled: calibrate each paradigm in isolation → the co-load contention is unmeasured → admission over-admits → the combined workload underruns under real co-residency.

### ARCH-100 — The bottleneck stage is the binding admission constraint, not the AR stage
- Level: EXTREME
- Pipeline: AR (cheap) → CFM/codec/vocoder (the actual bottleneck), many streams
- Axes: arch:AR, arch:flow, arch:feedforward, bottleneck-admission, §6
- Scenario: A DAG where the CFM/codec/vocoder — not the AR — is the slowest stage.
- System must: admit against the BOTTLENECK stage's SLO (§6 — often the CFM/codec/vocoder, not the AR; every stage carries its own SLO + duty entry); test the bottleneck stage, not the AR stage; back-pressure parks the upstream AR when the downstream bottleneck queue fills (never drops).
- If mishandled: AR-only admission → admit more streams than the CFM/vocoder can sustain → the bottleneck queue fills → back-pressure stalls AR → cascading underruns (the exact §3.2/§6 bug).

### ARCH-101 — A masked-diffusion fixed-K stream and a variable-NFE nested stream: different "step" meanings
- Level: EXTREME
- Pipeline: SoundStorm (fixed-K parallel-unmask) + CALM-class (variable-NFE inner) co-resident
- Axes: arch:masked, arch:nested, NFE-vs-K, bucket-key
- Scenario: One stream's "steps" are K confidence-unmask iterations (no KV); another's are inner-NFE ODE steps per AR frame.
- System must: bucket the masked stream by (model, length, K) and the nested stream's inner by (model, NFE) — these are DIFFERENT step semantics (parallel-unmask vs sequential-solve) and DIFFERENT batchers; they share the GPU temporally via the duty ledger, never co-batched.
- If mishandled: conflate K-iterations with NFE-steps in one bucket → shape/semantic mismatch → either a crash or a fused pass that mis-denoises both.

### ARCH-102 — Heterogeneous placement: AR on GPU ∥ CNN-vocoder on NPU ∥ conv-encoder on CPU-AMX, zero-copy
- Level: EXTREME
- Pipeline: AR (GPU) ∥ CNN vocoder (NPU) ∥ conv STT encoder (CPU-AMX), coherent memory
- Axes: arch:AR, arch:feedforward, arch:encoder, placement, zero-copy, §3.4
- Scenario: A full duplex agent with three stages, each on its best substrate, on a coherent-memory box.
- System must: follow the immovable weights (AR's 3–6 GB on GPU, codec's small weights on NPU, §3.4) + paradigm×substrate affinity (AR→GPU, conv→NPU/CPU-AMX); cross each boundary ZERO-copy via `SharedHostBufType` (the copy degenerates to a pointer alias on coherent memory, §3.4); budget the shared bandwidth so the 3-way split doesn't oversubscribe.
- If mishandled: place AR on the NPU (breaks the static-shape contract → no realtime) or insert a real copy at each coherent boundary (pays DMA that should be a pointer alias) or oversubscribe the shared bus (3 engines saturate one ceiling).

### ARCH-103 — Per-codebook delay ring + nested inner solve + ring-KV wraparound, all at once
- Level: EXTREME
- Pipeline: delay-pattern nested AR (inner CFM) over a long utterance past ctx
- Axes: arch:AR, arch:nested, delay-pattern, kv:ring, F4+F8
- Scenario: A delay-pattern codec-LM with a nested inner CFM, running long enough to wrap the ring.
- System must: simultaneously honor the per-codebook delay ring (max_delay+2, F8) AND the temporal ring-KV wraparound mask (logical-position causal+window, F4) AND the inner-CFM per-frame batched solve — three independent ring/mask correctness rules in one forward.
- If mishandled: get any one wrong (delay collision, wraparound future-attend, or inner-solve desync) → corruption that only appears at the intersection (long utterance + delay pattern + nesting), nearly impossible to repro without the combined load.

### ARCH-104 — One model exercises ALL THREE execution classes across its stages
- Level: EXTREME
- Pipeline: encoder (bucket) → AR-outer (lockstep) → nested variable-NFE inner head (third class) → diffusion vocoder (bucket)
- Axes: arch:encoder, arch:AR, arch:nested, arch:diffusion, three-classes, L5
- Scenario: A single model whose stages span lockstep-AR, step-bucket, AND the third "AR-outer + generative-inner variable-NFE" class.
- System must: place the encoder + diffusion-vocoder in step-bucket, the AR-outer in lockstep, and the nested variable-NFE middle stage in the THIRD class (compose lockstep-outer + variable-NFE-inner step-bucket per step, L5); four declared batch policies in one manifest DAG; admission sums all four stages' duty.
- If mishandled: collapse the three classes into two batchers → the variable-NFE middle stage gets forced into either lockstep (desync across NFEs) or pure step-bucket (loses the outer frame clock) → wrong audio + budget violations.

### ARCH-105 — Spill/rebalance a co-resident-paradigm stream without dropping a frame
- Level: EXTREME
- Pipeline: AR + nested-CFM streams, DC replica rebalance under load
- Axes: arch:AR, arch:nested, kv-migration, L16/§6
- Scenario: A replica is overloaded; a stream (carrying ring-KV + inner-solver state) must move to another replica.
- System must: migrate via constant-time append-only KV migration (Llumnix; sub-ms–5 ms for voice ctx via NIXL/FlowKV, L16) — BUT one decode-step > one frame, so mid-stream migration drops ≥1 frame UNLESS the CLIENT PLAYBACK BUFFER masks it (L16/L9); only migrate streams with enough playback buffer to absorb the gap.
- If mishandled: migrate a low-buffer stream → the ≥1-frame migration gap surfaces as an audible glitch; or migrate without moving the inner-solver state → the destination resumes the nested forward from a stale latent.

### ARCH-106 — MTP acoustic path + separate diffusion vocoder + KV-quant big-KV S2S, one box
- Level: EXTREME
- Pipeline: MTP-talker (direct-emit) → diffusion vocoder (loose) + a co-resident Moshi-7B S2S (int4 KV)
- Axes: arch:MTP, arch:diffusion, arch:AR, kv:quant, co-residency
- Scenario: An MTP-TTS (2-5× via direct-emit) with a separate diffusion vocoder, sharing a box with a big-KV Moshi-7B S2S that needs int4 KV for concurrency.
- System must: run the MTP talker as direct-emit lockstep (rectangular-preserving, NOT spec-decode, L14), the diffusion vocoder as a separate step-bucket node, and the Moshi-7B with int4 KV-quant (25→101 streams, the dominant concurrency lever for its 32 KV heads, §1.6); three cohorts on the duty ledger + the int4-KV stream budgeted for its larger slot ceiling.
- If mishandled: apply spec-decode to the MTP path (0.98× slowdown, destroys the rectangular batch, L13) or leave Moshi-7B at fp16 KV (cap 25 when 101 was reachable) → both the wrong lever for the wrong paradigm.

### ARCH-107 — NFE=1 distilled stream collapses to feedforward WHILE a NFE=10 stream still solves, same bucket family
- Level: EXTREME
- Pipeline: IntMeanFlow (NFE=1) + CosyVoice-CFM (NFE=10), same flow family, co-resident
- Axes: arch:flow, NFE:1-and-10, bucket-collapse, L15
- Scenario: A distilled NFE=1 stream (feedforward) and a full NFE=10 stream of the same flow family arrive together.
- System must: route the NFE=1 stream as a single feedforward pass (no solver loop, the bucket collapses, L15/ARCH-9) and the NFE=10 stream through the full step-bucket solve — different N → different buckets even in the same family; the NFE=1 stream needs no per-step schedule.
- If mishandled: force the NFE=1 stream through a 10-trip loop (10× wasted) or force the NFE=10 stream to feedforward (catastrophically under-denoised → noise).

### ARCH-108 — A 75 Hz nested-CFM stream: the tightest budget meets the heaviest inner solve
- Level: EXTREME
- Pipeline: nested AR (75 Hz, 13.3 ms budget) + inner CFM (N-step) per frame
- Axes: arch:nested, arch:flow, frame-rate:75Hz, budget-collision
- Scenario: The worst budget collision — a high-frame-rate (75 Hz, 13.3 ms) nested model running an inner N-step CFM per frame.
- System must: recognize `T_step = T_ar + inner_steps × T_inner` likely EXCEEDS 13.3 ms even at the 38×@64 tiny-T batch profile → either reduce inner-NFE (graceful, L9), drop to a lower-frame-rate codec, or REJECT (the inner solve can't fit a 75 Hz tick on this substrate, §4.4/§6).
- If mishandled: admit it at 75 Hz against `T_ar` alone → the inner solve blows the 13.3 ms tick every frame → continuous dropout (the tightest-budget + heaviest-inner is the canonical non-admit case).

### ARCH-109 — Cohort regroup churn: streams entering/leaving cohorts every few ticks
- Level: EXTREME
- Pipeline: many short utterances across several (model, frame_rate) cohorts
- Axes: arch:AR, cohort:churn, cuda-graph-stability, F7
- Scenario: High turnover — streams constantly entering/leaving cohorts as short utterances start/finish.
- System must: keep each cohort's CUDA graph stable by MASKING idle slots (not removing → B constant → one graph lasts the server lifetime, F7); admit new streams into masked slots of the right cohort (don't re-cohort/re-capture per stream); the slot count per cohort is fixed at capture.
- If mishandled: drop finished streams from the batch → B changes → re-capture the graph every few ticks → catastrophic capture overhead (F7 — "drop them → B changes → re-capture every frame").

### ARCH-110 — Out-of-order chunks across a multi-paradigm fan-in (text-only vs text+audio S2S)
- Level: EXTREME
- Pipeline: thinker (text) → talker (AR) → vocoder, with a conditional text-only terminal
- Axes: arch:AR, dag:multi-terminal, fan-in, G11
- Scenario: An S2S where some requests are text-only (no audio terminal) and some are text+audio, chunks arriving out of order.
- System must: collect partials by stage, gate on the PER-REQUEST expected terminal set (`wait_for_fn`, G11), let a request narrow its terminals (text-only vs text+audio), ignore inactive terminals; monotone chunk_id reorders; route_fn ∈ static `next` (analyzable).
- If mishandled: fixed multi-terminal wait → a text-only request hangs forever waiting for the audio terminal that will never fire (G11 fan-in deadlock).

### ARCH-111 — A nested stream's inner CFG-parallel must use a seeded generator or diverge
- Level: EXTREME
- Pipeline: nested AR + inner CFM with CFG-parallel inner solve, multi-slot
- Axes: arch:nested, arch:flow, CFG, determinism, B/catalog
- Scenario: The inner CFM solve runs CFG cond/uncond in parallel across the outer batch.
- System must: pass a seeded `generator` to the inner scheduler step (CFG-parallel vs sequential diverges without it — a non-determinism bug, catalog B); accept per-stream-only determinism (bitwise cross-stream is impossible, atomic reductions, H/vLLM-core); float64 Gumbel for the sampler.
- If mishandled: unseeded CFG-parallel inner solve → cond/uncond branches diverge from the sequential reference → per-frame audio inconsistency that only appears in the batched nested path.

### ARCH-112 — Five-paradigm worst case: AR + nested-variable-NFE + masked + diffusion + token-AR-STT, one GB10
- Level: EXTREME
- Pipeline: AR-codec-LM + nested(AR+variable-NFE CFM) + masked-diffusion + DDPM-head + token-AR-STT, all co-resident
- Axes: arch:AR, arch:nested, arch:masked, arch:diffusion, arch:encoder, max-co-residency
- Scenario: The absolute stress case — every paradigm and execution class at once on one GB10's shared 273 GB/s.
- System must: run FIVE batch policies (two lockstep cohorts: codec-LM + nested-outer; three step-bucket families: masked-K, DDPM-N, token-AR-STT-paged) each on its own thread/clock/NFE; admit IFF per-substrate compute duty ≤ S AND shared-bandwidth duty ≤ S·273 GB/s AND every stage's slot/KV/window reservable; the bottleneck (likely the 25-step DDPM or the variable-NFE inner) gates admission; reject/degrade per-paradigm before glitching.
- If mishandled: any of — AR-only admission (the compute-bound paradigms blow the budget), fused mixed-paradigm step (incompatible physics), unbudgeted shared bandwidth (5 engines oversubscribe one ceiling), or one flat watchdog/deadline across the 5 step-times → cascading failure that's invisible until the exact 5-way co-load.

### ARCH-113 — Two STT paradigms co-resident: frame-sync RNNT (lockstep) + AED (paged), shared encoder dedup
- Level: EXTREME
- Pipeline: parakeet RNNT (lockstep) + Whisper-AED (paged decode), possibly shared encoder
- Axes: arch:RNNT, arch:AR, arch:encoder, two-STT-paradigms, dedup
- Scenario: A box serving both frame-sync RNNT STT and token-AR AED STT concurrently.
- System must: lockstep the RNNT (frame-sync emit, cache-aware encoder, L11) AND paged-decode the AED (text length ≠ frames, §4.1) — two STT paradigms, two batchers; dedup the shared conv encoder if both use it (one micro-batch feeding both decoders, §3.2); separate KV regimes.
- If mishandled: one STT batcher for both → force the AED into lockstep (frame-count mismatch) or the RNNT into paging (forfeit the frame-sync + cache-aware win) → wrong batcher for one paradigm.

### ARCH-114 — Inner head is compute-bound at tiny-T but a DDPM inner at large-T is not — admit accordingly
- Level: EXTREME
- Pipeline: (a) nested AR + T4 inner patch vs (b) nested AR + T64 DDPM inner
- Axes: arch:nested, arch:flow, arch:diffusion, batch-profile-split
- Scenario: Two nested models — one with a tiny-T (T4) inner that batches 38×@64, one with a large-T (T64) inner that batches like chunk-diffusion (10×@64) or collapses.
- System must: classify the inner head by its latent size — tiny-T inner admits with the AR-like 38× profile (nesting net-positive), large-T inner admits with the sublinear profile and may need chunking/amortization (§1.5); admission uses the MEASURED inner profile, not "it's nested so it's cheap."
- If mishandled: assume all nested inners batch 38× → over-admit the T64-DDPM-inner streams (their inner is compute-bound, not launch-bound) → budget blowout under batch.

### ARCH-115 — Live VoxServe-style binary-viability scheduling across mixed paradigms
- Level: EXTREME
- Pipeline: AR + CFM + masked streams, soft-deadline risk scheduling
- Axes: arch:AR, arch:flow, arch:masked, viability-objective, L3
- Scenario: Mixed-paradigm streams where, once a stream will deliver in time, further latency reduction is worthless (the binary streaming-viability objective).
- System must: schedule by RISK-OF-VIOLATION (VoxServe, 10-20× over vLLM/SGLang, L3) — prioritize the stream/stage closest to missing its deadline regardless of paradigm; a viable AR stream yields GPU to an at-risk CFM solve; cadence protected by the client playback buffer (no cross-replica migration needed, L3).
- If mishandled: minimize average latency uniformly → spend GPU shaving an already-viable AR stream while an at-risk diffusion solve misses its deadline → an avoidable glitch on the compute-bound paradigm.

## Coverage

This catalog enumerates **115 distinct scenarios** spanning every model paradigm WaaV Infer serves and their combinations, graded SIMPLE (ARCH-1–14: each paradigm landing in its correct batcher — AR→lockstep, flow/diffusion/masked→step-bucket, encoder-decoder split, NFE=1 collapse, MTP-not-spec-decode, delay-pattern lag, the two-clock derivation) → INTERMEDIATE (ARCH-15–39: single paradigm under frame-rate/cohort stress, idle-masked-slot cost, ring-KV wraparound, the F1 token-substitution + F4 logical-position correctness traps, the precision matrix fp8/int8/KV-quant + per-component + per-substrate, CFG-folding non-universality, delta-streaming, no-D2H-sync, tile-quantization, warmup/graph/eager) → COMPOUND (ARCH-40–80: nested AR+inner staying in-forward, the inner head batching across the outer batch, CosyVoice2-3-node vs dots.tts-2-node data-driven topology, the **THIRD execution class** with per-stream variable inner-NFE, DiTAR patch-stride, FlashTTS dual-batcher-break, Moshi temporal+depth, qwen3-tts talker+sub-talker, the can't-co-batch-AR+diffusion rule, per-stage independent batch sizes AR≥4/codec=1, prefix-key fingerprinting + hybrid-KV, dynamic fan-in deadlock, nested co-eviction, tiny-T-batches-like-AR, spec-decode scoping, long-form ring-overflow, cross-model codec dedup, full-duplex always-on barge-in, variable lifetime, marker/flush, one-model-three-batchers) → EXTREME (ARCH-81–115: the headline four-paradigm and five-paradigm co-residency on one GB10 each on its own clock/NFE, shared-273 GB/s-bandwidth contention, multi-frame-rate-clock co-residency, variable-NFE sub-bucketing within one nested batch, DC-scale three-batcher disaggregation, mixed per-component+per-substrate precision, crash-isolation + per-paradigm watchdog + graceful degradation across paradigms, intra-node spatial P/D vs chunked-prefill firewall, two-KV-regime co-residency, triple-nested co-eviction, calibration-under-co-load, bottleneck-stage admission, NFE-vs-K different step semantics, KV-migration playback-buffer masking, cohort-regroup graph stability, multi-terminal fan-in, CFG-parallel determinism, the tightest-budget-meets-heaviest-inner 75 Hz non-admit case, and VoxServe binary-viability risk scheduling). The relevance bar holds throughout: every scenario is a real model class (named where applicable — Orpheus, CosyVoice2, dots.tts, VibeVoice, SoundStorm, DiTAR, FlashTTS, CALM/VoxCPM, Moshi, qwen3-tts, parakeet, Whisper-AED, FlexiCodec) interacting with a specific, load-bearing engine mechanism (the two batchers, frame-rate cohorting, the nesting rule, the duty ledger, the ring-vs-paged KV split, per-component/per-substrate precision, and the F/G/H/I/L failure-catalog correctness traps), with no padding or duplication.

**File:** `/home/bud/ditto/waav/WaaV/inferv2/scenarios/04_arch.md`
