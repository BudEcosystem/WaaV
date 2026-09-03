# Path-B Arbitrary-Batch — FLEET-WIDE DEFECT / PERF RISK MATRIX

**Date:** 2026-06-26. **Host:** GB10 (Grace-Blackwell sm_121, 121 GiB unified CPU+GPU pool), aarch64.
**Source of truth:** every other tch/ORT model screened against the **Wave-1 defect catalog**
(`PATHB-ROLLOUT-WAVE1-STATUS.md` §4) + the **batch plan** (`PATHB-ARBITRARY-BATCH-PLAN.md` §3/§6/§7).
**Scope:** the post-Wave-1 fleet (Wave-1 itself = qwen3_tts SHIP / csm HOLD / dia2 + misotts REVERT).

## The Wave-1 defect classes (the screen)

- **D1 — PERMANENT WEDGE at B≥16.** Root: CFG cond+uncond models reserve a GROUPED ring (≥2 rows/slot) so the
  row count DOUBLES (or triples) and overflows a fixed `MAX_SLOTS` cap, OR a slow per-cohort serial model crosses
  the synth deadline and the §4.2 admission-slot ledger leaks → permanent denial-of-service until restart.
- **D2 — SAMPLED-RNG DIVERGENCE.** Root: sampled (not greedy) models share the **process-global tch `manual_seed`**;
  interleaved per-slot steps in the mux loop clobber each other's RNG stream → a cohort row diverges from its solo run.
- **P1 — depth-bound PARTIAL speedup.** A per-frame sub-decoder/depformer/dual-AR/flow-head stays per-slot → the
  ring only batches the backbone (~3% lever, the csm/dia2 lesson).
- **P2 — MoE per-expert token-gather.** Breaks the per-row reduction (zonos2).
- **P3 — FIG / hybrid AR+flow** (two batchers: AR ring + step-bucket/FIG).
- **P4 — heavy VOCODER single-transient OOM** (the S3Gen ~21.7 GiB lesson; competes with the resident rings).
- **P5 — per-stride READ-BACK cost** (contiguous/full_masked materialize; only `append_view` is zero-copy).
- **P6 — precision** (bf16/fp16 → Fork A1 still codes-identical, but the SOLO golden may itself be a batched-CFG run;
  fp16 has ZERO in-tree batched-vs-solo measurement → own oracle).

---

## 1. THE MATRIX (model × arch × sampling × cfg × sub-decoder × vocoder × defect-risks)

### Group: codec-AR remainder (lockstep ring)

| model | arch class | sampling | cfg | sub-decoder | vocoder | defect/perf risks |
|---|---|---|---|---|---|---|
| **dia** | Enc-dec codec-AR (Nari Dia-1.6B): 12L byte-encoder + 18L GQA cross-attn decoder, 9-ch delay stream; bf16/CUDA. dia.rs:1-11,91,79 | **GREEDY** (argmax, RNG-free) dia.rs:31-32,480,513 | **YES** — CFG always on (gs=3.0), 2-branch (cond+uncond) dia.rs:28,948-954; `KvCache::new(branches=2,…)` dia.rs:959 ✓verified | NONE — 9 codebooks PARALLEL via reshaped head dia.rs:420-421; delay [0,8,9..15] | HEAVY — descript/dac_44khz DAC whole-body, 44.1 kHz, compute dtype dia.rs:33-34,114 | **D1-HIGH** (CFG-grouped-ring twin of dia2, B=16→32 rows) · **P4-MED** (44.1 kHz DAC) · **P5-MED** (`append_contiguous_masked` MATH-SDPA, NOT graphable) |
| **higgs** | Codec-AR (Boson higgs-v3-4b, Qwen3-4B GQA, no DualFFN/MoE), 8-cb delay stream; **fp16**/CUDA. higgs.rs:1-11,40 | **SAMPLED prod default** (temp 0.8, top-k 50, MT19937) higgs.rs:48-49,566-587,610 ✓verified; greedy = gate only | **NO** — single forward, no uncond | NONE — 8 cb PARALLEL, tied head reshaped higgs.rs:263-265 | MODERATE — DAC-family, 24 kHz, 960 samp/frame higgs.rs:18-20,272-277 | **D2-HIGH (prod default)** (single `manual_seed(SEED)`/generate higgs.rs:610) · **P6-MED** (fp16, no in-tree fp16 oracle) · **P4-LOW/MED** (24 kHz DAC) |
| **higgs_v2** | Codec-AR (Boson higgs-v2-3B, LLaMA-3.2-3B GQA + per-LAYER DualFFN), 8-cb delay; **fp16**/CUDA. higgs_v2.rs:1-13 | **SAMPLED prod default** (temp 1.0 do_sample) higgs_v2.rs:125,608-633,651; greedy = gate only | **NO** | NONE depth — DualFFN is per-LAYER, branch-UNIFORM at decode (forward_uniform is_audio=true) higgs_v2.rs:264-300; NOT a per-row expert gather | MODERATE — DAC codec, 24 kHz higgs_v2.rs:29-34 | **D2-HIGH (prod default)** (single `manual_seed(SEED)` higgs_v2.rs:651) · **P6-MED** (fp16) · **P2-LOW (retire)** DualFFN is branch-uniform, NO permute hazard |
| **s2_pro** | DUAL-AR (fishaudio s2-pro): 36L Qwen3 slow-AR + 4L Qwen3 fast-AR (10 RVQ cb); firefly-DAC 44.1 kHz; bf16. s2_pro.rs:1-25,35 | **GREEDY ALWAYS** (RNG-free) s2_pro.rs:16,1035,1066-1074 | **NO** | **DUAL-AR (depformer-class)** — 4L fast-AR reset-per-frame s2_pro.rs:1076,1050 (csm pattern) | MOD-HEAVY — firefly modded-DAC (+8L transformer +ConvNeXt) 44.1 kHz s2_pro.rs:21-25 | **P1-HIGH** (fast-AR per-slot, backbone-only ring) · **P4-MED** (firefly DAC 44.1 kHz) · **P5-MED** (bf16 contiguous) |
| **neutts** | Single-codebook codec-AR (NeuTTS Air 0.5B, Qwen2), 1 FSQ code/frame, ext ONNX NeuCodec; bf16. neutts.rs:1-23 | **SAMPLED default** (temp 1.0, top-k 50, top-p 0.8, rep-pen 1.1) neutts.rs:152,160,742,755; greedy = gate | **NO** | NONE — single FSQ id/step neutts.rs:13-14,710 | **LIGHT (off-GPU)** — NeuCodec ONNX on CPU EP neutts.rs:32,314,481-482 | **D2-HIGH (prod default)** (`manual_seed(SEED)` neutts.rs:755 + per-row rep-penalty state) · **P4-N/A** (codec on CPU) · **P5-LOW/MED** |
| **zonos2** | MoE codec-AR (Zyphra Zonos2): 28L GQA backbone, MoE FFN L3..26 (16 exp top-1, L26 top-2, EDA), 9 PARALLEL DAC cb; bf16. zonos2.rs:1-32,38 | **GREEDY** (as implemented) zonos2.rs:36-37,855,915-946 | **NO** | NONE depth — 9 cb parallel; BUT EDA router-state carried ACROSS MoE layers per-row zonos2.rs:404,420-424 | HEAVY — descript/dac_44khz (VERBATIM from dia), 44.1 kHz zonos2.rs:4-5 | **P2-HIGH (headline)** per-token expert loop zonos2.rs:450-470 breaks per-row reduction → must mask-all-experts; EDA router per-row · **P4-MED** (44.1 kHz DAC) · **P6-MED** (f32-forced router, fragile bf16 reassoc) |

### Group: hybrid AR + flow / diffusion (two batchers)

| model | arch class | sampling | cfg | sub-decoder | vocoder | defect/perf risks |
|---|---|---|---|---|---|---|
| **cosyvoice3** | hybrid AR+flow (codec-AR speech-LM + CFM flow + HiFT); bf16 LM. cosyvoice3.rs:243 | **SAMPLED** (ras_sampling nucleus + RAS-fallback) cosyvoice3.rs:269,310-330,280-281 | **NO on AR** (CFG only at flow head, host batch-2) cosyvoice3.rs:711-719 | NONE (single speech token/step) | LIGHT/MOD — HiFT + ONNX flow estimator, 24 kHz cosyvoice3.rs:21-22 | **D2-HIGH** (ras_sampling on global RNG; RAS-fallback varies draw count cosyvoice3.rs:71) · **P3-CERTAIN** (AR ring + CFM batcher) · **P5-LOW/MED** (contiguous) · **P6-MED** |
| **voxtral_tts** | hybrid AR+flow (codec-AR semantic backbone + per-frame 7-step rectified-flow + FSQ); bf16. voxtral_tts.rs:786 | **MIXED**: cb0/AR-axis **GREEDY** f32 argmax voxtral_tts.rs:308-315; acoustic = per-frame flow noise voxtral_tts.rs:798 | **NO on AR** (CFG inside per-frame flow head, batch-2) voxtral_tts.rs:338-343 | per-frame FLOW head (7-step Euler ODE) voxtral_tts.rs:329-344 — P1-class | LIGHT — causal conv + 4 blocks, 24 kHz | **P3-CERTAIN** (greedy AR ring + flow batcher) · **D2-MED (flow-only)** x0 randn per frame voxtral_tts.rs:798; AR axis SAFE · **P1-MED** · **P6-LOW** (cb0 greedy f32). x0-replay seam voxtral_tts.rs:780 = ready determinism hook |
| **dots** | hybrid AR+flow CONTINUOUS-latent (Qwen2 ring + DiT flow head, no discrete codec); bf16. dots.rs:312 | **SAMPLED-via-noise, deterministic-decision** (per-patch randn dots.rs:1574,1624; EOS>0.8 dots.rs:115) `manual_seed(0)` dots.rs:1649 | **VARIANT** — flow_matching: host batch-2 CFG dots.rs:49-50; meanflow: NO CFG. NO CFG on backbone | DiT flow head (10-step Euler × 18 DiT × 2 CFG) + PatchEncoder dots.rs:550-551 | **HEAVY** — 48 kHz f32 BigVGAN AudioVAE dots.rs:18-23,35 | **P4-HIGH** (48 kHz f32 BigVGAN whole-body) · **P3-CERTAIN** · **D2-MED** (per-patch randn order dots.rs:44-46) · **P6-MED** (maxΔ=0 on FLOAT patch, not int codes) |
| **indextts2** | hybrid AR+flow (GPT-2 mel-LM + DiT-CFM + BigVGAN); **f32** AR. indextts2.rs:223,241-261 | **GREEDY** (do_sample=False) indextts2.rs:11,241,259; back-half CFM noise off-axis indextts2.rs:601-602 | **NO on AR** (CFG in back-half DiT-CFM batch-2) indextts2_backhalf.rs:332-342 | NONE on AR; DiT-CFM (25-step) runs once/utterance indextts2_backhalf.rs:23 | **HEAVY + CPU-FATAL** — BigVGAN v2 22 kHz, SIGSEGV on CPU (CUDA-only, no bit-twin) indextts2.rs:10,356 | **P6-FAVORABLE** f32+greedy = Fork-C free-vs-solo · **P4-HIGH** (BigVGAN, no CPU bit-twin) · **P3-CERTAIN** · **P1-LOW/MED**. Pos-skip scar indextts2.rs:248-250 |
| **vibevoice** | hybrid AR+diffusion DUAL-RING CFG (Qwen2 token-LM + DDPM head + streaming VAEs); bf16. vibevoice.rs:16 | **GREEDY** (argmax_constrained) vibevoice.rs:16,1000-1002; diffusion noise seeded+fix_std deterministic vibevoice.rs:961,20 | **YES — SECOND full backbone ring** (neg_caches forwarded each step) vibevoice.rs:975,1037 ✓verified + head batch-2 vibevoice.rs:911-925 | DDPM head (10 steps) + acoustic VAE decode + semantic VAE re-encode, all per-slot vibevoice.rs:905-927,1018-1021 | MODERATE — streaming SEANet VAE, 24 kHz, per-chunk vibevoice.rs:300-303 | **D1-HIGH** (2nd growing backbone ring → ~2·n rows + SLOW) · **P1-HIGH** (DDPM+dual-VAE per token) · **P3-CERTAIN** · **P6-MED** (dual-ring golden) |
| **vibevoice_realtime** | hybrid AR+diffusion TRIPLE-LM (base + pos TTS-LM + neg TTS-LM + DDPM + VAE); bf16. vibevoice_realtime.rs:516 | **SAMPLED, "not bit-exact"** (per-latent randn, no reseed) vibevoice_realtime.rs:33,446 | **YES — THIRD full backbone ring** (ntts_caches each step) vibevoice_realtime.rs:503,549 ✓verified + head batch-2 vibevoice_realtime.rs:449-455 | triple-LM/step + DDPM + acoustic VAE vibevoice_realtime.rs:524-553 | MODERATE — streaming VAE (reuses vibevoice), 24 kHz vibevoice_realtime.rs:339 | **D2-HIGH** (sampled, global RNG, no reseed) · **D1-HIGH** (3 growing rings → row-multiply, SLOW) · **P3-CERTAIN** · **P1-HIGH**. The WORST hybrid (stacks BOTH classes) |
| **voxcpm2** | hybrid diffusion-AR **FIG** (MiniCPM-4 continuous-latent, fused decode_step = 10-step CFM+CFG inside ONE ONNX graph); **ORT**. voxcpm2.rs:7,18-21 | **SAMPLED-via-noise, deterministic-graph** (noise PCG32 Box-Muller keyed on voice,text — NOT tch global) voxcpm2.rs:17,33; graph stop_flag voxcpm2.rs:22 | **CFG BAKED INTO graph** (DEFAULT_CFG=2.0) voxcpm2.rs:18-21,51 — NO host axis, NO grouped-ring | dual KV (28L base + 8L residual); CFM fused inside decode_step voxcpm2.rs:49-50 | **HEAVY** — 48 kHz audio VAE (separate ONNX graph) voxcpm2.rs:24-25 | **P3-FIG (unusual)** whole-[B] decode_step graph, maxΔ=0 latent · **P4-MED/HIGH** (48 kHz VAE) · **D2-SIDESTEPPED** (host PCG per-slot) · **P5-LOW** (ORT Path-A) |

### Group: diffusion / flow one-shot (step-bucket, mostly f32 EASY)

| model | arch class | sampling | cfg | sub-decoder | vocoder | defect/perf risks |
|---|---|---|---|---|---|---|
| **omnivoice** | Masked-diffusion-LM (MaskGIT/LLaDA, 28L BIDIRECTIONAL Qwen3-0.6B, [1,8,s] grid, 32-step reveal, NO KV ring); **f32**. omnivoice.rs:87-94 | **GREEDY-with-DEAD-RNG** (Gumbel all-NaN → flat-index argmax) cfm/masked.rs:151-156; `manual_seed(0)` omnivoice.rs:762-764 | **YES** GS=2.0 but run as TWO batch-1 forwards (file REFUSES batch-2: flips codes) omnivoice.rs:69-70,772-782 | none; no KV ring (in-place reveal) | LIGHT — HiggsV2 DAC on **CPU** omnivoice.rs:509 | **P1-partial (step-bucket-axis)** — own batch-2-flips-codes trap → per-slot B=1 dispatch · **D2-LOW (latent, dead Gumbel)** · D1-NONE · P4-NONE (CPU) · P6-NONE (f32 pinned) |
| **viitorvoice** | Masked-diffusion NAR (omnivoice sibling, 28L BIDIR Qwen3-0.6B, [1,12,s], 34-step); **f32 ONNX/ORT-CPU**. viitorvoice.rs:200,423-424 | **SAMPLED** (FINITE Gumbel, value-dependent reveal) viitorvoice.rs:541-547; `manual_seed(0)` viitorvoice.rs:558-560 | **NO** GS=0.0 (single forward) viitorvoice.rs:84-86 | none; no KV ring | LIGHT — DualCodec ONNX on CPU EP viitorvoice.rs:426-429 | **D2-REAL** (true value-dependent Gumbel on global seed) · **P1-partial** (f32 not free at H=1024) · **P3-MINOR** (ONNX-CPU backbone + tch-device embed) · embed device-sensitivity doc:55-58 |
| **irodori** | RF-DiT NON-AR flow-matching (Echo-TTS, continuous 32-dim/48 kHz DACVAE latents, N-step Euler, FIXED length); **f32**. irodori.rs:1064-1106 | **DETERMINISTIC** (one seeded init randn drawn on CPU then →device) irodori.rs:1051-1054 | **YES** dual-CFG (text gs3, spk gs5), separate dit_forwards (not stacked), `.copy()` aliasing fix irodori.rs:1070-1101,1074-1076 | none (single DiT; no KV ring) | MEDIUM — DACVAE 4-block, 48 kHz, on DEVICE irodori.rs:1111-1117 | **P1-partial** (step-bucket DiT, maxΔ=0 waveform) · **P4-MEDIUM** (device DACVAE 48 kHz) · **D2-LOW** (single CPU draw, serializable) · P6-NONE (f32 CPU-drawn) |
| **pocket_tts** | HYBRID continuous-latent flow-LM (Moshi-class 6L AR backbone + SimpleMLPAdaLN flow head, lsd 1-step); **f32**. pocket_tts.rs:768-821 | **GREEDY** temp=0 (flow noise all-ZEROS, lsd deterministic, EOS-threshold) pocket_tts.rs:846,844-845 | **NO** | FLOW-HEAD per frame (lsd_decode) pocket_tts.rs:847 — P1-class | LIGHT/MED — Mimi SEANet, 24 kHz, single-shot | **P3-REAL** (AR ring + flow-head batcher) · **P1** (flow head per-slot; 6L backbone small lever) · **D1-MED** (EOS-bounded, no wedge; slow serial → §4.2 amplifier) · P6-NONE (f32) |
| **rsb** | Score-based Schrödinger-Bridge SDE speech-**ENHANCE** (NCSN++ U-Net, 3-step VE-SB on complex STFT, NO transformer/codec/LLM/KV); **f32**. rsb.rs:819-834 | **DETERMINISTIC core** (net_forward RNG-free) + per-step SDE randn; ODE path RNG-free rsb.rs:744-753,806-807 | **NO** (conditions by channel-stack) rsb.rs:751 | none | none — iSTFT only rsb.rs:784-804 | **D2-REAL-if-SDE / NONE-if-ODE** (use ODE or per-row RNG) · fixed-SHAPE equal-shape cohort (easiest to batch) · P4-NONE · P6-NONE (f32 TF32-off). **WRONG FAMILY (enhance, no TtsModel)** |
| **supertonic** | Flow-matching TTS (4 ONNX graphs, vector_estimator Euler inside graph, 8-step, FIXED length); **PROVEN 1SHOT precedent**. supertonic.rs:236 | **DETERMINISTIC** (host-side GaussianNoise PCG keyed on voice,text — NOT torch global) supertonic.rs:303,543 | **NO** | none | ONNX vocoder, 44.1 kHz, batched [B,C,L] supertonic.rs:561 | **ALL-CLEAR** — `synthesize_batch` ALREADY EXISTS maxΔ=0.0, 2.33×@B8 supertonic.rs:385,660. D2-NONE, D1-NONE, P4-LOW. Only gotcha: (L,T)-bucketing (pad leaks) |
| **kokoro** | One-shot VITS-class TTS (single StaticGraph: ids,style,speed→24 kHz; duration+decode in-graph). kokoro.rs:110-150 | **DETERMINISTIC** one-shot (no host RNG) | **NO** | none | baked IN-graph, 24 kHz, light | **CLEAN** — 1SHOT candidate, NO synthesize_batch yet kokoro.rs:180. D2-NONE, D1-NONE, P4-NONE, P6-NONE. Needs equal-shape token-length bucketing |
| **melo** | One-shot multi-speaker VITS (single VITS ONNX graph, stochastic duration+flow INTERNAL). melo.rs:146 | STOCHASTIC-VITS but **GRAPH-INTERNAL** (fixed noise_scale scalars, no host RNG) melo.rs:167-169 | **NO** | none | baked IN-graph, 44.1 kHz | **CLEAN-ish** — 1SHOT, no synthesize_batch melo.rs:179. **D2 graph-internal must-gate** (RandomNormalLike [B] vs [1] batch-row-stability, the chatterbox-S3Gen class) · P6-MINOR (maybe fp16 export) |
| **vieneu** | **MISCLASSIFIED** — codec-AR TTS (Qwen3 768/12L GQA backbone + 2L acoustic DEPTH-decoder 16-cb RVQ + MOSS-Nano codec 48 kHz); ORT. vieneu.rs:414-477 | **SAMPLED** (per-cb rep-pen→temp→top-k→top-p→multinomial); u-draw = host PCG32 keyed on (voice,text), NOT global torch vieneu.rs:621,629 | **NO** | **DEPTH-decoder** (16 forced steps/frame) vieneu.rs:330-394 — csm/misotts pattern | MEDIUM — MOSS-Nano ONNX, 48 kHz stereo vieneu.rs:479-498 | **P1-partial** (depth-decoder per-slot; 768/12L small lever) · **D2-LOW (host PCG, NOT global)** if per-slot next_u closure kept vieneu.rs:456 · **D1-MED** (EOS-bounded; slow→§4.2) · **P4-MED** · **P6-LIKELY** (ORT fp16). **WRONG FAMILY (codec-AR)** |

### Group: STT decoder (two-stage: encoder cohort + AR decoder ring)

| model | arch class | sampling | cfg | sub-decoder | vocoder | defect/perf risks |
|---|---|---|---|---|---|---|
| **voxtral** | STT LLM-decoder AR ring (Mistral-4B, lockstep ASR); **fp16**. voxtral.rs:538,634,266 | **GREEDY** (argmax_first device tie-break, RNG-free) voxtral.rs:557,703 | **NO** (`KvCache::new(1,…)`) voxtral.rs:538 | NONE | NONE (STT→text) | **P6 (fp16, non-exempt)** own fp16 oracle; Fork-A1 only · **P5-LOW** (`append_view` zero-copy, the LOW-readback archetype) voxtral.rs:225 · D1-mitigated (no CFG). CLEANEST in group |
| **cohere** | STT AED (48L FastConformer enc ORT + 8L pre-LN AED decoder tch, plain MHA, learned abs-pos, cross-attn/layer); **fp16**. cohere.rs:414,228 | **GREEDY** (argmax, REPEAT_GUARD=24) cohere.rs:425,64 | **NO** | NONE (but per-layer CROSS-ATTN, enc K/V projected once at prefill — per-slot constant) cohere.rs:182-194 | NONE | **P6 (fp16)** own oracle; Fork-A1 · **P5-LOW** (`append_view`) cohere.rs:174 · **cross-attn ragged wrinkle** (per-row enc-K/V of per-row enc-length → extra mask convention) |
| **ark** | STT LLM-decoder AR ring (Whisper enc run-once + 24L Qwen2 decoder, audio-placeholder); **fp16**. ark.rs:520,266 | **GREEDY + BAD-WORDS suppression** (argmax_suppressed -inf mask) ark.rs:545,590,255 | **NO** | NONE | NONE | **P6 (fp16)** own oracle; Fork-A1 · **P5-LOW** (`append_view`) ark.rs:232 · per-row bad-words mask must apply per-row (Fork-A1-safe). voxtral-twin |
| **granite** | STT LLM-decoder AR ring (audio enc + projector + Granite decoder); **bf16**. granite.rs:846,547 | **GREEDY** (do_sample=False) granite.rs:858,969 | **NO** | NONE | NONE | **P6 (bf16, MEASURED flip 1/4@B4)** Fork-A1 only · **P5-MED** (`append_contiguous`, materializes/stride — CUDA-graph candidate) · has existing byte-id gate (trustworthy solo golden). Tier A1 |
| **canary_qwen** | STT LLM-decoder ASR (FastConformer enc + Qwen3-1.7B decoder + MERGED speech-LoRA + **multi-LoRA S-LoRA seam**); **bf16**. canary_qwen.rs:910,689 | **GREEDY** canary_qwen.rs:209,921 | **NO** | NONE (the complexity is the multi-LoRA seam) | NONE | **MULTI-LoRA CLOBBER (D2-shaped, deterministic)** — shared `Arc<Mutex>` selector, current `Single` mode clobbers cohort; FIX EXISTS unwired (`PerRow`/`set_active_per_row`) lora.rs:62,95 ✓verified, MUST use PerRow · **P6 (bf16)** Fork-A1 · **P5-MED** (`ViewContiguous`) |
| **higgs_stt** | STT LLM-decoder ASR (Whisper enc chunked ≤4s + Qwen3 decoder; DISTINCT from higgs TTS); **bf16**. higgs_stt.rs:583,388 | **GREEDY** higgs_stt.rs:255,595 | **NO** | NONE (≤4s windowing multiplies prefill, still one stream) higgs_stt.rs:116-121 | NONE | **P6 (bf16)** Fork-A1 · **P5-MED** (`ViewContiguous`) · variable-prefill wrinkle (wide ragged seqlens_k spread; footprint must use post-chunk count) |
| **vibevoice_asr** | STT LLM-decoder ASR (speech enc + connectors + Qwen2.5-**7B** decoder; NO diffusion head); **bf16**. vibevoice_asr.rs:351,168 | **GREEDY TEXT** (RNG-free decode); enc-side run-once sample() noise vibevoice_asr.rs:362,419 | **NO** | NONE | NONE | **P6 (bf16, LARGEST decoder H=3584 → highest flip Δ)** Fork-A1 · **D1-HIGHEST (serve-loop)** slowest decoder → most exposed to §4.2 slow-model wedge · **P5-HIGH** (`append_contiguous` + 7B) CUDA-graph candidate · enc sample() ratify concurrent-identical |

### Group: S2S (native duplex / turn-based)

| model | arch class | sampling | cfg | sub-decoder | vocoder | defect/perf risks |
|---|---|---|---|---|---|---|
| **hibiki** | native-S2S full-duplex Moshi RQ-Transformer (28L GQA backbone + 6L weights-per-step depformer + Mimi codec, 33-ch delay multistream); **f32**. hibiki.rs:718,721 | **GREEDY** (argmax, RNG-free) hibiki.rs:749,758,831,840 | **NO** (`KvCache::new(1,…)`) | **DEPFORMER** (16 serial steps/frame, reset per outer step) hibiki.rs:756-761,751-753 | MODERATE — native Mimi (enc+dec, split-RVQ); duplex pays BOTH encode + decode | **D1-LOW/INDIRECT** (greedy, no CFG; serve-loop only, K=1 today, no `DuplexStepModel`) · **P1-HIGH** (depformer + 2× Mimi per-slot) · **P4-LOW/MED** (2× Mimi/slot) · **P5-MED** (`append_contiguous`/`_masked`) · **P6-LOW** (f32; hand-rolled GQA to avoid flip hibiki.rs:189-192). NEEDS new `as_duplex`+`DuplexStepModel` workstream |
| **lfm2** | turn-based S2S audio-LLM (LFM2.5-Audio-1.5B, HYBRID 16L = 10 conv + 6 attn backbone + depthformer + STFT detok); **ORT** Path-A. lfm2_audio.rs:1-785 | **GREEDY** (argmax text + depthformer) lfm2_audio.rs:316-326,499-509 | **NO** (batch axis 1) | DEPTHFORMER (8 serial/frame, own 6L KV/frame) lfm2_audio.rs:462-512 | LIGHT — host ISTFT, 24 kHz | **P3-HIGHEST (own hybrid-cache primitive)** conv+attn cache can't ride attn-only ring lfm2_audio.rs:52-55,124-127 · **D1-INDIRECT** (serve-loop) · **P1** (depthformer per-slot) · **P5** (whole-present-cache absorb). Cleanest **StaysPerSlot** (turn-based) |
| **duplex_codec_ar** | **TEST-ONLY blueprint** — the proven batched `DuplexStepModel` (drives real chatterbox codec-AR Llama, single-cb D=1). duplex_codec_ar.rs:224 ✓verified (only impl) | **GREEDY/DETERMINISTIC** (inherits chatterbox argmax) | **NO** | NONE (single-cb) | N/A at seam (S3Gen decode downstream) | **NONE of D1/D2** — the POSITIVE CONTROL / golden blueprint (`s2s_duplex_ragged_concurrent_batched_bit_identical_and_scales`). P4 deferred downstream. Surfaces the gap: `LoadedModel` has no `as_duplex` ✓verified |

---

## 2. GROUPED BY WAVE-1 DEFECT CLASS

### D1 — PERMANENT WEDGE risk (CFG grouped-ring doubling OR slow-model serve-loop leak)
- **dia** — CFG-grouped-ring twin of dia2 (B=16→32 rows, `KvCache::new(branches=2)` dia.rs:959). HIGH.
- **vibevoice** — CFG realized as a 2nd growing backbone ring (~2·n rows) + slow (10-step DDPM + dual VAE/token). HIGH.
- **vibevoice_realtime** — CFG as a 3rd growing backbone ring (row-multiply) + slow + sampled. HIGH (worst).
- **vibevoice_asr** — no CFG doubling, but the LARGEST/SLOWEST decoder (Qwen2.5-7B) → highest §4.2 slow-model serve-loop wedge exposure. HIGH (serve-loop).
- *(Serve-loop-only, MITIGATED, indirect):* hibiki, lfm2, pocket_tts, vieneu, all STT decoder rings — bounded loops, no CFG doubling; inherit the §4.2 admission-leak only if/when flipped to a slow batched serve path.

### D2 — SAMPLED-RNG DIVERGENCE risk (process-global tch manual_seed clobber)
- **higgs** — sampled prod default (temp 0.8/top-k 50), single `manual_seed(SEED)`/generate. HIGH (prod default).
- **higgs_v2** — sampled prod default (temp 1.0 do_sample), single `manual_seed(SEED)`. HIGH (prod default).
- **neutts** — sampled prod default (temp 1.0/top-k/top-p/rep-pen), `manual_seed(SEED)` + per-row rep-pen state. HIGH (prod default).
- **cosyvoice3** — ras_sampling on global RNG; RAS-fallback varies the draw count (harder re-seed law). HIGH.
- **vibevoice_realtime** — sampled, "not bit-exact", per-latent randn on global RNG, no reseed. HIGH.
- **viitorvoice** — true value-dependent FINITE Gumbel on global seed. REAL.
- *(MED / contained):* voxtral_tts (flow-noise only; AR axis greedy-safe), dots (per-patch randn order).
- *(SIDESTEPPED by host-PCG, NOT global torch seed — keep per-slot instance):* voxcpm2, supertonic, vieneu, irodori.
- *(D2-shaped but deterministic — shared mutable state, not RNG):* **canary_qwen** multi-LoRA `Single`-mode clobber (fix = `PerRow`).
- *(LOW / latent — dead RNG):* omnivoice (Gumbel all-NaN → value-independent).

### P1 — depth-bound PARTIAL speedup (sub-decoder/flow-head stays per-slot)
- **s2_pro** (4L fast-AR), **vieneu** (16-step acoustic depth), **hibiki** (16-step depformer), **lfm2** (8-step depthformer) — depformer/dual-AR class.
- **pocket_tts**, **voxtral_tts**, **dots**, **indextts2**, **cosyvoice3** — per-frame/per-utterance flow-head class (also P3).
- **vibevoice**, **vibevoice_realtime** — DDPM head + VAE chain (HIGH, also D1).
- **omnivoice**, **viitorvoice**, **irodori** — step-bucket-axis partial (each slot's forward stays B=1 per the batch-2-flips trap / f32-not-free-at-H=1024).

### P2 — MoE per-expert token-gather (breaks per-row reduction)
- **zonos2** — the only true MoE breaker (per-token expert loop zonos2.rs:450-470; mask-all-experts + per-row EDA router). HIGH.
- *(retired):* higgs_v2 DualFFN is branch-UNIFORM at decode — NOT a permute hazard.

### P4 — heavy VOCODER single-transient OOM (competes with resident rings)
- **dots** — 48 kHz f32 BigVGAN AudioVAE whole-body. HIGH.
- **indextts2** — BigVGAN 22 kHz, CPU-SIGSEGV → CUDA-only, no CPU bit-twin. HIGH.
- **voxcpm2** — 48 kHz audio VAE (separate ONNX graph). MED/HIGH.
- *(MEDIUM):* dia, zonos2 (44.1 kHz DAC), s2_pro (firefly DAC 44.1 kHz), irodori (device DACVAE 48 kHz), vieneu (MOSS 48 kHz stereo).
- *(LOW/N-A):* higgs/higgs_v2 (24 kHz DAC), neutts/omnivoice/viitorvoice (codec on CPU), hibiki/lfm2/pocket_tts (light/streaming), supertonic/kokoro/melo (light in-graph), all STT (none).

### CLEAN — straightforward rollout (greedy + no-CFG + no-heavy-sub-decoder/wedge)
*(perf-clean for the AR/decode axis; precision/serve-loop still apply per fork)*
- **supertonic** — ALREADY DONE (`synthesize_batch` maxΔ=0.0, 2.33×@B8). The precedent.
- **kokoro** — clean 1SHOT (no AR/CFG/sampling/separate vocoder); just needs `synthesize_batch` wiring.
- **voxtral / cohere / ark** — greedy STT decoder rings, `append_view` zero-copy; only wrinkle is fp16 (P6, own oracle).
- **granite / higgs_stt / vibevoice_asr** — greedy STT decoder rings; bf16 (P6, Fork-A1) + read-back/serve-loop notes, but no D1-CFG / D2 / MoE.
- **indextts2** (AR axis) — f32 + greedy = Fork-C free-vs-solo, no D1/D2 on the ring (vocoder is the only headache).
- **duplex_codec_ar** — the positive control / golden blueprint (not a rollout target).

---

## 3. ROLLOUT PRE-EMPTION — the single shared fix per defect class

The Wave-1 RCA already isolated each root cause to a SHARED substrate fix, not a per-model patch. Apply each
ONCE so the remaining waves pre-empt the defect instead of rediscovering it 20× more.

### Pre-empt D1 (wedge) — TWO shared fixes, land BEFORE any slow/CFG model defaults on
1. **Serve-loop slow-model graceful-shed (the §1.4/§4.2 prerequisite — the actual Wave-1 blocker).** Make the
   admission slot charge/free **idempotent on mid-flight shed** in `serve_codec_ar_multiplexed_bounded` + the
   single `codec-ar-mux` thread, and stand up `serve_loop_graceful_shed_n_2_8_16_32` **including a SLOW model**.
   Root-cause the teardown SIGSEGV on the wedged state. This ONE fix unblocks every slow tch batcher
   (vibevoice/vibevoice_realtime/vibevoice_asr/hibiki/lfm2/pocket_tts/vieneu + dia2/misotts retro).
2. **Generalize the dia2 CFG-grouped-ring (`append_full_masked_group`/`forward_ring_grouped`) to a sized cap.**
   Make `MAX_SLOTS` count **physical ring rows**, with `group = branches` reserved per logical slot, so a CFG
   model never overflows. dia reuses it with the `append_contiguous_masked` (MATH-SDPA) read-back re-derived;
   vibevoice gets a **2-ring** layout (pos+neg), vibevoice_realtime a **3-ring** layout — all sized as
   `cap_rows / group` logical slots. The grouped read-back is byte-id-gated ONCE (the proven dia2 twin), then
   the CFG fleet inherits it.

### Pre-empt D2 (divergence) — ONE shared fix: per-slot RNG isolation
- Replace the process-global `tch::manual_seed` in the mux loop with **per-slot RNG-generator state** (or the
  dia2 per-(slot,step) re-seed law `SEED + slot*K + step`), so a row's draws are keyed only on `(slot,step)`,
  never on co-residents. Build it ONCE as a `PerSlotRng` the codec-AR-mux threads through `step_batch`; the
  dia2 force-solo oracle's "cohort-independence at a fixed slot" contract is the standing gate shape. Then
  **higgs / higgs_v2 / neutts / cosyvoice3 / vibevoice_realtime** inherit it directly; **viitorvoice**'s finite
  Gumbel inherits it via per-slot draw-order; **cosyvoice3**'s RAS-fallback needs the variable-draw-count
  variant of the law. The host-PCG models (voxcpm2/supertonic/vieneu/irodori) are pre-empted simply by keeping
  **one GaussianNoise/next_u instance per slot** (a closure-ownership rule, not new RNG code).
- **canary_qwen** is the deterministic D2-twin: the ONE fix is forcing the batched ring to use
  `AdapterSelection::PerRow` + `set_active_per_row` (already implemented, lora.rs:62,95) and supply per-slot
  `adapter_names` each step — never `Single`. Gate with a MIXED-adapter ragged cohort.

### Pre-empt P1 (partial) — ONE shared accounting rule + a deferred sub-decoder workstream
- **Declare backbone-ring coverage PARTIAL and report END-TO-END speedup, never AR-loop-only**, for every
  depformer/dual-AR/flow-head model (s2_pro, vieneu, hibiki, lfm2, csm/dia2/misotts retro, + the flow-head
  hybrids). The per-frame sub-decoder cross-slot batching is a **single SEPARATE workstream** (its own B27
  projector-rounding oracle), gated independently — not re-litigated per model.

### Pre-empt P2 (MoE) — ONE shared primitive
- A **mask-all-experts batched FFN** (every token runs every expert, then per-row top-k weighted sum) so the
  slot axis never enters an expert-gather permute, with **per-row EDA router-state** threaded by the ring and
  the router softmax/top-k forced f32. zonos2 is the only consumer today; building it once (with its dedicated
  MoE force-solo oracle + TF32-OFF class match) pre-empts any future MoE arch.

### Pre-empt P4 (vocoder-OOM) — ONE shared transient budget (the §4.4 fix)
- A **decode-concurrency semaphore + second `VramAccountant` leg** = `floor(free_arena /
  per_decode_transient(body_len))`, with **decode-batch width DECOUPLED from AR cohort width**. Land it
  **TOGETHER with the footprint fix** so admission cannot loosen ahead of the transient guard. This ONE budget
  protects every heavy-vocoder model (dots/indextts2/voxcpm2 HIGH; dia/zonos2/s2_pro/irodori/vieneu MED).
  IndexTTS-2 BigVGAN is CUDA-only-gated (no CPU bit-twin). Gate:
  `decode_concurrency_transient_budget_refuses_not_ooms`.

### Pre-empt P5 (read-back) + P6 (precision) — declare-once, gate-once
- **P5:** declare each arch's read-back class (`append_view` zero-copy: voxtral/cohere/ark; `contiguous`/
  `full_masked`/`ViewContiguous` materialize: granite/higgs_stt/canary/vibevoice_asr/csm/dia2/s2_pro/hibiki) and
  **route the materializing majority through CUDA-graph capture-once-replay** (graphable archs) so the
  device-residency win isn't eroded per stride. Budget per-stride read-back-bytes in the knee model.
- **P6:** build the reusable **`force_solo_oracle::<M: ArStepModel>`** (all-rows integer-code compare on a
  ragged staggered mid-finish cohort, narrow-argmax-gap corpus) ONCE; every bf16 model is Fork-A1 (per-row B=1
  reducing dispatch, fused-[B] forbidden with a live solo). **fp16 is NON-EXEMPT** and gets its OWN force-solo
  oracle (zero in-tree fp16 measurement) — first exercised on **higgs** (no CFG/depth, the simplest fp16 row),
  then voxtral/cohere/ark/higgs_v2. f32+greedy archs (indextts2/omnivoice/irodori/pocket_tts/rsb) are Fork-C
  free up to B_inv.

### The cross-cutting ordering (so each wave pre-empts, not rediscovers)
`Phase 0` footprint BUG-1(n_layers)/BUG-2(dtype) + min(arena,free_mem) budget + TF32-class co-residency reject
**must land first** (else any ring flip box-kills the unified pool). `Phase 1` the grouped/ragged ring + 4
read-backs. `Phase 2` the `PerSlotRng` + `force_solo_oracle` (incl. fp16) + bf16-floor kit. `Phase 3` the
serve-loop graceful-shed (D1 prerequisite) on the qwen3 pilot. Only then fan out the CFG fleet
(dia/vibevoice*), the sampled fleet (higgs*/neutts/cosyvoice3), the MoE (zonos2), the hybrids, the STT rings,
and the S2S `as_duplex` workstream — each inheriting the shared fix already proven.

---

## 4. SUMMARY

Screening the post-Wave-1 fleet against the two Wave-1 live-serve defects (D1 wedge, D2 divergence) plus the
perf hazards (P1 partial, P2 MoE, P4 vocoder-OOM, P5 read-back, P6 precision):

- **D1-wedge:** dia (CFG-grouped-ring twin of dia2), vibevoice (2-ring), vibevoice_realtime (3-ring),
  vibevoice_asr (slowest decoder → serve-loop leak). The CFG models double/triple the ring rows; the slow
  models leak the admission ledger. Both root causes are SHARED with dia2/misotts and pre-empted by the §1.4
  serve-loop graceful-shed fix + the sized grouped-ring cap.
- **D2-divergence:** higgs, higgs_v2, neutts, cosyvoice3, vibevoice_realtime (process-global-seed sampled),
  viitorvoice (finite Gumbel), plus the deterministic D2-twin canary_qwen (multi-LoRA `Single` clobber). The
  host-PCG models (voxcpm2/supertonic/vieneu/irodori) SIDESTEP D2 if each slot keeps its own noise instance.
  One shared `PerSlotRng` fix pre-empts the whole class.
- **P1-partial:** every depformer/dual-AR/flow-head model (s2_pro/vieneu/hibiki/lfm2/pocket_tts + the flow
  hybrids) — backbone-ring is a small lever; declare coverage PARTIAL.
- **P2-MoE:** zonos2 only (mask-all-experts + per-row EDA); higgs_v2 DualFFN retired (branch-uniform).
- **P4-OOM:** dots/indextts2/voxcpm2 HIGH (48 kHz/BigVGAN whole-body); pre-empted by the one §4.4
  decode-transient semaphore.
- **CLEAN:** supertonic (done), kokoro, voxtral/cohere/ark (fp16 only), granite/higgs_stt/vibevoice_asr (bf16
  Fork-A1), indextts2 AR axis (f32 Fork-C), duplex_codec_ar (the golden blueprint).

The headline: the Wave-1 defects are **not per-model surprises** — each maps to ONE shared substrate fix
(serve-loop shed + sized grouped-ring for D1; per-slot RNG for D2; mask-all-experts for P2; decode-transient
budget for P4; the reusable force-solo oracle incl. an fp16 variant for P6). Landing those six shared fixes in
Phase 0-3 lets the remaining waves PRE-EMPT the defects rather than rediscovering them, with the serve-loop
graceful-shed + sizing/footprint fixes as the hard ordering gate before any slow/CFG model defaults on.
