# WaaV Infer — Real-World Scenario Catalog: Text-to-Speech (TTS) Pipeline

Family scope: the TTS synthesis pipeline end-to-end — streaming first-audio/TTFA, the per-paradigm batch profiles (one-shot VITS; codec-AR lockstep; flow/CFM N-step; diffusion DDPM; masked-diffusion; AR+inner-diffusion nested; multi-AR/MTP), delta-vs-cumulative streaming correctness, chunk overlap-add/holdback, voice cloning + ref-audio conditioning, multilingual frontend/G2P, SSML/prosody, long-form context-consistency, sample-rate→transport, precision/quant numerics, concurrent multi-voice batching, barge-in, and the sampler/codec edge cases.

Grounded in onboarded TTS families: **kokoro / StyleTTS2** (one-shot, dur-predictor + style-vector), **melo** (VITS), **supertonic** (flow-matching CFM, bit-identical-to-ref), **chatterbox / dia / dia2 / csm** (codec-AR), **higgs** (codec-AR, 4B), **qwen3-tts** (talker + MTP), **dots.tts** (AR + inner flow-DiT nested), **vibevoice** (AR + DDPM), **cosyvoice3** (flow), **neutts** (Qwen2-0.5B codec-AR backbone — the §1 keystone benchmark model), **omnivoice** (masked-diffusion). Codecs: **Mimi 12.5 Hz**, **DAC**, **SNAC**, **EnCodec**. Hardware: GB10 unified, H200/B200/MI300X/RTX, CPU, NPU, HPU.

Schema: each scenario states the situation, the optimal KISS handling, and the failure if mishandled. Levels: **SIMPLE** (single mechanism), **INTERMEDIATE** (a few interacting), **COMPOUND** (multi-stage/multi-tenant), **EXTREME** (adversarial / tail / cross-cutting).

---

## SIMPLE

### TTS-1 — Offline one-shot synthesis (kokoro/melo) returns whole utterance
- Level: simple
- Pipeline: TTS (one-shot VITS/dur-predictor)
- Axes: paradigm:one-shot, mode:offline, batch:1
- Scenario: A non-streaming REST caller asks kokoro to synthesize a short sentence and expects the full 24 kHz waveform in one response.
- System must: Run the coarse `synthesize` path inline (B=1, no scheduler/queues per §8 Inline mode), return the concatenated `Vec<(ChunkMeta, i16)>` in one shot; no tick loop or ledger.
- If mishandled: Spinning up DC machinery (queues/admission/ledger) for a single offline call adds latency and complexity the edge never needs.

### TTS-2 — Streaming first-audio (TTFA) on codec-AR (chatterbox/neutts)
- Level: simple
- Pipeline: TTS (codec-AR + codec_stream)
- Axes: paradigm:codec-ar, streaming:ttfa, frame-rate:12.5
- Scenario: A live caller wants the first audio chunk to play as soon as possible from a codec-AR model emitting Mimi frames at 12.5 Hz.
- System must: Emit the first decoded chunk as soon as `initial_chunk_frames` are ready (TTFA = frame_period + acoustic_delay·frame_period + step_time), stream subsequent frames delta-only; do not wait for end-of-utterance.
- If mishandled: Buffering to utterance-end before first audio destroys perceived latency (caller hears nothing for seconds), defeating realtime use.

### TTS-3 — Delta streaming, not cumulative re-decode
- Level: simple
- Pipeline: TTS (codec_stream egress)
- Axes: streaming:delta, correctness:O(N), bug:most-common
- Scenario: A streaming TTS yields audio chunk-by-chunk; the engine must send only newly produced samples each step.
- System must: Yield delta (new samples only) per step — O(N) total; the WS frame carries `ChunkMeta`; assert `offline_concat == stream_concat` byte-for-byte (catalog I1).
- If mishandled: Cumulative re-decode from step 0 is O(N²) and users hear replays/truncation — "the MOST COMMON silent bug" (offline RTF still passes).

### TTS-4 — Explicit FINAL frame terminates the stream
- Level: simple
- Pipeline: TTS (codec_stream egress)
- Axes: protocol:final-frame, correctness:eos
- Scenario: A streaming synthesis finishes; the consumer must distinguish "done" from "producer stalled."
- System must: Send an explicit FINAL/`is_done` sentinel frame (catalog G2); never infer completion from absence-of-chunks; "closed without FINAL" is a failure signal.
- If mishandled: A consumer inferring done from silence cannot tell completion from a stalled producer → premature close or indefinite hang.

### TTS-5 — Repetition penalty 1.0 for TTS (match reference)
- Level: simple
- Pipeline: TTS (codec-AR sampler)
- Axes: sampler:rep-penalty, parity:reference
- Scenario: A codec-AR model (dia/csm/higgs) is onboarded; its reference uses repetition_penalty = 1.0 (no penalty) for natural prosody.
- System must: Use the reference's exact repetition penalty (often 1.0 for TTS); pin it in the manifest, never inherit a text-LLM default.
- If mishandled: A non-1.0 penalty flattens prosody or breaks codec-token statistics → robotic/garbled audio that WER-style gates can miss.

### TTS-6 — Codec decoder runs in fp32 (no autocast)
- Level: simple
- Pipeline: TTS (codec_stream / vocoder)
- Axes: precision:fp32-codec, numerics
- Scenario: The Mimi/DAC/SNAC/EnCodec decoder converts codes to a waveform; the AR backbone may be bf16, but the codec stage is numerically sensitive.
- System must: Keep codec/vocoder decode in fp32 regardless of backbone dtype; autocast corrupts codec audio (catalog Phase-2 + I7).
- If mishandled: bf16/fp16 autocast on the codec silently degrades audio quality (no crash, no error signal), passing text-only gates.

### TTS-7 — Speed/rate control on a one-shot dur-predictor model
- Level: simple
- Pipeline: TTS (one-shot, dur-predictor)
- Axes: prosody:speed, paradigm:one-shot
- Scenario: A caller requests 1.25× speaking rate from kokoro/StyleTTS2, which exposes a duration scale.
- System must: Apply speed via the model's native duration-predictor scale (the coarse `synthesize(text, voice, speed)` already carries `speed`); resample only if the model lacks native rate control.
- If mishandled: Faking speed by post-hoc resampling shifts pitch (chipmunk/baritone artifact) instead of changing tempo.

### TTS-8 — Voice/speaker selection from the voice bank
- Level: simple
- Pipeline: TTS (one-shot / codec-AR)
- Axes: voice:selection, conditioning:speaker-id
- Scenario: A caller picks a named built-in voice; the model conditions on a precomputed style/speaker embedding.
- System must: Resolve the voice id to its precomputed embedding via `voices`/`default_voice`; condition the synthesis on it; reject unknown voice ids with a typed error.
- If mishandled: Silent fallback to default voice (or a panic on unknown id) gives the caller the wrong speaker with no signal.

### TTS-9 — Stop token honored: not-stops-early, not-never-stops
- Level: simple
- Pipeline: TTS (codec-AR generation)
- Axes: generation:stop-token, bug:length
- Scenario: A codec-AR model must end synthesis exactly at its learned stop/EOS token for the given text.
- System must: Use the reference's exact stop-token id and detection; terminate generation on it (per-stream), with a max-length safety cap as backstop.
- If mishandled: Wrong stop id → never-stops (runaway babble, slot never frees) or stops-early (truncated/clipped final words).

### TTS-10 — Single-step flow/CFM (supertonic) offline
- Level: simple
- Pipeline: TTS (flow-CFM)
- Axes: paradigm:flow, nfe:fixed, mode:offline
- Scenario: Supertonic synthesizes offline with a fixed N-step CFM solve and must reproduce the reference bit-faithfully (proven maxΔ=0.0000).
- System must: Run the fixed NFE Euler/ODE solve with the precomputed timestep schedule; keep solver math in fp32; return the whole waveform (one-shot consumer).
- If mishandled: A drifting NFE count or non-fp32 solver step diverges from the bit-identical reference, silently regressing audio fidelity.

### TTS-11 — Sample rate → 8 kHz G.711 telephony egress
- Level: simple
- Pipeline: TTS (+resample/transport)
- Axes: sample-rate:24k->8k, transport:g711, anti-alias
- Scenario: A 24 kHz TTS feeds a PSTN/SIP leg that requires 8 kHz G.711 (µ-law/A-law) in 20 ms RTP packets.
- System must: Downsample model-SR→8 k with anti-alias always-on (§5.1), G.711-encode, repacketize to fixed 20 ms RTP via a jitter buffer; resample is a post-batch CPU/NPU-offloadable stage.
- If mishandled: Skipping the anti-alias filter aliases high frequencies into the 8 k band → tinny/harsh telephony audio; non-20 ms framing breaks the RTP pacing.

### TTS-12 — Sample rate → 48 kHz Opus HD egress
- Level: simple
- Pipeline: TTS (+resample/transport)
- Axes: sample-rate:24k->48k, transport:opus-hd
- Scenario: A WebRTC HD caller wants 48 kHz Opus; the model emits 24 kHz.
- System must: Upsample 24 k→48 k (FFT fixed-ratio rubato), Opus-encode for the HD leg; keep the resample off the AR clock.
- If mishandled: Mismatched SR to the Opus encoder produces wrong-pitch/wrong-duration playback or encoder rejects the frame size.

### TTS-13 — temp=0 folds to greedy (sampler eps guard)
- Level: simple
- Pipeline: TTS (codec-AR sampler)
- Axes: sampler:temp-0, numerics:eps
- Scenario: A caller sets temperature 0 (deterministic) on a sampling codec-AR model.
- System must: Fold temp < `_SAMPLING_EPS` (1e-5) to greedy/argmax (catalog H5) so there is no division by ~0.
- If mishandled: Dividing logits by ~0 yields inf/NaN → garbage codec token → audible pop with no error.

### TTS-14 — Empty / whitespace-only text
- Level: simple
- Pipeline: TTS (frontend)
- Axes: input:empty, edge-case
- Scenario: A caller submits an empty or whitespace-only synthesis request.
- System must: Return empty audio (or a typed 400) cleanly — no model invocation, FINAL frame immediately for streaming; never enter the generation loop.
- If mishandled: Feeding empty text to the AR loop can never-stop (no content to reach EOS) or crash the frontend G2P.

### TTS-15 — Pitch/prosody via SSML on a model that supports it
- Level: simple
- Pipeline: TTS (frontend + prosody)
- Axes: ssml:prosody, frontend
- Scenario: A caller sends SSML with `<prosody pitch>` / `<break>` to a model whose frontend understands prosody marks.
- System must: Parse SSML in the text frontend, map supported tags to the model's prosody/pause controls, and pass unsupported tags through as silence/no-op (documented) rather than literal spoken text.
- If mishandled: Treating SSML tags as plain text makes the TTS literally speak "prosody pitch high," or a parse error rejects a valid request.

---

## INTERMEDIATE

### TTS-16 — Chunk overlap-add / holdback prevents boundary clicks
- Level: intermediate
- Pipeline: TTS (codec_stream windowed)
- Axes: streaming:overlap-add, holdback:frames
- Scenario: A streaming vocoder emits audio in chunks; naive concatenation at chunk boundaries produces audible clicks/discontinuities.
- System must: Use left-context + crossfade overlap-add (e.g. codec_chunk_frames=25, codec_left_context_frames=25, catalog) with per-codec holdback in frames; holdback is zero for causal codecs like Mimi.
- If mishandled: Hard-cut chunk boundaries inject periodic clicks/pops at the chunk rate — clearly audible artifacts.

### TTS-17 — Initial chunk larger for quality, then smaller for latency
- Level: intermediate
- Pipeline: TTS (codec_stream, TTFA ramp)
- Axes: streaming:ttfa-ramp, chunk-sizing
- Scenario: First audio should be high-quality (enough context) yet subsequent chunks should be small for low latency.
- System must: Make the first chunk larger (more frames → quality/stability) then ramp down to small chunks for steady-state latency (the streaming-window archetype).
- If mishandled: Uniformly tiny chunks make the opening unstable/artifacty; uniformly large chunks inflate TTFA.

### TTS-18 — Flow/CFM N-step solve must amortize over frames, not run per-frame
- Level: intermediate
- Pipeline: TTS (flow-CFM chunked)
- Axes: paradigm:flow, batch:compute-bound, frame-budget
- Scenario: A chunk-CFM model (cosyvoice3/dots.tts-class) runs a 10-step solve; at batch 64 that is ~110 ms — exceeding a 40 ms frame budget if done per-frame.
- System must: Amortize the chunk-level diffusion over multiple frames (chunked/lookahead), run the fixed-NFE solve once per chunk with the precomputed schedule, micro-batch by (model, latent-shape, NFE) — never per-frame (§1.5).
- If mishandled: Running the multi-step CFM per frame blows the frame budget → underruns/dropouts under any concurrency.

### TTS-19 — CFG folds cond+uncond into the batch (×2)
- Level: intermediate
- Pipeline: TTS (flow/diffusion step-bucket)
- Axes: cfg:double-batch, step-bucket
- Scenario: A CFM/diffusion model uses classifier-free guidance, doubling the transformer forwards per step.
- System must: Fold conditional+unconditional into the batch dimension (×2) in one kernel (catalog B), key the bucket on `(model, latent-shape, step-schedule, CFG)`, and pass a seeded generator so CFG-parallel matches sequential.
- If mishandled: Running cond and uncond as two separate forwards halves throughput; an unseeded generator makes CFG-parallel diverge from sequential (non-determinism).

### TTS-20 — DDPM head (vibevoice) 25-step solve budgeting
- Level: intermediate
- Pipeline: TTS (diffusion DDPM)
- Axes: paradigm:diffusion, nfe:25, collapse:b128
- Scenario: VibeVoice's DDPM head runs 25 steps; measured 90 ms@B1, 624 ms@B64, and collapses at B128 (§1.5).
- System must: Treat diffusion as compute-bound — bound the batch well below the collapse knee, amortize the 25-step solve over a chunk, precompute the timestep schedule once at load; never lockstep it on the AR frame clock.
- If mishandled: Batching DDPM like AR drives the per-step time past the budget and collapses at high batch → total dropout for all co-batched streams.

### TTS-21 — Nested AR+inner-diffusion (dots.tts) stays in one forward
- Level: intermediate
- Pipeline: TTS (AR-outer + inner flow-DiT nested)
- Axes: paradigm:nested, batch:38x, in-forward
- Scenario: dots.tts is AR-outer with an inner flow-DiT per frame; separating the inner loop into its own process would balloon per-step latency.
- System must: Keep the inner loop INSIDE one stage's single batched forward (one `StageNode` + `[stage.nested]`); batch the inner step across the outer lockstep batch (all B slots at the same inner step k) — nested per-frame patch batches 38×@64 because the tiny latent is launch-bound (§3.3).
- If mishandled: Hoisting the inner diffusion to a cross-process stage adds per-frame IPC + balloons per-step latency, destroying the nested batchability win.

### TTS-22 — Tight vs loose feedback decides fused-node vs separate-node
- Level: intermediate
- Pipeline: TTS (DAG topology)
- Axes: dag:fused-vs-stage, feedback-tightness
- Scenario: CosyVoice2 (ar_semantic→cfm_chunk→vocoder) and dots.tts (ar_talker{nested cfm}→audiovae) need different DAG shapes from the same engine.
- System must: Fuse when feedback is tight/per-frame (AR→code-predictor → one node), split when feedback is loose/chunk-consuming (talker→chunk-CFM→vocoder → separate nodes); express both as data (§3.3).
- If mishandled: Fusing a loose chunk-consumer stalls the AR thread on the slow CFM; splitting a tight per-frame feedback loop pays IPC every frame.

### TTS-23 — Sampler runs outside the CUDA graph (multinomial not graph-safe)
- Level: intermediate
- Pipeline: TTS (codec-AR, CUDA-graph)
- Axes: sampler:multinomial, cuda-graph:capture
- Scenario: The lockstep step is CUDA-graphed (1.21×@B1 on edge) but TTS sampling needs `multinomial`, which is not graph-safe (only `argmax` is).
- System must: Sample OUTSIDE the captured region OR use a graph-safe gumbel-argmax inside (catalog C2/F7); the captured region stays fixed-shape, the sampler is appended after replay.
- If mishandled: Capturing multinomial into the graph silently breaks sampling (stale/garbage tokens) or forces eager — losing the edge latency win.

### TTS-24 — tiny-temp clamp prevents exp overflow
- Level: intermediate
- Pipeline: TTS (codec-AR sampler)
- Axes: sampler:tiny-temp, numerics:overflow
- Scenario: A caller sets an extremely small but non-zero temperature (e.g. 1e-4) on a sampling model.
- System must: Clamp temperature to `_MAX_TEMP` (1e-2) with a warn (catalog H5) so the divided logits don't overflow exp; guarantee ≥1 survivor after top-p (`top_p_mask[:,-1]=False`).
- If mishandled: Tiny-temp exp overflow produces inf/NaN logits → garbage codec token (catalog #1623 signature).

### TTS-25 — Voice cloning from ref-audio conditioning
- Level: intermediate
- Pipeline: TTS (codec-AR + ref-audio prefix)
- Axes: voice-clone:ref-audio, conditioning:prefix
- Scenario: A caller supplies a short reference clip; a codec-AR model (higgs/csm/chatterbox) clones the voice by prepending the encoded ref sequence as a prefix.
- System must: Encode the ref clip to codec tokens, prefill the prefix into the slot's KV, then generate; reset the slot's clone state on stream end.
- If mishandled: Leaking ref-clone state into the next stream in a recycled slot synthesizes the previous caller's voice (privacy/quality disaster, ties TTS-49).

### TTS-26 — Prefix-cache reuse for repeated ref-audio (86% hit)
- Level: intermediate
- Pipeline: TTS (codec-AR, prefix-cache)
- Axes: voice-clone:prefix-cache, hit-rate:86%
- Scenario: An agent uses the same cloned voice across many turns; reusing the same voice yields ~86% (>90% peak) prefix-cache hit (catalog L1, Fish S2).
- System must: Cache the deterministic ref-audio + system-prompt prefix KV (radix/prefix-cache) and reuse it across slots/requests; ring-only per-utterance suffix (HYBRID KV, §L1 fix).
- If mishandled: A fixed per-slot ring recomputes the ref-audio KV every request → forfeits ~86% cacheable work on the top commercial (cloned-voice/agent) workload.

### TTS-27 — Anti-contamination fingerprint on the prefix key
- Level: intermediate
- Pipeline: TTS (codec-AR, prefix-cache)
- Axes: voice-clone:fingerprint, cross-contamination, all-codebooks
- Scenario: Two requests with identical text but different ref-audios paste embeds at the same placeholder positions → identical token-ids → radix concludes prefixes match (catalog G1).
- System must: Set `extra_key = blake2b(full N-codebook ref sequence)` — hash over ALL codebooks (cb0-only collides); zero-shot (no ref) → `extra_key=None` so legit prefix-sharing survives.
- If mishandled: Cross-contaminated KV between different ref-audios → silent WRONG-VOICE output (no crash, only under concurrency).

### TTS-28 — Multilingual: per-language frontend / G2P
- Level: intermediate
- Pipeline: TTS (frontend, multilingual)
- Axes: multilingual:g2p, frontend:per-language
- Scenario: A multilingual model (melo/cosyvoice3/qwen3-tts) synthesizes text in a language requiring its own grapheme-to-phoneme + text normalization.
- System must: Select the per-language frontend/G2P from the requested/declared language (`set_language` + `supported_languages`), normalize numbers/dates per locale, then synthesize.
- If mishandled: Running the wrong-language G2P mispronounces or skips characters (e.g. CJK through a Latin frontend) → unintelligible output.

### TTS-29 — Language notation normalization (canonical→model-native)
- Level: intermediate
- Pipeline: TTS (frontend, standardize)
- Axes: multilingual:notation, standardize:alias
- Scenario: One model wants a string language code, another wants an integer id; the user relies on ONE canonical notation across all models.
- System must: Map canonical→model-native via `resolve_alias`/`NotationMap` (components/standardize.rs) — string-native vs int-native resolved per model (live-verified en→id4); same for precision (half→fp16), device (gpu→cuda).
- If mishandled: Passing a canonical code a model doesn't recognize falls back to a default language or errors — wrong-language synthesis or rejection.

### TTS-30 — Code-switching within one utterance
- Level: intermediate
- Pipeline: TTS (frontend, multilingual)
- Axes: multilingual:code-switch, frontend
- Scenario: A single sentence mixes two languages (e.g. an English brand name inside a Hindi sentence) for a model that claims multilingual support.
- System must: Segment by script/language, apply the right G2P per span, and join phonemes/codes before synthesis (if the model supports it); else document the limitation and pick the dominant-language frontend.
- If mishandled: A single-frontend pass mangles the embedded foreign span (spells it out or drops it).

### TTS-31 — Long-form audiobook: context consistency across chunks
- Level: intermediate
- Pipeline: TTS (codec-AR/flow, long-form)
- Axes: long-form:context, consistency:voice-prosody
- Scenario: An audiobook chapter is synthesized as many sentences; voice timbre and prosody must stay consistent across all of them.
- System must: Carry rolling context (prior frames/style) across sentence boundaries within the model's context window; for flow models keep the same ref/style embedding; pin attention-sink tokens for very long runs (catalog L12).
- If mishandled: Per-sentence cold starts drift the voice (timbre/energy jumps between sentences) — an audibly inconsistent audiobook.

### TTS-32 — Long-form ring is lossy → escape to paged/full-context
- Level: intermediate
- Pipeline: TTS (long-form KV)
- Axes: long-form:ring-lossy, escape:paged
- Scenario: A 10-minute continuous narration exceeds the fixed ring's context; the ring silently wraps and forgets early context (catalog L12, AudioKV).
- System must: Pin attention-sink tokens + provide a paged/full-context escape hatch for long-form TTS; detect when context exceeds the ring and switch path rather than wrap-and-corrupt.
- If mishandled: StreamingLLM-style sliding-window forgetting + wraparound instability degrades prosody/coherence mid-narration with no signal.

### TTS-33 — Per-stream rubato resampler state (no zipper noise)
- Level: intermediate
- Pipeline: TTS (+resample)
- Axes: resample:per-stream-state, transport
- Scenario: A continuous stream is resampled (e.g. 44.1 k→48 k) in chunks; the resampler must maintain filter state across chunks per stream.
- System must: Keep a persistent per-stream rubato instance (FFT fixed-ratio default, sinc for fractional ratios) so filter history carries across chunks; free it on stream end.
- If mishandled: Re-initializing the resampler per chunk injects discontinuities (zipper noise) at chunk boundaries.

### TTS-34 — 44.1 kHz model → 48 kHz transport (fractional ratio)
- Level: intermediate
- Pipeline: TTS (+resample)
- Axes: sample-rate:44.1k->48k, fractional
- Scenario: A 44.1 kHz model must feed a 48 kHz pipeline — a fractional resample ratio (160/147).
- System must: Use the sinc/fractional rubato path for the non-integer ratio (not the FFT fixed-ratio fast path); maintain per-stream state.
- If mishandled: Forcing a fixed-ratio resampler on a fractional ratio drifts sample alignment over a long stream → gradual pitch/timing error.

### TTS-35 — NaN logit → reject the frame (not glitch)
- Level: intermediate
- Pipeline: TTS (codec-AR sampler)
- Axes: numerics:nan, policy:reject-frame
- Scenario: A transient numerical issue produces a NaN/Inf logit row during codec-AR decode.
- System must: Run an always-on `logits.isnan().any()` reduction (catalog H1) and reject-frame on NaN — repeat previous frame / emit codec-silence / greedy-resample — never argmax a NaN row.
- If mishandled: Argmaxing a NaN row picks a garbage codec token → audible pop with zero error signal (vLLM's default behavior, the single most important inversion).

### TTS-36 — Concurrent multi-voice batching on one GB10
- Level: intermediate
- Pipeline: TTS (codec-AR lockstep)
- Axes: concurrency:multi-voice, batch:55x, frame-sync
- Scenario: 16+ live callers each use a different built-in voice on a codec-AR model on one GB10.
- System must: Lockstep-batch them on the stream axis (fixed slots, per-stream exec-mask, per-slot ring KV) at the shared 12.5 Hz tick — different voices co-batch freely (same model ⟹ same frame-rate); decode is flat to 64 (§1.1).
- If mishandled: Serializing one stream at a time (the current `Arc<Mutex>` single-in-flight) wastes the near-free 55×@64 batching headroom — needless rejection/queueing.

### TTS-37 — Cohort by (model, frame-rate); never mix clocks
- Level: intermediate
- Pipeline: TTS (lockstep cohort)
- Axes: cohort:frame-rate, batch:no-mix-clocks
- Scenario: One stream uses a 12.5 Hz Mimi model and another a 75 Hz EnCodec model; they cannot share a lockstep tick.
- System must: Batch by `(model, frame_rate)` cohort (§4.2); a 12.5 Hz and a 75 Hz stream have no common realtime tick → separate cohorts that share the GPU temporally via the duty ledger, not within one fused step.
- If mishandled: Lockstep-mixing different frame-rate clocks is impossible — either a hard crash on shape mismatch or one clock starves the other.

### TTS-38 — EnCodec-48k 150 Hz is sub-realtime even at batch 1
- Level: intermediate
- Pipeline: TTS (codec_stream, high-FR codec)
- Axes: frame-rate:150hz, budget:sub-realtime
- Scenario: A model on EnCodec-48k emits at 150 Hz (6.7 ms/frame) — the step budget is tiny even for a single stream.
- System must: Recognize low-frame-rate as the biggest realtime lever (§4.4); for a 150 Hz codec, batch shrinks toward 1 and the model may be non-realtime on this substrate — admit accordingly or prefer a lower-FR codec variant.
- If mishandled: Admitting many streams on a 150 Hz codec as if it batched like 12.5 Hz overruns the 6.7 ms budget → continuous underruns.

### TTS-39 — Barge-in cancels mid-synthesis within ≤1 tick
- Level: intermediate
- Pipeline: TTS (codec-AR, cancellation)
- Axes: barge-in:cancel, latency:1-tick
- Scenario: The user starts speaking while the TTS is mid-utterance; the synthesis must stop immediately.
- System must: Treat barge-in as a control message that jumps every stage's queue and frees the slot/KV/window within ≤1 tick (§6); the cancelled stream emits a terminal frame distinguishable from completion.
- If mishandled: Continuing to synthesize after barge-in talks over the user; an indistinguishable cancel-vs-complete terminal confuses the consumer.

### TTS-40 — Per-language number/date/currency normalization
- Level: intermediate
- Pipeline: TTS (frontend normalization)
- Axes: frontend:tts-normalization, locale
- Scenario: Text contains "$3,500 on 3/4/2026" that must be expanded to spoken words per the synthesis language/locale.
- System must: Run locale-aware text normalization in the frontend (currency, date order, ordinals) before G2P; pick the expansion rules from the synthesis language.
- If mishandled: Wrong-locale expansion speaks "three slash four" or the wrong date order, or leaves "$" / digits unspoken.

### TTS-41 — Multinomial inside a CUDA graph via gumbel-argmax
- Level: intermediate
- Pipeline: TTS (codec-AR, CUDA-graph sampler)
- Axes: sampler:gumbel-argmax, cuda-graph
- Scenario: An edge deployment wants the sampler inside the captured graph for max latency win but still needs stochastic sampling.
- System must: Use a graph-safe gumbel-argmax (add Gumbel noise then argmax — both graph-safe) inside the captured region (catalog F7), seeded per stream; float64 Gumbel for determinism.
- If mishandled: Falling back to multinomial breaks capture; an unseeded Gumbel makes per-stream output non-reproducible.

### TTS-42 — Dynamic-frame-rate codec (FlexiCodec 3–12.5 Hz variable stride)
- Level: intermediate
- Pipeline: TTS (codec_stream, variable-stride)
- Axes: codec:flexicodec, frame-rate:3-12.5, data-dependent
- Scenario: A FlexiCodec model varies frame-rate 3–12.5 Hz per-utterance AND per-frame, not known a-priori (catalog L6).
- System must: Generalize lockstep to "advance a model-dependent variable stride"; the cohort key tolerates unknown-a-priori rates; the duty ledger budgets against the worst-case (densest) stride.
- If mishandled: A fixed-rate cohort assumption (§4.2/§5.1 "2 intrinsic constants") cannot batch a variable-rate codec → mis-paced ticks and shape mismatches.

### TTS-43 — NFE distillation: 2–4 step meanflow head
- Level: intermediate
- Pipeline: TTS (flow, distilled NFE)
- Axes: nfe:2-4-meanflow, distillation, per-stream-dial
- Scenario: A distilled model (FlashTTS 2-NFE meanflow, DMOSpeech2) runs the inner generative head in 2–4 steps, and NFE is a per-stream runtime dial (catalog L5/L15).
- System must: Let the step-bucket key accept per-request variable N (including N=1/feedforward IntMeanFlow); compose the inner solve at that stream's NFE inside one AR step (variable-NFE micro-batch).
- If mishandled: A fixed-NFE bucket can't co-batch streams running different step counts → either re-bucketing thrash or wrong step count (quality loss).

### TTS-44 — Masked-diffusion (omnivoice) parallel decode
- Level: intermediate
- Pipeline: TTS (masked-diffusion)
- Axes: paradigm:masked-diffusion, batch:bucketed-K-iter
- Scenario: OmniVoice decodes via masked-diffusion (K parallel iterations over a fixed-length token grid), not AR-per-frame.
- System must: Use the step-bucket batcher (length-bucketed, K iterations, CFG-folded if used); precompute masks/schedule once; this paradigm has no KV and is not lockstep (§4.1).
- If mishandled: Forcing masked-diffusion onto the lockstep AR path (expecting one-frame-per-tick + KV) is a category error → wrong control flow / crash.

### TTS-45 — Multi-AR / MTP talker emits multiple tokens per step
- Level: intermediate
- Pipeline: TTS (qwen3-tts talker+MTP)
- Axes: paradigm:mtp, batch:rectangular-preserved
- Scenario: qwen3-tts uses multi-token-prediction (talker + MTP heads) emitting several codec tokens per step (2–5× quality-neutral, catalog L14).
- System must: Treat the Depformer/code-predictor as the MTP mechanism (direct-emit) which PRESERVES rectangular lockstep; batch MTP heads across the outer batch like the nested case; do NOT add EAGLE/Medusa draft-spec-decode.
- If mishandled: Adding draft-spec-decode destroys the rectangular lockstep (variable accept-length) and is a 0.98× net slowdown on acoustic tokens (catalog L13).

### TTS-46 — Codec stage batch size = 1 (decoupled from AR≥4)
- Level: intermediate
- Pipeline: TTS (codec_stream, per-stage batch)
- Axes: stage:codec-bs1, decoupled-batch, rfc-2568
- Scenario: The AR stage pipelines at `max_num_seqs ≥ 4`; the shared codec/vocoder stage should run at batch 1 (its window round-robins).
- System must: Pin per-stage batch sizes independently — AR ≥ 4, codec = 1 (catalog C6, RFC #2568); the codec micro-batch stage must NOT inherit the AR batch size.
- If mishandled: A uniform batch default of 1 everywhere (or AR's size on the codec) causes audio gaps under concurrency as the codec window round-robins across requests.

### TTS-47 — Power-of-two chunk/token counts (tile quantization)
- Level: intermediate
- Pipeline: TTS (prefill/chunk sizing)
- Axes: tiling:power-of-two, perf:257-cliff
- Scenario: A prefill or chunk is sized at 257 tokens; 257 is ~32% slower than 256 due to tile quantization (§4.5).
- System must: Keep chunk/prefill token counts power-of-two-aligned to the GPU tile; capture exact slot counts (1,2,4…) for graphs to sidestep the 257→272/257→512 cliff (catalog H4).
- If mishandled: Off-tile sizes pay a ~32% per-step penalty and trigger CUDA-graph re-capture cliffs → frame-deadline misses.

### TTS-48 — Pre-kernel input substitution for idle/warming slots
- Level: intermediate
- Pipeline: TTS (lockstep, masked-not-absent)
- Axes: lockstep:input-substitution, masked-not-absent
- Scenario: A fixed-slot batch has idle or still-warming rows; the dense kernel reads/writes every row including idle ones.
- System must: Force masked-or-warming rows to a valid `initial`/BOS token via `where(is_init, initial, gathered)` BEFORE embedding (catalog F1) so the KV-gather never reads sentinel/stale.
- If mishandled: The KV-gather reads sentinel/-2/stale for idle rows → CUDA illegal-memory/NaN that kills the WHOLE batch (all 64 users).

### TTS-49 — Transactional slot recycling (reset_slot) — no cross-voice leak
- Level: intermediate
- Pipeline: TTS (lockstep, slot recycle)
- Axes: lockstep:reset-slot, privacy:no-contamination
- Scenario: A caller in slot 7 disconnects; a new caller is admitted into slot 7.
- System must: Run ONE transactional `reset_slot(7)` fanning out to KV pointers + conv rings + sampler RNG + ref-clone state + word buffers + offset; a monotonic `channel_id` drops any in-flight output for the old occupant (catalog F3).
- If mishandled: Without reset, the new caller's attention sees the old caller's KV/ref-clone state → cross-caller voice/content contamination (privacy disaster).

### TTS-50 — All-idle path: don't run the kernel on an all-False batch
- Level: intermediate
- Pipeline: TTS (lockstep, idle loop)
- Axes: lockstep:all-idle, no-busy-spin
- Scenario: At some ticks no slot is active (all callers between utterances).
- System must: Apply admissions/resets first, compute exec_mask, run the kernel ONLY if `exec_mask.any()` else short-sleep 1–2 ms (catalog F6/G3); never busy-spin, never run a kernel on an all-False batch.
- If mishandled: Running the kernel on an all-False batch wastes a step; busy-spinning the loop starves co-located stages (the GIL-starvation→core-starvation hazard).

---

## COMPOUND

### TTS-51 — Three-node CosyVoice2 DAG streams first-audio sub-300 ms
- Level: compound
- Pipeline: TTS (ar_semantic → cfm_chunk → vocoder)
- Axes: dag:3-node, ttfa:sub-300ms, pipeline-overlap
- Scenario: A CosyVoice2-class model is a 3-node DAG (AR semantic tokens → chunk-CFM → vocoder) and must stream first audio under 300 ms (M3 acceptance).
- System must: Run decoupled per-stage micro-engines with bounded typed channels (`LatentChunk{latent,chunk_idx,left_context}`); the AR thread lockstep-ticks while the CFM/vocoder threads micro-batch their chunks (pipeline overlap); ramp first chunk for TTFA.
- If mishandled: One batch loop across stages head-of-line-blocks AR on the slow CFM → first-audio blows past 300 ms and steady-state gaps.

### TTS-52 — Two-node nested dots.tts DAG streams sub-300 ms
- Level: compound
- Pipeline: TTS (ar_talker{nested cfm} → audiovae)
- Axes: dag:2-node-nested, ttfa:sub-300ms
- Scenario: dots.tts is a 2-node DAG with an in-forward nested CFM in the talker node, streaming first audio sub-300 ms (M3 acceptance).
- System must: Keep the nested CFM in-forward (one node, batched 38×@64 across the outer batch), stream `TokenFrame`/latent to the audiovae node which micro-batches; the codec stage no longer head-of-line-blocks AR.
- If mishandled: Externalizing the nested CFM or coupling the vocoder batch to AR re-introduces per-frame stalls → TTFA miss.

### TTS-53 — Codec stage offloaded to CPU/NPU frees GPU bandwidth
- Level: compound
- Pipeline: TTS (heterogeneous placement)
- Axes: placement:codec-on-npu, zero-copy, bandwidth:1.3x
- Scenario: On GB10, the terminal codec/vocoder stage is offloaded to NPU/CPU to free GPU bandwidth for more AR streams (M4 acceptance: ≥1.3× more AR streams).
- System must: Place the conv-codec on NPU/CPU (paradigm×substrate affinity §2.3), pass a `ZeroCopyBuffer` across the coherent boundary (zero-copy on GB10 NVLink-C2C), and budget the shared ~273 GB/s ceiling so the split doesn't oversubscribe (§3.4 contention guard).
- If mishandled: Offloading without budgeting the shared bandwidth oversubscribes the one LPDDR ceiling → both AR and codec slow down; a non-coherent copy adds DMA latency.

### TTS-54 — Concurrent crosstalk: per-slot sidecar state keyed + freed
- Level: compound
- Pipeline: TTS (Path-B torch sidecar)
- Axes: sidecar:per-slot-state, crosstalk:under-load
- Scenario: Multiple concurrent streams share a Path-B torch sidecar that holds Python codec/sliding-window/streaming-generator state.
- System must: Key sidecar state by slot-id (`self._state: dict[slot_id, State]`) and free on slot-reset (catalog C3/I5); the lockstep per-slot discipline extends into the sidecar's Python state.
- If mishandled: A shared buffer corrupts audio across concurrent requests → crosstalk or truncation that only appears under load.

### TTS-55 — Zero D2H syncs in the per-frame sidecar loop
- Level: compound
- Pipeline: TTS (Path-B sidecar, hot loop)
- Axes: sidecar:no-d2h-sync, perf:2400-syncs
- Scenario: A naive sidecar calls `.item()/.cpu()/.tolist()` per step in the AR/CFM/vocoder loop.
- System must: Make every per-step loop GPU-sync-free — `dst.copy_(src)` not `fill_(src.item())`, `torch.where` not Python branches, `torch.compile(forward, fullgraph=False)` (catalog I3); assert zero D2H syncs via a CUDA-event/profiler guard during decode.
- If mishandled: "10 steps × 60 frames × 4 ops = 2400 syncs per request" → latency collapse; the clean 9 ms/step assumption is forfeited.

### TTS-56 — Streaming iterator threaded through the Rust boundary
- Level: compound
- Pipeline: TTS (Path-B sidecar streaming)
- Axes: streaming:sidecar-iterator, m3-fix
- Scenario: The Python `TtsRunner.synthesize` already returns a streaming `Iterator[np.ndarray]`, but the Rust `TorchSidecarTts` currently collects it to one Vec (streaming lost at the boundary, catalog D).
- System must: Thread the iterator through the framed stdin/stdout protocol (add a streaming `step`/`chunk` op carrying per-chunk PCM) so the Rust side forwards each chunk as it arrives (M3).
- If mishandled: Collecting to one Vec re-introduces the offline-buffer latency — the sidecar streaming benefit is dead at the Rust boundary.

### TTS-57 — Admission rejects rather than glitches at saturation
- Level: compound
- Pipeline: TTS (scheduler admission)
- Axes: admission:reject-dont-glitch, bottleneck-stage, typed-429
- Scenario: GB10 is at slot/bandwidth capacity and a new realtime stream arrives.
- System must: Admit IFF every stage has a free slot + reservable KV/window/workspace AND per-substrate duty ≤ S AND (on unified) shared-bandwidth duty ≤ S·ceiling — test the BOTTLENECK stage (often CFM/codec, not AR); else typed 429/503 + Retry-After; never admit-and-degrade (§6).
- If mishandled: Admitting on AR-fit alone (ignoring the codec the AR can't sustain) glitches every in-flight stream — the exact bug the per-stage duty ledger prevents.

### TTS-58 — Prefill firewall: ≤1 new stream's prefill per K frames
- Level: compound
- Pipeline: TTS (prefill firewall)
- Axes: prefill:firewall, tbt:28x, chunked
- Scenario: A new clone-voice stream's ref-audio prefill spikes while existing streams are mid-utterance.
- System must: Admit ≤1 new stream's prefill per K frames and chunk any prefill exceeding one frame-budget's tokens (Sarathi-Serve token budget keyed on the audio frame deadline, §4.5); keep chunk counts power-of-two.
- If mishandled: A naive prefill+decode hybrid inflates per-token TBT up to 28.3× (P99 1.76 s = 17–22 dropped frames at 80 ms) → total dropout for live streams.

### TTS-59 — Prefill firewall control variable is predicted latency, not token count
- Level: compound
- Pipeline: TTS (prefill firewall, refined)
- Axes: prefill:latency-budget, kv-length-aware
- Scenario: For a long ref-audio prefill, token-count alone underestimates compute as context grows (catalog L10, DuetServe: >4× variation at token-budget=8).
- System must: Switch the firewall to a KV-length-aware PREDICTED-latency budget (attention/context features, catalog L10 SlidingServe MAE 2.5 ms); align the fused batch width to GB10 tiles.
- If mishandled: A token-count firewall lets a long-context prefill silently exceed the frame budget despite a "small" token count → tail-latency spikes.

### TTS-60 — Intra-node spatial P/D vs chunked-prefill firewall (measured A/B)
- Level: compound
- Pipeline: TTS (prefill/decode disaggregation)
- Axes: disagg:intra-node-spatial, ab-test, tbt-spike
- Scenario: On GB10 the chunked-prefill firewall is predicted to cause ~8× TBT tail spikes that intra-node SM-partition P/D avoids (catalog L4: TaiChi/Nexus/MORI-IO).
- System must: A/B-test intra-node spatial prefill/decode partitioning vs the chunked-prefill firewall on GB10 (strict-TPOT/relaxed-TTFT quadrant = the isochronous frame-clock); adopt whichever protects the decode cadence.
- If mishandled: Assuming "disagg is DC-only" leaves the ~8× chunked-prefill TBT spike on the table — a real, un-evaluated competitor for the frame-deadline metric.

### TTS-61 — Progress watchdog keyed on last-audio-emitted
- Level: compound
- Pipeline: TTS (liveness/watchdog)
- Axes: watchdog:last-audio, liveness, device-aware-deadline
- Scenario: A synthesis session goes "alive but zero forward progress" (a loop that passes every health check, catalog H9 #39863).
- System must: Track monotonic "last-audio-emitted-at T" per session, checked by an independent thread; no audio for > N×frame-interval on an active session → kill/restart sidecar; per-inference deadline DEVICE+MODEL-aware (a 1.5B AR-TTS step ≠ a CTC step), not a flat 300 s.
- If mishandled: A stalled synthesis loop answers /health 200 while emitting no audio → the caller hangs indefinitely with no recovery.

### TTS-62 — Sidecar crash = failed requests, not a hang
- Level: compound
- Pipeline: TTS (sidecar crash detection)
- Axes: crash:3-layer, dead-flag-fanout
- Scenario: The Path-B torch sidecar dies mid-synthesis (CUDA error / OOM / SIGKILL).
- System must: Use 3-layer detection — sentinel wait on the child, in-band `ENGINE_CORE_DEAD` byte, passive waitpid/pidfd re-poll (catalog H6/G7) — and one `dead` flag drives BOTH admission-reject AND error fan-out into every live WS send so all sessions fail-fast in ~1 s.
- If mishandled: The parent answers /health 200 while throughput→0, or live streams hang waiting for chunks that will never come.

### TTS-63 — PR_SET_PDEATHSIG so a killed parent doesn't orphan the GPU sidecar
- Level: compound
- Pipeline: TTS (sidecar teardown)
- Axes: teardown:pdeathsig, vram-leak
- Scenario: The parent server is SIGKILLed; the GPU TTS sidecar is mid-CUDA-kernel and can't poll a death-pipe.
- System must: Set `prctl(PR_SET_PDEATHSIG, SIGTERM)` at sidecar entry (kernel-guaranteed even under SIGKILL, catalog H7); teardown order abort-collectives → SIGTERM→grace→SIGKILL; never hard-cut mid-utterance, never unbounded drain.
- If mishandled: The orphaned GPU sidecar pins VRAM into the next process (#34643) → the restarted server can't allocate, crash-loop.

### TTS-64 — Bounded drop-oldest egress for a slow TTS consumer
- Level: compound
- Pipeline: TTS (egress backpressure)
- Axes: egress:bounded-queue, slow-consumer, drop-oldest
- Scenario: A live WS consumer drains audio slower than realtime; the per-stream egress buffer grows.
- System must: Use bounded queues — for the ordered egress, sender-side credit backpressure (never reorder audio); for stale-worthless audio under HWM pressure, bounded drop-oldest (catalog H2/G6); cap all per-stream bookkeeping (trim 10000→5000).
- If mishandled: HWM=0 unbounded queues let a slow consumer silently accumulate GBs of stale audio → OOM; an uncapped bookkeeping set leaks over a long-lived server.

### TTS-65 — Warm-up gates readiness (no first-request capture cliff)
- Level: compound
- Pipeline: TTS (lifecycle/readiness)
- Axes: readiness:warmup-gate, capture-cliff, calibration
- Scenario: A fresh TTS process must not serve until CUDA-graph capture + per-stage calibration complete.
- System must: Warm up 2–3 full-mask steps with `synchronize()` (fills conv/KV boundary state + forces graph capture OFF the hot path, catalog F6/C7); `/readyz` returns non-200 until warmup+calibration done — not process-up.
- If mishandled: Serving on process-up makes request-1 pay seconds of graph capture (the first-request cliff) or hit an uncalibrated admission ledger.

### TTS-66 — CUDA-graph capture-OOM detected at boot, not request-1
- Level: compound
- Pipeline: TTS (CUDA-graph, GB10/sm120)
- Axes: cuda-graph:capture-oom, sm120, pre-flight
- Scenario: On GB10/sm120 the CUDA-graph pool capture OOMs AFTER /health passes (catalog H4 #44209 crash-loop).
- System must: Reserve the CUDA-graph-pool delta BEFORE admitting; run a pre-capture feasibility check at boot (fail at boot, not request-1); capability-driven graph ladder auto-downgrades to eager, never crashes; freeze GC during capture, weak-ref outputs.
- If mishandled: Capture-OOM after /health passes crash-loops the process under load — the worst sm120 graph scar.

### TTS-67 — enforce_eager as a first-class OOM/capture-failure escape
- Level: compound
- Pipeline: TTS (kernel fallback)
- Axes: fallback:enforce-eager, oom-ladder
- Scenario: On a low-VRAM RTX box, CUDA-graph + compile capture for a large TTS model costs real memory and can OOM.
- System must: Expose `enforce_eager` as a first-class config + automatic fallback on capture failure (catalog C8); OOM ladder: enforce-eager → cpu-offload → layerwise/block offload → (codec) slicing → reduce shape → TP.
- If mishandled: Treating eager as only a debug flag leaves no escape when graph/compile capture OOMs → the model can't load on the edge tier at all.

### TTS-68 — Reliable barge-in abort (not fire-and-forget PUB/SUB)
- Level: compound
- Pipeline: TTS (multi-stage cancellation)
- Axes: barge-in:reliable-abort, per-stage-ack
- Scenario: A barge-in must cancel a multi-stage TTS DAG (AR→CFM→vocoder) reliably; a PUB/SUB broadcast can drop to a not-yet-connected stage (catalog G9).
- System must: Use a reliable abort channel with per-stage ack (not fire-and-forget); fail-fast so one terminal cancel aborts the request across all stages within ≤1 tick.
- If mishandled: A best-effort abort published before a late stage connects is LOST → the vocoder keeps emitting audio after the user barged in.

### TTS-69 — Per-stage micro-batch never mixes streaming and non-streaming
- Level: compound
- Pipeline: TTS (micro-batch collector)
- Axes: micro-batch:no-mix-stream, 2ms-deadline
- Scenario: The CFM/vocoder micro-batch collector drains its inbox with concurrent streaming (live) and non-streaming (offline) requests.
- System must: Collect up to `max_batch_size=4` within a ~2 ms deadline but NEVER mix streaming and non-streaming in one batch (catalog "out-of-order arrival"); bucket by length.
- If mishandled: Mixing streaming and offline in one batch couples their latencies — the offline request's size inflates the live request's per-step time → live underrun.

### TTS-70 — Out-of-order arrival: vocoder receives stream chunks before its payload
- Level: compound
- Pipeline: TTS (DAG, out-of-order)
- Axes: dag:pre-payload, opt-in, chunk-id
- Scenario: On parallel DAG paths, the vocoder node receives AR stream-chunks BEFORE its own request payload arrives (catalog G "out-of-order arrival").
- System must: Make pre-payload stream acceptance EXPLICIT opt-in (`can_accept_stream_before_payload`) with a monotone `chunk_id` per (req,target); the vocoder latches the codec contract from whichever (payload|chunk-meta) arrives first; else hard-fail.
- If mishandled: Silently accepting pre-payload chunks without the opt-in corrupts the vocoder state (wrong codec contract) → garbled audio.

### TTS-71 — Conditional fan-in for the thinker→talker→vocoder S2S DAG
- Level: compound
- Pipeline: TTS (S2S DAG fan-in)
- Axes: dag:dynamic-fan-in, deadlock, multi-terminal
- Scenario: A text+audio S2S DAG (thinker→talker→vocoder) has a request whose branch won't fire (text-only turn → no audio encoder output), and a fixed `wait_for=[...]` would deadlock (catalog G11).
- System must: Use `wait_for_fn(req)→expected_sources` (dynamic per-request fan-in), constrain `route_fn` to the static topology, support multi-terminal merge (text-only vs text+audio narrowing).
- If mishandled: A fixed fan-in waits forever for an audio output that a text-only turn never produces → the DAG deadlocks.

### TTS-72 — Per-component mixed precision: quant the GEMMs, keep norms/RoPE/codec fp32
- Level: compound
- Pipeline: TTS (precision, codec-AR)
- Axes: precision:mixed, quant:gemm-only, fp32:norm-rope-codec
- Scenario: A 4B codec-AR model (higgs) is quantized for the edge; the big LM GEMMs tolerate int8/fp8 but norms/RoPE/sampling/codec must stay high-precision (§5.2).
- System must: Apply per-component precision (`component_precision{logical→prec}`); architecture defaults keep norms/RoPE/codec/head high-precision with zero user config — quant noise compounds across AR frames.
- If mishandled: Quantizing norms/RoPE/codec drifts the AR generation (the WER-flat/MOS-crash signature) — text gates pass while audio quality crashes.

### TTS-73 — TTS accuracy gate includes a perceptual/MOS check
- Level: compound
- Pipeline: TTS (accuracy gate)
- Axes: gate:mos, validation-pyramid, fail-closed
- Scenario: A quantized TTS variant is loaded; a text-only (WER round-trip) gate would pass the exact AR-drift bugs WaaV hit (§5.2).
- System must: The TTS load-time gate MUST include a perceptual/MOS check vs `reference_precision` on fixtures + streaming-playback + concurrent-load layers (catalog I4 validation pyramid); persist a `verified{substrate,precision,metric}` stamp; unverified ⇒ refuse or fall back + emit `waav_quant_gate_failed`.
- If mishandled: A WER-only gate passes a variant that sounds broken (the WER-flat/MOS-crash signature) → degraded audio ships silently.

### TTS-74 — Format must match substrate: int8 file never lands on ORT-CUDA
- Level: compound
- Pipeline: TTS (precision×substrate)
- Axes: precision:int8, substrate:ort-cuda-cant, fallback
- Scenario: An int8 TTS checkpoint is selected on GB10 where the ORT CUDA-EP cannot run int8/4-bit GEMM (silently partitions to CPU: measured 12 ms fp → 232 ms int8, §5.2).
- System must: Resolve precision per-substrate (`$WAAV_PRECISION → by_substrate[ep] → precision → fp32`) so an int8 file never lands on ORT-CUDA; reach int8/fp8/fp4 tensor cores via the TensorRT EP or the torch sidecar tier (which owns kernels via torchao).
- If mishandled: An int8 GEMM silently partitions to the CPU EP → a ~19× per-step latency LOSS (the memory win becomes a latency loss).

### TTS-75 — fp8 is a DC-batch lever, bf16 for edge batch-1
- Level: compound
- Pipeline: TTS (precision tiering)
- Axes: precision:fp8-dc, bf16-edge, batch-tiered
- Scenario: The same codec-AR model serves edge (GB10, N=1) and DC (B200, large N); fp8 is 0.62× at M=64 but 2.1× at M=4096 (§1.6).
- System must: Tier precision like kernels — bf16 for edge/batch-1 (fp8 hurts there), fp8/mxfp4 for DC/large-batch where it helps; resolve per active EP + batch regime.
- If mishandled: Forcing fp8 at batch-1 on the edge makes the model SLOWER (0.62×); forcing bf16 at B200 scale leaves the 2.1× DC throughput on the table.

### TTS-76 — KV-quant scales concurrency for a big-KV TTS (higgs-7B-class)
- Level: compound
- Pipeline: TTS (KV precision)
- Axes: kv-quant:int4, concurrency-lever, big-kv
- Scenario: A higgs-7B-class (8 kv_heads) TTS is concurrency-limited by KV (268 MB/stream → 149 streams/40 GB; int4 → 596, §1.6).
- System must: KV-quant (int4) to raise the slot ceiling ~4× for big-KV models (gate it through the MOS-inclusive accuracy gate); for small codec-LMs (neutts 0.5B, 25 MB/stream) KV-quant is irrelevant — don't pay its accuracy risk.
- If mishandled: Skipping KV-quant on a big-KV TTS caps concurrency ~4× too low; applying it to a small codec-LM adds accuracy risk for ~zero concurrency gain.

### TTS-77 — Calibration measures step-time WITHOUT the profiler
- Level: compound
- Pipeline: TTS (calibration)
- Axes: calibration:no-profiler, warmup-excluded, per-stage
- Scenario: The duty ledger needs `T_step(B_active)` per stage per substrate; running calibration under torch-profiler distorts latency ("Command Buffer Full" is profiler overhead, catalog B).
- System must: Measure step-time WITHOUT the profiler, exclude the first-request lazy-init, A/B one variable, report in ms; persist keyed `sha256 × device × driver × warm-set` (§6).
- If mishandled: Calibrating under the profiler inflates measured step-time → the admission ledger under-admits (or, if measured idle, over-admits and glitches).

### TTS-78 — Graceful degradation beats hard-reject at 50% overload
- Level: compound
- Pipeline: TTS (overload policy)
- Axes: overload:graceful, niyama, playback-buffer
- Scenario: At 50% overload, hard-reject yields <20% deadlines met while graceful relegation/brownout yields 95%+ (catalog L9: Niyama/BrownoutServe).
- System must: Make deadline-aware admission + graceful degradation PRIMARY (EDF↔SRPF interp, slack-driven dynamic chunking, quality-brownout); hard reject only at true saturation; cadence protected by the CLIENT PLAYBACK BUFFER; reject-don't-glitch is the backstop not the only tool.
- If mishandled: Crude hard-reject at the first sign of overload sheds far more sessions than a deadline-aware scheduler needs to (the reject-don't-glitch baseline is beaten).

### TTS-79 — Same-process fan-out clones the owned container (no aliasing)
- Level: compound
- Pipeline: TTS (DAG, fan-out)
- Axes: fan-out:move-ownership, aliasing-bug
- Scenario: One AR result fans out to two downstream stages in the same process; passing by `Arc<Mutex<Payload>>` re-introduces the aliasing hazard (catalog G5).
- System must: Move `Box<Payload>` across the in-process channel (borrow-checker enforces non-aliasing for free); on fan-out, clone the owned container, share `Arc` only for immutable tensor leaves; serialize ONLY cross-process.
- If mishandled: Sharing one mutable payload across two stages → a mutation in one corrupts the other (silent cross-stage corruption).

### TTS-80 — Concurrent 64-stream lockstep: idle slots stay masked
- Level: compound
- Pipeline: TTS (lockstep, 64 streams)
- Axes: lockstep:masked-not-removed, cuda-graph-stable
- Scenario: 64 codec-AR streams run lockstep; some idle between utterances each tick.
- System must: Keep idle slots in the fixed batch (masked-not-removed) so B/T/cache shapes stay constant → one CUDA graph lasts the server lifetime (catalog F7); drop them and B changes → re-capture every frame (catastrophic).
- If mishandled: Removing idle slots to "save compute" changes the batch shape every tick → CUDA-graph re-capture per frame → catastrophic stalls.

### TTS-81 — Idle-slot energy/bandwidth is budgeted under heterogeneous residency
- Level: compound
- Pipeline: TTS (lockstep, masked-waste)
- Axes: masked:not-free, energy:48pct, repack-or-budget
- Scenario: Under heterogeneous residency (barge-in/EOS/VAD), many slots idle; padding waste 13%@BS1→40%@BS32, idle-lane energy ~48% (catalog L8).
- System must: Either compact/repack active slots OR explicitly budget the masked-slot energy/bandwidth cost in the duty ledger (§L8) — masked slots are NOT free under heterogeneous residency.
- If mishandled: Treating masked slots as free over-admits and burns ~48% serving energy on idle lanes; the "flat-to-64" assumption (which holds for active slots) is misapplied to a sparse batch.

### TTS-82 — Ring-KV wraparound: logical-position masking on long synthesis
- Level: compound
- Pipeline: TTS (ring-KV, wraparound)
- Axes: ring-kv:wraparound, logical-pos-mask, test-vectors
- Scenario: A long synthesis exceeds the ring context; once offset > context, physical slot order ≠ time order and a naive causal mask attends to FUTURE tokens (catalog F4).
- System must: Store logical position per cell; mask by `pos ≤ my_pos` (causal) AND window AND never-written⇒-1 (kv_cache.rs); bake the Kyutai pre-wrap/exact-fill/post-wrap/mixed-mask test vectors.
- If mishandled: A naive `j≤i` mask after wrap attends to future/recycled cells → corrupted prosody/garbled audio on any long-form synthesis.

### TTS-83 — Future-step marker/flush so the tail isn't truncated
- Level: compound
- Pipeline: TTS (codec-AR, acoustic delay)
- Axes: marker:future-step, acoustic-delay, no-truncation
- Scenario: A codec-AR model with an acoustic delay must emit its last frames after the input ends; signaling "done" at input-exhaustion truncates the tail (catalog F5).
- System must: Fire "stream done" at `now + acoustic_delay + buffered_frames` via a step-ordered `BinaryHeap<Marker>`; for the one-shot/POST path append trailing silence to flush the delay pipeline; terminate on the marker NOT input-exhaustion; free slot only after offset ≥ real_end (NEVER on disconnect alone — tail still draining).
- If mishandled: Terminating on input-exhaustion cuts off the final words/phonemes of every utterance (truncated audio).

### TTS-84 — Acoustic-delay per-codebook ring sized max_delay+2
- Level: compound
- Pipeline: TTS (codec-AR, multi-codebook delay)
- Axes: acoustic-delay:per-codebook, off-by-one, pad-force
- Scenario: A multi-codebook acoustic-delay model (Moshi-class TTS) writes/reads codebooks at staggered delays; a too-small ring collides the max-delay write with the oldest read.
- System must: Size the per-codebook delay cache at `max_delay+2` (the +2 = off-by-one guard); write `(offset+delays[k])%CT`, read `(offset-max_delay+gen_delays[k])%CT`; teacher-force codebooks≥1 to PAD before `step<acoustic_delay`.
- If mishandled: A `max_delay+1` (or naive) ring collides write/read → corrupted codebook alignment → garbled/buzzy audio; missing the pad-force reads non-existent acoustic tokens in the warm-up window.

### TTS-85 — Slot-leak-on-disconnect: free from inside the step loop (multi-trigger)
- Level: compound
- Pipeline: TTS (slot lifecycle)
- Axes: slot-leak:multi-trigger, gauge:used-total
- Scenario: A live TTS caller disconnects mid-utterance; a disconnect callback alone can be missed (catalog F9).
- System must: Free the slot from INSIDE the step loop on ANY of {receiver closed, sender disconnected, send error, ping-timeout 20s, idle-timeout 120s}; expose an open-slots/`used/total_slots` gauge (autoscale signal); ping every 10 s.
- If mishandled: Relying solely on a disconnect callback leaks slots on missed callbacks → capacity silently erodes until the server rejects everyone.

### TTS-86 — WS write-coalescing disabled for per-frame flush
- Level: compound
- Pipeline: TTS (transport/egress)
- Axes: transport:no-coalesce, jitter, write-buffer-0
- Scenario: A streaming TTS route uses default WS write coalescing, which adds tens-of-ms jitter to an 80 ms frame budget (catalog F10).
- System must: Set `write_buffer_size(0)` on every streaming route so each audio frame flushes immediately; meter per-stream buffer depth + per-step wall time as first-class metrics.
- If mishandled: Default coalescing batches frames → tens-of-ms jitter on an 80 ms budget → audible stutter despite RTF<1.

### TTS-87 — Pin attention-sink tokens for a many-turn agent voice
- Level: compound
- Pipeline: TTS (long-context, agent)
- Axes: long-context:attention-sink, many-turn, no-wraparound-instability
- Scenario: A voice agent holds a long multi-turn conversation; the TTS context grows past the ring and naive sliding-window forgetting destabilizes prosody (catalog L12, StreamingLLM).
- System must: Pin attention-sink tokens (the initial tokens) + provide a paged/full-context escape; reuse the cached system-prompt prefix (ties TTS-26) across turns.
- If mishandled: Sliding-window wraparound without pinned sinks causes prosody instability/forgetting across a long agent session.

### TTS-88 — DC spill: constant-time KV migration without a glitch
- Level: compound
- Pipeline: TTS (DC scale, rebalance)
- Axes: dc:kv-migration, llumnix, playback-buffer-masked
- Scenario: At B200 fleet scale, a TTS stream must move to a less-loaded replica for rebalancing (M5/§6).
- System must: Use Llumnix-style constant-time (sub-ms–5 ms for voice ctx via NIXL) append-only KV migration; one decode-step > one frame so mid-stream migration drops ≥1 frame UNLESS playback-buffer-masked (catalog L16) — migrate during a buffered moment.
- If mishandled: A non-constant-time or unmasked migration drops audible frames mid-utterance for the migrated caller.

### TTS-89 — Determinism: per-stream reproducible, not bitwise-global
- Level: compound
- Pipeline: TTS (sampler determinism)
- Axes: determinism:per-stream, float64-gumbel, atomic-reductions
- Scenario: A caller expects reproducible audio for a fixed seed, but bitwise-global determinism is impossible under atomic reductions (catalog H "Determinism", #24067).
- System must: Accept per-stream-only determinism — seed the sampler RNG per stream (gate the offset through the masked-select, F2), use float64 Gumbel; document that cross-batch bitwise reproducibility is not guaranteed.
- If mishandled: Promising bitwise-global determinism is unmeetable; failing to seed per-stream makes even single-stream output non-reproducible.

### TTS-90 — Cohort A/B share the GPU temporally, not in one step
- Level: compound
- Pipeline: TTS (multi-cohort scheduling)
- Axes: cohort:temporal-share, duty-ledger, two-models
- Scenario: A 12.5 Hz cosyvoice cohort and a 25 Hz chatterbox cohort co-reside on one GB10 (different frame-rates → different ticks).
- System must: Have the cohorts share the GPU temporally via the per-substrate duty ledger (sum both cohorts' duty ≤ S, §4.2); never fuse them into one step; admit a new stream only if its cohort's tick fits within the remaining duty.
- If mishandled: Co-scheduling without a duty ledger lets one cohort's ticks starve the other → the lower-priority cohort misses its frame budget.

### TTS-91 — Multilingual model: voice ≠ language decoupling
- Level: compound
- Pipeline: TTS (multilingual, voice×language)
- Axes: multilingual:voice-language-cross, conditioning
- Scenario: A caller wants voice A (an English speaker's timbre) speaking French on a model that decouples speaker embedding from language.
- System must: Condition on the speaker embedding for voice A AND select the French G2P/frontend independently; validate the model supports cross-lingual voice transfer (else document the constraint).
- If mishandled: Coupling voice to language forces the French-accented default voice, or the English G2P mispronounces the French text.

### TTS-92 — Repacketize variable codec chunks to fixed 20 ms RTP
- Level: compound
- Pipeline: TTS (+transport, telephony)
- Axes: transport:rtp-20ms, jitter-buffer, variable-chunk
- Scenario: The codec emits variable-size chunks (e.g. 80 ms Mimi frames) but the SIP/RTP leg needs fixed 20 ms packets.
- System must: Buffer codec output in a per-stream jitter buffer and repacketize to fixed 20 ms RTP (§5.1); decouple the codec frame size from the RTP packet size.
- If mishandled: Emitting 80 ms (or variable) packets onto a 20 ms RTP leg breaks pacing → the far end stutters or drops packets.

### TTS-93 — Sentence lookahead for flow models (chunk amortization)
- Level: compound
- Pipeline: TTS (flow, lookahead)
- Axes: flow:sentence-lookahead, ttfa-vs-quality
- Scenario: A flow/CFM model (cosyvoice3) benefits from a small lookahead window to stabilize chunk boundaries while keeping TTFA low.
- System must: Process a bounded lookahead (e.g. next sentence/phrase boundary) to give the CFM context, but emit the current chunk as soon as its frames are stable (overlap-add with left-context); cap the lookahead so TTFA stays bounded.
- If mishandled: No lookahead causes chunk-boundary instability (artifacts); unbounded lookahead defeats streaming (buffers to sentence-end).

---

## EXTREME

### TTS-94 — Adversarial never-stops text drives runaway generation
- Level: extreme
- Pipeline: TTS (codec-AR, runaway)
- Axes: adversarial:never-stops, max-length-cap, slot-free
- Scenario: A crafted input (repetitive tokens / a model in a degenerate state) never reaches the stop token, generating indefinitely and pinning the slot.
- System must: Enforce a hard max-frame cap per utterance as a backstop to the learned stop token; on cap, emit FINAL + free the slot; surface a `runaway_capped` metric.
- If mishandled: No cap → the slot never frees (capacity leak) and the caller hears endless babble; one bad input degrades the whole server.

### TTS-95 — Repetition loop: model gets stuck repeating a phrase
- Level: extreme
- Pipeline: TTS (codec-AR, repetition)
- Axes: adversarial:repetition-loop, detection
- Scenario: A codec-AR model enters a repetition loop (same codec frames cycling) — distinct from never-stops in that output is periodic.
- System must: Detect frame-level repetition (n-gram over codec tokens) and break/terminate; keep repetition_penalty at the reference value (TTS-5) so detection complements, not fights, the model's statistics.
- If mishandled: An undetected repetition loop produces a stuck-record artifact until the max-length cap, wasting compute and slot residency.

### TTS-96 — Mid-batch NaN must not kill the other 63 streams
- Level: extreme
- Pipeline: TTS (lockstep, fault isolation)
- Axes: fault:per-stream-nan, batch-isolation, reject-frame
- Scenario: One stream in a 64-slot lockstep batch produces a NaN logit; the dense kernel shares the batch with 63 healthy streams.
- System must: Per-row NaN detection (H1) rejects only the offending stream's frame (repeat-prev/silence) while the other 63 rows proceed; the masked-select discipline (F2) ensures one bad row can't corrupt others' state.
- If mishandled: A single NaN row that propagates (via an ungated mutation or a shared reduction) corrupts or crashes the whole batch — all 64 callers glitch.

### TTS-97 — FlashTTS: MTP-3 + 2-NFE meanflow breaks BOTH batchers in one model
- Level: extreme
- Pipeline: TTS (multi-AR + inner-flow, third class)
- Axes: paradigm:third-class, mtp-3+meanflow, variable-stride+variable-nfe
- Scenario: FlashTTS emits MTP-3 (3 tokens/step) AND runs a 2-NFE meanflow inner head — two lockstep violations at once (catalog L5).
- System must: Use the THIRD execution class "AR-outer + generative-INNER head" — lockstep advances a variable stride (MTP-3) while the inner variable-NFE flow/consistency micro-batch composes INSIDE one AR step (the nested batcher composes two batchers per step, not picks one).
- If mishandled: A batcher that assumes one-frame-per-tick + fixed-NFE can't represent FlashTTS at all → the production model is unservable (or runs at 1/3 the intended rate).

### TTS-98 — Streams at different inner-NFE can't share a lockstep tick
- Level: extreme
- Pipeline: TTS (variable-NFE, mixed)
- Axes: nfe:per-stream-runtime-dial, mixed-trajectory, calm
- Scenario: Two cloned-voice streams run the same outer model but different inner NFE (one prioritizes quality NFE=10, one latency NFE=2) — a per-stream runtime dial (catalog L5: CALM/VoxCPM).
- System must: Fold the inner solve as a per-stream variable-NFE micro-batch composed inside the AR step; pad/align the shorter-NFE streams (run their no-op steps masked) OR bucket by inner-NFE within the cohort; the bucket key accepts mixed trajectories (L15).
- If mishandled: Assuming a single global NFE forces both streams to one step count → either the latency stream pays quality's cost or the quality stream is degraded.

### TTS-99 — Audiobook 30k-token context: generic LLM-KV methods fail on audio
- Level: extreme
- Pipeline: TTS (long-form, 10min+)
- Axes: long-form:30k-tokens, audiokv, ring-escape
- Scenario: A 10-minute continuous audiobook generation reaches 30k+ tokens where generic LLM-KV compression methods FAIL on audio (catalog L12: AudioKV).
- System must: Escape the lossy ring to a paged/full-context path with pinned attention sinks; use audio-aware KV management (not generic text KV eviction); chunk at sentence boundaries with carried style to bound per-segment context.
- If mishandled: Applying generic text-KV eviction (or wrapping the ring) at 30k tokens degrades prosody/coherence audibly mid-book.

### TTS-100 — Barge-in storm: 50 callers interrupt simultaneously
- Level: extreme
- Pipeline: TTS (barge-in, storm)
- Axes: barge-in:storm, reliable-abort, slot-churn
- Scenario: A broadcast event makes 50 concurrent callers barge-in within one tick window (mass cancellation + immediate slot reuse).
- System must: Process all 50 reliable aborts (per-stage ack, G9) + 50 transactional `reset_slot`s (F3) before the next compute step; the channel-id guard drops the 50 stale outputs; admit replacements only after reset completes.
- If mishandled: A best-effort abort drops some cancels (callers keep hearing audio); un-transactional resets let new callers inherit the previous occupants' state (50 cross-contaminations at once).

### TTS-101 — GB10 shared-bandwidth saturation: AR + codec + STT contend
- Level: extreme
- Pipeline: TTS (unified-memory contention)
- Axes: unified:bandwidth-divide, 273gbs-ceiling, co-tenant
- Scenario: On GB10, AR-TTS decode (GPU) + codec decode (NPU) + a co-tenant STT encoder all contend for the one ~273 GB/s LPDDR ceiling (§3.4 contention guard).
- System must: Budget aggregate memory bandwidth as a schedulable resource (the shared-pool ledger, §6); prefer to overlap the memory-bound AR with a compute-bound conv-codec; co-locate + time-share when both saturate bandwidth; reject admission if the shared-pool duty would exceed S·ceiling.
- If mishandled: Concurrent engines divide the one ceiling unaccounted → all three slow down → TTS underruns even though no single engine is "overloaded."

### TTS-102 — Multinomial-in-CUDA-graph + per-stream seed + NaN-reject compose correctly
- Level: extreme
- Pipeline: TTS (sampler, composed edge cases)
- Axes: sampler:compose, gumbel-argmax+seed+nan-reject, graph-safe
- Scenario: An edge GB10 deployment captures the step in a CUDA graph, needs stochastic per-stream sampling, AND must reject NaN frames — all inside/around the fixed-shape captured region.
- System must: Gumbel-argmax inside the graph (graph-safe stochastic, F7) with a per-stream float64 seed offset gated through the masked-select (F2/H5), and the NaN check (H1) as a cheap reduction on the post-replay logits before the sampler; reject-frame on NaN by substituting the previous frame.
- If mishandled: Any one mishandled — multinomial breaks capture, an ungated seed corrupts on idle-resume, or an un-checked NaN argmaxes to garbage — produces a silent audible defect under concurrency.

### TTS-103 — Codec fp32 numerics under a quantized AR backbone
- Level: extreme
- Pipeline: TTS (precision boundary)
- Axes: precision:fp32-codec-island, quantized-backbone, dtype-boundary
- Scenario: A heavily int4-quantized AR backbone feeds a codec decoder that MUST stay fp32 — a dtype boundary crosses inside one forward (graph-driven dtype, per WaaV's q4f16 learning).
- System must: Make the dtype boundary graph-driven via `StaticGraph::input_types()` — backbone outputs upcast to fp32 at the codec input; codec/vocoder weights+math fp32; empty-tensor/KV dtypes match the backbone (f16) but `input_features`/`audio_embeds` stay f32 (the proven q4f16 recipe generalizes).
- If mishandled: A hardcoded dtype at the boundary either runs the codec in the backbone's low precision (corrupts audio, autocast-style) or mismatches an empty-tensor dtype (graph error / silent garbage).

### TTS-104 — Concurrent multi-voice WITH per-stream voice-clone prefixes
- Level: extreme
- Pipeline: TTS (codec-AR, multi-clone batch)
- Axes: concurrency:multi-clone, prefix-fingerprint+slot-reset, batch
- Scenario: 16 concurrent streams each clone a DIFFERENT ref-audio on the same codec-AR model — each slot has a distinct prefix, all co-batched in one lockstep tick.
- System must: Fingerprint each slot's prefix over all codebooks (G1) so the prefix-cache never cross-contaminates; prefill each slot's distinct ref independently (firewall ≤1 prefill/K frames, TTS-58); reset clone state per slot on recycle (F3); lockstep-batch the steady-state decode across all 16 distinct-voice slots.
- If mishandled: A cb0-only or position-only prefix key collides the 16 different refs → callers hear each other's cloned voices (mass wrong-voice contamination under load).

### TTS-105 — Dynamic frame-rate codec + lockstep batch (variable stride per slot)
- Level: extreme
- Pipeline: TTS (FlexiCodec, batched variable-stride)
- Axes: codec:flexicodec, batch:per-slot-variable-stride, data-dependent
- Scenario: Multiple FlexiCodec streams co-batch, but each slot's frame-rate varies 3–12.5 Hz data-dependently per-frame — slots advance different strides on the same tick (catalog L6 + L5).
- System must: Advance the densest common stride per tick; slots at a coarser current rate run masked no-op sub-steps (catch up on their next active frame); the duty ledger budgets the worst-case (12.5 Hz) stride; cohort key tolerates the unknown-a-priori rate.
- If mishandled: Forcing all slots to one fixed stride either over-generates (wastes compute on coarse-rate slots) or mis-paces (under-generates fine-rate slots) → audible artifacts on the variable-rate streams.

### TTS-106 — One bad slot's runaway must not starve the lockstep tick
- Level: extreme
- Pipeline: TTS (lockstep, fairness)
- Axes: fairness:slowest-paces-all, runaway-isolation, budget
- Scenario: One slot hits a pathological long inner-NFE / repetition loop that inflates that slot's per-step cost, and lockstep makes the slowest stream pace all (catalog L8).
- System must: Cap per-slot inner-step work to the cohort's budget; the runaway slot is reject-framed/terminated (TTS-94/95) rather than allowed to stretch the tick; FCFS-within-slot-pool + wall-clock aging (H8) keeps the others fair.
- If mishandled: The slow slot stretches every tick → all 63 healthy streams miss their frame budget (one bad input degrades everyone).

### TTS-107 — Mixed offline + streaming on one model without latency coupling
- Level: extreme
- Pipeline: TTS (mixed workload)
- Axes: workload:offline+streaming, no-mix-batch, priority
- Scenario: One TTS model serves both a batch audiobook job (offline, latency-tolerant) and live calls (streaming, isochronous) concurrently.
- System must: Never mix streaming and non-streaming in one batch (TTS-69); priority Realtime > Batch per stage (Sarathi piggyback Batch into leftover budget); the offline job fills idle slots/duty but yields immediately to live admission.
- If mishandled: Co-batching the offline job with live streams couples their step-times → live underruns; starving the offline job entirely wastes the idle capacity it could safely use.

### TTS-108 — TTFA ramp + prefill firewall + barge-in compose on a cold clone
- Level: extreme
- Pipeline: TTS (compound first-audio path)
- Axes: ttfa:ramp+firewall+barge-in, cold-clone, sub-300ms
- Scenario: A new cloned-voice stream must hit sub-300 ms first-audio (large first chunk, ramp down) WHILE its ref-prefill is firewalled AND the user may barge-in during the opening.
- System must: Chunk the ref-prefill (≤1/K frames, power-of-two, TTS-58/59), start the larger first chunk as soon as enough frames decode (TTFA ramp, TTS-17), and keep barge-in able to cancel the cold stream within ≤1 tick (reliable abort + reset, TTS-68/100) even mid-prefill.
- If mishandled: An un-firewalled prefill spikes TBT for everyone, a uniform tiny first chunk misses the 300 ms target unstable, or a barge-in during prefill can't cancel → the caller talks over the opening.

### TTS-109 — Edge↔DC: same dots.tts DAG, only executor + precision differ
- Level: extreme
- Pipeline: TTS (config-scaling)
- Axes: config-scaling:edge-dc, inline-vs-stage-batched, bf16-vs-fp8
- Scenario: The identical 2-node nested dots.tts manifest runs on GB10 (N=1, bf16, CUDA-graph, Inline mode) and on a B200 fleet (large lockstep N, fp8/mxfp4, Stage-batched, KV migration).
- System must: Keep the DAG/stages/nested-loops/placement-hints IDENTICAL across modes; the engine picks the executor from config+load (Inline calls the same stage-forward with B=1 — no second implementation, §8); only the batch ceiling and precision tier change.
- If mishandled: Forking a separate edge vs DC code path doubles maintenance and drifts behavior; running B200's large-batch fp8 kernels on GB10 N=1 makes the edge SLOWER (the batch-tiered-kernel inversion).

### TTS-110 — Spec-decode ban is scoped, not blanket (acoustic vs long-context-STT)
- Level: extreme
- Pipeline: TTS (speculative decode scope)
- Axes: spec-decode:scoped-ban, acoustic-no, mtp-yes
- Scenario: A request to "speed up TTS with speculative decoding" must be evaluated against the evidence that draft-spec-decode is a 0.98× net slowdown on acoustic tokens (catalog L13/L14).
- System must: NOT bolt EAGLE/Medusa on the acoustic-AR TTS path (token→audio is many-to-one; draft-spec-decode destroys rectangular lockstep and nets slower); ADOPT MTP (Depformer/code-predictor as the MTP mechanism, 2–5× quality-neutral) which preserves lockstep; reserve sparse-KV spec-decode for the long-context token-AR-STT paging path only.
- If mishandled: Adding draft-spec-decode to acoustic TTS makes it slower while breaking the rectangular lockstep batch — a double regression sold as an optimization.

### TTS-111 — Out-of-tile prefill (257) + sm120 graph re-capture cascade
- Level: extreme
- Pipeline: TTS (tiling × graph capture)
- Axes: tiling:257-cliff, sm120:recapture-cascade, power-of-two
- Scenario: A prefill sized 257 tokens lands on sm120 where it both pays the ~32% tile penalty AND triggers a 257→272/257→512 power-of-two graph re-capture (catalog H4 + §4.5).
- System must: Quantize prefill/chunk token counts to power-of-two and capture EXACT slot counts (1,2,4…N) with zero padding so the 257-cliff is structurally sidestepped; `dst.zero_()` any padded slots (#43810 wrote -1 into a real KV slot).
- If mishandled: The off-tile size pays 32% per-step AND re-captures the graph on sm120 (a known hang/OOM-prone path) → cascading frame-deadline misses or a capture-OOM crash-loop.

### TTS-112 — VibeVoice DDPM at B64 (624 ms solve) co-tenant with realtime calls
- Level: extreme
- Pipeline: TTS (diffusion, co-tenant budget)
- Axes: diffusion:624ms-solve, co-tenant, duty-budget
- Scenario: A VibeVoice (DDPM, 25-step) batch job at B64 takes a 624 ms solve while live codec-AR calls need their 80 ms ticks on the same GB10 (§1.5).
- System must: Treat the 624 ms DDPM solve as a coarse-grained duty block that the ledger schedules in the leftover budget around the realtime ticks (chunk-amortized, off the AR clock); cap the DDPM batch below the collapse knee; never let the diffusion solve occupy the window a realtime tick needs.
- If mishandled: Letting the 624 ms solve run uninterrupted on the shared GPU starves the live ticks for ~8 frame periods → total dropout for every realtime caller.

### TTS-113 — SSML on a model with no prosody support: degrade, don't speak the tags
- Level: extreme
- Pipeline: TTS (frontend, capability mismatch)
- Axes: ssml:unsupported-model, graceful-degrade
- Scenario: A caller sends rich SSML (`<emphasis>`, `<prosody>`, `<phoneme>`) to a model whose frontend has no prosody/phoneme controls.
- System must: Strip/normalize unsupported SSML to plain text (apply `<break>` as inserted silence if cheap), honor `<phoneme>` only if the model accepts phonemes, and never speak tag literals; surface a capability flag so callers know what was applied.
- If mishandled: Passing SSML through to a tag-unaware G2P speaks "emphasis" / "prosody" aloud, or a strict parser rejects a request the model could have served as plain text.

### TTS-114 — Codec decode offloaded to NPU that only supports static shapes
- Level: extreme
- Pipeline: TTS (codec on NPU, static-shape)
- Axes: placement:codec-npu, static-shape-rigidity, fixed-chunk
- Scenario: The conv-vocoder is placed on a Hexagon/QNN NPU (VTCM ~8 MB, strictly static fixed-shape AOT, §2.2) while the AR backbone runs on GPU.
- System must: Compile the codec for a FIXED chunk shape (the static-conv-GOOD affinity §2.3), pad/segment variable codec output to that fixed chunk before the NPU call, and keep the dynamic AR on the GPU; zero-copy the fixed-shape buffer across the coherent boundary.
- If mishandled: Feeding variable-length codec frames to a static NPU graph fails the fixed-shape contract (re-compile per shape or hard error) → the offload is unusable; exceeding VTCM silently spills to LPDDR (slow).

### TTS-115 — Streaming consolidation path drops the audio key (offline≠stream)
- Level: extreme
- Pipeline: TTS (consolidation correctness)
- Axes: consolidation:audio-key, offline-vs-stream, byte-identical
- Scenario: An offline consumer reconstructs the full audio from streamed chunks, but a consolidation path `continue`s past the audio modality key (catalog I1) → the offline result has only the final chunk.
- System must: Audit emit→consolidate→consume end-to-end so the consolidation concats the audio key (not skips it); the explicit invariant test `offline_concat == stream_concat` byte-for-byte must gate the egress.
- If mishandled: The streamed and offline reconstructions diverge silently (offline gets one chunk) — a correctness bug invisible to RTF/duration checks.

### TTS-116 — Empty/degenerate ref-audio for voice cloning
- Level: extreme
- Pipeline: TTS (voice-clone, bad ref)
- Axes: voice-clone:bad-ref, validation, fingerprint-none
- Scenario: A caller supplies a silent, too-short, or corrupt reference clip for cloning.
- System must: Validate the ref (min length, non-silence, decodable codes) before prefill; on a degenerate ref, reject with a typed error OR fall back to a neutral default voice (documented), and set `extra_key=None` only if truly no usable ref (so it doesn't collide with the zero-shot cache).
- If mishandled: Prefilling a silent/corrupt ref produces a broken/whispered clone, or a degenerate ref's fingerprint collides with another request's cache → wrong-voice output.

### TTS-117 — Frame-budget overrun policy is explicit (buffer+autoscale vs drop)
- Level: extreme
- Pipeline: TTS (overrun policy)
- Axes: overrun:explicit-policy, buffer-vs-drop, rtf
- Scenario: A transient load spike pushes per-step wall time over the frame budget for several ticks (RTF momentarily >1, catalog F10).
- System must: Have an EXPLICIT overrun policy — Moshi-style buffer-in-per-stream-VecDeque + rely on avg RTF<1 + autoscale (no built-in frame-drop), with per-step-wall-time and per-stream-buffer-depth as first-class metrics; shed newest Realtime ≤1/tick only after sustained p99 breach (drift response, §6).
- If mishandled: An implicit/undefined overrun behavior either silently drops frames (audible gap) or unboundedly buffers (growing latency) — neither chosen nor measured.

### TTS-118 — Co-tenant model load promotes Inline → Stage-batched lazily
- Level: extreme
- Pipeline: TTS (mode promotion)
- Axes: mode:auto-promote, lazy-ledger, second-stream
- Scenario: A GB10 box serves one TTS stream Inline (no scheduler); a 2nd concurrent stream or a co-tenant model arrives (§8 `mode=auto`).
- System must: Promote lazily to Stage-batched (the duty ledger spins up on demand) when the 2nd stream/co-tenant appears — the DAG/stages/nested-loops/placement hints stay identical, only the executor changes (Inline → decoupled micro-engines + admission).
- If mishandled: Eagerly running Stage-batched machinery for a single stream wastes edge resources; failing to promote when the 2nd stream arrives serializes them (the `Arc<Mutex>` single-in-flight bottleneck) → needless rejection.

### TTS-119 — CFG-folded batch with a stale shared generator diverges
- Level: extreme
- Pipeline: TTS (flow/diffusion, CFG determinism)
- Axes: cfg:shared-generator, divergence, seeded
- Scenario: A CFG-folded step-bucket batch reuses a generator across the cond/uncond halves and across requests, causing CFG-parallel to diverge from sequential (catalog B).
- System must: Pass a per-request seeded `generator` to the scheduler step so the folded CFG batch is bit-reproducible and matches the sequential reference; the bucket precomputes the schedule once but the noise is seeded per stream.
- If mishandled: A shared/stale generator makes the CFG-folded result non-deterministic and divergent from the reference → unreproducible audio + a parity-gate failure.

### TTS-120 — Higgs-4B on 8 GiB edge: only practical quantized + offload ladder
- Level: extreme
- Pipeline: TTS (large model, edge tier)
- Axes: large-model:4b-on-8gib, quant-mandatory, offload-ladder
- Scenario: Higgs (codec-AR, 4B) must run on an 8 GiB RTX/edge box where it fits only quantized (§5.2: "on 8 GiB edge a 3B model is only practical quantized").
- System must: Load a published int4/AWQ/GPTQ variant (manifest selects, zero-code), gate it through the MOS-inclusive accuracy gate (TTS-73), enforce-eager to save capture memory, and use the OOM offload ladder (cpu/layerwise) if still tight; budget the CUDA-graph-pool delta before admitting.
- If mishandled: Loading the fp16 4B model on 8 GiB OOMs at load; loading an int8 variant onto ORT-CUDA silently CPU-partitions (TTS-74) → the model is unservable or non-realtime on the edge tier.

### TTS-121 — Hibiki-style cross-lingual TTS: language delay-sign switches the task
- Level: extreme
- Pipeline: TTS (multistream delay, S2S-adjacent)
- Axes: multistream:delay-sign, cross-lingual, kyutai
- Scenario: A Kyutai-class multistream model uses one knob (text↔audio delay sign) to switch between STT/TTS/S2S/translation; the TTS/translation mode needs the right delay configuration (catalog §9 item 5).
- System must: Configure K=2Q+1 streams with per-stream integer delays, write/read at `(offset+delay)%context`; set the text↔audio delay sign for TTS/translation; the same lockstep engine serves it via data (no special-case path).
- If mishandled: A wrong delay sign/offset runs the model in the wrong mode (e.g. STT instead of TTS) or misaligns the streams → no coherent audio output.

### TTS-122 — Codec dedup: Mimi/DAC/SNAC decoder shared across co-resident TTS models
- Level: extreme
- Pipeline: TTS (codec dedup, multi-model)
- Axes: codec:shared-decoder, dedup, terminal-node
- Scenario: Three co-resident TTS models (neutts, csm, a Moshi-TTS) all decode through the same Mimi codec; the terminal codec node is the highest-value cross-model dedup point (§3.2).
- System must: Load the shared Mimi decoder ONCE and route all three models' `TokenFrame` outputs to the single codec micro-engine (batch=1 per-stage, TTS-46); the codec is also the one stage safe to offload (CPU/NPU).
- If mishandled: Loading three copies of the Mimi decoder wastes memory/bandwidth on the shared LPDDR ceiling; the dedup opportunity (and the clean offload point) is lost.

### TTS-123 — Tail-latency jitter: p99 first-audio under a noisy co-tenant
- Level: extreme
- Pipeline: TTS (tail latency, jitter)
- Axes: tail:p99-ttfa, jitter-margin, allocation-jitter
- Scenario: Under a bursty co-tenant, TTS first-audio p99 spikes from allocation jitter / GC / coalescing even though the median is fine.
- System must: Eliminate allocation jitter (per-slot fixed ring/arena, no per-step alloc, §4.3), no `empty_cache` in the hot loop, freeze GC during capture, disable WS coalescing (TTS-86), target ~70–80% utilization with a jitter margin (§4.4) so p99 stays under the rated TTFA budget.
- If mishandled: Per-step allocation/GC/coalescing jitter blows p99 first-audio past the SLO even at moderate load — the median-looks-fine, tail-is-broken trap.

### TTS-124 — Mid-utterance must NEVER be preempted (no vLLM admit-then-evict)
- Level: extreme
- Pipeline: TTS (admission, no preemption)
- Axes: admission:non-preemptible, whole-stream-fit, no-thrash
- Scenario: Under load, a scheduler is tempted to preempt a running TTS to admit a higher-priority one (the vLLM admit-then-preempt thrash, catalog H2).
- System must: NEVER preempt mid-utterance; gate admission on WHOLE-STREAM fit (not first-chunk); shed load by reject-at-admission, not admit-then-evict; this deletes the KV use-after-free / num_computed_tokens-reset corruption bug class (#37076/#39146/#36755).
- If mishandled: Preempting a half-utterance recomputes its prefill, evicts it again (thrash), and produces an audible glitch — and re-opens the worst correctness-bug class.

### TTS-125 — Multistream interleave: one engine, text+audio delay table for translation
- Level: extreme
- Pipeline: TTS (multistream, delay engine)
- Axes: multistream:interleave, delay-table, full-duplex-adjacent
- Scenario: A speech-translation TTS interleaves K=2Q+1 streams (source text, target text, target audio) with per-stream integer delays in one ring (catalog §9 item 5, M5).
- System must: Implement the multistream interleave + delay engine — per-stream delays write/read `(offset+delay)%context`; the delay table is the only thing that distinguishes translation from TTS/STT; share the lockstep tick across all K streams.
- If mishandled: Treating each stream as a separate model (or a wrong delay table) loses the frame-synchronous coupling → the target audio desyncs from the source/translation.

### TTS-126 — Step-bucket erosion: length-decoupled step count (LLaDA-TTS)
- Level: extreme
- Pipeline: TTS (step-bucket, length-decoupled)
- Axes: step-bucket:length-decoupled, llada-tts, variable-n
- Scenario: An LLaDA-TTS-class model's cost = T passes INDEPENDENT of output length (catalog L15), eroding the step-bucket's "fixed-N/length-bucketed" assumption.
- System must: Make the bucket key accept per-request variable N (including length-decoupled step count and N=1 feedforward), and not assume CFG-folding is universal (CFG-free flow exists); bucket by the actual (model, step-count, latent-shape) at runtime.
- If mishandled: A rigid length-bucketed/fixed-N bucket mis-batches a length-decoupled model (wrong step count or re-bucketing thrash) → quality loss or wasted compute.

### TTS-127 — Crash blast-radius: flaky encoder/sidecar isolated from the hot AR
- Level: extreme
- Pipeline: TTS (process isolation)
- Axes: isolation:separate-process, blast-radius, crash-layers
- Scenario: A flaky component (a custom codec sidecar, an encoder) shares a process with the hot AR stage; if it raises, the whole process group exits (catalog G7).
- System must: Run the hot AR stage and the flaky encoder/sidecar as SEPARATE processes (stage=process for the hot path); exclusivity invariants are AssertionErrors not comments; the 3 crash-detection layers ensure a dead sidecar = failed requests, not a hang.
- If mishandled: Co-locating the flaky component crashes the AR stage too (max→sum blast radius) → every co-resident TTS stream dies on one component's fault.

### TTS-128 — VoxServe-style binary streaming-viability + risk scheduling
- Level: extreme
- Pipeline: TTS (scheduler objective)
- Axes: objective:binary-viability, risk-of-violation, voxserve
- Scenario: A direct "vLLM-for-voice" competitor (VoxServe, catalog L3) shows 10–20× over vLLM/SGLang/Triton via a BINARY streaming-viability objective (deliver-in-time → further latency worthless) + risk-of-violation scheduling.
- System must: Adopt the binary-viability objective (once first-audio + cadence are met, extra latency is worthless — don't optimize it) and prioritize streams by RISK-OF-VIOLATION (soft-deadline), with cadence protected by the client playback buffer; benchmark against VoxServe/Nexus/DuetServe.
- If mishandled: Optimizing raw latency below the viability threshold (instead of admitting more streams / protecting at-risk ones) leaves the 10–20× goodput VoxServe captures on the table.

### TTS-129 — Marker dropped on disconnect-during-tail: free only after offset≥real_end
- Level: extreme
- Pipeline: TTS (lifecycle, tail draining)
- Axes: lifecycle:tail-draining, marker-not-dropped, no-free-on-disconnect
- Scenario: A caller's connection drops while the acoustic-delay tail is still draining (the last frames not yet emitted, catalog F5 lifecycle).
- System must: Model the ACTIVE→MARKER_RECEIVED→IS_EOS lifecycle; free the slot ONLY after offset ≥ real_end (NEVER on disconnect alone — the tail is still draining); the step-ordered marker heap must not drop the pending marker on disconnect.
- If mishandled: Freeing the slot on disconnect mid-tail truncates the final frames AND can recycle the slot before its marker fires → cross-stream contamination of the late marker.

### TTS-130 — Greenfield serving discipline applied to today's coarse seam (M2 unlock)
- Level: extreme
- Pipeline: TTS (seam migration)
- Axes: seam:arstep-sibling, coarse-to-stepped, registry-unchanged
- Scenario: Today's `TtsModel::synthesize` returns the whole Vec (coarse) and the server holds `Arc<Mutex>` (one in-flight); the lockstep unlock needs a stepped contract WITHOUT touching the 16 registry arms (catalog D).
- System must: Add a sibling `trait ArStepModel` (`prefill(slot,conditioning)`, `step(active_slots)→per-slot frame`, `reset_slot`, `kv_footprint`) that AR TTS models OPTIONALLY implement; one-shot models (kokoro/melo/supertonic) keep ONLY the coarse trait and ride a micro-batch stage; the lockstep scheduler drives `ArStepModel` across a fixed slot table — `StaticGraph` and the registry arms unchanged (M2).
- If mishandled: Rewriting the coarse trait or the registry to add stepping churns all 16 arms and risks every model; keeping only the coarse trait + `Arc<Mutex>` caps the engine at one in-flight stream (forfeits the 55×@64 batching the whole architecture is built on).

---

## Coverage

This catalog enumerates **130 distinct TTS-pipeline scenarios** spanning SIMPLE → INTERMEDIATE → COMPOUND → EXTREME, each a real situation WaaV Infer's TTS path faces.

- **Paradigm coverage:** one-shot VITS/dur-predictor (kokoro/StyleTTS2/melo: TTS-1,7,15); flow-CFM (supertonic/cosyvoice3: TTS-10,18,93,119,126); diffusion DDPM (vibevoice: TTS-20,112); masked-diffusion (omnivoice: TTS-44); codec-AR lockstep (chatterbox/dia/csm/higgs/neutts: TTS-2,9,25,36,80,94); nested AR+inner-flow (dots.tts: TTS-21,22,52); multi-AR/MTP (qwen3-tts/FlashTTS: TTS-45,97,110); and the explicit THIRD execution class (TTS-97,98,105).
- **Streaming correctness:** TTFA ramp + delta-vs-cumulative + FINAL frame + consolidation (TTS-2,3,4,17,56,115); chunk overlap-add/holdback/initial-chunk (TTS-16,17); the offline==stream byte-identical invariant (TTS-3,115).
- **Voice cloning:** ref-audio conditioning, 86% prefix-cache reuse, all-codebooks anti-contamination fingerprint, bad-ref validation, concurrent multi-clone batch (TTS-25,26,27,104,116).
- **Multilingual/frontend/SSML:** per-language G2P, notation normalization, code-switching, locale normalization, voice×language decoupling, SSML degrade (TTS-15,28,29,30,40,91,113).
- **Long-form:** context-consistency, ring-lossy escape, attention-sink pinning, 30k-token audio-KV (TTS-31,32,87,99).
- **Sample-rate→transport:** 24k/44.1k/48k → 8k G.711 telephony + 48k Opus HD, fractional resample, per-stream rubato, 20 ms RTP repacketize (TTS-11,12,33,34,92).
- **Precision/numerics:** per-component mixed, fp32 codec island under quantized backbone, fp8-DC-vs-bf16-edge, KV-quant concurrency, format×substrate (int8∉ORT-CUDA), MOS-inclusive gate, NaN-reject-frame (TTS-6,35,72,73,74,75,76,96,103,120).
- **Batching mechanics:** CFG-doubles-batch, NFE distillation, dynamic-frame-rate FlexiCodec, sampler-in-CUDA-graph (temp=0/tiny-temp/multinomial/gumbel-argmax), cohort-by-frame-rate, masked-not-absent, per-stage codec-bs=1, tile quantization (TTS-19,23,24,37,41,42,43,46,47,48,80,81,105).
- **Concurrency/scheduling:** lockstep 64-stream, admission reject-don't-glitch, prefill firewall (token vs latency vs spatial-P/D), graceful degradation, duty ledger, KV migration, watchdog/crash/teardown, slot recycle/leak, barge-in (single + storm), no-preemption (TTS-36,49,50,53,57,58,59,60,61,62,63,68,77,78,85,88,90,100,124).
- **Failure/recovery/EXTREME:** never-stops/repetition-loop, mid-batch NaN isolation, runaway-slot fairness, frame-budget overrun policy, crash blast-radius, mode promotion, edge↔DC config-scaling, scoped spec-decode ban, VoxServe binary-viability, marker-on-disconnect, and the M2 seam migration (TTS-94,95,96,106,107,109,110,117,118,127,128,129,130).

Every scenario maps to a concrete mechanism in INFER_ENGINE.md (§1–§10) or a mined failure mode in the production catalog (A–I), grounded in the actual onboarded model families, codecs (Mimi/DAC/SNAC/EnCodec), and the GB10/H200/B200/MI300X/RTX/CPU/NPU substrate matrix — no padding, no speculation.
