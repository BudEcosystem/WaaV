# WaaV Infer — Real-World Scenario Catalog · Family 01: Speech-to-Text (STT)

> Scope: the STT pipeline only — streaming/batch transcription, partial/final finality, long-audio windowing, endpointing/VAD, the encoder-vs-decoder split profiles, cache-aware streaming-encoder state, sample-rate normalization, multilingual/code-switch, noisy/far-field, multi-speaker, word timestamps, biasing, hallucination guards, quant divergence, barge-in/abort, reconnect/marker-flush, and concurrent multi-stream batching.
>
> Grounded in the actually-onboarded STT families: **whisper** (enc-dec AED, AR decoder), **moonshine** (raw-audio enc-dec AED), **parakeet** (NeMo FastConformer TDT/RNN-T, one path reads joint width for duration heads), **nemo_ctc** (FastConformer-CTC, frame-sync), **sensevoice** (CTC + LFR/CMVN), **canary** (NeMo AED, ASR+translate), **cohere** (FastConformer + Cohere transformer decoder, merged-KV), **funasr_nano** (SenseVoice encoder + Qwen3-0.6B LM decoder, caller-managed per-layer KV), **voxtral_realtime** (causal audio encoder + Mistral LM, 1:1 audio↔text lockstep streaming), **qwen3_asr** (audio encoder + Qwen3 LM decoder), **nemotron** (cache-aware FastConformer-RNNT, channel/time caches — the 560-streams/H100 contract). Hardware spans GB10 unified, H200/B200/MI300X/RTX, CPU AMX/NEON, Hexagon-HMX/ANE/TPU, Gaudi-HPU.
>
> Axis legend: `arch:` model family · `hw:` substrate · `mem:` memory/KV · `batch:` batching method · `slo:` latency/finality budget · `lang:` language · `seqlen:` context length · `worker:` placement/process · `scale:` concurrency · `fail:` failure mode · `priority:` realtime vs batch · `feat:` STT feature.

---

## SIMPLE

### STT-1 — Single short clip, batch transcription
- Level: Simple
- Pipeline: STT (batch, whole-utterance)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:en, seqlen:short, worker:inline, scale:1, feat:transcribe
- Scenario: A 4-second 16 kHz English WAV is POSTed to the OpenAI-compatible `/v1/audio/transcriptions` endpoint; one whisper-tiny.en model is loaded, no other streams.
- System must: Run Inline mode (§8) — encoder forward, then AR decode to EOS on the calling thread, return the final string; no scheduler, queue, tick loop, or ledger spun up.
- If mishandled: Spinning DC machinery (ledger/admission/lockstep) for N=1 adds latency and contradicts the edge-never-pays-for-DC-machinery contract.

### STT-2 — Frame-synchronous CTC greedy decode, one stream
- Level: Simple
- Pipeline: STT (frame-sync, CTC)
- Axes: arch:nemo_ctc, hw:GB10, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:short, worker:gpu, scale:1, feat:greedy-ctc
- Scenario: A parakeet-ctc / SenseVoice stream transcribes a single sentence; the encoder is compute-bound, the CTC head emits one label-or-blank per frame, collapsed greedily.
- System must: Run the FastConformer encoder as a micro-batch stage (compute-bound, §3.2), then collapse blanks/repeats in host code; no KV cache exists for CTC, so no ring/paged allocation.
- If mishandled: Treating CTC like an AR ring-KV path wastes a KV arena and mis-sizes the slot; not collapsing blanks yields a frame-rate-length garbage transcript.

### STT-3 — 8 kHz telephony input resampled to 16 k
- Level: Simple
- Pipeline: STT (ingress resample)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:en, seqlen:short, worker:cpu, feat:resample-8k
- Scenario: A G.711 µ-law PSTN call arrives at 8 kHz narrowband; the STT model expects 16 kHz mel input.
- System must: Apply the ingress `any → 16 k` resample contract (§5.1) via persistent per-stream rubato before the mel frontend; upsampling 8→16 k needs no anti-alias but must reconstruct band-limited samples.
- If mishandled: Feeding 8 kHz samples as if 16 k halves the effective frame rate, shifting all mel bins → systematic WER collapse (telephony sounds "chipmunked" to the encoder).

### STT-4 — 44.1 kHz HD capture downsampled to 16 k
- Level: Simple
- Pipeline: STT (ingress resample, anti-alias)
- Axes: arch:moonshine, hw:CPU, mem:none, batch:inline, slo:batch, lang:en, seqlen:short, worker:cpu, feat:resample-downsample
- Scenario: A browser MediaRecorder uploads 44.1 kHz audio; the model frontend is 16 kHz.
- System must: Downsample 44.1→16 k with an anti-alias low-pass (§5.1) before framing; for the moonshine raw-audio frontend, resample the waveform itself (no mel) at the correct rate.
- If mishandled: Decimating without anti-aliasing folds >8 kHz energy back as in-band aliasing artifacts → spurious phonemes and hallucinated words on sibilants/music.

### STT-5 — Language explicitly specified
- Level: Simple
- Pipeline: STT (forced language)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:multi, seqlen:short, feat:set-language
- Scenario: The caller passes `language=de`; whisper-large-v3 multilingual is loaded.
- System must: Honor `set_language` to force the decoder's language token, skipping the language-detect pass; the supported-languages list gates the request (typed error if `de` unsupported).
- If mishandled: Ignoring the forced language runs auto-detect on a short clip → mis-detect to a near-language, then decode in the wrong language entirely.

### STT-6 — Empty / silence-only input
- Level: Simple
- Pipeline: STT (degenerate input)
- Axes: arch:nemo_ctc, hw:GB10, mem:none, batch:inline, slo:batch, lang:en, seqlen:short, fail:empty, feat:endpoint
- Scenario: A 2-second clip of pure silence (or a zero-length payload after VAD trims everything) is submitted.
- System must: Return an empty (not null, not error) transcript with `is_final=true`; CTC collapses to all-blank → empty string; the contract distinguishes "no speech" from "failure".
- If mishandled: Emitting a hallucinated phrase on silence (the classic whisper "thank you" / "subtitles by..." artifact) or throwing an error breaks downstream turn logic.

### STT-7 — Very short utterance ("yes", "no", "okay")
- Level: Simple
- Pipeline: STT (sub-second utterance)
- Axes: arch:moonshine, hw:CPU, mem:none, batch:inline, slo:realtime, lang:en, seqlen:micro, feat:short-utterance
- Scenario: A 350 ms one-word confirmation in an IVR flow; latency to final is the whole UX.
- System must: Decode the short clip without padding it to a 30 s window (moonshine's variable-length raw-audio frontend is built for this); emit final quickly.
- If mishandled: Padding a 350 ms clip to whisper's fixed 30 s mel window pays ~85× wasted encoder compute and adds latency to a latency-critical confirmation.

### STT-8 — Word-level timestamps requested
- Level: Simple
- Pipeline: STT (alignment)
- Axes: arch:parakeet, hw:GB10, mem:none, batch:micro_batch, slo:batch, lang:en, seqlen:short, feat:word-timestamps
- Scenario: A captioning job needs per-word `start_ms`/`end_ms`; a parakeet TDT model is loaded.
- System must: Populate `WordTiming{word,start_ms,end_ms,confidence}` from the transducer's frame indices (TDT duration heads give token durations directly); emit on the `words[]` field (FR-D2).
- If mishandled: Returning text-only when timestamps were requested silently drops a contracted feature; faking uniform timestamps misaligns captions.

### STT-9 — Model declares its (sample_rate, frame_rate) constants
- Level: Simple
- Pipeline: STT (clock derivation)
- Axes: arch:nemo_ctc, hw:GB10, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:short, feat:frame-rate
- Scenario: A FastConformer model declares SR=16 k, FR≈12.5 Hz (80 ms encoder hop); the engine must derive its step budget and cohort key.
- System must: Read the two intrinsic constants and derive `T_f=1000/FR`, `samples_per_frame=SR/FR`, and the cohort key (§5.1) — none of these are hand-tuned per model.
- If mishandled: Hardcoding a frame budget that doesn't match the model's hop makes admission/SLO math wrong, either over-admitting (glitches) or under-admitting (wasted capacity).

### STT-10 — Confidence score on a clean transcript
- Level: Simple
- Pipeline: STT (confidence)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:en, seqlen:short, feat:confidence
- Scenario: A downstream agent gates on transcript confidence to decide whether to ask the user to repeat.
- System must: Populate `Transcript.confidence` from the decoder's token probabilities (default 1.0 only when the model exposes nothing); keep it on the typed result, not a side channel.
- If mishandled: Always returning confidence=1.0 makes the gate useless; the agent never re-prompts on a genuinely uncertain transcript.

---

## INTERMEDIATE

### STT-11 — Streaming partials then a single final (3-level finality)
- Level: Intermediate
- Pipeline: STT (streaming, partial→final)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, feat:finality-3level
- Scenario: A live mic stream feeds voxtral_realtime (1:1 audio↔text lockstep); the UI shows interim text that updates, then locks a final segment at the turn boundary.
- System must: Emit `is_final=false` partials as the decoder advances, promote to `is_final=true` (immutable text) at a segment, and set `is_speech_final` at the utterance/turn boundary (the protocol's three levels).
- If mishandled: Collapsing the three levels to one boolean forces the gateway adapter to guess turn boundaries → premature turn-end or never-ending interim text (the TS-SDK empty-transcript class of bug).

### STT-12 — Cache-aware streaming encoder, deltas-only state
- Level: Intermediate
- Pipeline: STT (streaming encoder state)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:many, feat:cache-aware
- Scenario: Nemotron-3.5 cache-aware FastConformer-RNNT streams a long call; each chunk feeds only the new audio plus the carried channel/time caches (not a re-encode of history) — the 560-streams/H100 contract (L11).
- System must: Persist the per-stream encoder channel/time cache across chunks, feed deltas only, and size the cache bound from `genai_config.json` (chunk size / pre-encode cache) so other chunk-size exports load with no code change.
- If mishandled: Re-encoding the whole history every chunk turns a 3× streaming-throughput model into a quadratic re-prefill, forfeiting the headline 560-stream concurrency.

### STT-13 — Long audio >30 s windowed (whisper 30 s mel cap)
- Level: Intermediate
- Pipeline: STT (long-audio windowing)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:en, seqlen:long, feat:windowing
- Scenario: A 4-minute voicemail exceeds whisper's fixed 30 s mel receptive field.
- System must: Window into ≤30 s chunks with overlap, decode each, and stitch on word/segment boundaries (the torch-sidecar `transcribe` already windows >30 s internally; the Path-A host loop must do the same), carrying language across windows.
- If mishandled: Truncating at 30 s drops 87% of the voicemail; naive non-overlapping windows clip words at every boundary.

### STT-14 — Hour-long lecture, bounded memory
- Level: Intermediate
- Pipeline: STT (long-form, memory bound)
- Axes: arch:parakeet, hw:H200, mem:none, batch:micro_batch, slo:batch, lang:en, seqlen:hours, feat:long-form
- Scenario: A 90-minute lecture is batch-transcribed; a CTC/transducer encoder has no KV but the host must not buffer the whole audio + all logits in RAM.
- System must: Stream the audio through the encoder in fixed windows, emit segments incrementally, and free per-window logits after greedy collapse — memory stays O(window), not O(lecture).
- If mishandled: Materializing 90 min of mel + frame logits at once OOMs the box; the "bounded context" assumption (§4.3) silently fails for long-form.

### STT-15 — Endpointing: silence triggers a final + turn boundary
- Level: Intermediate
- Pipeline: STT (VAD/endpoint)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, feat:endpoint-vad
- Scenario: A speaker pauses ~700 ms; the engine must finalize the current utterance and signal end-of-turn so the downstream LLM can respond.
- System must: Detect the silence via a semantic-VAD / end-of-turn head (a generic per-step linear head, §9.7) or energy endpointer, set `is_speech_final=true`, and flush the delayed pipeline before declaring the turn done.
- If mishandled: Firing turn-end on the first silence frame cuts the speaker mid-thought; never firing makes the bot wait for the WS to close → dead air.

### STT-16 — Auto language detection on unknown input
- Level: Intermediate
- Pipeline: STT (language detect)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:auto, seqlen:short, feat:lang-detect
- Scenario: A multilingual call center gets audio with no language hint; whisper must detect from the first seconds.
- System must: Run the language-detect pass on an initial window, set `Transcript.language`, then decode forced to that language; cache the detection for the rest of the stream (don't re-detect every chunk).
- If mishandled: Re-detecting per chunk flip-flops language mid-utterance; detecting on too short a window mis-IDs accented English as a different language.

### STT-17 — Multilingual transcription with the 8k-vocab parakeet v3
- Level: Intermediate
- Pipeline: STT (multilingual transducer)
- Axes: arch:parakeet, hw:GB10, mem:none, batch:micro_batch, slo:realtime, lang:multi, seqlen:short, feat:multilingual
- Scenario: parakeet-tdt-v3 with an 8k multilingual BPE vocab transcribes Spanish; v2 (English vocab) is also deployable.
- System must: Read vocab size / blank id from `vocab.txt` so v2↔v3 and TDT↔RNN-T load with no code change; the one decode path treats only trailing joint logits as TDT duration heads (a pure RNN-T joint has none).
- If mishandled: Hardcoding the English vocab or assuming TDT duration heads on an RNN-T joint mis-indexes labels → wrong tokens or an index panic.

### STT-18 — Code-switching mid-sentence (Hinglish)
- Level: Intermediate
- Pipeline: STT (code-switch)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:codeswitch, seqlen:short, feat:code-switch
- Scenario: A speaker switches English↔Hindi within one sentence; a multilingual AED model is loaded.
- System must: Keep the decoder in a single multilingual mode (not lock to one language token) so it can emit tokens from both scripts; `language` on the result reflects the dominant or per-segment language.
- If mishandled: Forcing a single language token at utterance start truncates or romanizes the other-language span; flip-flopping detect corrupts both.

### STT-19 — Noisy far-field with low SNR
- Level: Intermediate
- Pipeline: STT (noise robustness)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:low-snr, feat:far-field
- Scenario: A smart-speaker mic 3 m away in a noisy kitchen; reverb + appliance hum drop SNR well below clean-speech assumptions.
- System must: Run the model at its trained precision (don't quantize the encoder front-end harder for noisy audio — quant noise compounds with acoustic noise, §5.2); pass the audio through unaltered (the model is the robustness, not a hand-rolled denoiser).
- If mishandled: Aggressive int8 on the encoder under low SNR crosses the accuracy cliff exactly when the audio is hardest → WER spikes precisely in the deployment that needs robustness most.

### STT-20 — Reverberant room, repeated-token hallucination guard
- Level: Intermediate
- Pipeline: STT (hallucination guard)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:en, seqlen:medium, fail:hallucination, feat:repeat-guard
- Scenario: Long reverb tails make whisper's AR decoder loop ("the the the the…" / a repeated phrase) on a low-information segment.
- System must: Apply a repeat-token / no-repeat-ngram guard and a max-segment-length cap in the AR loop; on detecting a degenerate loop, truncate the segment and emit what's confident.
- If mishandled: Without the guard the decoder emits hundreds of repeated tokens, blowing the segment length and latency, and pollutes the transcript with a stutter that was never spoken.

### STT-21 — Domain-term biasing (product names, drug names)
- Level: Intermediate
- Pipeline: STT (biasing)
- Axes: arch:parakeet, hw:H200, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:short, feat:biasing
- Scenario: A medical dictation app must transcribe drug names; a generic model mis-spells them.
- System must: Apply word/phrase biasing — for transducers, a shallow-fusion or token-bias list keyed at decode; for AED, a prompt/initial-tokens bias — pinned per-stream and reset on slot recycle.
- If mishandled: Leaking one tenant's bias list into another's recycled slot contaminates transcripts (privacy + correctness); no biasing path means systematic mis-transcription of the exact terms that matter.

### STT-22 — Word timestamps from an AED model (no native durations)
- Level: Intermediate
- Pipeline: STT (forced alignment)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:inline, slo:batch, lang:en, seqlen:medium, feat:alignment
- Scenario: whisper (AED) is asked for word timestamps but, unlike a TDT, has no per-token duration head — only cross-attention.
- System must: Derive timestamps from cross-attention alignment (DTW over attention weights) or the model's timestamp tokens; mark confidence lower than a transducer's exact frame timings.
- If mishandled: Claiming exact timestamps from AED cross-attention without DTW gives jittery, sometimes-monotonicity-violating times that break caption sync.

### STT-23 — int8 quantized STT, accuracy gate at load
- Level: Intermediate
- Pipeline: STT (quant accuracy gate)
- Axes: arch:voxtral, hw:CPU, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:quant, feat:quant-gate
- Scenario: An int8 voxtral_realtime variant is selected for a CPU edge box (validated int8-on-CPU); the engine must prove parity before serving.
- System must: Run the load-time `stt_eval` gate vs `reference_precision` on fixtures, persist a `verified{substrate,precision,metric}` stamp, and refuse/fall back on failure (§5.2) — production load is a cheap stamp-check.
- If mishandled: Serving an unverified int8 variant that diverges on hard audio (the funasr int8-decode-divergence lesson) ships silent WER regressions with no signal.

### STT-24 — int8 STT mistakenly routed to ORT-CUDA EP
- Level: Intermediate
- Pipeline: STT (precision×substrate guard)
- Axes: arch:funasr_nano, hw:GB10, mem:ring, batch:micro_batch, slo:realtime, lang:en, seqlen:short, fail:ep-mismatch, feat:precision-substrate
- Scenario: A funasr_nano int8 export (sherpa-onnx) is loaded on GB10 with the CUDA EP active; ORT-CUDA cannot run int8 GEMM.
- System must: Resolve precision per-substrate (`by_substrate[ep]`, §5.2) — on ORT-CUDA fall back to the fp16/bf16 variant (or TensorRT-EP / torch sidecar for true int8), never let `MatMulInteger` silently partition to the CPU EP.
- If mishandled: ORT-CUDA silently offloads int8 GEMM to CPU → the measured 12 ms→232 ms collapse; the "memory win" becomes a 19× latency loss that breaks the frame budget.

### STT-25 — Concurrent multi-stream CTC batching, different lengths
- Level: Intermediate
- Pipeline: STT (concurrent batch, ragged)
- Axes: arch:nemo_ctc, hw:H200, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:mixed, scale:many, feat:length-bucket
- Scenario: 12 live streams hit the encoder micro-batch stage simultaneously with chunks of different lengths.
- System must: Coalesce into the micro-batch within a ~2 ms deadline, bucket by length, run one graph per bucket (§3.2); streaming and non-streaming never mix in one batch.
- If mishandled: Padding all to the longest wastes compute (13%@BS1→40%@BS32 padding, L8); mixing a 30 s batch job into the live micro-batch head-of-line-blocks the realtime streams.

### STT-26 — Lockstep STT decode batched across sessions
- Level: Intermediate
- Pipeline: STT (lockstep AR decode)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:16, feat:lockstep
- Scenario: 16 voxtral_realtime calls run concurrently; each emits one frame per fixed tick at the same frame rate.
- System must: Drive the fixed-slot masked lockstep batcher — gather→step→scatter over a rectangular batch with a per-stream exec-mask, per-slot ring KV, wall-clock paced (§4.2); same model ⟹ same frame rate ⟹ freely co-batchable.
- If mishandled: Running 16 independent decode loops serializes on the single model mutex (today's `Arc<Mutex>`), serving one stream at a time and missing every other stream's frame budget.

### STT-27 — Masked idle slot must use a substituted valid token
- Level: Intermediate
- Pipeline: STT (lockstep correctness)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:masked-slot, feat:exec-mask
- Scenario: In a 16-slot lockstep batch only 9 streams are active; 7 slots are idle but stay in the rectangular batch (MASKED ≠ ABSENT, F1).
- System must: Before the forward, force masked/warming rows to the BOS/`initial` token via `where(is_init, initial, gathered)`; the dense kernel reads every row, so a sentinel/stale token in an idle row must be harmless.
- If mishandled: An idle row's KV-gather reads sentinel -2 / stale → CUDA illegal-memory or NaN that kills the whole batch (all 16 streams), not just the idle one.

### STT-28 — Mixed-precision STT: encoder int8, head/decoder high-precision
- Level: Intermediate
- Pipeline: STT (per-component precision)
- Axes: arch:funasr_nano, hw:CPU, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:quant-noise, feat:mixed-precision
- Scenario: funasr_nano (SenseVoice encoder + Qwen3-0.6B LM decoder) is quantized; the big LM/encoder GEMMs tolerate int8 but norms/RoPE/sampling must not.
- System must: Apply per-component precision (`component_precision`, §5.2) with per-architecture defaults keeping norms/RoPE/sampling-head high-precision; quant noise compounds across decode steps.
- If mishandled: int8 RMSNorm/RoPE accumulates drift over the AR decode → text degrades on longer utterances even though offline WER on short clips looked fine.

### STT-29 — Reconnect mid-stream with marker flush
- Level: Intermediate
- Pipeline: STT (reconnect, marker/flush)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:reconnect, feat:marker-flush
- Scenario: A WS drops at second 40 of a streaming call; the client reconnects and the engine must not lose the in-flight tail still in the model's delay pipeline.
- System must: On graceful end-of-input, fire the future-step marker at `now + asr_delay + buffered_frames` via a step-ordered heap (F5) and terminate on the marker, not input-exhaustion; on reconnect, resume the stream's encoder cache state rather than cold-restart.
- If mishandled: Echoing "done" before the delayed model emits the last words truncates the final words of the segment; cold-restarting loses the carried cache and re-transcribes (duplicate) audio.

### STT-30 — Barge-in aborts an in-flight transcription
- Level: Intermediate
- Pipeline: STT (barge-in/abort)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:abort, feat:barge-in
- Scenario: The user starts speaking while a long final is still being decoded (or while the bot is mid-response); the session must abort the current decode and free the slot.
- System must: Treat barge-in as a control message that jumps the queue and frees the stream's slot/KV/window within ≤1 tick (§6); cancelled must be distinguishable from completed (an explicit terminal frame, not silence, G2).
- If mishandled: A best-effort/fire-and-forget abort that drops the message (G9) leaves the slot decoding stale audio; an inferred-from-silence cancel can't tell abort from a finished turn.

### STT-31 — One-shot POST over a streaming-only core (silence flush)
- Level: Intermediate
- Pipeline: STT (batch-over-streaming)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:batch, lang:en, seqlen:medium, feat:pad-flush
- Scenario: A non-streaming `/transcriptions` POST is served by a streaming-core model (nemotron) that has an internal delay pipeline.
- System must: Append the real audio + a marker + trailing silence (≈10 s zeros) to flush the delay, and terminate on the marker (F5); a single slot in the lockstep table services the whole clip.
- If mishandled: Terminating on input-exhaustion (no flush) drops the last delayed words of every POSTed clip — a systematic tail-truncation on the batch endpoint.

### STT-32 — Transcript text immutability across partials
- Level: Intermediate
- Pipeline: STT (delta/finality correctness)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, feat:immutable-final
- Scenario: A UI appends finalized segments and replaces only the trailing interim; a re-emitted "final" that changes earlier text corrupts the display.
- System must: Guarantee `is_final=true` text is immutable (never revised) while `is_final=false` interim text may be replaced; the egress contract makes offline-concat == stream-concat for the final segments (I1, C1).
- If mishandled: Revising already-final text forces the UI to re-render history (flicker) and breaks any consumer that committed the final downstream (e.g. sent it to an LLM).

### STT-33 — Speaker-attributed transcription (diarization-adjacent)
- Level: Intermediate
- Pipeline: STT (+ speaker tags)
- Axes: arch:parakeet, hw:H200, mem:none, batch:micro_batch, slo:batch, lang:en, seqlen:long, feat:speaker-attr
- Scenario: A two-party meeting recording needs "Speaker A/B" labels alongside the words.
- System must: Run STT for the words and timestamps and attach speaker turns from a separate diarization stage (a different pipeline node) keyed by the same timeline; STT itself stays speaker-agnostic.
- If mishandled: Bolting speaker logic inside the STT decode conflates two models' concerns; misaligned timelines tag words to the wrong speaker.

### STT-34 — Word confidence per-token for selective re-ask
- Level: Intermediate
- Pipeline: STT (per-word confidence)
- Axes: arch:parakeet, hw:GB10, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:short, feat:word-confidence
- Scenario: A form-filling bot wants to re-ask only the low-confidence words (an account number), not the whole utterance.
- System must: Populate per-`WordTiming.confidence` from the transducer's per-token logprob; expose it so the agent can target the uncertain span.
- If mishandled: Only a single utterance-level confidence forces an all-or-nothing re-ask, degrading UX on otherwise-correct transcripts.

### STT-35 — Streaming encoder on NPU, AR decoder on GPU (split placement)
- Level: Intermediate
- Pipeline: STT (heterogeneous split)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:hetero, feat:placement
- Scenario: On GB10/UMA the whisper conv-encoder maps perfectly to the NPU/idle-SMs while the AR decoder needs the GPU (the recipe Qualcomm/Apple ship, §2.3).
- System must: Place the static conv-encoder on the NPU and the dynamic AR decoder on the GPU; hand off the encoder output as a zero-copy `SharedHostBufType` view on coherent memory (§3.4).
- If mishandled: Pinning AR decode to the NPU breaks the static-shape contract (variable per-token shape, growing KV) → either a re-capture storm or a fallback that's slower than the GPU.

---

## COMPOUND

### STT-36 — Idle-then-resume slot, byte-identical state
- Level: Compound
- Pipeline: STT (lockstep, idle/resume)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:many, fail:idle-resume, feat:gated-mutation
- Scenario: In a multi-tenant lockstep batch a stream goes idle (VAD silence) for 2 s, then resumes; meanwhile other slots advanced many frames.
- System must: Gate every per-slot mutation through `where(exec_mask, new, old)` — offset, KV scatter, KV end-offset, conv ring `previous`, `first`-frame flag, sampler RNG offset, partial-word buffers (F2) — so an idle slot's state is frozen, not advanced.
- If mishandled: One ungated mutation (e.g. an offset that ticks while masked) causes a RoPE phase jump / poisoned ring cell on resume → silent corruption invisible in single-stream tests, only appearing under multi-tenant idle-then-resume.

### STT-37 — Slot recycling after disconnect (cross-user privacy)
- Level: Compound
- Pipeline: STT (slot recycle)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:many, fail:cross-tenant, priority:realtime, feat:reset-slot
- Scenario: User in slot 7 disconnects; a new user is admitted into slot 7 immediately; the new user's attention must not see the old user's KV or word buffer.
- System must: Run one transactional `reset_slot(7)` fanning out to KV pointers + conv rings + sampler + word buffers + offset + host item-state (F3); a monotonic `channel_id` drops any late output/marker whose id ≠ the live occupant; correctness relies on positions/indices=0 + mask making stale bytes unreachable.
- If mishandled: Skipping the reset leaks the previous user's transcript into the new user's output — a privacy disaster that only manifests under churned multi-tenant load.

### STT-38 — Ring-KV wraparound on a long utterance (logical-position mask)
- Level: Compound
- Pipeline: STT (ring-KV wraparound)
- Axes: arch:funasr_nano, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:long, fail:wraparound, feat:logical-pos
- Scenario: A long monologue pushes a stream's ring-KV `offset` past `context`; physical slot order no longer equals time order.
- System must: Reconstruct each cell's logical position and mask by `pos ≤ my_pos` (causal) AND window AND never-written⇒-1 (F4); bake the Kyutai pre-wrap/exact-fill/post-wrap/mixed-mask test vectors as unit tests.
- If mishandled: A naive `j ≤ i` causal mask attends to FUTURE tokens in recycled cells after wraparound → the transcript degrades partway through long utterances with no error.

### STT-39 — Token-AR STT with a 30k-token transcript (ring → paged escape)
- Level: Compound
- Pipeline: STT (long-context, paged escape)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:batch, lang:en, seqlen:30k, fail:lossy-ring, feat:attention-sink
- Scenario: A 10-minute audiobook chapter on the LLM-decoder STT path (qwen3_asr) grows the text transcript past 30k tokens; a fixed ring is silently lossy here (L12).
- System must: For the long-variable-transcript token-AR path, escape to paged KV + pin attention-sink tokens (StreamingLLM) so the early context isn't forgotten/wrapped (§4.3 "reach for paging only here", L12); admit/evict on this path, not lockstep.
- If mishandled: A fixed ring wraps and drops early context → the model loses the start of the chapter (names, terms) and StreamingLLM-style wraparound instability corrupts the tail.

### STT-40 — Prefill firewall: a long audio-prompt encode must not break cadence
- Level: Compound
- Pipeline: STT (prefill firewall)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:many, fail:prefill-spike, feat:chunked-prefill
- Scenario: A new stream joins a 30-slot live batch carrying a long audio context that must be encoded (prefilled) before it can stream; a naive prefill+decode batch inflates per-frame time.
- System must: Admit ≤1 new stream's prefill per K frames and chunk any prefill exceeding one frame-budget's tokens (§4.5), keying the budget on the audio frame deadline; keep chunk token counts power-of-two (257 ~32% slower than 256, tile quantization).
- If mishandled: A prefill spike inflates TBT up to 28.3× → 17–22 dropped frames on the other 29 streams = a total audible dropout for everyone while one stream prefills.

### STT-41 — Prefill firewall budget by predicted latency, not token count
- Level: Compound
- Pipeline: STT (KV-length-aware admission)
- Axes: arch:qwen3_asr, hw:GB10, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:long, fail:budget-mismatch, feat:latency-predictor
- Scenario: On the token-AR STT path the prefill chunk that fits a fixed token budget still overruns because attention cost grows with the already-encoded context (DuetServe Obs.2, L10).
- System must: Switch the firewall control variable to a KV-length-aware predicted-latency budget (attention/context features, not raw token count) so the fused chunk width tracks actual GB10 tile cost.
- If mishandled: A token-budget=8 chunk shows >4× latency variation as context grows → the firewall lets through a chunk that blows the frame budget exactly on long-context streams.

### STT-42 — Concurrent STT across two frame-rate cohorts
- Level: Compound
- Pipeline: STT (cohort batching)
- Axes: arch:mixed, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:many, fail:cohort-mix, feat:cohort-key
- Scenario: One box serves voxtral_realtime streams (one frame rate) and nemotron streams (a different encoder hop / frame rate) at once.
- System must: Batch by `(model, frame_rate)` cohort and never lockstep-mix clocks (§4.2); the two cohorts time-share the GPU via the duty ledger, not within a fused step.
- If mishandled: Forcing two frame-rate clocks into one lockstep tick has no common realtime tick → one cohort starves or both desync, audible on both.

### STT-43 — NaN logit during decode → reject the frame
- Level: Compound
- Pipeline: STT (numerics guard)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:nan-logit, feat:reject-frame
- Scenario: A quantized decoder produces a NaN logit row on a hard/clipped audio frame; an argmax over NaN yields a garbage token (H1).
- System must: Run an always-on `logits.isnan().any()` reduction and reject the frame (repeat-prev / greedy-resample), never argmax a NaN row (the H1 policy inversion); fp32 sampler numerics regardless of model dtype (H5).
- If mishandled: Argmaxing a NaN row emits a garbage codec/text token with zero error signal → a wrong word silently enters the transcript and may poison the AR history.

### STT-44 — CUDA-graph the lockstep step, sampler outside the capture
- Level: Compound
- Pipeline: STT (CUDA-graph + sampling)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:graph-sampler, feat:cuda-graph
- Scenario: To hit the edge latency win the fixed-shape lockstep step is CUDA-graphed, but STT sampling (when non-greedy) needs multinomial, which isn't graph-safe (C2).
- System must: Capture the fixed-shape forward in the graph and sample OUTSIDE the captured region (or use graph-safe gumbel-argmax); wrap graphed callables with a shape+scalar-identity assert (F7) so a shape change is a loud crash, not silent stale-graph corruption.
- If mishandled: Capturing multinomial inside the graph silently breaks sampling or forces eager (losing the 1.21×@B1 edge win); an unguarded graph replays stale shapes → corruption.

### STT-45 — CUDA-graph capture OOMs after /health passes on sm120
- Level: Compound
- Pipeline: STT (capability-driven graph ladder)
- Axes: arch:nemotron, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:capture-oom, feat:eager-fallback
- Scenario: On GB10/sm120 the CUDA-graph pool capture OOMs after readiness passed (the #44209 sm120 crash-loop); capture must not crash the server.
- System must: Use a capability-driven graph ladder with robust eager fallback — `enforce_eager` as a first-class config + automatic downgrade on capture failure (H4, C8); reserve the graph-pool delta and run a pre-capture feasibility check before admitting (fail at boot, not request-1).
- If mishandled: Capture-OOM-after-health is a crash loop that passes readiness then dies on the first real request — the exact sm120 scar; no eager fallback means the box is unusable for that model.

### STT-46 — Concurrent ragged-length batch: 30 s job + sixteen 80 ms live frames
- Level: Compound
- Pipeline: STT (mixed batch/realtime)
- Axes: arch:nemo_ctc, hw:H200, mem:none, batch:micro_batch, slo:mixed, lang:en, seqlen:mixed, scale:many, priority:realtime, fail:hol-block, feat:priority
- Scenario: A batch transcription job (30 s chunk) arrives while 16 realtime streams need their 80 ms-cadence encoder forwards.
- System must: Keep Realtime > Batch priority per stage; piggyback the Batch job into leftover budget (Sarathi-style) or run it in a separate length bucket so it never head-of-line-blocks the live micro-batch (§6).
- If mishandled: Co-batching the 30 s job with live frames stalls all 16 streams for the job's duration → every live call drops frames (the RFC-#2568 codec-window-gap analog for STT encoders).

### STT-47 — Admission rejects rather than glitches at saturation
- Level: Compound
- Pipeline: STT (SLO-aware admission)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:saturate, fail:overload, priority:realtime, feat:reject-dont-glitch
- Scenario: GB10 is at its calibrated lockstep-slot ceiling; a 33rd stream requests admission.
- System must: Test all stages' free slots + KV/workspace reservable + per-substrate duty ≤ S (and shared-bandwidth duty on the unified pool), and REJECT with a typed 429/503 + Retry-After rather than admit-and-degrade (§6, P-4).
- If mishandled: Admitting the 33rd stream pushes step-time over the frame budget for ALL 33 streams → a synchronized glitch across every active call instead of one rejection.

### STT-48 — Graceful overload: relegate to a degraded queue, don't drop frames
- Level: Compound
- Pipeline: STT (graceful degradation)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:overload, fail:load-shed, priority:realtime, feat:deadline-aware
- Scenario: A traffic spike at 50% overload would, with naive reject, violate most deadlines; a deadline-aware scheduler can do better (Niyama, L9).
- System must: Use deadline-aware admission + graceful relegation (degrade lower-priority streams to a relaxed queue, schedule by risk-of-violation à la VoxServe) as PRIMARY; hard-reject only at true saturation; cadence protected by the client playback buffer.
- If mishandled: Naive reject-everything yields <20% deadlines met at overload vs 95%+ with relegation; dropping frames for everyone is the worst outcome (Niyama 8.6% vs 80% violations).

### STT-49 — Sustained p99 breach on the bottleneck stage → shed
- Level: Compound
- Pipeline: STT (drift response)
- Axes: arch:whisper, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:overload, fail:drift, priority:realtime, feat:hysteresis-shed
- Scenario: A thermal/contention drift slowly pushes the encoder (the bottleneck stage) past its per-stage SLO p99 for a sustained window.
- System must: On sustained breach, stop admitting → shed Batch → only then shed the newest Realtime ≤1/tick with 60 s hysteresis (FR-S3b); shed is the backstop, admission is the primary mechanism (§6).
- If mishandled: No drift response means the breach compounds until every stream glitches; shedding too aggressively (no hysteresis) flaps streams on transient spikes.

### STT-50 — Zero D2H syncs in the per-frame decode loop (sidecar)
- Level: Compound
- Pipeline: STT (sidecar hot-loop discipline)
- Axes: arch:qwen3_asr, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:sidecar, fail:d2h-sync, feat:sync-free
- Scenario: The Path-B torch sidecar runs an LLM-decoder STT per-frame loop; a naive `.item()`/`.cpu()`/`.tolist()` per step is a GPU→CPU sync (I3).
- System must: Keep the per-step loop sync-free — `dst.copy_(src)` not `dst.fill_(src.item())`, `torch.where`/masking not Python branches; a profiler/CUDA-event guard asserts zero D2H syncs during decode (C5).
- If mishandled: "10 steps × 60 frames × 4 ops = 2400 syncs/request" → latency collapse; the clean 9 ms/step assumption evaporates and the stream misses cadence.

### STT-51 — Sidecar per-slot state crosstalk under concurrency
- Level: Compound
- Pipeline: STT (sidecar state isolation)
- Axes: arch:funasr_nano, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, worker:sidecar, scale:many, fail:crosstalk, feat:slot-keyed-state
- Scenario: The torch sidecar holds Python streaming-encoder / sliding-window state; under 4+ concurrent streams it must not share buffers across slots (I5, C3).
- System must: Key all sidecar Python state by slot-id (`self._state[slot]`) and free it on slot-reset; a concurrent-crosstalk test gates it (the lockstep multi-session step verb carries slot-id, §D).
- If mishandled: A shared sliding-window buffer corrupts audio across concurrent streams → crosstalk/truncation that only appears under load, invisible single-stream.

### STT-52 — Dead sidecar → failed requests, not a hang
- Level: Compound
- Pipeline: STT (sidecar crash detection)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, worker:sidecar, fail:sidecar-death, feat:dead-flag
- Scenario: The torch sidecar process dies mid-stream (CUDA fault / OOM-kill); the parent must not answer /readyz 200 while throughput is zero (H6/G7).
- System must: Use the 3-layer detection — sentinel-byte (`ENGINE_CORE_DEAD`), out-of-band waitpid/pidfd watcher, passive sentinel re-poll — driving one `dead` flag that both rejects new admissions and fans a failure into every live WS send within ~1 s (H6); `PR_SET_PDEATHSIG` so the orphaned GPU child can't pin VRAM (H7).
- If mishandled: A blind health check keeps reporting OK while every session hangs forever; an orphaned sidecar pins VRAM into the next process.

### STT-53 — Progress watchdog: "alive but zero forward progress"
- Level: Compound
- Pipeline: STT (liveness watchdog)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:stall, feat:watchdog
- Scenario: A decode loop is alive (servicing the queue) but emits no transcript for an active stream (a wedged kernel / deadlock) — passes every health check (H9, #39863).
- System must: Track a monotonic "last-transcript-emitted-at T" per session, checked by an independent thread; no output for >N×frame-interval on an active stream → kill/restart the sidecar; the per-inference deadline is device+model-aware (a CTC step ≠ a 1.5B LLM-decoder step), not a flat 300 s.
- If mishandled: A wedged stream sits forever passing readiness; a flat 300 s deadline either kills a legitimately-slow CPU batch job or lets a wedged realtime stream hang for 5 minutes.

### STT-54 — Disable WS write-coalescing for the 80 ms frame budget
- Level: Compound
- Pipeline: STT (transport jitter)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, worker:transport, fail:jitter, feat:flush-per-frame
- Scenario: Default WS write coalescing adds tens of ms of buffering to an 80 ms-budget streaming route (F10).
- System must: Set `write_buffer_size(0)` (flush per frame) on every streaming STT route; meter per-step wall time vs frame budget and per-stream buffer depth as first-class metrics.
- If mishandled: Default coalescing adds 10s-of-ms jitter to an 80 ms budget → partials arrive bunched and late, the UI stutters, and end-of-turn detection lags.

### STT-55 — Slot leak on disconnect (multi-trigger free)
- Level: Compound
- Pipeline: STT (slot lifecycle)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:many, fail:slot-leak, feat:multi-trigger-free
- Scenario: A client vanishes without a clean close (network drop); the disconnect callback may be missed, leaking the slot.
- System must: Free the slot from INSIDE the step loop on ANY of {receiver closed, sender disconnected, send error, ping-timeout 20 s, idle-timeout 120 s} (F9); ping every 10 s; expose used/total_slots as the autoscale gauge.
- If mishandled: Relying solely on a disconnect callback leaks slots until the fixed table fills → the server rejects all new streams while "serving" zombie slots.

### STT-56 — Backpressure parks the bottleneck stage, not drops audio
- Level: Compound
- Pipeline: STT (inter-stage backpressure)
- Axes: arch:cohere, hw:H200, mem:ring, batch:micro_batch, slo:realtime, lang:en, seqlen:medium, scale:many, fail:backpressure, feat:bounded-queue
- Scenario: In a multi-stage STT DAG (mel preprocessor → encoder → AED decoder) a downstream decoder queue fills under load.
- System must: Use bounded inter-stage queues that PARK the upstream stage on a full downstream (never drop); admission therefore tests the BOTTLENECK stage, not the entry stage (§3.2, §6).
- If mishandled: Dropping on a full queue loses audio frames (glitch); admitting on the entry stage's capacity over-commits the real bottleneck (the AR decoder) and everything backs up.

### STT-57 — STT→translate (Canary AED translation mode)
- Level: Compound
- Pipeline: STT (transcribe→translate, one model)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, feat:translate
- Scenario: Canary is asked to transcribe German speech and emit English text (its multilingual ASR+translation mode).
- System must: Set the decoder's task/target-language tokens for translation; the same AED path serves transcribe and translate by the prompt tokens (one model, switched by conditioning), reset per slot.
- If mishandled: Mixing the transcribe and translate prompt tokens, or leaking the translate target into a recycled transcribe slot, yields the wrong task on the next stream.

### STT-58 — STT feeding a downstream LLM with conditioning-hashed KV
- Level: Compound
- Pipeline: STT (KV key fingerprints conditioning)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:medium, scale:many, fail:kv-contaminate, feat:extra-key
- Scenario: Two concurrent LLM-decoder STT requests have identical text-prompt token ids but different audio conditioning scattered at placeholder positions; a prefix cache that keys on token ids alone would contaminate (G1).
- System must: Make any KV-reuse/prefix key include a content hash of the injected audio conditioning over every channel (`extra_key = blake2b(full audio sequence)`); zero-shot (no special conditioning) → `extra_key=None` so genuine prefix-sharing survives.
- If mishandled: RadixAttention concludes the prefixes match → cross-contaminates KV between different audio inputs = silently wrong transcript, only under concurrency.

### STT-59 — Hybrid KV: radix prefix-cache for system prompt + ring for utterance
- Level: Compound
- Pipeline: STT (hybrid KV)
- Axes: arch:funasr_nano, hw:H200, mem:hybrid, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:many, fail:recompute, feat:prefix-cache
- Scenario: A multi-tenant agent reuses the same long system prompt / biasing context across thousands of STT-LLM requests; a fixed per-slot ring recomputes that prefix every request (L1, ~86% cacheable).
- System must: Use a HYBRID KV — radix/prefix-cache (paged) for the deterministic shared prefix + ring for the per-utterance suffix (L1, the #1 reframing fix); the ring stays the fast path for the live suffix.
- If mishandled: Recomputing the shared prefix every request forfeits ~86% cacheable work (Fish-S2 hit rate) on exactly the top commercial workload (cloned-voice / multi-tenant agent).

### STT-60 — Overlapping speakers, multi-talker confusion
- Level: Compound
- Pipeline: STT (overlap)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:overlap, feat:multi-speaker
- Scenario: Two people talk over each other on a conference bridge; a single-speaker STT model must degrade gracefully.
- System must: Transcribe the dominant speaker and mark reduced confidence on overlap regions (the model isn't multi-talker-separating); a separate source-separation/diarization stage handles attribution if required.
- If mishandled: Emitting interleaved garbage from both speakers as one transcript, at confidence=1.0, misleads the downstream consumer that the text is reliable.

### STT-61 — Quant divergence surfaces only on hard audio (streaming+concurrent gate)
- Level: Compound
- Pipeline: STT (validation pyramid)
- Axes: arch:voxtral, hw:CPU, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, scale:many, fail:quant-divergence, feat:validation-pyramid
- Scenario: An int8 variant passes offline WER on clean fixtures but diverges on accented/noisy audio under concurrent load (I4, the layer-1-only weakness, C4).
- System must: Gate on the full validation pyramid — (1) offline parity + (2) streaming-playback + (3) concurrent-load (4+ parallel) — not offline parity alone; persist the stamp only if all three pass.
- If mishandled: A layer-1-only gate passes a variant that crosstalks or drifts under concurrency ("declaring done without all three has shipped regressions more than once").

### STT-62 — Warm-up gates readiness (first-request cliff)
- Level: Compound
- Pipeline: STT (readiness lifecycle)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:cold-start, feat:warmup-gate
- Scenario: The first streaming request after boot pays seconds of CUDA-graph capture + lazy init if served before warmup (F6, C7).
- System must: Run 2–3 warm-up steps with a full mask + `synchronize()` at startup (fills conv/KV boundary state, captures graphs off the hot path) and gate /readyz on warmup+calibration complete, NOT process-up.
- If mishandled: /readyz returns 200 on process-up → the load balancer routes the first real call into a multi-second capture stall (the first-request cliff).

### STT-63 — Calibration measured without the profiler
- Level: Compound
- Pipeline: STT (calibration discipline)
- Axes: arch:parakeet, hw:GB10, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:short, fail:calib-distortion, feat:calibration
- Scenario: The duty-ledger calibration measures `T_step(B_active)` per stage to set admission limits; running it under a profiler distorts the numbers (Section B measurement discipline).
- System must: Measure calibration WITHOUT the torch profiler (it distorts latency), exclude the first request (warmup), A/B one variable at a time, and persist keyed by `sha256 × device × driver × warm-set`.
- If mishandled: Profiler-distorted calibration sets the admission ceiling wrong → either over-admits (glitches in production) or under-admits (wasted capacity), and the numbers don't reproduce.

### STT-64 — Mid-utterance, never preempt (whole-stream admission fit)
- Level: Compound
- Pipeline: STT (non-preemptible admission)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:saturate, fail:preempt-thrash, priority:realtime, feat:non-preempt
- Scenario: Under load the temptation is to preempt a running stream to admit a higher-priority one (vLLM admit-then-preempt → recompute entire prefill, H2).
- System must: NEVER preempt mid-utterance; gate admission on WHOLE-STREAM fit (not first-chunk), shed by reject-at-admission; the fixed-slot non-preemptible design structurally avoids the preempt-thrash + KV use-after-free bug class.
- If mishandled: Preempting a half-utterance is an audible glitch and triggers recompute-thrash (the victim even loses re-admission priority, #41951) — a self-inflicted overload spiral.

### STT-65 — Far-field + low SNR + int4 quant on the encoder (compounded)
- Level: Compound
- Pipeline: STT (compounded degradation)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:compound-quant, feat:precision-floor
- Scenario: An edge box runs nemotron int4 (baked) on far-field noisy audio; quant noise + acoustic noise + reverb stack.
- System must: Keep the encoder front-end (mel, norms) high-precision even when the GEMMs are int4 (§5.2), and the accuracy gate's fixtures must include hard (noisy/far-field) audio so the int4 stamp reflects the deployment, not just clean speech.
- If mishandled: An int4 stamp earned on clean fixtures crosses the WER cliff on far-field noise → the worst accuracy in the deployment that already has the worst audio.

### STT-66 — Streaming partial stability vs latency tradeoff
- Level: Compound
- Pipeline: STT (partial stability)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, feat:partial-stability
- Scenario: Emitting partials at every frame makes the UI flicker (text revised constantly); emitting only at endpoints adds latency.
- System must: Emit interim hypotheses but stabilize the prefix that won't change (commit a sub-span to `is_final` once the model's lookahead confirms it), balancing the stability/latency tradeoff explicitly.
- If mishandled: Per-frame raw partials flicker badly; over-stabilizing delays the visible text → the realtime UX feels laggy or jittery, both bad.

### STT-67 — Acoustic-delay off-by-one in the streaming pipeline
- Level: Compound
- Pipeline: STT (acoustic-delay alignment)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:delay-offbyone, feat:delay-ring
- Scenario: The streaming encoder/decoder has an intrinsic look-ahead/acoustic delay; the marker and timestamp math must align reads/writes by exactly that delay (F8).
- System must: Size the delay handling so max-delay write and oldest read don't collide (the +2 guard), and before `step < acoustic_delay` force the warm-up window appropriately; the marker fires at `now + asr_delay + buffered_frames`.
- If mishandled: An off-by-one in the delay alignment mis-times word timestamps and either truncates or duplicates the boundary frame's tokens.

### STT-68 — Power-of-two chunk sizing for prefill/window (tile quantization)
- Level: Compound
- Pipeline: STT (tile-aligned chunking)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:medium, fail:tile-quant, feat:pow2-chunk
- Scenario: A long audio-prompt encode is chunked; 257 tokens runs ~32% slower than 256 due to GPU tile/wave quantization (§4.5, L10 Bullet 19.4% SM-idle).
- System must: Keep prefill/window chunk token counts power-of-two and aligned to the GB10/H200 kernel tile; align the fused chunk+piggybacked-decode width to avoid wave-quantization SM idle.
- If mishandled: A 257-token chunk wastes ~32% of the prefill budget and can tip a borderline stream over its frame deadline.

### STT-69 — Determinism per-stream (atomic-reduction reality)
- Level: Compound
- Pipeline: STT (determinism scope)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:many, fail:nondeterminism, feat:per-stream-determinism
- Scenario: A test/replay harness expects bit-identical transcripts, but GPU atomic reductions (and batch-dependent reduction order) make cross-run bitwise identity impossible (H "other").
- System must: Accept per-stream-only determinism (a stream's output independent of co-tenant batch composition), not global bitwise reproducibility; use float64 Gumbel where sampling determinism matters; the exec-mask must make a stream's result independent of which other slots are active.
- If mishandled: Promising global bitwise determinism is unachievable and hides a real bug — a stream whose transcript changes with batch composition (a masked-slot leak), which IS a correctness failure.

### STT-70 — Reconnect resumes encoder cache without re-transcribing
- Level: Compound
- Pipeline: STT (stateful reconnect)
- Axes: arch:nemotron, hw:GB10, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:reconnect-dup, feat:cache-resume
- Scenario: A flaky mobile network reconnects a streaming STT session; the client replays the last 500 ms it isn't sure landed.
- System must: Resume the per-stream encoder channel/time cache and de-duplicate the overlap (the marker/offset state tracks what's been emitted) so re-sent audio doesn't produce duplicate words.
- If mishandled: A cold-restart re-encodes from zero (losing carried context and re-transcribing) or naively appends the replayed 500 ms → duplicate words at the seam.

---

## EXTREME

### STT-71 — 1000-stream STT fleet on B200, lockstep at big N
- Level: Extreme
- Pipeline: STT (DC-scale lockstep)
- Axes: arch:voxtral, hw:B200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:1000, priority:realtime, feat:dc-scale
- Scenario: A B200 fleet serves ~1000 concurrent realtime STT streams; the same lockstep loop that ran N=1 on GB10 must push toward compute-bound at big N with fp8/mxfp4.
- System must: Scale the fixed-slot lockstep batcher to big N (B200 needs a bigger batch just to fill its SMs, §2.1), tier precision to fp8/mxfp4 for the compute-bound DC regime (§5.2), and CUDA-graph the largest cohort first — only the batch ceiling and precision tier change vs edge (§8).
- If mishandled: Reusing the edge bf16+CUDA-graph-@batch-1 config on B200 leaves the SMs under-occupied (severe at small batch) → a fraction of achievable throughput; CUDA-graph @ high batch is the 0.72× regression.

### STT-72 — Nemotron 560-streams/H100 contract under sustained load
- Level: Extreme
- Pipeline: STT (cache-aware concurrency ceiling)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:560, priority:realtime, fail:concurrency-ceiling, feat:cache-aware-560
- Scenario: A single H100/H200 must hold the headline 560 cache-aware streaming-encoder streams (3× the non-cache baseline, L11) for hours of mixed-length calls.
- System must: Maintain bounded per-stream encoder cache (deltas-only), admit exactly to the calibrated 560 ceiling, and reject the 561st (§6); the cache bound is the concurrency lever, not KV-quant (a CTC/RNNT encoder has small/no decode KV).
- If mishandled: Any per-chunk re-encode or unbounded cache growth collapses 560→a fraction; over-admitting past the calibrated ceiling glitches all 560 at once.

### STT-73 — DC spill/rebalance: migrate a stream between replicas without a glitch
- Level: Extreme
- Pipeline: STT (KV migration)
- Axes: arch:qwen3_asr, hw:B200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:long, scale:fleet, fail:migration-glitch, feat:kv-migration
- Scenario: A hot replica must shed an in-flight long token-AR STT stream to a cooler replica during a fleet rebalance.
- System must: Use constant-time append-only KV migration (Llumnix-style, ~sub-ms-to-5 ms for voice ctx, L16) and mask the migration behind the client playback buffer so no frame is dropped; one decode-step > one frame, so the migration must complete within the buffered slack.
- If mishandled: A migration longer than the buffered slack drops ≥1 frame at the seam (audible/visible); a naive migrate that re-prefills on the target stalls the stream entirely.

### STT-74 — AR-outer + generative-inner-head STT-adjacent arch (variable stride)
- Level: Extreme
- Pipeline: STT (third execution class)
- Axes: arch:funasr_nano, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:variable-stride, feat:nested-batcher
- Scenario: A next-gen LLM-decoder ASR advances by a patch (not a frame) with an inner ODE/consistency head at a per-stream runtime NFE (DiTAR/FlashTTS-class generalization to STT, L5) — breaking both WaaV batchers in one model.
- System must: Generalize lockstep to "advance a model-dependent VARIABLE STRIDE" and compose two batchers per step — the outer AR lockstep fans each tick's hidden states into the inner variable-NFE micro-batch INSIDE one step (the nested-batcher composes, doesn't pick).
- If mishandled: Treating it as plain per-frame lockstep mis-paces the stride; treating the inner head as a separate stage balloons per-step latency (the SGLang Talker+MTP lesson) — either way cadence breaks.

### STT-75 — Dynamic frame-rate codec front-end (rate unknown a-priori)
- Level: Extreme
- Pipeline: STT (variable frame-rate cohort)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, scale:many, fail:dynamic-fr, feat:variable-cohort
- Scenario: A FlexiCodec-style front-end emits 3–12.5 Hz data-dependent per-utterance AND per-frame (L6); the cohort key can't assume a fixed declared frame rate.
- System must: Make lockstep "advance variable stride" and let the cohort key tolerate unknown-a-priori rates (re-cohort as the rate shifts), rather than the static `(model, frame_rate)` assumption (§4.2/§5.1 insufficient).
- If mishandled: A fixed-rate cohort assumption desyncs the moment the codec changes rate mid-utterance → the stream falls out of its lockstep tick and glitches.

### STT-76 — Many-turn agent: 10-minute context, generic LLM-KV methods fail
- Level: Extreme
- Pipeline: STT (long-context audio KV)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:30k, fail:long-ctx-forget, feat:attention-sink
- Scenario: A 10-minute many-turn voice-agent transcript (30k+ tokens) on the LLM-decoder path; generic LLM-KV compression methods FAIL on audio (AudioKV, L12).
- System must: Pin attention-sink tokens + a paged/full-context escape hatch (StreamingLLM + paging) and use audio-aware KV handling (not a generic text-KV evictor) so the early turns aren't silently forgotten.
- If mishandled: A generic text-KV evictor or a plain sliding window forgets early turns and destabilizes the tail (StreamingLLM wraparound) → the agent loses earlier conversation context with no error.

### STT-77 — Intra-node spatial P/D vs chunked-prefill firewall (A/B on GB10)
- Level: Extreme
- Pipeline: STT (spatial prefill/decode split)
- Axes: arch:qwen3_asr, hw:GB10, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:long, fail:tbt-spike, feat:spatial-pd
- Scenario: On GB10 the chunked-prefill firewall still causes a TBT tail spike (Nexus: 250 ms mixed vs 15 ms decode-only, >8× TBT, L4); intra-node SM-partition P/D may avoid it.
- System must: Treat intra-node spatial prefill/decode partitioning as a measured option for the isochronous-frame-clock quadrant (strict-TPOT/relaxed-TTFT, TaiChi +77%), A/B-tested vs the chunked-prefill firewall on GB10 — not conflated with the (correctly-rejected) cross-node physical P/D.
- If mishandled: Assuming chunked prefill is free leaves an ~8× TBT spike on long-context streams; assuming all disaggregation is DC-only forfeits a real intra-node goodput win.

### STT-78 — Compact/repack active slots under heterogeneous residency
- Level: Extreme
- Pipeline: STT (masked-slot energy/bw)
- Axes: arch:voxtral, hw:B200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:many, fail:idle-waste, feat:slot-compaction
- Scenario: At scale with high churn, a lockstep batch is mostly idle/masked slots (barge-in/EOS/VAD create variable residency); padding hits 40%@BS32 and idle-lane energy is ~48% of serving energy (L8).
- System must: Either compact/repack active slots into a dense sub-batch OR explicitly budget the masked-slot energy/bandwidth cost in the duty ledger — under heterogeneous residency the masked slots are NOT free (L8 corrects §4.3's "masked is free").
- If mishandled: Institutionalizing slowest-stream-paces-all with 40% padding wastes ~half the serving energy and a third of the compute on idle lanes at fleet scale.

### STT-79 — Reliable barge-in abort across a multi-stage STT DAG
- Level: Extreme
- Pipeline: STT (reliable cancellation)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, worker:multistage, fail:abort-dropped, feat:reliable-abort
- Scenario: A barge-in must cancel a request that spans preprocessor → encoder → AED-decoder stages (possibly across processes); a fire-and-forget PUB/SUB abort drops messages to late-connecting stages (G9).
- System must: Use a reliable abort channel with per-stage ack (not fire-and-forget PUB/SUB), fail-fast so one terminal cancel aborts the request across ALL stages, and free every stage's slot/buffer; cancelled produces a distinct terminal frame (G2).
- If mishandled: A dropped abort leaves a downstream stage decoding stale audio after the user already moved on → a ghost transcript and a leaked slot.

### STT-80 — Fan-in deadlock on a conditional STT→translate→TTS branch
- Level: Extreme
- Pipeline: STT (dynamic fan-in)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, worker:multistage, fail:fanin-deadlock, feat:dynamic-fanin
- Scenario: An STT→translate→TTS DAG where a text-only request won't produce an audio-encoder output; a fixed `wait_for=[a,b,c]` deadlocks waiting for the branch that never fires (G11).
- System must: Compute the expected source set PER REQUEST (`wait_for_fn`), constrain routing to the static topology (forbid empty/unknown next), and support multi-terminal merge (text-only vs text+audio) so a request can narrow its terminals.
- If mishandled: A static fan-in waits forever for a branch a given request will never produce → the request hangs and its slot leaks (and the whole DAG can wedge under mixed traffic).

### STT-81 — GIL-style co-located encoder starvation (Rust translation)
- Level: Extreme
- Pipeline: STT (scheduler starvation)
- Axes: arch:parakeet, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:colocated, fail:starvation, feat:idle-block
- Scenario: A busy AR-decode loop co-located with the encoder forward starves the encoder's CPU core/runtime (the SGLang ~600× slowdown analog, G3).
- System must: Block on `recv_timeout`/`Notify` when idle (hog the core only when busy, the 2 ms-sleep discipline, F6), and prefer stage=process for the hot AR vs the encoder (SGLang moved to encoder disaggregation because co-location interferes); colocation must be starvation-load-tested.
- If mishandled: A `loop { try_recv() }` busy-wait starves the co-located encoder → audio QPS collapses (>10 to <0.5) exactly the way SGLang measured under the GIL.

### STT-82 — Crash blast-radius: flaky encoder isolated from the hot AR stage
- Level: Extreme
- Pipeline: STT (crash isolation)
- Axes: arch:qwen3_asr, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:multiprocess, fail:crash-blast, feat:stage-isolation
- Scenario: A flaky audio-encoder stage occasionally faults; if it shares a process with the AR decode, one fault exits the whole process group (G7).
- System must: Run the hot AR stage and the flaky encoder as SEPARATE processes (exclusivity invariants as assertions, not comments), wire all 3 crash-detection layers (scheduler-thread handler, task done-callbacks, process-liveness monitor), so a dead encoder fails its requests, not a server-wide hang.
- If mishandled: A single encoder fault kills every concurrent stream's process; silent background-task death wedges the stage with no failure signal.

### STT-83 — Heterogeneous DAG saturates the GB10 shared 273 GB/s ceiling
- Level: Extreme
- Pipeline: STT (shared-bandwidth arbiter)
- Axes: arch:whisper, hw:GB10, mem:unified, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:hetero, scale:many, fail:bw-oversubscribe, feat:bandwidth-ledger
- Scenario: Encoder placed on the NPU + AR decode on the GPU both contend for GB10's single ~273 GB/s LPDDR ceiling; zero-copy removes transfer cost but not shared-bandwidth cost (§3.4 contention guard).
- System must: Budget aggregate memory bandwidth as a schedulable resource (the shared-bandwidth ledger), admit only if `Σ bandwidth_duty ≤ S·ceiling`, and prefer to overlap a memory-bound stage (AR decode) with a compute-bound one (conv-encoder), co-locating + time-sharing when both saturate bandwidth.
- If mishandled: Placing both bandwidth-heavy stages concurrently oversubscribes the one ceiling → both slow down, and the "placement frees the GPU" win becomes a net loss for every stream.

### STT-84 — Cross-replica determinism for compliance replay (call-center transcripts)
- Level: Extreme
- Pipeline: STT (replay/audit)
- Axes: arch:whisper, hw:H200, mem:ring, batch:lockstep, slo:batch, lang:multi, seqlen:long, scale:fleet, fail:replay-divergence, feat:audit-determinism
- Scenario: A regulated call center must reproduce a stored transcript from stored audio for audit, but the original ran in a different batch composition on a different replica.
- System must: Guarantee per-stream determinism (output independent of co-tenant batch, STT-69) so a single-stream replay reproduces the audited transcript bit-for-bit; store the model sha + precision stamp + seed so the replay uses the exact verified variant.
- If mishandled: A transcript that changes with batch composition can't be reproduced for audit (a compliance failure) and signals an undetected masked-slot leak.

### STT-85 — Tail-latency under a noisy-neighbor co-tenant model load
- Level: Extreme
- Pipeline: STT (multi-tenant tail)
- Axes: arch:voxtral, hw:GB10, mem:unified, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:many, worker:multitenant, fail:tail-jitter, priority:realtime, feat:lazy-promote
- Scenario: A second model loads on GB10 (cold-start weight streaming) while live STT streams run; the load's bandwidth burst spikes the streams' p99 frame time.
- System must: Lazily promote to Stage-batched mode when the co-tenant loads (§8 `mode=auto`), budget the load's bandwidth in the shared ledger, and protect the live streams' cadence via the client playback buffer; never scale-to-zero/cold-start a serving path (warm over-provision, L9).
- If mishandled: An unbudgeted weight-load burst steals the shared bandwidth → every live stream's p99 spikes into glitch territory during the co-tenant load.

### STT-86 — GPU fault mid-stream → restart sidecar, fail in-flight cleanly
- Level: Extreme
- Pipeline: STT (GPU-fault recovery)
- Axes: arch:funasr_nano, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:sidecar, fail:gpu-fault, feat:recovery
- Scenario: A CUDA ECC/Xid fault corrupts the device mid-decode across a 32-stream lockstep batch.
- System must: Detect via the watchdog/sentinel (STT-52/53), fail every in-flight stream cleanly with a typed error (not a hang), restart the sidecar with `PR_SET_PDEATHSIG` so no VRAM is orphaned (H7), and reject new admissions until /readyz re-passes after warmup.
- If mishandled: A device fault hangs all 32 streams (blind health check), or the restart orphans VRAM into the next process and the box degrades on every subsequent load.

### STT-87 — Code-switch + far-field + 8 k telephony + biasing, all at once
- Level: Extreme
- Pipeline: STT (compounded real-world)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:codeswitch, seqlen:medium, fail:compound-acoustic, feat:multi-feature
- Scenario: A contact-center call: 8 kHz µ-law, far-field speakerphone, Hinglish code-switching, with a per-tenant biasing list for product names — every hard axis stacked.
- System must: Resample 8→16 k (anti-alias not needed upward), keep the AED multilingual (no single-language lock), apply per-stream biasing reset on slot recycle, run at trained precision (don't over-quant the hardest audio), and emit per-segment language — all on one stream without leaking biasing across slots.
- If mishandled: Any single mishandling (language lock, biasing leak, over-quant, wrong resample) compounds with the others → a transcript that's wrong on names, language, AND acoustics simultaneously.

### STT-88 — Hours-long live broadcast captioning, never-restart, bounded drift
- Level: Extreme
- Pipeline: STT (marathon streaming)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:hours, scale:many, fail:long-run-drift, feat:marathon
- Scenario: A 6-hour live broadcast caption stream must run without restart, with bounded memory and no accumulated drift, while other streams come and go.
- System must: Keep encoder cache bounded (deltas-only) and ring-KV wraparound-correct (logical-position mask, F4) for the entire run; cap all per-stream bookkeeping maps/sets (G6) so a long-lived session doesn't leak; segment-and-flush on natural pauses so memory stays O(window).
- If mishandled: Unbounded per-stream bookkeeping leaks over 6 hours (OOM); a ring-wraparound bug after the first wrap silently degrades captions for the rest of the broadcast.

### STT-89 — Lazy/background CUDA-graph capture: serve request-1 eager
- Level: Extreme
- Pipeline: STT (background capture)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:capture-stall, feat:lazy-capture
- Scenario: Even with warmup, capturing graphs for every slot-count cohort (1,2,4,…,N) at boot delays serving; the first burst of distinct cohort sizes each triggers a capture.
- System must: Capture exact slot counts (0 padding, sidesteps the 257→272 power-of-2 cliff, H4), capture the largest cohort first, and consider lazy/background capture (serve the first request eager while capturing in the background) — eager fallback is always available.
- If mishandled: A synchronous capture on the first appearance of each cohort size stalls those streams for seconds; padding to a fixed N reintroduces the power-of-2 capture cliff.

### STT-90 — Spec-decode scoped to the long-context paging path ONLY
- Level: Extreme
- Pipeline: STT (scoped speculative decode)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:batch, lang:en, seqlen:30k, fail:specdecode-misuse, feat:sparse-kv-spec
- Scenario: Speculative decode is a 0.98× net SLOWDOWN on acoustic tokens (PCG, L13) but a 2.51× win for long-context KV-memory-bound decode (MagicDec, batch 32–256) — the temptation is to apply it everywhere.
- System must: SCOPE spec-decode to the long-context token-AR-STT paging path only (sparse-KV spec-decode), and explicitly NOT bolt EAGLE/Medusa onto the frame-sync acoustic path (it destroys the rectangular lockstep, L13/L14).
- If mishandled: Blanket spec-decode slows the frame-sync STT path (the common case) by ~2% and breaks lockstep; banning it everywhere forfeits a 2.51× win on the long-context paging path.

### STT-91 — Per-tenant cache-salt to close a KV latency side-channel
- Level: Extreme
- Pipeline: STT (multi-tenant cache isolation)
- Axes: arch:qwen3_asr, hw:H200, mem:hybrid, batch:continuous, slo:realtime, lang:en, seqlen:medium, scale:fleet, worker:multitenant, fail:side-channel, feat:cache-salt
- Scenario: In a shared hybrid-KV deployment, prefix-cache hits create a measurable latency difference that leaks whether another tenant transcribed similar audio/prompt (H "other").
- System must: Use a per-tenant `cache_salt` on block-0 and a sha256 (never xxhash) prefix hash so cross-tenant collisions can't leak KV and the latency side-channel is closed; `extra_key` still fingerprints conditioning (G1) within a tenant.
- If mishandled: A shared prefix cache without per-tenant salt leaks a timing side-channel (and an xxhash collision could cross-tenant-leak KV outright) — a multi-tenant security failure.

### STT-92 — Gaudi-HPU static-shape STT encoder with a graph cache
- Level: Extreme
- Pipeline: STT (HPU placement)
- Axes: arch:nemo_ctc, hw:Gaudi-HPU, mem:none, batch:micro_batch, slo:realtime, lang:en, seqlen:short, worker:hpu, fail:dynamic-shape, feat:static-graph
- Scenario: A FastConformer-CTC encoder is deployed on Gaudi-HPU, which (like NPUs) heavily prefers static shapes and recompiles on shape changes.
- System must: Bucket inputs to fixed shapes (length buckets, static graph per bucket, §2.2 static-required substrates) so the HPU graph cache hits; place the static conv-encoder here while any AR decode stays on a dynamic-friendly engine (§2.3).
- If mishandled: Feeding variable-length audio to the HPU triggers a recompile per shape → catastrophic per-request compile cost, exactly the NPU/Hexagon fixed-shape-rigidity failure.

### STT-93 — Hexagon-HMX phone: encoder-on-DSP, AR decoder elsewhere
- Level: Extreme
- Pipeline: STT (phone split, VTCM cap)
- Axes: arch:whisper, hw:Hexagon-HMX, mem:vtcm, batch:static, slo:realtime, lang:en, seqlen:short, worker:dsp, fail:vtcm-overflow, feat:aot-static
- Scenario: On-device whisper on a phone: the conv-encoder must fit Hexagon's ~8 MB VTCM scratchpad (HMX reads only VTCM) with a strictly-static AOT context binary; AR decode can't run here (§2.2/2.3).
- System must: Compile the encoder as a fixed-shape AOT context binary that fits VTCM, run the AR decoder on the phone CPU/GPU, and use true-int4/int8 weights (HMX native) — the exact split Qualcomm ships for whisper.
- If mishandled: Trying to run the AR decoder on the DSP breaks the static contract (growing KV, per-token host round-trip); an encoder graph exceeding VTCM thrashes to LPDDR and loses realtime.

### STT-94 — ANE static encoder on Apple UMA, zero-copy to GPU decoder
- Level: Extreme
- Pipeline: STT (Apple UMA split)
- Axes: arch:whisper, hw:ANE, mem:uma, batch:static, slo:realtime, lang:en, seqlen:short, worker:ane, feat:uma-zerocopy
- Scenario: On Apple silicon the whisper conv-encoder runs on the ANE (fp16/int8 static) and hands its output to the AR decoder on the GPU over UMA (the exact split Apple ships, §2.3).
- System must: Place the static encoder on ANE, advertise a `SharedHostBufType` so the GPU decoder consumes the encoder output with ZERO copy on UMA (§3.4), and keep the AR decoder dynamic on the GPU.
- If mishandled: Inserting a copy at the ANE→GPU boundary on coherent memory wastes UMA bandwidth (the very resource that's shared); forcing AR onto the ANE breaks the static contract.

### STT-95 — int8-on-CPU-AMX STT as the only realtime path (no GPU)
- Level: Extreme
- Pipeline: STT (CPU-AMX realtime)
- Axes: arch:voxtral, hw:CPU, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:few, worker:cpu-amx, feat:amx-int8
- Scenario: A GPU-less x86 edge box must serve a few realtime STT streams; only AMX int8 makes an LLM-decoder STT realtime on CPU (the validated int8-Voxtral-on-CPU path, §5.2).
- System must: Route int8 to AMX (bf16/int8 native, ~8× VNNI), keep the batch knee small (CPU saturates at batch 1–4, §2.2), and run the encoder/feedforward stages on CPU too; the accuracy gate's int8 stamp must be earned on this substrate.
- If mishandled: Running fp32 on the Grace-class ARM CPU is ~24× slower (§1.7) → not realtime even at batch 1; routing int8 to a non-AMX path loses the 8× and misses cadence.

### STT-96 — MI300X: max co-resident small STT models + huge KV
- Level: Extreme
- Pipeline: STT (model co-residency)
- Axes: arch:mixed, hw:MI300X, mem:hbm, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, scale:many, worker:multimodel, feat:co-residency
- Scenario: One MI300X (192 GB HBM3, 256 MB cache) hosts many small STT models (whisper-tiny, parakeet, sensevoice, voxtral) co-resident to serve a polyglot fleet from one box.
- System must: Exploit MI300X's capacity (the bottleneck is under-occupancy, not capacity, §2.2) to keep all models warm with huge per-model KV headroom; route each request to its model's cohort; quantize weights (universal decode win) to widen each model's batch knee.
- If mishandled: Treating capacity as the constraint (evicting/reloading models) reintroduces cold-start stalls on a box whose whole advantage is co-residency; ignoring under-occupancy leaves the SMs idle at small per-model batch.

### STT-97 — RTX prosumer box: VRAM capacity is the wall
- Level: Extreme
- Pipeline: STT (VRAM-bound few-stream)
- Axes: arch:funasr_nano, hw:RTX, mem:vram, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:few, fail:vram-oom, feat:capacity-wall
- Scenario: A 24 GB RTX 4090 serves a handful of LLM-decoder STT streams; VRAM (not bandwidth) is the binding constraint (§2.2), and CUDA-graph + compile capture costs real memory.
- System must: Budget the CUDA-graph-pool delta + compile memory before admitting, use the OOM ladder (enforce-eager → cpu-offload → reduce shape) when capture would OOM (Section B, C8), and cap the batch at the tens-range knee.
- If mishandled: Capturing graphs without reserving the pool OOMs the 24 GB card on the first cohort; running an unquantized 3B model leaves no headroom for KV at any useful concurrency.

### STT-98 — Streaming RNN-T: emit immediately vs wait for context
- Level: Extreme
- Pipeline: STT (transducer emission timing)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:premature-emit, feat:rnnt-emission
- Scenario: A cache-aware RNN-T can emit a token as soon as the joint fires, but a too-eager emission with insufficient right-context produces a token it would have revised (the partial-stability tradeoff at the transducer level).
- System must: Respect the model's configured look-ahead (right-context cache) before committing a token as final; emit it as interim earlier, promote to final only when the cache-aware window confirms it (ties STT-66/98 to the cache-aware contract).
- If mishandled: Emitting finals at zero right-context yields frequent revisions that violate the `is_final` immutability contract; waiting for full context adds the model's max look-ahead to every token's latency.

### STT-99 — Multi-codebook / extra-head STT (semantic-VAD + EoT) per frame
- Level: Extreme
- Pipeline: STT (extra per-step heads)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, fail:head-desync, feat:extra-heads
- Scenario: A streaming STT model carries extra per-step linear heads (semantic-VAD, end-of-turn) alongside the token head (§9.7), all advancing on the same lockstep tick.
- System must: Run the extra heads as generic per-step linear heads inside the same batched forward (one node), gated by the same exec-mask so an idle slot's VAD/EoT head is a no-op; the EoT head drives `is_speech_final`.
- If mishandled: Running the heads off-tick or ungated desyncs end-of-turn from the transcript (turn fires before/after the words) and an ungated head poisons idle slots (F2).

### STT-100 — Determinism vs MTP acoustic-path head on STT-adjacent model
- Level: Extreme
- Pipeline: STT (MTP multi-token emit)
- Axes: arch:funasr_nano, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:mtp-lockstep-break, feat:mtp
- Scenario: An LLM-decoder STT adopts MTP (multi-token prediction, 2–5× quality-neutral, L14) to emit several tokens per step — but it must preserve the rectangular lockstep (unlike draft-spec-decode).
- System must: Treat the code-predictor/MTP heads as a direct-emit mechanism that PRESERVES rectangular lockstep (fixed extra heads, not EAGLE/Medusa), folding the inner emit into the outer step time `T_step = T_ar + heads×T_inner` (L14, §3.3).
- If mishandled: Bolting EAGLE/Medusa-style speculative draft heads destroys the rectangular lockstep (variable accepted-token count per stream) → the batch desyncs; mis-accounting the head time blows the per-step budget.

### STT-101 — Hot-swap a model variant (int8→fp16) without dropping streams
- Level: Extreme
- Pipeline: STT (rolling variant swap)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:many, fail:swap-glitch, feat:rolling-swap
- Scenario: A precision/variant change (int8 fails its gate, fall back to fp16) must roll out while live streams run, without glitching them.
- System must: Load the new variant alongside, drain new admissions to it, let existing streams finish on the old variant (never preempt mid-utterance, H2), and gate the new variant (validation pyramid, STT-61) before it serves any stream.
- If mishandled: Swapping the model under live streams mid-decode corrupts their KV/state (a glitch on every in-flight call); serving an ungated variant ships the regression the gate exists to catch.

### STT-102 — Bursty admission: 200 streams arrive in one second
- Level: Extreme
- Pipeline: STT (burst admission)
- Axes: arch:voxtral, hw:B200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:burst, fail:cold-burst, priority:realtime, feat:warm-capacity
- Scenario: A scheduled event dumps 200 new STT streams in ~1 s; cold-start is 1.7–12.8 s (BLITZSCALE) and bursts cost 2.3 s (TokenScale, L9).
- System must: Use WARM over-provisioning + warm-capacity repurposing (never scale-to-zero/cold-start), admit up to the calibrated ceiling and reject the overflow with Retry-After (graceful, STT-47/48), and stagger prefill admission (≤1 per K frames, STT-40) so the burst doesn't spike every existing stream.
- If mishandled: Cold-starting on the burst adds seconds of dead air to 200 callers; admitting all 200 prefills at once drops frames for every already-live stream.

### STT-103 — Single-sentence-context inadequacy for long-form TTS-grade STT
- Level: Extreme
- Pipeline: STT (cross-sentence context)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:batch, lang:en, seqlen:long, fail:context-truncation, feat:cross-sentence
- Scenario: A long-form transcription (audiobook QA) needs cross-sentence context to disambiguate homophones/references; single-sentence windows are inadequate (Audiobook-CC, L12).
- System must: Carry cross-sentence context on the long-form path (paged KV + attention-sink, STT-39/76) rather than resetting per sentence; balance the context window against the paging cost.
- If mishandled: Resetting context per sentence loses referential disambiguation (homophones, pronouns) → systematically wrong word choices on exactly the long-form content where accuracy is graded hardest.

### STT-104 — Force-chunk a long audio-prompt to avoid 147× TTFT HoL
- Level: Extreme
- Pipeline: STT (long audio-prompt encode)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:long, scale:many, fail:ttft-hol, feat:force-chunk
- Scenario: One stream carries a very long audio prompt to encode; encoding it in one shot head-of-line-blocks every other stream's TTFT (#37308: 147× TTFT HoL).
- System must: Force-chunk the long audio-prompt encode (`long_prefill_token_threshold`) so it interleaves with other streams' frames; separate the control-plane (small msgpack) from the data-plane (raw PCM, zero-copy, ref-held until send done) (H "other").
- If mishandled: A single long encode blocks the head of line for 147× normal TTFT → every concurrent stream's first transcript is catastrophically delayed.

### STT-105 — End-of-stream is an explicit FINAL frame, never inferred from silence
- Level: Extreme
- Pipeline: STT (explicit termination)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:silent-close, feat:explicit-final
- Scenario: A consumer of streaming partials must distinguish "transcription done" from "producer stalled"; absence of chunks is ambiguous (G2).
- System must: Send an explicit FINAL frame (`is_speech_final` / a done sentinel) at end-of-stream; "closed without FINAL" is the client-side failure signal; barge-in cancel sends a DISTINCT terminal frame from completion (G2, STT-30).
- If mishandled: Inferring done from absence-of-chunks closes prematurely on a slow producer or hangs forever on a stalled one — and can't tell a cancelled stream from a completed one.

### STT-106 — Cap every per-stream bookkeeping map (long-lived server leak)
- Level: Extreme
- Pipeline: STT (bookkeeping bounds)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:fleet, fail:unbounded-map, feat:capped-bookkeeping
- Scenario: A long-lived STT server accumulates per-stream chunk counters / closed-sets / aborted-sets; unbounded, they leak over days (G6).
- System must: Cap every per-stream bookkeeping set/map (e.g. 10000→trim-5000) and purge per-slot maps on slot-free (ties STT-37/55); keep the ordered egress queue unbounded only with sender-side credit backpressure (never drop/reorder audio, but bound everything else).
- If mishandled: An uncapped counter map grows until the server OOMs after days of uptime — a leak invisible in any short test, fatal in production.

### STT-107 — Out-of-order arrival: decoder stage gets chunks before its payload
- Level: Extreme
- Pipeline: STT (out-of-order DAG)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:multistage, fail:ooo-arrival, feat:opt-in-prepayload
- Scenario: In a parallel-path STT DAG, the decoder stage can receive encoder stream-chunks BEFORE its own request payload (no cross-path ordering) (G "out-of-order").
- System must: Make pre-payload stream acceptance an EXPLICIT opt-in (`can_accept_stream_before_payload`), use a monotone `chunk_id` per (req,target), latch the contract from whichever (payload|chunk-meta) arrives first, and hard-fail (not silently corrupt) if not opted in.
- If mishandled: Silently accepting out-of-order chunks without opt-in corrupts the decoder's input ordering → a scrambled transcript with no error.

### STT-108 — fp16 overflow on long-context attention → NaN cascade
- Level: Extreme
- Pipeline: STT (attention numerics)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:batch, lang:en, seqlen:30k, fail:fp16-overflow, feat:bf16-attention
- Scenario: A long-context token-AR STT decode in fp16 overflows attention scores >65504 → inf → NaN (#1448/#2064, H5).
- System must: PREFER bf16 over fp16 for long-context attention (fp16 overflows; bf16's range doesn't), keep sampler/softmax math in fp32, and the always-on NaN-detector rejects any frame that still produces NaN (STT-43).
- If mishandled: fp16 long-context attention overflows to NaN → the NaN cascades through the AR history and corrupts the rest of the transcript (or, unguarded, argmaxes to garbage tokens).

### STT-109 — Bitwise-impossible determinism forces per-stream-only contract under spill
- Level: Extreme
- Pipeline: STT (determinism under migration)
- Axes: arch:qwen3_asr, hw:B200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:long, scale:fleet, fail:migration-nondeterminism, feat:per-stream-under-spill
- Scenario: A stream is migrated mid-decode between replicas (STT-73); atomic-reduction nondeterminism (#24067) means even the same stream can't be bitwise-identical across the migration boundary.
- System must: Hold the per-stream-determinism contract only WITHIN a replica; across a migration, guarantee transcript-equivalence (same words) not bitwise identity, and complete the migration within the playback-buffer slack so no frame drops; document that bitwise cross-replica identity is unachievable.
- If mishandled: Promising bitwise identity across migration is impossible and masks a real bug (a migration that changes the transcript content, not just its bits) — the latter is a correctness failure to catch.

### STT-110 — Full heterogeneous DAG: ANE encoder ∥ GPU decoder ∥ CPU resample, one stream
- Level: Extreme
- Pipeline: STT (full hetero zero-copy DAG)
- Axes: arch:whisper, hw:Apple-UMA, mem:uma, batch:lockstep, slo:realtime, lang:multi, seqlen:medium, worker:hetero, fail:boundary-copy, feat:full-dag
- Scenario: On Apple UMA, one streaming STT request runs the mel/resample on CPU, the conv-encoder on ANE, and the AR decoder on the GPU — three substrates, two zero-copy boundaries, one isochronous tick.
- System must: Pin each stage to its weights' resident substrate (§3.4 follow-the-weights), cross both boundaries with `SharedHostBufType` zero-copy on UMA, budget the shared UMA bandwidth (STT-83), and keep the whole DAG inside the frame budget (`T_step = Σ stage times`, schedulability folds the pipeline).
- If mishandled: A copy at either coherent-memory boundary wastes the shared UMA bandwidth (the binding resource); mis-placing AR on the ANE breaks the static contract; ignoring the bandwidth budget oversubscribes UMA and the stream glitches.

### STT-111 — Many-language polyglot fleet: per-request model+language routing
- Level: Extreme
- Pipeline: STT (polyglot routing)
- Axes: arch:mixed, hw:MI300X, mem:hbm, batch:lockstep, slo:realtime, lang:multi, seqlen:short, scale:fleet, worker:multimodel, fail:misroute, feat:lang-routing
- Scenario: A global fleet routes each call to the best (model, language) — parakeet-v3 for some languages, canary for translation, voxtral for streaming English — across co-resident models on MI300X (STT-96).
- System must: Route by detected/declared language to the right model's cohort, batch within each (model, frame_rate) cohort (§4.2), and keep all models warm (co-residency); a language the loaded models don't cover gets a typed unsupported error, not a silent wrong-language transcript.
- If mishandled: Routing a language to a model that doesn't support it produces a confident wrong-language transcript; cross-cohort mixing desyncs the lockstep tick.

### STT-112 — Calibration-stale after driver upgrade invalidates admission ceiling
- Level: Extreme
- Pipeline: STT (calibration lifecycle)
- Axes: arch:nemotron, hw:H200, mem:cache-aware, batch:lockstep, slo:realtime, lang:en, seqlen:medium, scale:many, fail:stale-calib, feat:recalibrate
- Scenario: A CUDA driver/firmware upgrade changes `T_step(B_active)`; the persisted admission ceiling (keyed `sha256 × device × driver × warm-set`) is now stale.
- System must: Invalidate the calibration stamp on a driver/warm-set key change and re-run calibration (without the profiler, STT-63) before serving at the old ceiling; until re-calibrated, admit conservatively.
- If mishandled: Serving at a stale (too-high) ceiling after a driver change that slowed the step glitches every stream; the keyed stamp exists precisely to catch this — ignoring the key serves on wrong numbers.

### STT-113 — Watermark computed exactly from fixed slots (no preempt storm)
- Level: Extreme
- Pipeline: STT (KV watermark)
- Axes: arch:qwen3_asr, hw:H200, mem:paged, batch:continuous, slo:realtime, lang:en, seqlen:medium, scale:many, fail:watermark-storm, feat:exact-watermark
- Scenario: On the token-AR paging path, a too-low KV watermark triggers a preempt storm (vLLM watermark=0.0 → 1065 preempts, H3).
- System must: Compute the watermark EXACTLY from the fixed slots — `Σ per-slot next-frame KV growth × lookahead` (strictly better than vLLM's heuristic fraction) — and reserve the CUDA-graph-pool delta before admitting; pre-capture feasibility at boot.
- If mishandled: A heuristic-fraction watermark either storms (too low → preempt thrash on the paging path) or wastes capacity (too high); the exact computation from fixed slots is the whole point of the fixed-slot design.

### STT-114 — Per-inference deadline is device+model-aware, not flat
- Level: Extreme
- Pipeline: STT (adaptive deadline)
- Axes: arch:mixed, hw:CPU, mem:ring, batch:lockstep, slo:mixed, lang:en, seqlen:long, fail:flat-deadline, feat:adaptive-deadline
- Scenario: One server hosts a CTC step (~ms) and a 1.5B LLM-decoder STT step on a CPU batch path that legitimately needs 3600 s, not 300 s (#45135, H9).
- System must: Set the progress-watchdog/per-inference deadline per device+model (a CTC step ≠ a 1.5B AR step; CPU needs 3600 s not 300 s), NOT vLLM's flat 300 s, so a slow-but-healthy job isn't killed and a wedged realtime stream is.
- If mishandled: A flat 300 s deadline kills a legitimately-slow CPU batch transcription mid-run AND lets a wedged realtime stream hang for 5 minutes — wrong on both ends.

### STT-115 — Aging promotes a starved low-priority batch transcript (no dropped call)
- Level: Extreme
- Pipeline: STT (wall-clock aging)
- Axes: arch:parakeet, hw:H200, mem:none, batch:micro_batch, slo:batch, lang:en, seqlen:long, scale:overload, fail:starvation, priority:batch, feat:aging
- Scenario: Under sustained realtime load, a low-priority batch transcription waits forever (vLLM has no deadline/aging promotion anywhere, H8).
- System must: Promote a waiting job after `max_wait` (wall-clock aging), keep FCFS-within-slot-pool + hard per-slot fairness; if a priority key exists it MUST include an age/preemption term (the #41951 omission).
- If mishandled: A batch job starves indefinitely behind realtime streams (no aging) → it never completes; a priority key without an age term re-victimizes the same job forever.

### STT-116 — Step-bucket key accepts per-request variable N (eroding fixed-NFE)
- Level: Extreme
- Pipeline: STT (variable-N step bucket)
- Axes: arch:funasr_nano, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, fail:fixed-nfe-assumption, feat:variable-n-bucket
- Scenario: An STT-adjacent generative inner head uses a per-request variable NFE (incl N=1 feedforward, length-decoupled, mixed trajectories) — the "fixed-N / length-bucketed / CFG-folded" step-bucket assumption erodes (IntMeanFlow/LLaDA-TTS, L15).
- System must: Make the step-bucket key accept per-request variable N (incl N=1), length-decoupled step count, and mixed teacher/student trajectories; CFG-folding is NOT universal (some are CFG-free), so don't hardcode the ×2.
- If mishandled: A hardcoded fixed-N / CFG-×2 step bucket can't batch the variable-NFE streams together → they fall to bs=1 each, losing the whole step-bucket throughput.

### STT-117 — Padded slot must be zeroed, not left with a stale -1 KV write
- Level: Extreme
- Pipeline: STT (padded-slot safety)
- Axes: arch:voxtral, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:many, fail:padding-corrupt, feat:zero-padded-slot
- Scenario: When a cohort capture pads slot counts, a padded slot's KV write can land a -1 / sentinel into a real KV slot (#43810, H4).
- System must: `dst.zero_()` padded slots, capture EXACT slot counts to avoid padding entirely (H4), and freeze GC + weak-ref outputs during capture; a padded write must never touch a live slot's KV.
- If mishandled: A padding write of -1 into a real KV slot silently corrupts that stream's attention → a wrong transcript on a stream that did nothing wrong.

### STT-118 — Async-decode lookahead is net-negative at bs=1 (gate on batch)
- Level: Extreme
- Pipeline: STT (bs=1 fast path)
- Axes: arch:voxtral, hw:GB10, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:short, scale:1, fail:bs1-overhead, feat:sync-fastpath
- Scenario: A single live STT call (bs=1, the heavy-hitter case) gets a one-step-lookahead async-decode optimization that costs more than it saves at bs=1 (fixed event/pingpong overhead, G8).
- System must: Gate pipelining/double-buffering on batch size (`async_decode_min_batch_size=2`) and fall back to the simple synchronous path at bs=1 (matches the measured CUDA-graph-hurts-@batch-32 inversion); any in-flight-step optimization must handle abort/finish-during-overrun (double-free landmine).
- If mishandled: Forcing async lookahead at bs=1 adds overhead to the single-call common case AND opens a stale-batch double-free (re-running a finished req frees KV twice).

### STT-119 — Notify-before-wait on the sidecar↔stage relay (deadlock guard)
- Level: Extreme
- Pipeline: STT (relay deadlock)
- Axes: arch:qwen3_asr, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:sidecar, fail:relay-deadlock, feat:notify-before-wait
- Scenario: The sidecar↔stage relay uses a pull/RDMA-style transport (receiver-initiates-read); waiting for transfer completion BEFORE sending the data-ready control message deadlocks (NIXL, G4).
- System must: Send the data-ready CONTROL message BEFORE awaiting transfer completion (notify-before-wait, a tested per-transport property), use credit back-pressure (default credits=2) on the relay, make double-release a HARD error, and run an orphan-reaper for shm (receiver-owns-unlink).
- If mishandled: Wait-then-notify deadlocks the relay (both sides block); no credit back-pressure overflows the pool/OOMs; an orphaned shm leak accumulates if the receiver crashes.

### STT-120 — Fan-out by reference aliasing on a shared STT result
- Level: Extreme
- Pipeline: STT (fan-out ownership)
- Axes: arch:canary, hw:H200, mem:ring, batch:lockstep, slo:realtime, lang:en, seqlen:medium, worker:multistage, fail:aliasing, feat:move-ownership
- Scenario: One STT result fans out to N downstream stages (e.g. transcript → both a translation node and a logging node) in-process; sharing a mutable container by reference lets one stage's mutation corrupt the others (G5).
- System must: MOVE ownership across in-process channels (Rust borrow-checker enforces it for free with `Box<Payload>`); clone-on-fan-out the owned container, share `Arc` only for immutable tensor leaves; serialize ONLY cross-process — never reach for `Arc<Mutex<Payload>>` on fan-out (reintroduces the aliasing hazard).
- If mishandled: An `Arc<Mutex>` fan-out lets the translation node mutate the shared transcript the logging node still reads → corrupted/torn data, only under the specific fan-out timing.

---

## Coverage

**Axes exercised**

- **arch (model family):** whisper (enc-dec AED), moonshine (raw-audio AED), parakeet (TDT/RNN-T, duration-head path), nemo_ctc (FastConformer-CTC), sensevoice (CTC + LFR/CMVN), canary (NeMo AED + translate), cohere (FastConformer + transformer decoder, merged-KV), funasr_nano (SenseVoice enc + Qwen3-0.6B LM, caller-managed KV), voxtral_realtime (causal enc + Mistral LM, 1:1 lockstep), qwen3_asr (audio enc + Qwen3 LM, paging path), nemotron (cache-aware FastConformer-RNNT, 560-stream), plus `mixed` co-residency.
- **hw (substrate):** GB10 unified, H200, B200, MI300X, RTX, CPU (AMX/NEON/Grace-ARM), Hexagon-HMX (VTCM), ANE/Apple-UMA, Gaudi-HPU.
- **mem:** ring (per-slot fixed), paged (token-AR escape), hybrid (radix prefix + ring), cache-aware (delta encoder state), unified/UMA/VTCM/VRAM/HBM, none (CTC/transducer).
- **batch:** inline (N=1 edge), micro_batch (compute-bound encoder, length-bucketed), lockstep (frame-sync AR), continuous (long-variable token-AR STT), static (NPU/HPU/Hexagon).
- **slo / priority:** realtime vs batch vs mixed; Realtime > Batch; deadline-aware/aging; reject-don't-glitch vs graceful-degrade.
- **lang:** en, de, multilingual, code-switch (Hinglish), auto-detect, translate, polyglot-routing.
- **seqlen:** micro (<1 s), short, medium, long (>30 s), hours, 30k+ tokens.
- **worker / scale:** inline, sidecar, hetero/zero-copy, colocated, multistage, multiprocess, multimodel, multitenant; scale 1 → 16 → 560 → 1000 → fleet/burst.
- **fail:** empty/silence, hallucination/repeat, low-SNR, overlap, quant-divergence, ep-mismatch, masked-slot, idle-resume, cross-tenant/privacy, wraparound, lossy-ring, prefill-spike, cohort-mix, nan-logit, fp16-overflow, graph-sampler/capture-OOM, hol-block, overload/load-shed/drift, d2h-sync, sidecar/GPU/crash recovery, slot-leak, backpressure, kv-contaminate/side-channel, migration-glitch, variable-stride/dynamic-FR, starvation, relay-deadlock, aliasing, tile-quant, padding-corrupt, stale-calib, flat-deadline, watermark-storm, misroute, bs1-overhead.
- **feat (STT features):** transcribe/greedy-ctc, 3-level finality, partial/interim stability, cache-aware streaming, long-audio windowing, endpoint/VAD/EoT, language-detect/forced/code-switch/translate, word-timestamps/alignment/per-word-confidence, biasing, repeat/hallucination guard, short-utterance, quant gate / validation pyramid / precision-substrate / per-component precision, resample (8 k/44.1 k/anti-alias), barge-in/abort/reliable-cancel, reconnect/marker-flush/cache-resume, lockstep correctness (exec-mask, gated-mutation, reset-slot, logical-pos, padded-slot, extra-heads, MTP), admission/duty-ledger/watermark/aging/cohort, heterogeneous placement/zero-copy/bandwidth-ledger, sidecar discipline (sync-free, slot-keyed-state, dead-flag, watchdog, notify-before-wait, move-ownership), DC scale/KV-migration/spatial-PD/co-residency, calibration lifecycle.

**Level distribution** (120 scenarios)

- **Simple:** STT-1 … STT-10 (10) — single-stream, one feature, KISS baseline behaviors.
- **Intermediate:** STT-11 … STT-35 (25) — one realistic complication each (streaming finality, windowing, endpointing, multilingual, noise, biasing, quant gate, single-EP/precision guard, basic concurrency, reconnect, barge-in, split placement).
- **Compound:** STT-36 … STT-70 (35) — multiple interacting concerns (lockstep correctness traps, prefill firewall, cohort batching, numerics/graph guards, admission/overload/drift, sidecar hot-loop + crash + watchdog, hybrid KV + conditioning-hash, long-context paging escape).
- **Extreme:** STT-71 … STT-120 (50) — DC-scale + heterogeneous-hardware + multi-failure + research-frontier (1000-stream/560-stream ceilings, KV migration, variable-stride/dynamic-FR third execution class, every-substrate splits, multi-tenant security/determinism, relay/fan-out/aliasing, calibration/watermark/aging lifecycle, compounded real-world stacks).
