# WaaV Infer — Real-World Scenario Catalog · Family 10: Feature-Composition / Multi-Feature DAG

Scope: the engine composing voice FEATURES (STT, TTS, STS, text MT + speech translation, audio-processing — denoise/enhance/dereverb/AGC/VAD, diarization, language-detection, speaker-verification/ID, keyword-spotting/wake-word, audio super-resolution/bandwidth-extension, punctuation/ITN) into **typed stage-DAGs** (INFER_ENGINE §3), and the correctness/scheduling traps those compositions create (fan-in deadlock G11, conditional branches, multi-terminal, per-stage placement §3.4, end-to-end streaming with FINAL propagation G2/I1, per-feature SLO budgets §6, vendor-mixed stages, barge-in cancellation through a DAG G9). Levels: SIMPLE (one/two stages) → INTERMEDIATE (3+ stages, one cross-cutting concern) → COMPOUND (multiple concerns interacting) → EXTREME (full heterogeneous multi-tenant meeting-assistant).

Axes legend: `feat:*` (translate|enhance|diarize|clone|verify|wake|sr|vad|lang|punct|kws|denoise|dereverb|agc), `dag:*` (linear|fanin|fanout|conditional|multiterminal|nested), `place:*` (gpu|npu|cpu|hetero|zerocopy), `stream`, `bargein`, `slo`, `vendor:mixed`, `tenancy:multi`.

---

### FEAT-1 — Bare STT, single feature, single stage
- Level: SIMPLE
- Pipeline: `audio-in → STT → text-out`
- Axes: feat:stt, dag:linear, stream
- Scenario: A caller streams 16 kHz mic audio; the engine returns a running transcript with no other processing.
- System must: Run a one-node DAG (entry=final_output), stream delta text frames, emit explicit FINAL on marker (G2), never inferred from silence.
- If mishandled: Transcript truncated at clip end if FINAL is inferred from absence-of-chunks rather than the marker.

### FEAT-2 — Bare TTS, single feature, single stage
- Level: SIMPLE
- Pipeline: `text-in → TTS → audio-out`
- Axes: feat:tts, dag:linear, stream
- Scenario: A chatbot hands a finished sentence to TTS and plays the synthesized audio to the user.
- System must: Stream delta audio samples only (I1), never cumulative re-decode from step 0 (O(N²)); offline-concat == stream-concat byte-identical.
- If mishandled: User hears replays or truncation; offline RTF passes while live playback is wrong (the most common silent streaming bug).

### FEAT-3 — Speech-to-speech, single STS stage (no cascade)
- Level: SIMPLE
- Pipeline: `audio-in → STS(full-duplex) → audio-out`
- Axes: feat:sts, dag:linear, stream, bargein
- Scenario: A Moshi-class full-duplex model takes user audio and emits assistant audio on the same frame clock, no intermediate text.
- System must: Run frame-synchronous lockstep (§4.2) on one (model, frame_rate) cohort; model the user stream always (barge-in = always-modeled input).
- If mishandled: Half-duplex stalls; barge-in not detectable because the user channel isn't kept live.

### FEAT-4 — Denoise → STT (front-end cleanup before recognition)
- Level: SIMPLE
- Pipeline: `audio-in → denoise(DeepFilterNet) → STT → text-out`
- Axes: feat:denoise, feat:stt, dag:linear, place:hetero
- Scenario: Noisy café audio is cleaned by DeepFilterNet, then transcribed.
- System must: Two-node DAG; denoise is `feedforward`/micro-batch, STT downstream; place denoise on NPU/CPU (static conv) and keep STT-encoder where its weights live (§2.3, §3.4).
- If mishandled: Denoise inherits the AR batch policy and either head-of-line-blocks STT or runs on the wrong substrate, wasting GPU bandwidth.

### FEAT-5 — VAD → STT (gate recognition on speech presence)
- Level: SIMPLE
- Pipeline: `audio-in → VAD → (speech?) → STT → text-out`
- Axes: feat:vad, feat:stt, dag:conditional
- Scenario: A push-to-talk app only transcribes frames VAD marks as speech, dropping silence.
- System must: VAD as a lightweight feedforward gate feeding a conditional edge; route_fn returns a target ∈ statically-declared `next` (G11) — silence routes to an explicit terminal sink, never an empty set.
- If mishandled: Empty route silently drops frames, or non-static targets make the topology unanalyzable.

### FEAT-6 — Wake-word → STT activation
- Level: SIMPLE
- Pipeline: `audio-in → KWS(wake) → (detected?) → STT → text-out`
- Axes: feat:wake, feat:kws, feat:stt, dag:conditional, place:npu
- Scenario: A device runs always-on wake-word spotting; only after "hey assistant" does it spin up the STT path.
- System must: KWS is a static conv model pinned to NPU (fixed=1, AOT); the wake event is a control message that admits the STT stream into a free slot.
- If mishandled: Always-running STT wastes the GPU; or wake detection on the GPU steals AR bandwidth from live streams.

### FEAT-7 — Language-detection → routed STT
- Level: SIMPLE
- Pipeline: `audio-in → langID → (route by lang) → STT[lang] → text-out`
- Axes: feat:lang, feat:stt, dag:conditional
- Scenario: An inbound call of unknown language is sniffed, then sent to the matching STT model/config.
- System must: langID feedforward stage; route_fn picks one of the statically-declared STT branches (G11); all candidate STT models are in the static topology even if only one fires per request.
- If mishandled: Routing to a non-declared branch (ValueError) or to a model not loaded → request hangs or crashes.

### FEAT-8 — STT → punctuation/ITN restoration
- Level: SIMPLE
- Pipeline: `STT(raw) → punct+ITN → formatted-text-out`
- Axes: feat:punct, feat:stt, dag:linear, stream
- Scenario: A raw lowercase no-punctuation transcript is post-processed to add casing, punctuation, and inverse-text-normalization ("twenty twenty six" → "2026").
- System must: Punct/ITN as a downstream feedforward/text stage; stream incrementally but commit only on stable spans so casing doesn't flicker.
- If mishandled: Re-punctuating the whole transcript each chunk (O(N²)) or flickering output as later context rewrites earlier words.

### FEAT-9 — STT → text-MT translation (text-only output)
- Level: SIMPLE
- Pipeline: `audio-in → STT → text-MT → translated-text-out`
- Axes: feat:translate, feat:stt, dag:linear, stream
- Scenario: An English call is transcribed and the text translated to Spanish for a subtitle feed (no spoken output).
- System must: Linear 3-stage DAG terminating in text; stream partial translations but treat MT as needing stable source spans (sentence/clause boundaries) before emitting.
- If mishandled: Translating unstable partial transcripts produces churning, contradictory subtitles.

### FEAT-10 — Bandwidth-extension / super-resolution (8 k → 16 k) before STT
- Level: SIMPLE
- Pipeline: `audio-in(8k PSTN) → super-res(8k→16k) → STT → text-out`
- Axes: feat:sr, feat:stt, dag:linear, place:hetero
- Scenario: A narrowband telephony call is bandwidth-extended to 16 kHz so a 16 kHz STT model recognizes it accurately.
- System must: SR stage as feedforward on CPU/NPU; respect the two-clock contract (§5.1) — SR changes sample-rate, not the STT frame-rate; resample ingress any→16 k is a post/pre-batch stage off the AR clock.
- If mishandled: Feeding 8 kHz into a 16 kHz STT (WER cliff), or running SR on the GPU and stealing AR bandwidth.

### FEAT-11 — Speaker-verification gate (voice biometric auth)
- Level: SIMPLE
- Pipeline: `audio-in → speaker-verify → (match?) → {grant | reject}`
- Axes: feat:verify, dag:conditional, place:npu
- Scenario: A banking IVR verifies the enrolled speaker before granting access; a mismatch routes to fallback auth.
- System must: Verify is a feedforward embedding+score stage; the match/no-match decision is a route_fn into static branches; the enrolled-speaker embedding is request-keyed state, not shared model state (I5).
- If mishandled: Enrolled embedding cached on the model leaks across requests → wrong-speaker grants under concurrency.

### FEAT-12 — Voice-clone TTS from a reference clip
- Level: SIMPLE
- Pipeline: `(text + ref-audio) → clone-TTS → audio-out(cloned voice)`
- Axes: feat:clone, feat:tts, dag:linear, stream
- Scenario: A user uploads a 6-second reference and the engine synthesizes new text in that voice.
- System must: If any KV/prefix reuse exists, the cache key MUST fingerprint the full N-codebook ref sequence (blake2b over all codebooks, G1/L1), not just token-ids; zero-shot (no ref) → key=None so legit prefix-sharing survives.
- If mishandled: Two requests with identical text but different ref-audio collide in the prefix cache → silent wrong-voice output, only under concurrency.

### FEAT-13 — Diarization-only (who-spoke-when, no transcript)
- Level: SIMPLE
- Pipeline: `audio-in → diarize(pyannote) → speaker-timeline-out`
- Axes: feat:diarize, dag:linear
- Scenario: A 2-person recording is segmented into "speaker A: 0–3s, speaker B: 3–7s…" with no words.
- System must: Diarization as a feedforward/windowed stage producing a labeled timeline; bound per-stream clustering state and free it on stream end.
- If mishandled: Unbounded speaker-embedding accumulation leaks memory on long sessions.

### FEAT-14 — Dereverb → STT (far-field cleanup)
- Level: SIMPLE
- Pipeline: `audio-in(reverberant) → dereverb → STT → text-out`
- Axes: feat:dereverb, feat:stt, dag:linear, place:hetero
- Scenario: A conference-room far-field mic with heavy reverberation is dereverberated before recognition.
- System must: Dereverb feedforward stage placed on a static-friendly engine; the boundary to STT crosses zero-copy on coherent memory (§3.4).
- If mishandled: Cross-substrate copy at the boundary adds latency; or reverb tail confuses STT word boundaries.

### FEAT-15 — AGC normalization before STT
- Level: SIMPLE
- Pipeline: `audio-in(varying level) → AGC → STT → text-out`
- Axes: feat:agc, feat:stt, dag:linear
- Scenario: A caller alternates between whispering and shouting; AGC normalizes level so STT stays accurate.
- System must: AGC as an inline/feedforward stage with per-stream gain state keyed by slot (I5), reset on slot recycle.
- If mishandled: Shared AGC gain state bleeds one caller's level dynamics into another under concurrency.

### FEAT-16 — Denoise → STT → LLM → TTS (the canonical voice-agent loop)
- Level: INTERMEDIATE
- Pipeline: `audio-in → enhance → STT → LLM → TTS → audio-out`
- Axes: feat:enhance, feat:stt, feat:tts, dag:linear, stream, bargein
- Scenario: A phone agent: clean the mic, transcribe, run an LLM turn, synthesize the reply, stream it back, with barge-in.
- System must: 5-stage linear DAG; each stage streams to the next; FINAL marker propagates end-to-end (G2); barge-in is a control message that jumps every stage's queue and frees the slot within ≤1 tick (§6, G9).
- If mishandled: Barge-in only cancels TTS while STT/LLM keep running → assistant talks over the user; or FINAL lost mid-chain truncates the reply.

### FEAT-17 — STT → translate → TTS (the EN→Hindi multivendor speech-translation path)
- Level: INTERMEDIATE
- Pipeline: `audio-in(EN) → STT → text-MT → TTS(Hindi) → audio-out(Hindi)`
- Axes: feat:translate, feat:stt, feat:tts, dag:linear, stream, vendor:mixed
- Scenario: English speech becomes Hindi speech; the proven path mixes vendors (e.g. Deepgram STT + Sarvam MT + ElevenLabs TTS) or runs all-local.
- System must: Linear 3-feature DAG with per-stage substrate/vendor; segment on clause boundaries so MT→TTS gets stable spans; propagate FINAL so the Hindi tail isn't cut.
- If mishandled: Streaming unstable partials into MT churns the Hindi output; or a per-stage timeout fires before the slow MT stage completes.

### FEAT-18 — Diarize + STT (per-speaker transcript, fan-out then label-join)
- Level: INTERMEDIATE
- Pipeline: `audio-in → {diarize, STT} → join-by-time → per-speaker-transcript`
- Axes: feat:diarize, feat:stt, dag:fanout, dag:fanin
- Scenario: A meeting clip yields "Alice: hello / Bob: hi" by running diarization and STT in parallel and stitching words to speaker segments by timestamp.
- System must: Fan-out the same audio (clone-on-fan-out the owned container, G5 — never alias a mutable buffer); fan-in joins on a `wait_for_fn` expecting both branches per request (G11).
- If mishandled: Aliased fan-out buffer mutated by one branch corrupts the other; or fixed wait_for deadlocks if one branch produces no output for a segment.

### FEAT-19 — STT → LLM → TTS with diarization conditioning
- Level: INTERMEDIATE
- Pipeline: `audio-in → diarize → STT(per-speaker) → LLM(speaker-aware) → TTS → audio-out`
- Axes: feat:diarize, feat:stt, feat:tts, dag:linear, stream
- Scenario: An agent that addresses speakers by role (e.g. "the manager asked…") uses diarization to tag the transcript before the LLM turn.
- System must: Diarization output feeds the STT/LLM stages as side conditioning on a typed edge; the join gates on the expected per-request source set.
- If mishandled: LLM fires before diarization labels arrive → speaker attribution wrong or missing.

### FEAT-20 — Multi-terminal: emit BOTH translated text and translated audio
- Level: INTERMEDIATE
- Pipeline: `audio-in → STT → MT → {text-out, TTS → audio-out}`
- Axes: feat:translate, feat:tts, dag:multiterminal, stream
- Scenario: A live-translation UI shows subtitles AND plays dubbed audio from the same translated text.
- System must: Multi-terminal DAG — collect partials by stage, gate on the expected terminal set, let the request narrow its terminals (text-only vs text+audio); each terminal sends its own FINAL.
- If mishandled: One terminal's FINAL closes the whole request → the other terminal truncates (G11 multi-terminal merge bug).

### FEAT-21 — Conditional branch: text-only vs audio path (dynamic wait_for_fn)
- Level: INTERMEDIATE
- Pipeline: `STT → MT → route_fn → {text-terminal | TTS-terminal}`
- Axes: feat:translate, feat:tts, dag:conditional, dag:fanin
- Scenario: Per request, the client asks for either subtitles only or dubbed audio; the DAG must not wait on the TTS branch when it won't fire.
- System must: `wait_for_fn(req)` computes the expected source set per request so the text-only branch doesn't block on a never-firing audio encoder (G11); route_fn targets stay in the static `next`.
- If mishandled: Fixed `wait_for=[text, audio]` deadlocks on text-only requests (the canonical conditional-branch fan-in deadlock).

### FEAT-22 — Fan-in merge: multimodal aggregate waits on multiple encoders
- Level: INTERMEDIATE
- Pipeline: `{audio-enc, text-enc} → mm_aggregate → decoder → out`
- Axes: dag:fanin, feat:stt, stream
- Scenario: A speech+text-prompted model (e.g. an audio-LLM with a system prompt) must merge an audio-encoder output and a text-encoder output before decoding.
- System must: mm_aggregate is a fan-in node gating on the per-request expected encoder set (G11); out-of-order arrival is explicit opt-in (`can_accept_stream_before_payload`), else hard-fail not silent-corrupt.
- If mishandled: Decoder fires on partial fan-in → garbled multimodal fusion; or a late-arriving encoder output is silently mismatched.

### FEAT-23 — Denoise → diarize → STT (clean, then attribute, then transcribe)
- Level: INTERMEDIATE
- Pipeline: `audio-in → denoise → diarize → STT → per-speaker-text`
- Axes: feat:denoise, feat:diarize, feat:stt, dag:linear, place:hetero
- Scenario: A noisy multi-party recording is cleaned, diarized, then transcribed per speaker.
- System must: Three feedforward/windowed front stages each on their best substrate (denoise/diarize NPU-friendly), STT downstream; zero-copy boundaries on coherent memory.
- If mishandled: Diarizing noisy audio mislabels speakers; or each boundary forces a copy, blowing the per-stage SLO.

### FEAT-24 — STT → translate → TTS with diarization (per-speaker dubbing)
- Level: COMPOUND
- Pipeline: `audio-in → diarize → STT(per-speaker) → MT → TTS(per-speaker voice) → audio-out`
- Axes: feat:diarize, feat:translate, feat:tts, feat:clone, dag:linear, stream
- Scenario: A two-person foreign dialogue is dubbed so each original speaker keeps a distinct target-language voice.
- System must: Diarization labels drive per-speaker TTS voice selection; the clone/voice KV key fingerprints each speaker's reference (G1) so speaker A's voice can't contaminate speaker B's.
- If mishandled: Speaker voices swap or collide in the prefix cache; or interleaved speakers desync because the join doesn't gate on per-segment expected sources.

### FEAT-25 — Per-stage substrate placement across a full feature-DAG
- Level: COMPOUND
- Pipeline: `audio-in → denoise[NPU] → STT-enc[NPU] → AR[GPU] → codec[CPU] → audio-out`
- Axes: place:hetero, place:zerocopy, feat:denoise, feat:stt, dag:linear
- Scenario: On GB10, the feature-DAG is split by paradigm: static convs on NPU, AR decode on GPU, codec on CPU.
- System must: StagePlacer follows the ggml decision order (§3.4) — capability predicate, follow immovable weights, paradigm×substrate affinity, boundary minimization; coherent-memory boundaries pass `ZeroCopyBuffer` (pointer alias, no DMA).
- If mishandled: A stage placed off its weights forces weight reload; or a boundary the consumer can't view inserts a copy, and concurrent engines oversubscribe the one ~273 GB/s ceiling.

### FEAT-26 — Shared-bandwidth contention across a heterogeneous feature-DAG
- Level: COMPOUND
- Pipeline: `denoise[NPU] ∥ AR[GPU] ∥ codec[CPU]` on one coherent pool
- Axes: place:hetero, place:zerocopy, slo, feat:denoise
- Scenario: Placing denoise on NPU "frees the GPU," but NPU+GPU+CPU all draw from the same LPDDR ceiling concurrently.
- System must: Admission budgets the shared bandwidth (`Σ bandwidth_duty ≤ S·ceiling`, §3.4/§6); prefer overlapping a memory-bound stage (AR) with a compute-bound one (conv-codec); co-locate + time-share when both saturate.
- If mishandled: Zero-copy removes transfer cost but the split oversubscribes the shared ceiling → every stage misses its frame budget at once.

### FEAT-27 — Streaming a feature-DAG end-to-end with FINAL-marker propagation
- Level: COMPOUND
- Pipeline: `audio-in → enhance → STT → MT → TTS → audio-out` (each stage streams to next)
- Axes: stream, dag:linear, feat:enhance, feat:translate, feat:tts
- Scenario: Every stage streams deltas to the next, and the end-of-utterance marker must traverse all five stages so the final audio chunk is the last one.
- System must: Delta-only at every hop (I1); the FINAL/marker is an explicit in-band sentinel forwarded stage→stage (G2); a stage emits its own FINAL only after it has flushed its delay tail (F5 future-step marker).
- If mishandled: A stage emits FINAL before its delayed tail drains → the next stage closes early and the last words/audio are lost.

### FEAT-28 — Per-feature SLO budgets summing to a session SLO
- Level: COMPOUND
- Pipeline: `enhance(b1) → STT(b2) → MT(b3) → TTS(b4)` with `b1+b2+b3+b4 ≤ session TTFA`
- Axes: slo, dag:linear, feat:enhance, feat:translate, feat:tts
- Scenario: A 700 ms first-audio target is decomposed into per-stage budgets, and the bottleneck stage (often TTS/codec, not STT) is the binding constraint.
- System must: Each stage carries its own SLO + duty entry; admission tests the BOTTLENECK stage, not the first stage (§6); reject (typed 429/503 + Retry-After) rather than admit-and-glitch.
- If mishandled: Admitting on STT-stage capacity while the codec can't sustain the frame rate → audio gaps under concurrency (the AR≥4/codec=1 RFC #2568 trap).

### FEAT-29 — Vendor-mixed DAG: one local stage, one cloud-provider stage
- Level: COMPOUND
- Pipeline: `audio-in → STT[WaaV-local] → MT[cloud provider] → TTS[WaaV-local] → audio-out`
- Axes: vendor:mixed, feat:translate, dag:linear, stream, slo
- Scenario: Local STT/TTS for latency+privacy, but a cloud MT API for translation quality; the cloud hop adds network latency and can fail.
- System must: The cloud stage is a node with its own (larger) SLO budget and a credit/backpressure relay (G4); a cloud failure produces a terminal failure that fans out to all stages (fail-fast, G2 cancelled≠completed), never a silent hang.
- If mishandled: A stalled cloud MT call blocks the DAG with no FINAL → downstream TTS waits forever; or the local stages' tight SLO is applied to the cloud hop.

### FEAT-30 — Cancellation propagating through a DAG (barge-in aborts ALL stages)
- Level: COMPOUND
- Pipeline: `enhance → STT → LLM → TTS` ; barge-in mid-TTS
- Axes: bargein, dag:linear, feat:tts, stream
- Scenario: The user starts talking while the assistant is mid-reply; every in-flight stage for that stream must abort and free resources within one tick.
- System must: Reliable abort with per-stage ack (NOT fire-and-forget PUB/SUB — that drops messages, G9); one terminal cancel fans out to all stages; barge-in frees slot/KV/window ≤1 tick (§6); cancelled emits a terminal frame DISTINCT from completion (G2).
- If mishandled: A best-effort abort published before a late stage subscribes is lost → that stage keeps generating; assistant audio continues over the user.

### FEAT-31 — Wake-word → STT → LLM → TTS (always-on device assistant)
- Level: COMPOUND
- Pipeline: `audio-in → KWS[NPU] → (wake) → STT[NPU enc/GPU dec] → LLM → TTS → audio-out`
- Axes: feat:wake, feat:kws, feat:stt, feat:tts, dag:conditional, place:hetero, bargein
- Scenario: A smart speaker idles on NPU wake-word spotting at near-zero GPU cost, then activates the full agent loop on detection.
- System must: KWS static on NPU (fixed=1); wake event admits the STT stream; the heavy AR stages spin up only post-wake (mode=auto promotes lazily, §8); follow-on barge-in cancels the whole loop.
- If mishandled: The GPU pipeline stays warm/admitted with no wake → wasted bandwidth and slots; or wake→STT handoff drops the first user word.

### FEAT-32 — Speaker-verify → STT → translate → TTS (authenticated dubbed agent)
- Level: COMPOUND
- Pipeline: `audio-in → verify → (match) → STT → MT → TTS → audio-out`
- Axes: feat:verify, feat:translate, feat:tts, dag:conditional, stream
- Scenario: A cross-border support line authenticates the enrolled caller, then transcribes+translates+dubs the conversation.
- System must: Verify gate routes into the static success branch; the enrolled embedding is request-keyed (I5); the pipeline's FINAL traverses all stages; a verify-fail routes to an explicit reject terminal.
- If mishandled: Verify state leaks across callers; or a verify-fail produces an empty route that drops the request instead of a clean rejection.

### FEAT-33 — Language-detect → per-language STT → unified MT → TTS
- Level: COMPOUND
- Pipeline: `audio-in → langID → route → STT[lang] → MT(→target) → TTS → audio-out`
- Axes: feat:lang, feat:translate, feat:tts, dag:conditional, dag:fanin, stream
- Scenario: A multilingual hotline auto-detects the caller's language, transcribes with the right model, and always replies in one target language.
- System must: langID routes to one of several statically-declared STT branches (G11); all branches converge (fan-in) on the shared MT node gating on the actually-fired branch via wait_for_fn.
- If mishandled: Fan-in waits on all language branches when only one fired → deadlock; or routing to an unloaded language model crashes.

### FEAT-34 — SR (8 k→16 k) → denoise → STT for degraded telephony
- Level: COMPOUND
- Pipeline: `audio-in(8k noisy) → super-res → denoise → STT → text-out`
- Axes: feat:sr, feat:denoise, feat:stt, dag:linear, place:hetero
- Scenario: A bad PSTN line is bandwidth-extended, denoised, then transcribed for accuracy.
- System must: SR and denoise as ordered feedforward stages on NPU/CPU; honor the resample/two-clock contract (anti-alias only when downsampling; SR is upsampling here, §5.1); STT on its native substrate.
- If mishandled: SR after denoise (wrong order) re-introduces artifacts; or both run on GPU and starve any co-resident AR streams.

### FEAT-35 — Diarize + STT + langID (per-speaker, per-language transcript)
- Level: COMPOUND
- Pipeline: `audio-in → {diarize, langID} → STT(per-speaker,per-lang) → labeled-transcript`
- Axes: feat:diarize, feat:lang, feat:stt, dag:fanout, dag:fanin
- Scenario: A bilingual meeting where speakers switch languages; the transcript tags each utterance with speaker AND language.
- System must: Fan-out audio to diarize and langID (clone-on-fan-out, G5); fan-in their labels with STT output, gating on the per-segment expected set; per-stream clustering/embedding state bounded and freed.
- If mishandled: Code-switching mid-utterance with a per-stream-fixed language → wrong STT model on half the words; or fan-out aliasing corrupts a branch.

### FEAT-36 — Streaming STT → sentence-aggregated MT → streaming TTS (lookahead dubbing)
- Level: COMPOUND
- Pipeline: `STT(delta) → sentence-aggregator → MT(per-sentence) → TTS(streaming) → audio-out`
- Axes: feat:translate, feat:tts, dag:linear, stream, slo
- Scenario: Live dubbing that translates and speaks each completed sentence while the next is still being transcribed.
- System must: A sentence-aggregation stage buffers stable spans (only commit on sentence boundary), MT translates the committed sentence, TTS streams it; pipeline overlap means TTS of sentence N runs while STT collects sentence N+1.
- If mishandled: MT on partial sentences churns; or no aggregation → MT re-translates the whole growing transcript each chunk (O(N²)).

### FEAT-37 — Nested-stage feature: AR-talker {inner CFM} → vocoder in a translation DAG
- Level: COMPOUND
- Pipeline: `MT-text → ar_talker{nested cfm} → audiovae → audio-out`
- Axes: dag:nested, feat:tts, feat:translate, stream
- Scenario: The TTS leg of a translation pipeline is a dots.tts-class model whose code-predictor CFM loop is fused inside one AR step.
- System must: The nested inner loop stays INSIDE one StageNode's batched forward (§3.3) — not a cross-process stage; schedulability folds inner steps into `T_step = T_ar + inner_steps × T_inner`; the DAG sees one node.
- If mishandled: Splitting the tight AR→code-predictor feedback into separate stages balloons per-step latency and breaks the frame cadence.

### FEAT-38 — Loose-coupled feature DAG: ar_semantic → cfm_chunk → vocoder (CosyVoice2-class TTS leg)
- Level: COMPOUND
- Pipeline: `text → ar_semantic → cfm_chunk → vocoder → audio-out`
- Axes: dag:linear, feat:tts, stream, slo
- Scenario: A 3-node TTS leg where the CFM consumes completed chunks from the AR semantic stage (loose feedback), each stage independently batched.
- System must: Three separate nodes (loose feedback → separate stages, §3.3); AR lockstep (B≥4 to pipeline), CFM/vocoder micro-batch with their OWN batch sizes (codec=1, not inheriting AR's, C6/RFC #2568); chunk overlap (left-context + crossfade) prevents boundary artifacts.
- If mishandled: Uniform batch size everywhere → codec window round-robins → audible gaps under concurrency.

### FEAT-39 — Mixed batch hazard: streaming and non-streaming requests in one feature pipeline
- Level: COMPOUND
- Pipeline: `STT → MT → TTS` serving both live calls (streaming) and batch file jobs (non-streaming)
- Axes: stream, slo, tenancy:multi, feat:translate, feat:tts
- Scenario: The same DAG serves real-time calls and offline batch transcription/dubbing of uploaded files simultaneously.
- System must: Streaming and non-streaming NEVER mix in one micro-batch (G11); Realtime > Batch priority per stage; Batch piggybacks into leftover budget (Sarathi); a batch file's long prefill is force-chunked so it can't break a live call's cadence (§4.5 prefill firewall).
- If mishandled: A batch job's bulk prefill inflates a live call's per-frame latency 28× → 17–22 dropped frames = total dropout for the live caller.

### FEAT-40 — Audio super-resolution on the TTS EGRESS (model-SR → HD transport)
- Level: COMPOUND
- Pipeline: `LLM → TTS(24k) → super-res(24k→48k) → audio-out(HD)`
- Axes: feat:sr, feat:tts, dag:linear, stream
- Scenario: A 24 kHz TTS model's output is upsampled to 48 kHz for high-fidelity playback on good speakers.
- System must: SR is a post-codec egress stage off the AR clock (§5.1); egress resample model-SR→transport-SR with persistent per-stream rubato; the SR stage streams delta chunks aligned to frame boundaries.
- If mishandled: Re-running SR over the whole accumulated buffer each chunk (O(N²)); or resample discontinuities at chunk boundaries produce clicks.

### FEAT-41 — Per-speaker streaming STT fan-out (one mic, N speaker streams)
- Level: COMPOUND
- Pipeline: `audio-in → diarize → demux → {STT_A, STT_B, STT_C} → merge → transcript`
- Axes: feat:diarize, feat:stt, dag:fanout, dag:fanin, stream
- Scenario: A single far-field mic captures three speakers; diarization demultiplexes into three concurrent per-speaker STT streams that re-merge into one timeline.
- System must: Each per-speaker STT is its own slot (cohort by model+frame_rate); demux clones owned buffers per branch (G5); merge gates on whichever speakers are active per window (dynamic wait_for_fn, G11).
- If mishandled: Overlapping speech with fixed wait_for deadlocks; or a speaker who goes silent stalls the merge waiting for their (never-arriving) STT output.

### FEAT-42 — Out-of-order stage arrival: vocoder receives AR chunks before its payload
- Level: COMPOUND
- Pipeline: `ar_semantic → (chunks) → vocoder` ∥ `request-payload → vocoder`
- Axes: dag:fanin, feat:tts, stream
- Scenario: In a parallel-path TTS DAG, the vocoder can receive AR stream-chunks before its own request payload arrives (no cross-path ordering).
- System must: `can_accept_stream_before_payload` is EXPLICIT opt-in (G11/out-of-order); monotone chunk_id per (req,target); the vocoder latches the codec contract from whichever (payload | chunk-meta) arrives first; else hard-fail, never silent-corrupt.
- If mishandled: Vocoder consumes chunks against a default/wrong codec contract → garbled audio with no error.

### FEAT-43 — Drop a feature dynamically: skip denoise when SNR is high (cost-adaptive)
- Level: COMPOUND
- Pipeline: `audio-in → SNR-probe → route_fn → {denoise→STT | STT}`
- Axes: feat:denoise, feat:stt, dag:conditional, slo
- Scenario: To save compute, the DAG bypasses denoise for clean audio and only invokes it for noisy audio.
- System must: SNR probe feeds a route_fn choosing between two statically-declared paths (both in topology, G11); wait_for_fn expects the denoise source only on the noisy branch; the bypass path is an explicit edge, not an absent one.
- If mishandled: A fixed wait_for expecting denoise output deadlocks on the clean (bypass) branch.

### FEAT-44 — KWS-spotting within a live STT stream (in-band command detection)
- Level: COMPOUND
- Pipeline: `audio-in → {STT, KWS} → fuse → {transcript, command-events}`
- Axes: feat:kws, feat:stt, dag:fanout, multiterminal, stream
- Scenario: While transcribing a call, a parallel keyword-spotter fires command events ("transfer", "agent") without interrupting the transcript.
- System must: Fan-out audio to STT and KWS (clone-on-fan-out, G5); two terminals (transcript stream + command-event stream), each with its own FINAL; KWS is a cheap static stage that doesn't steal AR bandwidth.
- If mishandled: One terminal's FINAL closes both → either transcript or command stream truncates.

### FEAT-45 — Feature-DAG with a slow optional enrichment stage (degrade gracefully)
- Level: COMPOUND
- Pipeline: `STT → MT → TTS` with optional `→ sentiment-tag` (best-effort terminal)
- Axes: dag:multiterminal, slo, feat:translate, feat:tts
- Scenario: A dubbing pipeline also emits a best-effort sentiment label, but the core audio must never wait on it.
- System must: The sentiment terminal is best-effort — the request can narrow its terminals so the audio terminal's FINAL doesn't block on sentiment; the slow enrichment relegates to a degraded queue under load (don't drop core frames).
- If mishandled: The audio terminal blocks on the slow enrichment terminal → first-audio SLO blown for an optional feature.

### FEAT-46 — Per-feature precision tiering across a heterogeneous DAG
- Level: COMPOUND
- Pipeline: `denoise[fp16] → STT-enc[fp16] → AR[int4 weights]/codec[fp32] → out`
- Axes: place:hetero, slo, feat:denoise, feat:stt
- Scenario: Each stage runs at its own precision: quantized AR weights for throughput, but fp32 codec/norms for fidelity.
- System must: Per-component mixed precision (§5.2) — AR GEMMs int4/int8, but codec/vocoder/RoPE/sampler stay high-precision; an int8 file never lands on ORT-CUDA (resolve by_substrate[ep]); each stage's quant variant passes the accuracy gate before serving.
- If mishandled: Quantizing the codec corrupts audio (autocast/int8-decode divergence); or int8 weights silently fall back to CPU EP (12 ms → 232 ms).
### FEAT-47 — Feature-DAG admission under a saturated bottleneck stage
- Level: COMPOUND
- Pipeline: `enhance → STT → MT → TTS` ; TTS/codec at slot ceiling
- Axes: slo, tenancy:multi, feat:enhance, feat:translate, feat:tts
- Scenario: STT has free capacity but the TTS/codec stage is at its calibrated slot ceiling; a new dubbing request arrives.
- System must: Admission tests EVERY stage and the per-substrate duty + shared-bandwidth ledger (§6); reject (typed 429/503 + Retry-After) because the bottleneck can't take it — never admit on the non-bottleneck stage's headroom.
- If mishandled: Admit-on-STT-capacity → codec window round-robins across too many streams → audio gaps for everyone.

### FEAT-48 — Long-form feature pipeline: context grows past the ring (lossy escape)
- Level: COMPOUND
- Pipeline: `audio-in → STT(long-form) → MT → TTS(long narration)`
- Axes: feat:translate, feat:tts, dag:linear, stream
- Scenario: A 20-minute lecture is transcribed, translated, and re-narrated; context exceeds the bounded ring KV.
- System must: For long-form, pin attention-sink tokens + provide a paged/full-context escape hatch (L12) rather than silently wrapping the ring; the per-stage SLO holds because long context is benign on the AR clock (§1.2) up to the wall.
- If mishandled: Fixed ring wraps and silently forgets early context → translation/narration loses coherence on long inputs (StreamingLLM wraparound instability).

### FEAT-49 — Vendor-mixed S2S leg with a local enhancement front-end
- Level: COMPOUND
- Pipeline: `audio-in → denoise[local] → STS[cloud S2S API] → audio-out`
- Axes: vendor:mixed, feat:denoise, feat:sts, dag:linear, stream, bargein
- Scenario: Local denoise feeds a cloud speech-to-speech realtime API; barge-in must cancel the cloud session too.
- System must: The cloud STS node carries network-latency SLO + credit backpressure (G4); barge-in's reliable abort propagates to the cloud session (per-stage ack, G9); a cloud disconnect fails the request, not hangs (G2 cancelled≠completed).
- If mishandled: Barge-in cancels local denoise but the cloud STS keeps streaming audio → user talked over by a remote session that ignored the abort.

### FEAT-50 — Feature reuse: shared codec decoder across two TTS legs (dedup placement)
- Level: COMPOUND
- Pipeline: `{ar_A, ar_B} → shared-codec(Mimi/DAC) → {audio_A, audio_B}`
- Axes: place:hetero, feat:tts, dag:fanin, dag:fanout, tenancy:multi
- Scenario: Two different TTS models share the same Mimi/DAC decoder; the engine dedups it as one offloadable codec stage.
- System must: The terminal codec node is the cross-model dedup point (§3.2) and the one safe to offload (CPU/other EP); per-slot codec state keyed by (model,slot) so the two legs' streaming windows don't cross-talk (I5).
- If mishandled: Shared codec sliding-window state bleeds audio between the two models' streams under concurrency (crosstalk only under load).

### FEAT-51 — Diarize → per-speaker verify → labeled+authenticated transcript
- Level: COMPOUND
- Pipeline: `audio-in → diarize → per-segment verify → STT → {name-labeled transcript}`
- Axes: feat:diarize, feat:verify, feat:stt, dag:linear, dag:fanin
- Scenario: A meeting transcript labels each speaker by VERIFIED identity (not just "Speaker 1") by matching each diarized segment against enrolled profiles.
- System must: Diarization segments fan into a verify stage scoring against enrolled embeddings (request-keyed, I5); STT fan-in joins words+verified-name per segment on the expected source set (G11).
- If mishandled: Enrolled-profile state shared across requests mislabels speakers; or join deadlocks when a segment has no verify match (must route to "unknown" terminal).

### FEAT-52 — Crash isolation across a multi-process feature-DAG
- Level: COMPOUND
- Pipeline: `STT[proc1] → MT[proc2] → TTS[proc3]` (hot stages as separate processes)
- Axes: dag:linear, tenancy:multi, slo, feat:translate, feat:tts
- Scenario: The flaky cloud-MT sidecar crashes mid-stream; the STT and TTS stages must not hang.
- System must: Hot AR vs flaky stages = separate processes (G3/G7); 3-layer crash detection (sentinel byte, done-callbacks, liveness monitor); a dead stage fails in-flight requests with a terminal failure to every stream-queue (G2/H6), not a silent wedge.
- If mishandled: MT process death → STT keeps producing into a full queue and TTS waits forever; parent answers /health 200 while throughput is zero (H6 blind spot).

### FEAT-53 — Streaming-viability scheduling across a feature-DAG (deliver-in-time)
- Level: COMPOUND
- Pipeline: `STT → MT → TTS → audio-out` with a client playback buffer
- Axes: slo, stream, feat:translate, feat:tts
- Scenario: Under load, the scheduler prioritizes the request most at-risk of missing its frame deadline rather than minimizing average latency.
- System must: Binary streaming-viability objective (deliver-in-time → further latency worthless, L3/VoxServe); soft-deadline scheduling by risk-of-violation; cadence protected by the client playback buffer, not cross-replica migration.
- If mishandled: Optimizing average latency starves the one stream about to underrun → an audible gap for that caller while others are over-served.

### FEAT-54 — Multi-tenant feature-DAG with per-tenant model variants
- Level: COMPOUND
- Pipeline: `audio-in → STT[tenant-model] → MT → TTS[tenant-voice] → audio-out`
- Axes: tenancy:multi, feat:translate, feat:tts, feat:clone, dag:linear
- Scenario: Tenant A and tenant B share the box but use different STT models and cloned voices; their state must never cross.
- System must: Per-tenant cache_salt on shared-prefix blocks (prevent cross-tenant KV leak, H-other); clone keys fingerprint per-tenant references (G1); per-slot state freed transactionally on slot recycle (F3) — a recycled slot for tenant B can't see tenant A's KV/buffers.
- If mishandled: Cross-tenant KV reuse leaks one tenant's audio/transcript into another's (a privacy disaster).

### FEAT-55 — Feature-DAG slot recycling: stage state reset between callers
- Level: COMPOUND
- Pipeline: `enhance → STT → MT → TTS` ; caller 1 disconnects, caller 2 admitted into the same slots
- Axes: dag:linear, tenancy:multi, feat:enhance, feat:translate
- Scenario: Caller 1 leaves; caller 2 is admitted into the freed slots across all stages.
- System must: One transactional reset_slot(i) fans out to EVERY stage's per-slot state — denoise gain (AGC), STT word buffers, MT context, TTS codec window, KV pointers, sampler RNG (F2/F3); monotonic channel_id drops any late output from caller 1 (F3).
- If mishandled: Caller 2 sees caller 1's residual buffers in any one stage → cross-user transcript/audio contamination.

### FEAT-56 — Eager-fallback across a feature-DAG when CUDA-graph capture fails
- Level: COMPOUND
- Pipeline: `AR[graphed]/codec[graphed] → out` on sm120 with OOM at capture
- Axes: place:gpu, slo, feat:tts
- Scenario: On GB10/sm120 the AR stage's CUDA-graph capture OOMs after /health passed; the DAG must degrade, not crash-loop.
- System must: enforce_eager is a first-class per-stage fallback (C8/H4); capability-driven graph ladder downgrades to eager/piecewise, never crashes; pre-capture feasibility check fails at boot, not request-1 (H3).
- If mishandled: Capture-OOM after health-pass → crash-loop (the documented sm120 #44209 scar) takes down the whole pipeline.

### FEAT-57 — Far-field meeting front-end: beamform-equivalent denoise → dereverb → diarize
- Level: COMPOUND
- Pipeline: `multichannel-in → denoise → dereverb → diarize → speaker-segments`
- Axes: feat:denoise, feat:dereverb, feat:diarize, dag:linear, place:hetero
- Scenario: A far-field array's audio is cleaned, dereverberated, and diarized for a meeting timeline before any transcription.
- System must: Three ordered front stages on static-friendly substrates (NPU/CPU) with zero-copy boundaries; bound diarization clustering state for long meetings; each stage micro-batches independently.
- If mishandled: Reverb tail leaks into diarization → speaker boundaries smear; or the front-end stages contend for GPU bandwidth they don't need.

### FEAT-58 — Conditional S2S: direct STS path vs cascade fallback
- Level: COMPOUND
- Pipeline: `audio-in → route_fn → {STS(direct) | STT→MT→TTS(cascade)}`
- Axes: dag:conditional, feat:sts, feat:translate, feat:tts, dag:fanin
- Scenario: For supported language pairs use a direct S2S model; otherwise fall back to the STT→MT→TTS cascade — both converge on one audio terminal.
- System must: route_fn picks one statically-declared path (G11); wait_for_fn expects only the chosen path's sources; both paths converge on the same audio terminal with a single FINAL contract.
- If mishandled: Fan-in waiting on BOTH the direct and cascade sources deadlocks (only one fires); or the two paths emit conflicting FINALs.

### FEAT-59 — Streaming-encoder STT feature with cache-aware state (delta encoder)
- Level: COMPOUND
- Pipeline: `audio-in → cache-aware-streaming-encoder → CTC/RNNT decode → text-out`
- Axes: feat:stt, dag:linear, stream, slo
- Scenario: A high-concurrency STT service uses a cache-aware streaming encoder that carries bounded encoder state and processes deltas only.
- System must: Encoder-state contract = deltas only, bounded memory (Nemotron 560-streams/H100, L11); lockstep chunk-batched encoder cohort (§4.1 frame-sync STT row); per-slot encoder cache reset on recycle.
- If mishandled: Re-encoding full context each chunk (O(N²)) caps concurrency far below the streaming-encoder baseline.

### FEAT-60 — Code-switching translation DAG (mid-utterance language change)
- Level: EXTREME
- Pipeline: `audio-in → frame-langID → dynamic-route → STT[lang]* → MT(→target) → TTS → audio-out`
- Axes: feat:lang, feat:translate, feat:tts, dag:conditional, stream
- Scenario: A speaker switches Hindi↔English mid-sentence; the pipeline must re-route STT per span and still produce coherent target-language audio.
- System must: Per-span langID drives routing within the static STT branch set (G11); the MT stage aggregates re-segmented multilingual spans into coherent target sentences before TTS; route changes never leave a dangling fan-in.
- If mishandled: Per-stream-fixed language transcribes half the words with the wrong model; or each language switch creates a fan-in waiting on a branch that won't fire → stall.

### FEAT-61 — Variable-frame-rate codec feature in a mixed-rate DAG
- Level: EXTREME
- Pipeline: `audio-in → STT → MT → TTS[FlexiCodec 3–12.5 Hz dynamic]`
- Axes: feat:tts, feat:translate, dag:linear, slo, stream
- Scenario: The TTS leg uses a dynamic-frame-rate codec whose rate is data-dependent per-frame and not known a-priori.
- System must: Lockstep "advance a model-dependent variable stride" (L5/L6) rather than a fixed tick; the cohort key tolerates unknown-a-priori rates; can't lockstep-mix this stream with a fixed-rate one in the same step.
- If mishandled: A fixed-rate cohort assumption mis-paces the variable-rate stream → frame-budget overruns and underruns.

### FEAT-62 — AR-outer + generative-inner feature (third execution class) in a translation DAG
- Level: EXTREME
- Pipeline: `MT-text → patch-AR-talker{inner variable-NFE flow} → vocoder → audio-out`
- Axes: dag:nested, feat:tts, feat:translate, stream, slo
- Scenario: The TTS is a DiTAR/FlashTTS-class model: patch-AR (advances by a patch, not a frame) with a per-stream variable-NFE inner flow head.
- System must: The third execution class (L5) — outer advances a variable stride; the inner solve is a per-stream variable-NFE micro-batch composed INSIDE one AR step (compose two batchers, not pick one); fold inner steps into T_step for schedulability.
- If mishandled: Treating it as plain AR (one frame/step, fixed inner) breaks both batchers in one model → cadence collapse.

### FEAT-63 — Hybrid-KV feature pipeline: radix prefix-cache for cloned voice + ring suffix
- Level: EXTREME
- Pipeline: `(system-prompt + ref-audio) → [radix-cached prefix] → ar_talker[ring suffix] → codec → audio-out`
- Axes: feat:clone, feat:tts, slo, tenancy:multi, stream
- Scenario: A multi-tenant agent reuses the same cloned voice + system prompt across thousands of requests (86%+ prefix-cache hit, L1).
- System must: HYBRID KV (L1) — radix/prefix-cache the deterministic ref+system prefix, ring for the per-utterance suffix; the radix key fingerprints the full ref-audio sequence (G1) so different voices don't collide; zero-shot → key=None.
- If mishandled: Pure per-slot ring recomputes the ref+prompt KV every request → forfeits ~86% cacheable work on the top commercial workload (cloned-voice multi-tenant agent).

### FEAT-64 — Live meeting-assistant: far-field → denoise → diarize → per-speaker streaming STT → translate → streaming TTS, with barge-in, heterogeneous, multi-tenant
- Level: EXTREME
- Pipeline: `multichannel far-field → denoise[NPU] → diarize[NPU] → demux → {per-speaker streaming STT[NPU-enc/GPU-dec]}* → MT → streaming TTS[GPU AR + CPU codec] → audio-out` ∥ per-tenant
- Axes: feat:denoise, feat:diarize, feat:stt, feat:translate, feat:tts, dag:fanout, dag:fanin, dag:conditional, place:hetero, place:zerocopy, stream, bargein, slo, tenancy:multi
- Scenario: A multilingual meeting bot serves several tenants on one GB10: far-field multi-speaker audio is cleaned, diarized, demuxed into per-speaker streaming STT, each translated, and re-spoken — with barge-in cancellation, per-stage heterogeneous placement, and reject-don't-glitch admission under multi-tenant load.
- System must: Compose ALL invariants — heterogeneous placement following weights with zero-copy coherent boundaries (§3.4); per-substrate duty + shared-bandwidth admission tested on the BOTTLENECK stage (§6); dynamic wait_for_fn/route_fn for active-speaker fan-in (silent speakers don't stall, G11); delta-streaming + per-terminal FINAL across every hop (I1/G2); reliable barge-in abort fanning out to all stages incl any cloud leg (G9); transactional slot recycle + per-tenant cache_salt + per-speaker clone keys (F3/G1/H-other); reject (typed 429) rather than admit-and-glitch when the codec/bottleneck saturates.
- If mishandled: Any single broken invariant cascades — fan-in deadlock on a silent speaker, codec crosstalk between tenants, lost FINAL truncating a translation, barge-in ignored by the cloud leg, or admit-on-non-bottleneck causing audio gaps for every participant at once.

### FEAT-65 — Per-speaker streaming STT with overlapping speech (concurrent active slots)
- Level: EXTREME
- Pipeline: `far-field → diarize → demux → {STT_A ∥ STT_B}(overlapping) → merge`
- Axes: feat:diarize, feat:stt, dag:fanout, dag:fanin, stream, slo
- Scenario: Two meeting participants talk over each other; both per-speaker STT streams are active in the same frames and must produce overlapping transcript spans.
- System must: Both speakers occupy distinct lockstep slots (masked≠absent, F1/F2) ticking the same frame clock; the merge accepts overlapping spans gated on the per-frame active set (G11); demux clones owned buffers per speaker (G5).
- If mishandled: Treating speakers as exclusive serializes overlap → one speaker's words dropped; or a shared demux buffer corrupts one stream.

### FEAT-66 — Multi-tenant feature-DAG overload: graceful degradation, not frame-drop
- Level: EXTREME
- Pipeline: `{tenant-1…N} → STT → MT → TTS` at 1.5× capacity
- Axes: tenancy:multi, slo, dag:linear, feat:translate, feat:tts
- Scenario: Demand spikes to 150% of the box's calibrated capacity across tenants; the engine must protect active streams' cadence.
- System must: Deadline-aware admission + graceful relegation to a degraded queue (Niyama 95%+ vs <20% reject, L9); stop admitting → shed Batch → shed newest Realtime ≤1/tick with hysteresis (§6); WARM over-provisioning, never scale-to-zero/cold-start; cadence held by the client playback buffer.
- If mishandled: Naive frame-drop or admit-and-degrade glitches every active stream (80% SLO violations vs 8.6% with deadline-aware, the L9/Niyama gap).

### FEAT-67 — Spatial intra-node P/D for a feature-DAG's prefill spike
- Level: EXTREME
- Pipeline: `STT prefill[SM-partition A] ∥ TTS decode[SM-partition B]` on one GPU
- Axes: place:gpu, slo, stream, feat:stt, feat:tts
- Scenario: A burst of new dubbing sessions creates STT-prefill spikes that threaten the cadence of in-flight TTS decode on the same GPU.
- System must: Evaluate intra-node spatial P/D (SM partition, L4 — NOT cross-node) as a measured alternative to the chunked-prefill firewall; isochronous frame-clock + filler-masked-first-audio is exactly TaiChi's strict-TPOT/relaxed-TTFT quadrant.
- If mishandled: Chunked-prefill mixed into the decode batch spikes TBT ~8× (Nexus 250 ms vs 15 ms) → audible gaps the spatial partition would have avoided.

### FEAT-68 — End-to-end MTP-accelerated translation TTS leg (quality-neutral speedup)
- Level: EXTREME
- Pipeline: `MT → ar_talker[MTP-3 acoustic] → codec → audio-out`
- Axes: feat:tts, feat:translate, dag:nested, slo, stream
- Scenario: The dubbing TTS uses multi-token-prediction (3 acoustic tokens/step) to hit the first-audio SLO under load.
- System must: MTP is direct-emit → PRESERVES rectangular lockstep (L14, unlike draft-spec-decode which destroys it); treat the Depformer/code-predictor as the MTP mechanism; do NOT bolt EAGLE/Medusa on the acoustic path (0.98× net slowdown, L13).
- If mishandled: Adding draft-spec-decode to acoustic tokens slows it (many-to-one token→audio) and destroys the lockstep batch shape.

### FEAT-69 — Disconnect mid-DAG: slot leak across all stages (layered defense)
- Level: EXTREME
- Pipeline: `enhance → STT → MT → TTS` ; client drops connection mid-utterance
- Axes: dag:linear, tenancy:multi, bargein, feat:translate
- Scenario: A caller's network drops while the assistant is mid-reply; the slots/KV/windows across all five stages must be reclaimed, but only after the tail drains where required.
- System must: Free slots from INSIDE the step loop on ANY of {receiver closed, send error, ping-timeout 20 s, idle-timeout 120 s} (F9, multi-trigger — don't rely on a disconnect callback); the 3-state lifecycle (ACTIVE→MARKER→EOS) frees only after offset≥real_end, never on disconnect alone if a tail is draining (F5).
- If mishandled: A missed disconnect callback leaks slots across every stage → the box silently fills with dead streams until it can't admit (slot-leak, F9).

### FEAT-70 — Determinism across a feature-DAG (per-stream reproducibility)
- Level: EXTREME
- Pipeline: `STT → MT → TTS` reproducing identical audio for an identical input under varying co-load
- Axes: slo, tenancy:multi, dag:linear, feat:translate, feat:tts
- Scenario: A compliance use-case requires that the same input produces byte-identical dubbed audio regardless of what else is co-batched.
- System must: Accept per-stream-only determinism (bitwise cross-stream is impossible due to atomic-reduction non-determinism, H-other); idle slots are masked-not-removed so an active slot's output is identical with or without co-tenants (F1/F2 masked≠absent); seeded per-stream RNG offset.
- If mishandled: A co-tenant's presence changes the batch and perturbs another stream's sampling → non-reproducible audio (and the F2 ungated-mutation/RoPE-phase-jump corruption under idle-then-resume).

## Coverage

70 distinct scenarios spanning the feature-composition / multi-feature DAG family, graded SIMPLE → INTERMEDIATE → COMPOUND → EXTREME.

- **Single-feature stages (SIMPLE, FEAT-1..15):** every named feature in isolation or as a 2-stage pre/post pair — STT, TTS, STS, denoise→STT, VAD-gate, wake/KWS, langID-route, punct/ITN, text-MT, super-res(8k→16k), speaker-verify, voice-clone, diarize-only, dereverb, AGC.
- **Linear feature cascades (INTERMEDIATE/COMPOUND):** the canonical agent loop (enhance→STT→LLM→TTS, FEAT-16), the EN→Hindi speech-translation path (FEAT-17), per-speaker dubbing with diarization (FEAT-24), sentence-aggregated lookahead dubbing (FEAT-36), and ordered front-end chains (SR→denoise→STT, denoise→dereverb→diarize).
- **DAG-structure correctness (the G11 core):** fan-out (diarize+STT, FEAT-18; KWS-in-STT, FEAT-44), fan-in merge (mm_aggregate, FEAT-22; per-speaker merge, FEAT-41), conditional branches with dynamic wait_for_fn/route_fn (text-only vs audio FEAT-21, SNR-bypass FEAT-43, direct-vs-cascade S2S FEAT-58, code-switch FEAT-60), multi-terminal text+audio (FEAT-20, FEAT-44, FEAT-45), and out-of-order stage arrival (FEAT-42).
- **Heterogeneous placement + zero-copy (§3.4):** per-stage substrate split (FEAT-25), shared-bandwidth contention (FEAT-26), per-feature precision tiering (FEAT-46), eager-fallback on capture-OOM (FEAT-56), spatial intra-node P/D (FEAT-67).
- **Streaming + FINAL propagation + cancellation (I1/G2/G9):** end-to-end delta-streaming with marker propagation (FEAT-27), barge-in fanning out to all stages incl cloud (FEAT-30, FEAT-49), disconnect/slot-leak layered defense (FEAT-69).
- **SLO / admission / multi-tenancy (§6):** per-feature budgets summing to a session SLO (FEAT-28), bottleneck-stage admission (FEAT-47), streaming-viability scheduling (FEAT-53), graceful overload (FEAT-66), crash isolation (FEAT-52), per-tenant/per-slot state isolation + recycle (FEAT-54, FEAT-55, FEAT-70).
- **Vendor-mixed DAGs:** local+cloud stage mixes for MT (FEAT-29) and S2S (FEAT-49).
- **Nested & third-class execution (§3.3 / L5):** fused AR-talker{inner CFM} (FEAT-37), loose-coupled 3-node CFM TTS (FEAT-38), AR-outer+generative-inner (FEAT-62), MTP acoustic path (FEAT-68), variable-frame-rate codec (FEAT-61), hybrid-KV radix+ring for cloned voice (FEAT-63).
- **Literature-grounded hard cases:** cache-aware streaming-encoder (FEAT-59), long-form context-past-ring escape (FEAT-48), mixed streaming/non-streaming batch hazard (FEAT-39), overlapping-speech concurrent slots (FEAT-65).
- **The EXTREME capstone (FEAT-64):** the full multilingual meeting-assistant — far-field → denoise → diarize → per-speaker streaming STT → translate → streaming TTS — composing every invariant on a heterogeneous, multi-tenant GB10 with barge-in and reject-don't-glitch.

Cross-cutting failure modes pinned at least once each: fan-in deadlock on conditional/silent branches (G11), empty/non-static route (G11), aliased fan-out mutation (G5), prefix-cache wrong-voice collision (G1/L1), lost/early FINAL truncation (G2/F5), cumulative-not-delta O(N²) streaming (I1), fire-and-forget barge-in dropped (G9), admit-on-non-bottleneck audio gaps + codec=1 vs AR≥4 (C6/RFC #2568), int8-on-ORT-CUDA fallback + codec-quant corruption (§5.2), transactional-reset/slot-recycle cross-user contamination (F3), masked≠absent determinism/idle-resume corruption (F1/F2), crash-isolation health-blind-spot (H6), and shared-bandwidth oversubscription on coherent memory (§3.4).
